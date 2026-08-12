//! Application state entity. Owns the scanner, the current focus node, and
//! status counters. The view observes this and re-renders on `cx.notify()`.

use std::path::PathBuf;

use daggy_core::{fmt, ScanEvent, Scanner, ScanState};
use gpui::{Context, EventEmitter, Task};

/// The currently-focused node in the tree (the sunburst center). Defaults to
/// the root once a scan starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Focus(pub Option<u32>);

/// Status-bar counters, updated incrementally from scan events.
#[derive(Clone, Copy, Debug, Default)]
pub struct Status {
    pub dirs_scanned: u64,
    pub items_total: u64,
}

/// A user-facing event emitted by the app state (e.g. "scan finished, you may
/// want to cache it"). The view subscribes to drive side effects like cache
/// writes.
#[derive(Clone, Debug)]
pub enum AppEvent {
    /// A root scan finished; a good time to persist the cache.
    ScanFinished { root_path: PathBuf },
}

/// The single source of truth for the UI.
pub struct AppState {
    pub scanner: Scanner,
    /// The root path of the current scan, if any.
    pub root_path: Option<PathBuf>,
    /// The focused node (sunburst center / list root).
    pub focus: Focus,
    pub status: Status,
    pub scanning: bool,
    /// Cancel token for the in-flight scan, kept alive for the scan's duration.
    _cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// Background tasks (event drainers, cache writes).
    _tasks: Vec<Task<()>>,
}

impl AppState {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            scanner: Scanner::new(),
            root_path: None,
            focus: Focus(None),
            status: Status::default(),
            scanning: false,
            _cancel: None,
            _tasks: Vec::new(),
        }
    }

    /// Pick a folder: try the on-disk cache first for an instant restore, then
    /// kick off a fresh phase-1 scan regardless (cheap on top of a cached tree,
    /// and it refreshes any stale subtrees). Runs the cache load off the main
    /// thread so the UI never blocks.
    pub fn open_cached_or_scan(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let load_path = path.clone();
        let scanner = self.scanner.tree();
        let task = cx.background_executor().spawn(async move {
            match daggy_cache::load(&load_path) {
                Ok(Some(tree)) => {
                    scanner.write().load_from(tree);
                    log::info!("restored cache for {:?}", load_path);
                }
                Ok(None) => log::debug!("no cache for {:?}", load_path),
                Err(e) => log::warn!("cache load failed for {:?}: {e}", load_path),
            }
        });
        let state = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            task.await;
            let _ = state.update(cx, |s, cx| s.scan(path, cx));
        })
        .detach();
    }

    /// Begin scanning `path`. Replaces any prior scan. Wires the scan event
    /// channel into GPUI by spawning a background task that drains events and
    /// calls `cx.notify()` so the view re-renders incrementally.
    pub fn scan(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let handle = self.scanner.scan_root(path.clone());
        self.root_path = Some(path);
        self.scanning = true;
        self.status = Status::default();
        let root_path = self.root_path.clone();
        // Keep the cancel token alive on the state; the handle's receiver is
        // moved into the draining task.
        self._cancel = Some(handle.cancel.clone());

        // Drain scan events on a background task, updating state + notifying.
        let task = cx.spawn(async move |this, cx| {
            let mut finished = false;
            let events = handle.events;
            while let Ok(ev) = events.recv().await {
                let mut should_save = false;
                match ev {
                    ScanEvent::ScanStarted { root } => {
                        this.update(cx, |s, cx| {
                            s.focus = Focus(Some(root));
                            cx.notify();
                        })
                        .ok();
                    }
                    ScanEvent::DirCompleted { .. } | ScanEvent::DirError { .. } => {
                        this.update(cx, |_s, cx| cx.notify()).ok();
                    }
                    ScanEvent::Progress {
                        dirs_scanned,
                        items_total,
                    } => {
                        this.update(cx, |s, cx| {
                            s.status.dirs_scanned += dirs_scanned;
                            s.status.items_total += items_total;
                            cx.notify();
                        })
                        .ok();
                    }
                    ScanEvent::ScanFinished { .. } => {
                        finished = true;
                        should_save = true;
                    }
                }
                if should_save {
                    this.update(cx, |s, cx| {
                        s.scanning = false;
                        if let Some(rp) = &root_path {
                            cx.emit(AppEvent::ScanFinished { root_path: rp.clone() });
                        }
                        cx.notify();
                    })
                    .ok();
                }
                if finished {
                    break;
                }
            }
        });
        self._tasks.push(task);
        cx.notify();
    }

    /// Lazily expand a directory node (descend its subtree).
    pub fn expand(&mut self, node: u32, cx: &mut Context<Self>) {
        let handle = self.scanner.expand(node);
        self.scanning = true;
        let task = cx.spawn(async move |this, cx| {
            while let Ok(_ev) = handle.events.recv().await {
                this.update(cx, |_s, cx| cx.notify()).ok();
            }
            this.update(cx, |s, cx| {
                s.scanning = false;
                cx.notify();
            })
            .ok();
        });
        self._tasks.push(task);
        cx.notify();
    }

    /// Set the focused node (drill in / out).
    pub fn set_focus(&mut self, node: Option<u32>, cx: &mut Context<Self>) {
        if self.focus.0 != node {
            self.focus = Focus(node);
            cx.notify();
        }
    }

    /// Navigate up one level: focus the parent of the current focus node.
    /// Bound to Escape. No-op when already at the root.
    pub fn go_up(&mut self, cx: &mut Context<Self>) {
        let parent = self.focus.0.and_then(|id| {
            let snap = self.scanner.snapshot();
            snap.get(id).and_then(|n| n.parent)
        });
        if let Some(parent) = parent {
            self.set_focus(Some(parent), cx);
        }
    }

    /// A cheap snapshot of the tree for rendering.
    pub fn snapshot(&self) -> daggy_core::TreeSnapshot {
        self.scanner.snapshot()
    }

    /// Human-readable summary for the status bar.
    pub fn status_text(&self) -> String {
        let snap = self.snapshot();
        let total = snap.root().map(|r| r.size).unwrap_or(0);
        if self.scanning {
            format!(
                "scanning… {} items · {}",
                fmt::human_count(self.status.items_total),
                fmt::human_bytes(total),
            )
        } else {
            format!(
                "{} items · {}",
                fmt::human_count(self.status.items_total),
                fmt::human_bytes(total),
            )
        }
    }

    pub fn focus_state(&self, id: u32) -> ScanState {
        self.snapshot()
            .get(id)
            .map(|n| n.state)
            .unwrap_or(ScanState::Unscanned)
    }
}

impl EventEmitter<AppEvent> for AppState {}

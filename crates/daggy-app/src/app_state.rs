//! Application state, scan generation routing, and dated-cache handoff.

use std::path::PathBuf;
use std::sync::Arc;

use daggy_core::{fmt, NodeId, ScanEvent, ScanSummary, Scanner, Tree, TreeProjection};
use gpui::{Context, Task};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplaySource {
    Empty,
    Loading,
    Cached,
    Live,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Status {
    pub dirs_scanned: u64,
    pub items_seen: u64,
    pub errors: u64,
    pub summary: Option<ScanSummary>,
}

pub struct AppState {
    scanner: Scanner,
    pub root_path: Option<PathBuf>,
    pub focus: Option<NodeId>,
    pub selected: Option<NodeId>,
    pub hovered: Option<NodeId>,
    pub source: DisplaySource,
    pub scanning: bool,
    pub status: Status,
    pub cache_written_at_millis: Option<u64>,
    cached_tree: Option<Arc<Tree>>,
    request_generation: u64,
    active_scan_generation: Option<u64>,
    tasks: Vec<Task<()>>,
}

impl AppState {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            scanner: Scanner::new(),
            root_path: None,
            focus: None,
            selected: None,
            hovered: None,
            source: DisplaySource::Empty,
            scanning: false,
            status: Status::default(),
            cache_written_at_millis: None,
            cached_tree: None,
            request_generation: 0,
            active_scan_generation: None,
            tasks: Vec::new(),
        }
    }

    /// Restore only when a previous root exists. A true first launch remains a
    /// calm empty state instead of immediately interrupting with a picker.
    pub fn restore_last_root(&mut self, cx: &mut Context<Self>) {
        match daggy_cache::load_last_root() {
            Ok(Some(path)) if path.is_dir() => self.open_path(path, cx),
            Ok(_) => {}
            Err(error) => log::warn!("recent root load failed: {error}"),
        }
    }

    pub fn open_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.request_generation = self.request_generation.wrapping_add(1);
        let request = self.request_generation;
        self.scanner.cancel_current();
        self.root_path = Some(path.clone());
        self.focus = None;
        self.selected = None;
        self.hovered = None;
        self.source = DisplaySource::Loading;
        self.scanning = true;
        self.status = Status::default();
        self.cached_tree = None;
        self.cache_written_at_millis = None;
        cx.notify();

        let cache_path = path.clone();
        let load = cx.background_executor().spawn(async move {
            let cache = daggy_cache::load(&cache_path);
            (cache_path, cache)
        });
        let state = cx.weak_entity();
        let task = cx.spawn(async move |_, cx| {
            let (path, cache) = load.await;
            let _ = state.update(cx, |state, cx| {
                if state.request_generation != request {
                    return;
                }
                match cache {
                    Ok(Some(hit)) => {
                        state.focus = hit.tree.root;
                        state.cache_written_at_millis = Some(hit.written_at_millis);
                        state.cached_tree = Some(Arc::new(hit.tree));
                        state.source = DisplaySource::Cached;
                    }
                    Ok(None) => state.source = DisplaySource::Live,
                    Err(error) => {
                        log::warn!("cache load failed for {:?}: {error}", path);
                        state.source = DisplaySource::Live;
                    }
                }
                state.start_scan(path, request, cx);
            });
        });
        self.tasks.push(task);
    }

    fn start_scan(&mut self, path: PathBuf, request: u64, cx: &mut Context<Self>) {
        let handle = self.scanner.scan_root(path.clone());
        let generation = handle.generation;
        self.active_scan_generation = Some(generation);
        self.scanning = true;
        if self.cached_tree.is_none() {
            self.source = DisplaySource::Live;
        }

        let state = cx.weak_entity();
        let task = cx.spawn(async move |_, cx| {
            while let Ok(event) = handle.events.recv().await {
                let finished = matches!(event, ScanEvent::ScanFinished { .. });
                let _ = state.update(cx, |state, cx| {
                    if state.request_generation != request
                        || state.active_scan_generation != Some(generation)
                    {
                        return;
                    }
                    match event {
                        ScanEvent::ScanStarted { root } => {
                            if state.cached_tree.is_none() {
                                state.focus = Some(root);
                            }
                        }
                        ScanEvent::VisibleReady { .. } => {}
                        ScanEvent::Progress {
                            dirs_scanned,
                            items_seen,
                            errors,
                        } => {
                            state.status.dirs_scanned = dirs_scanned;
                            state.status.items_seen = items_seen;
                            state.status.errors = errors;
                        }
                        ScanEvent::ScanFinished { root, summary } => {
                            state.status.dirs_scanned = summary.dirs_scanned;
                            state.status.items_seen = summary.items_seen;
                            state.status.errors = summary.errors;
                            state.status.summary = Some(summary);
                            state.scanning = false;
                            state.source = DisplaySource::Live;
                            state.cached_tree = None;
                            state.cache_written_at_millis = None;
                            state.focus = Some(root);
                            state.selected = None;
                            state.hovered = None;
                            state.persist_last_good(path.clone(), summary, cx);
                        }
                    }
                    cx.notify();
                });
                if finished {
                    break;
                }
            }
        });
        self.tasks.push(task);
        cx.notify();
    }

    fn persist_last_good(&self, path: PathBuf, summary: ScanSummary, cx: &mut Context<Self>) {
        let tree = self.scanner.tree_handle();
        cx.background_executor()
            .spawn(async move {
                if summary.complete {
                    let tree = tree.read();
                    if let Err(error) = daggy_cache::save(&tree, &path) {
                        log::warn!("cache save failed for {:?}: {error}", path);
                    }
                }
                if let Err(error) = daggy_cache::save_last_root(&path) {
                    log::warn!("recent root save failed for {:?}: {error}", path);
                }
            })
            .detach();
    }

    pub fn projection(&self) -> TreeProjection {
        if let Some(tree) = &self.cached_tree {
            tree.projection(self.focus, 3, 32, 4096, 2000)
        } else {
            self.scanner.projection(self.focus)
        }
    }

    pub fn activate_node(&mut self, node: NodeId, cx: &mut Context<Self>) {
        let projection = self.projection();
        let view = projection
            .rows
            .iter()
            .find(|view| view.id == node)
            .or_else(|| projection.chart_node(node).map(|chart| &chart.node));
        self.selected = Some(node);
        if view.is_some_and(|view| view.kind.is_dir()) {
            let path = if let Some(tree) = &self.cached_tree {
                tree.path_of(node)
            } else {
                self.scanner.tree_handle().read().path_of(node)
            };
            self.focus = Some(node);
            self.selected = None;
            self.hovered = None;
            if let Some(path) = path {
                self.scanner.prioritize_path(path);
            }
        }
        cx.notify();
    }

    pub fn go_up(&mut self, cx: &mut Context<Self>) {
        let parent = self.projection().focus_node.and_then(|node| node.parent);
        if let Some(parent) = parent {
            let path = if let Some(tree) = &self.cached_tree {
                tree.path_of(parent)
            } else {
                self.scanner.tree_handle().read().path_of(parent)
            };
            self.focus = Some(parent);
            self.selected = None;
            self.hovered = None;
            if let Some(path) = path {
                self.scanner.prioritize_path(path);
            }
            cx.notify();
        }
    }

    pub fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) -> Option<usize> {
        let projection = self.projection();
        if projection.rows.is_empty() {
            return None;
        }
        let current = self
            .selected
            .and_then(|selected| projection.rows.iter().position(|row| row.id == selected));
        let next = match (current, delta.is_negative()) {
            (None, _) => 0,
            (Some(index), true) => index.saturating_sub(delta.unsigned_abs()),
            (Some(index), false) => (index + delta as usize).min(projection.rows.len() - 1),
        };
        self.selected = Some(projection.rows[next].id);
        self.hovered = None;
        cx.notify();
        Some(next)
    }

    pub fn activate_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(selected) = self.selected {
            self.activate_node(selected, cx);
        } else {
            let first = self.projection().rows.first().map(|row| row.id);
            if let Some(first) = first {
                self.activate_node(first, cx);
            }
        }
    }

    pub fn set_hovered(&mut self, hovered: Option<NodeId>, cx: &mut Context<Self>) {
        if self.hovered != hovered {
            self.hovered = hovered;
            cx.notify();
        }
    }

    pub fn status_line(&self, projection: &TreeProjection) -> String {
        let allocated = projection.root.as_ref().map_or(0, |root| root.block_size);
        match self.source {
            DisplaySource::Empty => "No folder selected".into(),
            DisplaySource::Loading => "Opening cached snapshot".into(),
            DisplaySource::Cached => format!(
                "Cached snapshot  ·  refreshing  ·  {} allocated",
                fmt::human_bytes(allocated)
            ),
            DisplaySource::Live if self.scanning => format!(
                "Scanning  ·  {} folders  ·  {} items  ·  {} so far",
                fmt::human_count(self.status.dirs_scanned),
                fmt::human_count(self.status.items_seen),
                fmt::human_bytes(allocated)
            ),
            DisplaySource::Live => match self.status.summary {
                Some(summary) if summary.errors > 0 => format!(
                    "Finished with {} unreadable folders  ·  {} items  ·  {}",
                    fmt::human_count(summary.errors),
                    fmt::human_count(summary.items_seen),
                    fmt::human_bytes(allocated)
                ),
                Some(summary) => format!(
                    "Complete in {:.2}s  ·  {} items  ·  {} allocated",
                    summary.elapsed.as_secs_f64(),
                    fmt::human_count(summary.items_seen),
                    fmt::human_bytes(allocated)
                ),
                None => "Ready".into(),
            },
        }
    }

    pub fn center_note(&self, projection: &TreeProjection) -> &'static str {
        if self.source == DisplaySource::Cached {
            "cached snapshot"
        } else if !self.scanning && self.status.errors > 0 {
            "incomplete"
        } else if projection
            .focus_node
            .as_ref()
            .is_some_and(|node| !node.complete)
        {
            "calculating"
        } else {
            "allocated"
        }
    }
}

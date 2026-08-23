//! Application state, scan generation routing, and dated-cache handoff.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use daggy_core::{fmt, NodeId, NodeView, ScanEvent, ScanSummary, Scanner, Tree, TreeProjection};
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
    ranked_list: RefCell<Option<RankedList>>,
    tasks: Vec<Task<()>>,
}

struct RankedList {
    parent: NodeId,
    child_count: usize,
    size: u64,
    generation: u64,
    cached: bool,
    ids: Arc<Vec<NodeId>>,
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
            ranked_list: RefCell::new(None),
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
        if self.focus_existing(&path, cx) {
            return;
        }
        self.begin_fresh_scan(path, cx);
    }

    /// If `path` is already in the live or cached tree, keep that generation
    /// and only move focus. Opening an ancestor of the current root cannot
    /// invent missing parents, so it falls through to a new scan.
    fn focus_existing(&mut self, path: &Path, cx: &mut Context<Self>) -> bool {
        let id = self
            .cached_tree
            .as_ref()
            .and_then(|tree| {
                tree.find_path(path)
                    .filter(|id| tree.get(*id).is_some_and(|node| node.kind.is_dir()))
            })
            .or_else(|| {
                let tree = self.scanner.tree_handle();
                let tree = tree.read();
                tree.find_path(path)
                    .filter(|id| tree.get(*id).is_some_and(|node| node.kind.is_dir()))
            });
        let Some(id) = id else {
            return false;
        };
        self.root_path = Some(path.to_path_buf());
        self.selected = None;
        self.hovered = None;
        self.focus = Some(id);
        self.scanner.prioritize_path(path.to_path_buf());
        let persist = path.to_path_buf();
        cx.background_executor()
            .spawn(async move {
                if let Err(error) = daggy_cache::save_last_root(&persist) {
                    log::warn!("recent root save failed for {:?}: {error}", persist);
                }
            })
            .detach();
        cx.notify();
        true
    }

    fn begin_fresh_scan(&mut self, path: PathBuf, cx: &mut Context<Self>) {
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
        self.ranked_list.borrow_mut().take();
        cx.notify();

        let cache_path = path.clone();
        let load = cx.background_executor().spawn(async move {
            let cache = daggy_cache::load_covering(&cache_path);
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
                            let wanted = state.root_path.clone();
                            state.cached_tree = None;
                            state.cache_written_at_millis = None;
                            state.focus = wanted
                                .as_ref()
                                .and_then(|wanted| {
                                    state.scanner.tree_handle().read().find_path(wanted)
                                })
                                .or(Some(root));
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
        let last_root = self.root_path.clone().unwrap_or_else(|| path.clone());
        cx.background_executor()
            .spawn(async move {
                if summary.complete {
                    let tree = tree.read();
                    if let Err(error) = daggy_cache::save(&tree, &path) {
                        log::warn!("cache save failed for {:?}: {error}", path);
                    }
                }
                if let Err(error) = daggy_cache::save_last_root(&last_root) {
                    log::warn!("recent root save failed for {:?}: {error}", last_root);
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

    /// Largest-first child ids of the focused folder. Cached across frames so
    /// a directory with tens of thousands of entries does not get re-sorted
    /// unless its children actually change.
    pub fn ranked_child_ids(&self) -> Arc<Vec<NodeId>> {
        let Some(parent) = self.focus.or_else(|| self.with_tree(|tree| tree.root)) else {
            return Arc::new(Vec::new());
        };
        let Some((child_count, size)) = self.with_tree(|tree| {
            tree.get(parent)
                .map(|node| (node.children.len(), node.block_size))
        }) else {
            return Arc::new(Vec::new());
        };
        let cached = self.cached_tree.is_some();
        let generation = self.request_generation;
        if let Some(cache) = self.ranked_list.borrow().as_ref() {
            if cache.parent == parent
                && cache.child_count == child_count
                && cache.size == size
                && cache.generation == generation
                && cache.cached == cached
            {
                return cache.ids.clone();
            }
        }
        let ids = Arc::new(self.with_tree(|tree| tree.ranked_child_ids(parent)));
        *self.ranked_list.borrow_mut() = Some(RankedList {
            parent,
            child_count,
            size,
            generation,
            cached,
            ids: ids.clone(),
        });
        ids
    }

    pub fn node_view(&self, id: NodeId) -> Option<NodeView> {
        self.with_tree(|tree| tree.get(id).map(NodeView::from))
    }

    fn with_tree<R>(&self, f: impl FnOnce(&Tree) -> R) -> R {
        if let Some(tree) = &self.cached_tree {
            f(tree)
        } else {
            f(&self.scanner.tree_handle().read())
        }
    }

    pub fn activate_node(&mut self, node: NodeId, cx: &mut Context<Self>) {
        self.selected = Some(node);
        let is_dir = self.node_view(node).is_some_and(|view| view.kind.is_dir());
        if is_dir {
            let path = self.path_of(node);
            self.focus = Some(node);
            self.selected = None;
            self.hovered = None;
            self.ranked_list.borrow_mut().take();
            if let Some(path) = path {
                self.scanner.prioritize_path(path);
            }
        }
        cx.notify();
    }

    pub fn go_up(&mut self, cx: &mut Context<Self>) {
        let parent = self.projection().focus_node.and_then(|node| node.parent);
        if let Some(parent) = parent {
            let path = self.path_of(parent);
            self.focus = Some(parent);
            self.selected = None;
            self.hovered = None;
            self.ranked_list.borrow_mut().take();
            if let Some(path) = path {
                self.scanner.prioritize_path(path);
            }
            cx.notify();
        }
    }

    pub fn rescan(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.root_path.clone() {
            self.begin_fresh_scan(path, cx);
        }
    }

    pub fn open_or_focus(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if let Some(id) = self.find_path(&path) {
            self.activate_node(id, cx);
        } else if path.is_dir() {
            self.open_path(path, cx);
        }
    }

    pub fn path_of(&self, id: NodeId) -> Option<PathBuf> {
        self.with_tree(|tree| tree.path_of(id))
    }

    pub fn trash_size(&self) -> Option<u64> {
        let trash = dirs::home_dir()?.join(".Trash");
        let id = self.find_path(&trash)?;
        self.node_block_size(id)
    }

    pub fn files_for(&self, id: NodeId, projection: &TreeProjection) -> u64 {
        if projection.root.as_ref().is_some_and(|root| root.id == id) {
            if self.status.items_seen > 0 {
                self.status.items_seen
            } else {
                projection.total_nodes as u64
            }
        } else {
            projection
                .chart_node(id)
                .map(|chart| chart.node.child_count as u64)
                .or_else(|| {
                    projection
                        .rows
                        .iter()
                        .find(|row| row.id == id)
                        .map(|row| row.child_count as u64)
                })
                .or_else(|| {
                    projection
                        .focus_node
                        .as_ref()
                        .filter(|node| node.id == id)
                        .map(|node| node.child_count as u64)
                })
                .unwrap_or(0)
        }
    }

    fn find_path(&self, path: &Path) -> Option<NodeId> {
        self.with_tree(|tree| tree.find_path(path))
    }

    fn node_block_size(&self, id: NodeId) -> Option<u64> {
        self.with_tree(|tree| tree.get(id).map(|node| node.block_size))
    }

    pub fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) -> Option<usize> {
        let ids = self.ranked_child_ids();
        if ids.is_empty() {
            return None;
        }
        let current = self
            .selected
            .and_then(|selected| ids.iter().position(|id| *id == selected));
        let next = match (current, delta.is_negative()) {
            (None, _) => 0,
            (Some(index), true) => index.saturating_sub(delta.unsigned_abs()),
            (Some(index), false) => (index + delta as usize).min(ids.len() - 1),
        };
        self.selected = Some(ids[next]);
        self.hovered = None;
        cx.notify();
        Some(next)
    }

    pub fn activate_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(selected) = self.selected {
            self.activate_node(selected, cx);
        } else if let Some(first) = self.ranked_child_ids().first().copied() {
            self.activate_node(first, cx);
        }
    }

    pub fn set_hovered(&mut self, hovered: Option<NodeId>, cx: &mut Context<Self>) {
        if self.hovered != hovered {
            self.hovered = hovered;
            cx.notify();
        }
    }

    pub fn can_go_up(&self) -> bool {
        self.projection()
            .focus_node
            .is_some_and(|node| node.parent.is_some())
    }

    pub fn focused_or_selected_path(&self) -> Option<PathBuf> {
        let id = self.selected.or(self.hovered).or(self.focus)?;
        self.path_of(id).or_else(|| self.root_path.clone())
    }

    pub fn copy_label(&self) -> Option<String> {
        if let Some(path) = self.focused_or_selected_path() {
            return Some(path.to_string_lossy().into_owned());
        }
        self.root_path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
    }

    pub fn reveal_in_finder(&self) {
        let Some(path) = self.focused_or_selected_path() else {
            return;
        };
        let _ = std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn();
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
            "used"
        }
    }
}

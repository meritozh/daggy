//! Cache-friendly arena tree and compact read projections.

use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use rustc_hash::FxHashMap;

/// A stable handle to a node inside one scan generation.
pub type NodeId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    File = 0,
    Dir = 1,
    Symlink = 2,
    Other = 3,
}

impl Kind {
    pub fn is_dir(self) -> bool {
        matches!(self, Self::Dir)
    }
}

/// Direct-enumeration state. `complete` on [`Node`] separately records whether
/// every descendant has finished, which prevents a partial size looking final.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanState {
    Unscanned,
    Scanning { child_count: u32 },
    Scanned { child_count: u32 },
    Error,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    /// Last path component. The root stores the full selected path.
    pub name: Arc<OsStr>,
    pub kind: Kind,
    pub size: u64,
    pub block_size: u64,
    pub mtime: i64,
    pub inode: u64,
    pub dev: u64,
    pub state: ScanState,
    /// True only when this node and all descendants are accounted for.
    pub complete: bool,
    pub children: Vec<NodeId>,
}

impl Node {
    pub fn unscanned_dir(id: NodeId, parent: Option<NodeId>, name: impl Into<Arc<OsStr>>) -> Self {
        Self {
            id,
            parent,
            name: name.into(),
            kind: Kind::Dir,
            size: 0,
            block_size: 0,
            mtime: 0,
            inode: 0,
            dev: 0,
            state: ScanState::Unscanned,
            complete: false,
            children: Vec::new(),
        }
    }

    pub fn display_name(&self) -> String {
        self.name.to_string_lossy().into_owned()
    }
}

#[derive(Debug, Default, Clone)]
pub struct Tree {
    nodes: Vec<Option<Node>>,
    free: Vec<NodeId>,
    pub root: Option<NodeId>,
}

impl Tree {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn alloc(&mut self, mut node: Node) -> NodeId {
        if let Some(id) = self.free.pop() {
            node.id = id;
            self.nodes[id as usize] = Some(node);
            id
        } else {
            let id = self.nodes.len() as NodeId;
            node.id = id;
            self.nodes.push(Some(node));
            id
        }
    }

    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id as usize).and_then(Option::as_ref)
    }

    pub fn get_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(id as usize).and_then(Option::as_mut)
    }

    pub fn root(&self) -> Option<&Node> {
        self.root.and_then(|id| self.get(id))
    }

    pub fn len(&self) -> usize {
        self.nodes.iter().filter(|slot| slot.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn is_complete(&self) -> bool {
        self.root().is_some_and(|node| node.complete)
    }

    pub fn cap(&self) -> NodeId {
        self.nodes.len() as NodeId
    }

    pub fn iter(&self) -> impl Iterator<Item = &Node> {
        self.nodes.iter().filter_map(Option::as_ref)
    }

    pub fn path_of(&self, id: NodeId) -> Option<PathBuf> {
        let mut components: Vec<Arc<OsStr>> = Vec::new();
        let mut cursor = self.get(id);
        while let Some(node) = cursor {
            components.push(node.name.clone());
            cursor = node.parent.and_then(|parent| self.get(parent));
        }
        if components.is_empty() {
            return None;
        }
        let mut path = PathBuf::new();
        for component in components.into_iter().rev() {
            path.push(component.as_ref());
        }
        Some(path)
    }

    /// Resolve an exact path through already discovered children without a
    /// full arena scan. Missing descendants simply have not been reached yet.
    pub fn find_path(&self, path: &Path) -> Option<NodeId> {
        let root = self.root?;
        let root_path = Path::new(self.get(root)?.name.as_ref());
        let relative = path.strip_prefix(root_path).ok()?;
        let mut current = root;
        for component in relative.components() {
            let Component::Normal(name) = component else {
                continue;
            };
            current = self.get(current)?.children.iter().copied().find(|child| {
                self.get(*child)
                    .is_some_and(|node| node.name.as_ref() == name)
            })?;
        }
        Some(current)
    }

    /// Clone `id` and its descendants into a new arena whose root name is the
    /// full filesystem path. Ancestors of `id` are dropped so the subtree can
    /// stand in as a scan root.
    pub fn extract_subtree(&self, id: NodeId) -> Option<Tree> {
        let full_path = self.path_of(id)?;
        let source = self.get(id)?.clone();
        let mut tree = Tree::new();
        let mut remap = FxHashMap::default();
        let child_ids = source.children.clone();
        let mut root_node = source;
        root_node.parent = None;
        root_node.children.clear();
        root_node.name = arc_os(full_path.into_os_string());
        let new_root = tree.alloc(root_node);
        tree.root = Some(new_root);
        remap.insert(id, new_root);

        let mut queue = VecDeque::from(child_ids);
        while let Some(old) = queue.pop_front() {
            let Some(node) = self.get(old) else {
                continue;
            };
            queue.extend(node.children.iter().copied());
            let mut cloned = node.clone();
            cloned.parent = node.parent.and_then(|parent| remap.get(&parent).copied());
            cloned.children.clear();
            let new_id = tree.alloc(cloned);
            remap.insert(old, new_id);
        }
        for (old, new) in &remap {
            let Some(source) = self.get(*old) else {
                continue;
            };
            let children = source
                .children
                .iter()
                .filter_map(|child| remap.get(child).copied())
                .collect();
            if let Some(dest) = tree.get_mut(*new) {
                dest.children = children;
            }
        }
        Some(tree)
    }

    /// Children of `id`, largest allocated size first. Used by the inspector
    /// virtual list so the UI can page through a huge directory without
    /// cloning every `NodeView` up front.
    pub fn ranked_child_ids(&self, id: NodeId) -> Vec<NodeId> {
        let mut ids = self
            .get(id)
            .map(|node| node.children.clone())
            .unwrap_or_default();
        ids.sort_unstable_by(|left, right| {
            let left_size = self.get(*left).map_or(0, |node| node.block_size);
            let right_size = self.get(*right).map_or(0, |node| node.block_size);
            right_size.cmp(&left_size).then_with(|| left.cmp(right))
        });
        ids
    }

    /// Recompute a directory and every ancestor. Scan generations only grow,
    /// but this uses exact replacement so it also remains correct for tests and
    /// future incremental invalidation.
    pub fn recompute_upward(&mut self, id: NodeId) {
        let mut cursor = Some(id);
        while let Some(current) = cursor {
            let (parent, totals) = match self.get(current) {
                Some(node) if node.kind.is_dir() => {
                    let mut apparent = 0u64;
                    let mut allocated = 0u64;
                    for child in &node.children {
                        if let Some(child) = self.get(*child) {
                            apparent = apparent.saturating_add(child.size);
                            allocated = allocated.saturating_add(child.block_size);
                        }
                    }
                    (node.parent, Some((apparent, allocated)))
                }
                Some(node) => (node.parent, None),
                None => break,
            };
            if let Some((apparent, allocated)) = totals {
                if let Some(node) = self.get_mut(current) {
                    node.size = apparent;
                    node.block_size = allocated;
                }
            }
            cursor = parent;
        }
    }

    /// Mark a completed leaf and propagate subtree completeness as far upward
    /// as the direct-enumeration and descendant states permit.
    pub fn mark_complete_upward(&mut self, id: NodeId) {
        let mut cursor = Some(id);
        while let Some(current) = cursor {
            let (parent, complete) = match self.get(current) {
                Some(node) => {
                    let direct_done = matches!(
                        node.state,
                        ScanState::Scanned { .. } | ScanState::Error
                    );
                    let descendants_done = node.children.iter().all(|child| {
                        self.get(*child).is_some_and(|child| {
                            !child.kind.is_dir()
                                || child.complete
                                || matches!(child.state, ScanState::Error)
                        })
                    });
                    (node.parent, direct_done && descendants_done)
                }
                None => break,
            };
            if let Some(node) = self.get_mut(current) {
                node.complete = complete;
            }
            if !complete {
                break;
            }
            cursor = parent;
        }
    }

    /// Build only the data the current view can display. This avoids cloning a
    /// million-node arena on every GPUI paint while keeping the cache tree exact.
    pub fn projection(
        &self,
        focus: Option<NodeId>,
        chart_depth: usize,
        branch_limit: usize,
        chart_budget: usize,
        row_limit: usize,
    ) -> TreeProjection {
        let focus = focus.filter(|id| self.get(*id).is_some()).or(self.root);
        let root = self.root().map(NodeView::from);
        let Some(focus_id) = focus else {
            return TreeProjection::empty();
        };

        let mut ancestors = Vec::new();
        let mut cursor = self.get(focus_id);
        while let Some(node) = cursor {
            ancestors.push(NodeView::from(node));
            cursor = node.parent.and_then(|parent| self.get(parent));
        }
        ancestors.reverse();

        let focus_node = self.get(focus_id).map(NodeView::from);
        let mut row_ids = self
            .get(focus_id)
            .map(|node| node.children.clone())
            .unwrap_or_default();
        row_ids.sort_unstable_by(|a, b| {
            let a = self.get(*a).map_or(0, |node| node.block_size);
            let b = self.get(*b).map_or(0, |node| node.block_size);
            b.cmp(&a)
        });
        let omitted_rows = row_ids.len().saturating_sub(row_limit);
        let rows = row_ids
            .into_iter()
            .take(row_limit)
            .filter_map(|id| self.get(id).map(NodeView::from))
            .collect();

        let mut chart = FxHashMap::default();
        let mut queue = VecDeque::from([(focus_id, 0usize)]);
        while let Some((id, depth)) = queue.pop_front() {
            if chart.len() >= chart_budget {
                break;
            }
            let Some(node) = self.get(id) else {
                continue;
            };
            let mut children = Vec::new();
            let mut omitted_size = 0u64;
            let mut omitted_count = 0usize;
            if depth < chart_depth {
                let mut child_ids = node.children.clone();
                child_ids.sort_unstable_by(|a, b| {
                    let a = self.get(*a).map_or(0, |node| node.block_size);
                    let b = self.get(*b).map_or(0, |node| node.block_size);
                    b.cmp(&a)
                });
                for (index, child) in child_ids.into_iter().enumerate() {
                    if index < branch_limit && chart.len() + queue.len() + 1 < chart_budget {
                        children.push(child);
                        queue.push_back((child, depth + 1));
                    } else if let Some(child) = self.get(child) {
                        omitted_size = omitted_size.saturating_add(child.block_size);
                        omitted_count += 1;
                    }
                }
            }
            chart.insert(
                id,
                ChartNode {
                    node: NodeView::from(node),
                    children,
                    omitted_size,
                    omitted_count,
                },
            );
        }

        TreeProjection {
            root,
            focus,
            focus_node,
            ancestors,
            rows,
            omitted_rows,
            chart,
            total_nodes: self.len(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct NodeView {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    pub name: Arc<OsStr>,
    pub kind: Kind,
    pub size: u64,
    pub block_size: u64,
    pub state: ScanState,
    pub complete: bool,
    pub child_count: usize,
}

impl NodeView {
    pub fn display_name(&self) -> String {
        self.name.to_string_lossy().into_owned()
    }
}

impl From<&Node> for NodeView {
    fn from(node: &Node) -> Self {
        Self {
            id: node.id,
            parent: node.parent,
            name: node.name.clone(),
            kind: node.kind,
            size: node.size,
            block_size: node.block_size,
            state: node.state,
            complete: node.complete,
            child_count: node.children.len(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChartNode {
    pub node: NodeView,
    pub children: Vec<NodeId>,
    pub omitted_size: u64,
    pub omitted_count: usize,
}

#[derive(Debug, Clone)]
pub struct TreeProjection {
    pub root: Option<NodeView>,
    pub focus: Option<NodeId>,
    pub focus_node: Option<NodeView>,
    pub ancestors: Vec<NodeView>,
    pub rows: Vec<NodeView>,
    pub omitted_rows: usize,
    pub chart: FxHashMap<NodeId, ChartNode>,
    pub total_nodes: usize,
}

impl TreeProjection {
    pub fn empty() -> Self {
        Self {
            root: None,
            focus: None,
            focus_node: None,
            ancestors: Vec::new(),
            rows: Vec::new(),
            omitted_rows: 0,
            chart: FxHashMap::default(),
            total_nodes: 0,
        }
    }

    pub fn chart_node(&self, id: NodeId) -> Option<&ChartNode> {
        self.chart.get(&id)
    }
}

pub(crate) fn arc_os(value: OsString) -> Arc<OsStr> {
    Arc::<OsStr>::from(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(parent: NodeId, name: &str, size: u64) -> Node {
        Node {
            id: 0,
            parent: Some(parent),
            name: Arc::from(OsStr::new(name)),
            kind: Kind::File,
            size,
            block_size: size,
            mtime: 0,
            inode: size,
            dev: 1,
            state: ScanState::Scanned { child_count: 0 },
            complete: true,
            children: Vec::new(),
        }
    }

    #[test]
    fn recompute_is_exact_when_values_shrink() {
        let mut tree = Tree::new();
        let root = tree.alloc(Node::unscanned_dir(0, None, Arc::from(OsStr::new("/"))));
        tree.root = Some(root);
        let child = tree.alloc(file(root, "file", 100));
        tree.get_mut(root).unwrap().children.push(child);
        tree.recompute_upward(root);
        assert_eq!(tree.root().unwrap().size, 100);
        tree.get_mut(child).unwrap().size = 40;
        tree.get_mut(child).unwrap().block_size = 40;
        tree.recompute_upward(child);
        assert_eq!(tree.root().unwrap().size, 40);
    }

    #[test]
    fn completeness_propagates_from_leaf_directory() {
        let mut tree = Tree::new();
        let root = tree.alloc(Node::unscanned_dir(0, None, Arc::from(OsStr::new("/"))));
        tree.root = Some(root);
        let child = tree.alloc(Node::unscanned_dir(
            0,
            Some(root),
            Arc::from(OsStr::new("a")),
        ));
        tree.get_mut(root).unwrap().children.push(child);
        tree.get_mut(root).unwrap().state = ScanState::Scanned { child_count: 1 };
        tree.get_mut(child).unwrap().state = ScanState::Scanned { child_count: 0 };
        tree.mark_complete_upward(child);
        assert!(tree.get(child).unwrap().complete);
        assert!(tree.root().unwrap().complete);
    }

    #[test]
    fn projection_caps_rows_and_keeps_largest_first() {
        let mut tree = Tree::new();
        let root = tree.alloc(Node::unscanned_dir(0, None, Arc::from(OsStr::new("/"))));
        tree.root = Some(root);
        for (name, size) in [("small", 2), ("large", 9), ("mid", 4)] {
            let id = tree.alloc(file(root, name, size));
            tree.get_mut(root).unwrap().children.push(id);
        }
        tree.recompute_upward(root);
        let projection = tree.projection(Some(root), 3, 32, 128, 2);
        assert_eq!(projection.rows.len(), 2);
        assert_eq!(projection.rows[0].display_name(), "large");
        assert_eq!(projection.omitted_rows, 1);
    }

    #[test]
    fn extract_subtree_remaps_ids_and_root_path() {
        let mut tree = Tree::new();
        let home = tree.alloc(Node::unscanned_dir(
            0,
            None,
            Arc::from(OsStr::new("/Users/ada")),
        ));
        tree.root = Some(home);
        let downloads = tree.alloc(Node::unscanned_dir(
            0,
            Some(home),
            Arc::from(OsStr::new("Downloads")),
        ));
        tree.get_mut(home).unwrap().children.push(downloads);
        let nested = tree.alloc(file(downloads, "movie.mp4", 64));
        tree.get_mut(downloads).unwrap().children.push(nested);
        tree.get_mut(home).unwrap().state = ScanState::Scanned { child_count: 1 };
        tree.get_mut(downloads).unwrap().state = ScanState::Scanned { child_count: 1 };
        tree.recompute_upward(home);
        tree.mark_complete_upward(nested);

        let extracted = tree.extract_subtree(downloads).expect("downloads subtree");
        assert_eq!(extracted.len(), 2);
        let root = extracted.root().unwrap();
        assert!(root.parent.is_none());
        assert_eq!(root.name.as_ref(), OsStr::new("/Users/ada/Downloads"));
        assert_eq!(root.children.len(), 1);
        let child = extracted.get(root.children[0]).unwrap();
        assert_eq!(child.display_name(), "movie.mp4");
        assert_eq!(child.block_size, 64);
        assert_eq!(
            extracted.find_path(Path::new("/Users/ada/Downloads/movie.mp4")),
            Some(root.children[0])
        );
        assert!(extracted.find_path(Path::new("/Users/ada")).is_none());
        assert_eq!(
            extracted.path_of(root.id),
            Some(PathBuf::from("/Users/ada/Downloads"))
        );
        assert_eq!(tree.len(), 3);
    }

    #[test]
    fn find_path_does_not_invent_ancestors() {
        let mut tree = Tree::new();
        let home = tree.alloc(Node::unscanned_dir(
            0,
            None,
            Arc::from(OsStr::new("/Users/ada")),
        ));
        tree.root = Some(home);
        assert!(tree.find_path(Path::new("/Users/ada")).is_some());
        assert!(tree.find_path(Path::new("/Users/ada/Downloads")).is_none());
        assert!(tree.find_path(Path::new("/")).is_none());
    }

    #[test]
    fn unreadable_directory_does_not_block_root_completeness() {
        let mut tree = Tree::new();
        let root = tree.alloc(Node::unscanned_dir(0, None, Arc::from(OsStr::new("/"))));
        tree.root = Some(root);
        let readable = tree.alloc(file(root, "ok", 10));
        let denied = tree.alloc(Node::unscanned_dir(
            0,
            Some(root),
            Arc::from(OsStr::new("secret")),
        ));
        tree.get_mut(root).unwrap().children.extend([readable, denied]);
        tree.get_mut(root).unwrap().state = ScanState::Scanned { child_count: 2 };
        tree.get_mut(denied).unwrap().state = ScanState::Error;
        tree.get_mut(denied).unwrap().complete = true;
        tree.mark_complete_upward(denied);
        assert!(tree.get(denied).unwrap().complete);
        assert!(tree.root().unwrap().complete);
        assert!(tree.is_complete());
    }

    #[test]
    fn ranked_child_ids_are_largest_first() {
        let mut tree = Tree::new();
        let root = tree.alloc(Node::unscanned_dir(0, None, Arc::from(OsStr::new("/"))));
        tree.root = Some(root);
        for (name, size) in [("small", 2u64), ("large", 9), ("mid", 4)] {
            let id = tree.alloc(file(root, name, size));
            tree.get_mut(root).unwrap().children.push(id);
        }
        let ranked = tree.ranked_child_ids(root);
        let names: Vec<String> = ranked
            .iter()
            .map(|id| tree.get(*id).unwrap().display_name())
            .collect();
        assert_eq!(names, ["large", "mid", "small"]);
    }
}

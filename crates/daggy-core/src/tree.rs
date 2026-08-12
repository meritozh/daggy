//! Arena-backed directory tree.
//!
//! Nodes live in a flat `Vec` (a slotmap-style arena) rather than a recursive
//! `Box` tree. This keeps the structure cache-friendly, cheap to clone for UI
//! snapshots, and trivial to serialize flat into the on-disk cache.

use std::sync::Arc;

/// A cheap, stable handle to a node. `u32` so it copies for free through GPUI
/// click handlers.
pub type NodeId = u32;

/// Entry kind. Kept small (a byte) since it's stored per-node in the cache.
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
        matches!(self, Kind::Dir)
    }
}

/// Per-node scan progress. Drives both the UI (shimmer on in-progress dirs)
/// and cache invalidation (only `Unscanned`/dirty subtrees get re-walked).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanState {
    /// Never scanned. Cache hits start here for subtrees that changed.
    Unscanned,
    /// Scan in progress; `child_count` entries found so far.
    Scanning {
        child_count: u32,
    },
    /// Fully scanned; all direct children are present in the arena.
    Scanned {
        child_count: u32,
    },
    /// Could not be read (permissions, broken symlink, …).
    Error,
}

/// A single filesystem entry in the tree.
#[derive(Debug, Clone)]
pub struct Node {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    /// Last path component. The root node carries the full path.
    pub name: Arc<str>,
    pub kind: Kind,
    /// Apparent size in bytes. For directories this is the (lazily recomputed)
    /// sum of descendant sizes.
    pub size: u64,
    /// On-disk allocated size (`st_blocks * 512`).
    pub block_size: u64,
    /// Modification time in seconds since the epoch. Cache invalidation key.
    pub mtime: u64,
    /// Inode number, used to deduplicate hardlinks.
    pub inode: u64,
    /// Device id (combined with inode to identify hardlinks uniquely).
    pub dev: u64,
    pub state: ScanState,
    /// Direct child ids, in scan (i.e. arbitrary) order. Empty until scanned.
    pub children: Vec<NodeId>,
}

impl Node {
    /// Create a directory node that hasn't been scanned yet.
    pub fn unscanned_dir(id: NodeId, parent: Option<NodeId>, name: Arc<str>) -> Self {
        Node {
            id,
            parent,
            name,
            kind: Kind::Dir,
            size: 0,
            block_size: 0,
            mtime: 0,
            inode: 0,
            dev: 0,
            state: ScanState::Unscanned,
            children: Vec::new(),
        }
    }
}

/// The mutable tree, shared behind an `RwLock` by the scanner and UI.
#[derive(Debug, Default)]
pub struct Tree {
    nodes: Vec<Option<Node>>,
    free: Vec<NodeId>,
    /// Index of the root node, if any.
    pub root: Option<NodeId>,
}

impl Tree {
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate a fresh node id, reusing freed slots when possible.
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
        self.nodes.get(id as usize).and_then(|slot| slot.as_ref())
    }

    pub fn get_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(id as usize).and_then(|slot| slot.as_mut())
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

    /// Capacity of the underlying arena (max id ever assigned + 1). Used by
    /// the cache to size its id remap table.
    pub fn cap(&self) -> NodeId {
        self.nodes.len() as NodeId
    }

    /// Iterate over live nodes (in id order, skipping freed slots).
    pub fn iter(&self) -> impl Iterator<Item = &Node> {
        self.nodes.iter().filter_map(|slot| slot.as_ref())
    }

    /// Replace the entire contents of this tree with `other`. Used when the
    /// cache loads a tree to swap it into a scanner's shared tree.
    pub fn load_from(&mut self, other: Tree) {
        self.nodes = other.nodes;
        self.free = other.free;
        self.root = other.root;
    }

    /// Recompute a directory's `size`/`block_size` as the sum of its scanned
    /// children, then propagate the delta up to the root. Called whenever a
    /// subtree finishes scanning. O(depth) per update.
    pub fn recompute_upward(&mut self, id: NodeId) {
        // Sum the node's *scanned* children. Unscanned children contribute
        // nothing yet — their sizes will fold in as they complete.
        let (size, block_size) = {
            let node = match self.get(id) {
                Some(n) => n,
                None => return,
            };
            if !node.kind.is_dir() {
                return;
            }
            let mut size = 0u64;
            let mut block_size = 0u64;
            for &child in &node.children {
                if let Some(c) = self.get(child) {
                    size = size.saturating_add(c.size);
                    block_size = block_size.saturating_add(c.block_size);
                }
            }
            (size, block_size)
        };

        let delta_size;
        let delta_block;
        {
            let node = match self.get_mut(id) {
                Some(n) => n,
                None => return,
            };
            delta_size = size.saturating_sub(node.size);
            delta_block = block_size.saturating_sub(node.block_size);
            node.size = size;
            node.block_size = block_size;
        }

        // Propagate the (non-negative) delta to ancestors. A child completing
        // can only ever grow an ancestor.
        let mut cursor = self.get(id).and_then(|n| n.parent);
        while let Some(pid) = cursor {
            let node = match self.get_mut(pid) {
                Some(n) => n,
                None => break,
            };
            node.size = node.size.saturating_add(delta_size);
            node.block_size = node.block_size.saturating_add(delta_block);
            cursor = node.parent;
        }
    }
}

/// A cheap, coherent snapshot of the tree for the UI to render from. Clones
/// the arena `Arc`-sharing the name strings so even huge trees copy quickly.
#[derive(Debug, Clone)]
pub struct TreeSnapshot {
    pub nodes: Vec<Option<Node>>,
    pub root: Option<NodeId>,
}

impl TreeSnapshot {
    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id as usize).and_then(|slot| slot.as_ref())
    }

    pub fn root(&self) -> Option<&Node> {
        self.root.and_then(|id| self.get(id))
    }

    /// Capacity of the snapshot's node array (max id + 1).
    pub fn cap(&self) -> NodeId {
        self.nodes.len() as NodeId
    }

    pub fn children(&self, id: NodeId) -> impl Iterator<Item = &Node> {
        let ids = self
            .get(id)
            .map(|n| n.children.clone())
            .unwrap_or_default();
        ids.into_iter().filter_map(move |c| self.get(c))
    }
}

impl Tree {
    /// Take a snapshot of the current tree. The nodes vector is cloned, but
    /// each `name` is an `Arc<str>` so the string bytes are shared.
    pub fn snapshot(&self) -> TreeSnapshot {
        TreeSnapshot {
            nodes: self.nodes.clone(),
            root: self.root,
        }
    }
}

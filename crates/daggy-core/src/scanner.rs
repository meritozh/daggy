//! The lazy, parallel directory scanner.
//!
//! Design (see the project README for the rationale):
//!
//! - **Fast enumeration** uses the macOS `getattrlistbulk` syscall (one bulk
//!   call returns many entries *and* their metadata), with a portable
//!   `std::fs` fallback for other platforms.
//! - **Parallelism** is bounded by a 64-wide semaphore so we never spawn
//!   thousands of concurrent operations — keeping CPU low while idle.
//! - **Hardlinks** are deduplicated via a sharded inode set keyed on
//!   `(dev, inode)`.
//! - **Laziness**: `scan_root` populates the top two levels; `expand` walks a
//!   single subtree on demand. Each directory is scanned at most once.
//! - **Progress** flows out through an `smol` channel of `ScanEvent`s that the
//!   UI observes to render incrementally.
//!
//! The recursive walk itself is synchronous (OS threads, not async), which
//! keeps `parking_lot` guards off any async state machine and makes the whole
//! engine straightforwardly `Send`. The `smol` channel bridges worker threads
//! to the GPUI main-thread consumer.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::RwLock;
use smol::channel::{self, Receiver, Sender};

use crate::inode_set::ShardedInodeSet;
use crate::tree::{Kind, Node, NodeId, ScanState, Tree, TreeSnapshot};

/// Maximum number of directories scanned concurrently. 64 matches the fastest
/// macOS disk scanners and keeps the kernel's directory cache warm without
/// thrashing.
const MAX_CONCURRENT_DIRS: usize = 64;

/// A single progress notification emitted by the scanner. The UI subscribes to
/// these and re-renders the affected nodes incrementally.
#[derive(Debug, Clone)]
pub enum ScanEvent {
    /// A directory has finished scanning; its children are now in the tree.
    DirCompleted { id: NodeId, child_count: u32 },
    /// A directory's scan failed (permissions, IO error, …).
    DirError { id: NodeId },
    /// A root scan started (resets progress counters).
    ScanStarted { root: NodeId },
    /// A root scan is fully done.
    ScanFinished { root: NodeId },
    /// Running counters for the status bar.
    Progress {
        dirs_scanned: u64,
        items_total: u64,
    },
}

/// A handle returned to the UI. Holds a receiver for progress events.
pub struct ScannerHandle {
    pub events: Receiver<ScanEvent>,
    /// Set to true when the scan is cancelled (drop the handle to stop).
    pub cancel: Arc<AtomicBool>,
}

impl ScannerHandle {
    /// Request cancellation. In-flight directory reads finish, but no new
    /// subtrees are descended into.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The scanner owns the tree behind an `Arc<RwLock<Tree>>` so the UI can take
/// cheap snapshots while scans run in the background.
pub struct Scanner {
    tree: Arc<RwLock<Tree>>,
    inodes: Arc<ShardedInodeSet>,
    /// Counting semaphore bounding concurrent directory scans.
    sem: Arc<Semaphore>,
}

impl Scanner {
    pub fn new() -> Self {
        Self {
            tree: Arc::new(RwLock::new(Tree::new())),
            inodes: Arc::new(ShardedInodeSet::new()),
            sem: Arc::new(Semaphore::new(MAX_CONCURRENT_DIRS)),
        }
    }

    pub fn tree(&self) -> Arc<RwLock<Tree>> {
        self.tree.clone()
    }

    /// Replace the scanner's tree with a pre-built one (e.g. loaded from the
    /// cache). Used by the UI on startup to restore a previous scan instantly.
    pub fn load_tree(&self, tree: Tree) {
        let mut t = self.tree.write();
        t.load_from(tree);
    }

    /// A cheap coherent snapshot for rendering.
    pub fn snapshot(&self) -> TreeSnapshot {
        self.tree.read().snapshot()
    }

    /// Begin a fresh scan rooted at `path`. Replaces any existing tree.
    ///
    /// Phase 1: scan the root directory and one level of its directory children
    /// so the UI can show something immediately. Deeper subtrees are scanned
    /// lazily via [`Scanner::expand`] when the user drills into them.
    ///
    /// Returns a handle whose `events` receiver yields progress.
    pub fn scan_root(&self, path: PathBuf) -> ScannerHandle {
        let (tx, rx) = channel::unbounded::<ScanEvent>();
        let cancel = Arc::new(AtomicBool::new(false));

        self.inodes.clear();
        {
            let mut tree = self.tree.write();
            *tree = Tree::new();
        }

        let root_id = {
            let mut tree = self.tree.write();
            let name: Arc<str> = path.to_string_lossy().into();
            let id = tree.alloc(Node::unscanned_dir(0, None, name));
            tree.root = Some(id);
            id
        };

        let ctx = ScanCtx {
            tree: self.tree.clone(),
            inodes: self.inodes.clone(),
            sem: self.sem.clone(),
            tx,
            cancel: cancel.clone(),
        };

        // Spawn phase 1 on a worker thread: enumerate the root (level 0) and
        // each of its directory children (level 1), without recursing deeper.
        // The sunburst thus shows two rings immediately; deeper subtrees stay
        // `Unscanned` for lazy `expand`.
        let ctx_phase1 = ctx.clone();
        std::thread::Builder::new()
            .name("daggy-scan".into())
            .spawn(move || {
                let _ = ctx_phase1
                    .tx
                    .try_send(ScanEvent::ScanStarted { root: root_id });
                enumerate_dir(&ctx_phase1, root_id, &path);

                let child_dirs: Vec<(NodeId, PathBuf)> = {
                    let tree = ctx_phase1.tree.read();
                    let Some(root) = tree.get(root_id) else {
                        let _ = ctx_phase1
                            .tx
                            .try_send(ScanEvent::ScanFinished { root: root_id });
                        return;
                    };
                    root.children
                        .iter()
                        .filter_map(|&c| {
                            let n = tree.get(c)?;
                            n.kind.is_dir().then(|| (c, path.join(n.name.as_ref())))
                        })
                        .collect()
                };
                std::thread::scope(|s| {
                    for (cid, cpath) in child_dirs {
                        let ctx = ctx_phase1.clone();
                        s.spawn(move || enumerate_dir(&ctx, cid, &cpath));
                    }
                });

                let _ = ctx_phase1
                    .tx
                    .try_send(ScanEvent::ScanFinished { root: root_id });
            })
            .expect("spawn scan thread");

        ScannerHandle { events: rx, cancel }
    }

    /// Lazily scan a single directory subtree. Called when the user expands a
    /// node in the UI. No-op if the node is already scanned/scanning.
    pub fn expand(&self, node: NodeId) -> ScannerHandle {
        let (tx, rx) = channel::unbounded::<ScanEvent>();
        let cancel = Arc::new(AtomicBool::new(false));

        let path = {
            let tree = self.tree.read();
            tree.get(node).map(|n| path_of(&tree, n))
        };

        let ctx = ScanCtx {
            tree: self.tree.clone(),
            inodes: self.inodes.clone(),
            sem: self.sem.clone(),
            tx,
            cancel: cancel.clone(),
        };

        if let Some(path) = path {
            std::thread::Builder::new()
                .name("daggy-expand".into())
                .spawn(move || walk_subtree(&ctx, node, path))
                .expect("spawn expand thread");
        }

        ScannerHandle { events: rx, cancel }
    }
}

impl Default for Scanner {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared, clonable scan context handed to every worker.
#[derive(Clone)]
struct ScanCtx {
    tree: Arc<RwLock<Tree>>,
    inodes: Arc<ShardedInodeSet>,
    sem: Arc<Semaphore>,
    tx: Sender<ScanEvent>,
    cancel: Arc<AtomicBool>,
}

/// Enumerate one directory's direct children and insert them into the tree.
/// Idempotent: a no-op if the directory is already `Scanned` or `Scanning`.
/// Returns the `(child_dir_id, child_dir_path)` pairs that were found, in
/// addition to emitting progress events.
fn enumerate_dir(ctx: &ScanCtx, dir_id: NodeId, dir_path: &Path) {
    if ctx.cancel.load(Ordering::Relaxed) {
        return;
    }

    // Guard: only enumerate unscanned dirs; mark as scanning.
    {
        let mut tree = ctx.tree.write();
        let Some(node) = tree.get_mut(dir_id) else {
            return;
        };
        if matches!(node.state, ScanState::Scanned { .. } | ScanState::Scanning { .. }) {
            return;
        }
        node.state = ScanState::Scanning { child_count: 0 };
    }

    // Bound concurrency: hold one permit for the duration of enumeration.
    let _permit = ctx.sem.acquire();

    let dev = device_of(dir_path).unwrap_or(0);
    let entries = match list_dir(dir_path) {
        Ok(e) => e,
        Err(err) => {
            log::debug!("enumerate_dir {:?} failed: {err}", dir_path);
            let mut tree = ctx.tree.write();
            if let Some(node) = tree.get_mut(dir_id) {
                node.state = ScanState::Error;
            }
            let _ = ctx.tx.try_send(ScanEvent::DirError { id: dir_id });
            return;
        }
    };

    let mut items_total: u64 = 0;
    let child_count;
    {
        let mut tree = ctx.tree.write();
        for entry in &entries {
            items_total += 1;
            let kind = entry.kind;
            // Hardlink dedup: count a file's size only the first time we see
            // its (dev, inode). Directories always count.
            let counted = if kind.is_dir() {
                true
            } else {
                ctx.inodes.insert(dev, entry.inode)
            };
            let size = if counted { entry.size } else { 0 };
            let block_size = if counted { entry.alloc_size } else { 0 };

            let child = Node {
                id: 0,
                parent: Some(dir_id),
                name: entry.name.clone(),
                kind,
                size,
                block_size,
                mtime: entry.mtime,
                inode: entry.inode,
                dev,
                state: if kind.is_dir() {
                    ScanState::Unscanned
                } else {
                    ScanState::Scanned { child_count: 0 }
                },
                children: Vec::new(),
            };
            let cid = tree.alloc(child);
            tree.get_mut(dir_id)
                .expect("dir exists")
                .children
                .push(cid);
        }
        let node = tree.get_mut(dir_id).expect("dir exists");
        child_count = node.children.len() as u32;
        node.state = ScanState::Scanned { child_count };
        if let Some(meta) = dir_meta(dir_path) {
            node.mtime = meta.mtime;
            node.inode = meta.inode;
            node.dev = meta.dev;
        }
        tree.recompute_upward(dir_id);
    }

    let _ = ctx
        .tx
        .try_send(ScanEvent::DirCompleted { id: dir_id, child_count });
    let _ = ctx
        .tx
        .try_send(ScanEvent::Progress { dirs_scanned: 1, items_total });
}

/// Walk a subtree: enumerate `dir_id` (if needed), then recurse into every
/// still-unscanned child directory in parallel via scoped threads.
fn walk_subtree(ctx: &ScanCtx, dir_id: NodeId, dir_path: PathBuf) {
    if ctx.cancel.load(Ordering::Relaxed) {
        return;
    }

    // Make sure this directory's direct children are enumerated.
    enumerate_dir(ctx, dir_id, &dir_path);

    // Collect the child directories that still need descending into.
    let child_dirs: Vec<(NodeId, PathBuf)> = {
        let tree = ctx.tree.read();
        let Some(node) = tree.get(dir_id) else {
            return;
        };
        node.children
            .iter()
            .filter_map(|&c| {
                let n = tree.get(c)?;
                // Only descend into dirs we haven't walked yet.
                if n.kind.is_dir() && matches!(n.state, ScanState::Unscanned) {
                    Some((c, dir_path.join(n.name.as_ref())))
                } else {
                    None
                }
            })
            .collect()
    };

    if child_dirs.is_empty() {
        return;
    }

    std::thread::scope(|s| {
        for (cid, cpath) in child_dirs {
            let ctx = ctx.clone();
            s.spawn(move || walk_subtree(&ctx, cid, cpath));
        }
    });
}

/// Reconstruct a node's absolute path by walking parents.
fn path_of(tree: &Tree, node: &Node) -> PathBuf {
    let mut comps: Vec<Arc<str>> = Vec::new();
    let mut cursor = Some(node);
    while let Some(cur) = cursor {
        comps.push(cur.name.clone());
        cursor = cur.parent.and_then(|p| tree.get(p));
    }
    let mut path = PathBuf::new();
    for name in comps.into_iter().rev() {
        path.push(name.as_ref());
    }
    path
}

// ---------------------------------------------------------------------------
// Counting semaphore (blocking, OS-thread safe).
// ---------------------------------------------------------------------------

/// A blocking counting semaphore bounding concurrent work. Uses the std mutex
/// + condvar pair so the condvar and mutex are compatible.
struct Semaphore {
    state: std::sync::Mutex<usize>,
    cvar: std::sync::Condvar,
}

struct PermitGuard<'a> {
    sem: &'a Semaphore,
}

impl Drop for PermitGuard<'_> {
    fn drop(&mut self) {
        self.sem.release();
    }
}

impl Semaphore {
    fn new(count: usize) -> Self {
        Self {
            state: std::sync::Mutex::new(count),
            cvar: std::sync::Condvar::new(),
        }
    }

    /// Block until a permit is available, then return a guard that releases it
    /// on drop.
    fn acquire(&self) -> PermitGuard<'_> {
        let mut count = self.state.lock().unwrap();
        while *count == 0 {
            count = self.cvar.wait(count).unwrap();
        }
        *count -= 1;
        PermitGuard { sem: self }
    }

    fn release(&self) {
        let mut count = self.state.lock().unwrap();
        *count += 1;
        self.cvar.notify_one();
    }
}

// ---------------------------------------------------------------------------
// Platform directory enumeration.
// ---------------------------------------------------------------------------

/// A normalized entry returned by both the fast macOS path and the fallback.
struct RawEntry {
    name: Arc<str>,
    kind: Kind,
    size: u64,
    alloc_size: u64,
    mtime: u64,
    inode: u64,
}

struct DirMeta {
    mtime: u64,
    inode: u64,
    dev: u64,
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use getattrlistbulk::{ObjectType, RequestedAttributes};
    use std::time::SystemTime;

    pub fn list_dir(dir: &Path) -> std::io::Result<Vec<RawEntry>> {
        let attrs = RequestedAttributes {
            name: true,
            object_type: true,
            size: true,
            alloc_size: true,
            modified_time: true,
            inode: true,
            ..Default::default()
        };
        let reader = getattrlistbulk::read_dir_with_buffer(dir, attrs, 128 * 1024)
            .map_err(io_from_attr_error)?;
        let mut out = Vec::new();
        for entry in reader {
            let entry = entry.map_err(io_from_attr_error)?;
            out.push(RawEntry {
                name: entry.name.into(),
                kind: kind_of(entry.object_type),
                size: entry.size.unwrap_or(0),
                alloc_size: entry.alloc_size.unwrap_or_else(|| entry.size.unwrap_or(0)),
                mtime: entry
                    .modified_time
                    .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
                inode: entry.inode.unwrap_or(0),
            });
        }
        Ok(out)
    }

    fn kind_of(t: Option<ObjectType>) -> Kind {
        match t {
            Some(ObjectType::Directory) => Kind::Dir,
            Some(ObjectType::Regular) => Kind::File,
            Some(ObjectType::Symlink) => Kind::Symlink,
            _ => Kind::Other,
        }
    }

    fn io_from_attr_error(e: getattrlistbulk::Error) -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::Other, format!("getattrlistbulk: {e}"))
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::*;

    pub fn list_dir(dir: &Path) -> std::io::Result<Vec<RawEntry>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let meta = entry.metadata()?;
            use std::os::unix::fs::MetadataExt;
            let kind = if meta.file_type().is_dir() {
                Kind::Dir
            } else if meta.file_type().is_symlink() {
                Kind::Symlink
            } else if meta.file_type().is_file() {
                Kind::File
            } else {
                Kind::Other
            };
            out.push(RawEntry {
                name: entry.file_name().to_string_lossy().into(),
                kind,
                size: meta.len(),
                alloc_size: meta.blocks() * 512,
                mtime: meta.mtime() as u64,
                inode: meta.ino(),
            });
        }
        Ok(out)
    }
}

/// List the direct children of `dir` using the fastest available path.
fn list_dir(dir: &Path) -> std::io::Result<Vec<RawEntry>> {
    imp::list_dir(dir)
}

/// Stat the directory itself for mtime/inode/dev (used for cache invalidation).
fn dir_meta(dir: &Path) -> Option<DirMeta> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(dir).ok()?;
    Some(DirMeta {
        mtime: meta.mtime() as u64,
        inode: meta.ino(),
        dev: meta.dev(),
    })
}

/// Device id of a path (for hardlink keying).
fn device_of(dir: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(std::fs::symlink_metadata(dir).ok()?.dev())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Build a small fixture tree under a temp dir and return its path. Each
    /// call gets a unique directory so concurrent tests don't collide.
    fn fixture() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "daggy-test-{}-{}-{}",
            std::process::id(),
            n,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        fs::create_dir_all(dir.join("a/b")).unwrap();
        fs::create_dir_all(dir.join("c")).unwrap();
        fs::write(dir.join("root.txt"), vec![0u8; 1024]).unwrap();
        fs::write(dir.join("a/f1.bin"), vec![0u8; 2048]).unwrap();
        fs::write(dir.join("a/b/f2.bin"), vec![0u8; 4096]).unwrap();
        dir
    }

    /// Drain `events` until a `ScanFinished` arrives (or channel closes).
    fn drain_until_finished(handle: &ScannerHandle) {
        while let Ok(ev) = handle.events.recv_blocking() {
            if matches!(ev, ScanEvent::ScanFinished { .. }) {
                break;
            }
        }
    }

    #[test]
    fn scans_root_and_sums_sizes() {
        let dir = fixture();
        let scanner = Scanner::new();
        let handle = scanner.scan_root(dir.clone());
        drain_until_finished(&handle);

        let snap = scanner.snapshot();
        let root = snap.root().expect("root exists");
        // Phase 1 scans root + one level (a, c), non-recursively. So:
        //   root.txt (1024) + a/f1.bin (2048) = 3072
        // The deeper a/b/f2.bin (4096) is NOT scanned yet (lazy).
        assert_eq!(
            root.size,
            1024 + 2048,
            "phase-1 root size sums scanned files only"
        );

        // Expanding "a" descends into "b", adding f2.bin (4096). After that the
        // root total grows to 3072 + 4096 = 7168.
        let a_id = {
            let snap = scanner.snapshot();
            let mut id = None;
            for n in snap.children(snap.root.unwrap()) {
                if n.name.as_ref() == "a" {
                    id = Some(n.id);
                    break;
                }
            }
            id.expect("dir a exists")
        };
        // Expand "a": this walks the whole subtree (a → b → f2.bin). The worker
        // drops its sender when done, closing the channel.
        let h2 = scanner.expand(a_id);
        while h2.events.recv_blocking().is_ok() {}
        let root_after = scanner.snapshot().root().expect("root exists").size;
        assert_eq!(root_after, 1024 + 2048 + 4096, "root size after full expand");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn expand_descends_lazily() {
        let dir = fixture();
        let scanner = Scanner::new();
        let handle = scanner.scan_root(dir.clone());
        drain_until_finished(&handle);

        // After phase 1, "a" is scanned but its grandchild "a/b" should be
        // Unscanned (we only descended two levels: root + a/c).
        let a_id = {
            let snap = scanner.snapshot();
            let mut id = None;
            for n in snap.children(snap.root.unwrap()) {
                if n.name.as_ref() == "a" {
                    id = Some(n.id);
                    break;
                }
            }
            id.expect("dir a exists")
        };
        assert!(
            matches!(scanner.snapshot().get(a_id).unwrap().state, ScanState::Scanned { .. }),
            "dir a scanned in phase 1"
        );

        // Expanding "a" walks into "b", which should now be scanned and
        // contain f2.bin (4096 bytes).
        let h2 = scanner.expand(a_id);
        while let Ok(ev) = h2.events.recv_blocking() {
            // expand descends the whole subtree; break once "a" itself reports
            // done.
            if let ScanEvent::DirCompleted { id, .. } = &ev {
                if *id == a_id {
                    break;
                }
            }
        }

        let snap = scanner.snapshot();
        let b = snap
            .children(a_id)
            .find(|n| n.name.as_ref() == "b")
            .expect("dir b exists");
        assert!(
            matches!(b.state, ScanState::Scanned { .. }),
            "dir b scanned after expand"
        );
        assert_eq!(b.size, 4096, "b sums f2.bin");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancel_stops_descent() {
        let dir = fixture();
        let scanner = Scanner::new();
        let handle = scanner.scan_root(dir.clone());
        handle.cancel();
        // Drain whatever was emitted; must not hang.
        while handle.events.recv_blocking().is_ok() {}
        let _ = fs::remove_dir_all(&dir);
    }
}

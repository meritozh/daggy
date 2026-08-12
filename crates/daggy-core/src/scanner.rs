//! Visible-first, background-complete disk scanner.
//!
//! One coordinator owns each immutable scan generation. A fixed worker pool
//! pulls from urgent and background queues, so a click can promote the visible
//! subtree without creating recursive threads. UI notifications are bounded
//! and coalesced; the exact tree remains behind a read lock for cache export.

use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use smol::channel::{self, Receiver, Sender};

use crate::inode_set::ShardedInodeSet;
use crate::tree::{arc_os, Kind, Node, NodeId, ScanState, Tree, TreeProjection};

const MAX_WORKERS: usize = 4;
const MAX_BACKGROUND_WORKERS: usize = 2;
const INITIAL_VISIBLE_DEPTH: u8 = 2;
const EVENT_CAPACITY: usize = 256;
const PROGRESS_EVERY_DIRS: u64 = 32;

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
pub enum ScanEvent {
    ScanStarted {
        root: NodeId,
    },
    /// The root's direct children exist, so a live first ring can be painted.
    VisibleReady {
        root: NodeId,
    },
    /// Absolute counters, safe even when intermediate notifications are dropped.
    Progress {
        dirs_scanned: u64,
        items_seen: u64,
        errors: u64,
    },
    ScanFinished {
        root: NodeId,
        summary: ScanSummary,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct ScanSummary {
    pub dirs_scanned: u64,
    pub items_seen: u64,
    pub errors: u64,
    pub elapsed: Duration,
    pub complete: bool,
    pub workers: usize,
}

pub struct ScannerHandle {
    pub generation: u64,
    pub events: Receiver<ScanEvent>,
}

pub struct Scanner {
    current: Mutex<Current>,
    worker_count: usize,
}

struct Current {
    tree: Arc<RwLock<Tree>>,
    session: Option<Arc<Session>>,
}

impl Scanner {
    pub fn new() -> Self {
        Self {
            current: Mutex::new(Current {
                tree: Arc::new(RwLock::new(Tree::new())),
                session: None,
            }),
            worker_count: available_workers(),
        }
    }

    /// Override the bounded pool size for benchmarks or power-sensitive use.
    /// Values are clamped to the same conservative production ceiling.
    pub fn with_worker_count(worker_count: usize) -> Self {
        Self {
            current: Mutex::new(Current {
                tree: Arc::new(RwLock::new(Tree::new())),
                session: None,
            }),
            worker_count: worker_count.clamp(1, MAX_WORKERS),
        }
    }

    /// Start a new generation and cancel the old one. Old workers retain their
    /// old tree, so late writes can never corrupt the newly selected root.
    pub fn scan_root(&self, path: PathBuf) -> ScannerHandle {
        let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
        let (events_tx, events_rx) = channel::bounded(EVENT_CAPACITY);
        let mut tree = Tree::new();
        let root_name = arc_os(path.as_os_str().to_os_string());
        let root = tree.alloc(Node::unscanned_dir(0, None, root_name));
        tree.root = Some(root);
        if let Some(metadata) = dir_metadata(&path) {
            let node = tree.get_mut(root).expect("new root exists");
            node.mtime = metadata.mtime;
            node.inode = metadata.inode;
            node.dev = metadata.dev;
        }
        let tree = Arc::new(RwLock::new(tree));
        let workers = self.worker_count;
        let session = Arc::new(Session::new(root, tree.clone(), events_tx, workers));
        session.enqueue(Job {
            id: root,
            path,
            priority_depth: INITIAL_VISIBLE_DEPTH,
        });

        {
            let mut current = self.current.lock().expect("scanner lock poisoned");
            if let Some(old) = current.session.take() {
                old.cancel();
            }
            current.tree = tree;
            current.session = Some(session.clone());
        }

        let _ = session
            .events
            .send_blocking(ScanEvent::ScanStarted { root });
        for index in 0..workers {
            let session = session.clone();
            thread::Builder::new()
                .name(format!("daggy-scan-{index}"))
                .spawn(move || worker_loop(session))
                .expect("spawn bounded scan worker");
        }

        ScannerHandle {
            generation,
            events: events_rx,
        }
    }

    pub fn cancel_current(&self) {
        let session = self
            .current
            .lock()
            .expect("scanner lock poisoned")
            .session
            .clone();
        if let Some(session) = session {
            session.cancel();
        }
    }

    /// Promote the focused directory and its next two visible layers.
    pub fn prioritize(&self, node: NodeId) {
        let (tree, session) = {
            let current = self.current.lock().expect("scanner lock poisoned");
            (current.tree.clone(), current.session.clone())
        };
        let Some(session) = session else {
            return;
        };
        let job = {
            let tree = tree.read();
            let Some(node_ref) = tree.get(node) else {
                return;
            };
            if !node_ref.kind.is_dir() || node_ref.complete {
                return;
            }
            tree.path_of(node).map(|path| Job {
                id: node,
                path,
                priority_depth: INITIAL_VISIBLE_DEPTH,
            })
        };
        if let Some(job) = job {
            session.enqueue(job);
        }
    }

    /// Prioritize an exact cached/displayed path even when the fresh
    /// generation has not discovered its deepest node yet.
    pub fn prioritize_path(&self, path: PathBuf) {
        let (tree, session) = {
            let current = self.current.lock().expect("scanner lock poisoned");
            (current.tree.clone(), current.session.clone())
        };
        let Some(session) = session else {
            return;
        };
        session.prefer(path.clone());
        let job = {
            let tree = tree.read();
            tree.find_path(&path).and_then(|id| {
                let node = tree.get(id)?;
                (!node.complete && node.kind.is_dir()).then_some(Job {
                    id,
                    path,
                    priority_depth: INITIAL_VISIBLE_DEPTH,
                })
            })
        };
        if let Some(job) = job {
            session.enqueue(job);
        }
    }

    pub fn projection(&self, focus: Option<NodeId>) -> TreeProjection {
        let tree = self.tree_handle();
        let projection = tree.read().projection(focus, 3, 32, 4096, 2000);
        projection
    }

    pub fn tree_handle(&self) -> Arc<RwLock<Tree>> {
        self.current
            .lock()
            .expect("scanner lock poisoned")
            .tree
            .clone()
    }

    pub fn root_id(&self) -> Option<NodeId> {
        self.tree_handle().read().root
    }
}

impl Default for Scanner {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Scanner {
    fn drop(&mut self) {
        if let Ok(current) = self.current.lock() {
            if let Some(session) = &current.session {
                session.cancel();
            }
        }
    }
}

#[derive(Clone)]
struct Job {
    id: NodeId,
    path: PathBuf,
    priority_depth: u8,
}

struct QueueState {
    jobs: HashMap<NodeId, Job>,
    running: HashMap<NodeId, u8>,
    urgent: VecDeque<NodeId>,
    background: VecDeque<NodeId>,
    pending: usize,
    background_running: usize,
    closed: bool,
}

struct JobLease {
    job: Job,
    background: bool,
}

struct Session {
    root: NodeId,
    tree: Arc<RwLock<Tree>>,
    inodes: ShardedInodeSet,
    queue: Mutex<QueueState>,
    wake: Condvar,
    cancelled: AtomicBool,
    events: Sender<ScanEvent>,
    dirs_scanned: AtomicU64,
    items_seen: AtomicU64,
    errors: AtomicU64,
    started: Instant,
    workers: usize,
    preferred_path: Mutex<Option<PathBuf>>,
}

impl Session {
    fn new(
        root: NodeId,
        tree: Arc<RwLock<Tree>>,
        events: Sender<ScanEvent>,
        workers: usize,
    ) -> Self {
        Self {
            root,
            tree,
            inodes: ShardedInodeSet::new(),
            queue: Mutex::new(QueueState {
                jobs: HashMap::new(),
                running: HashMap::new(),
                urgent: VecDeque::new(),
                background: VecDeque::new(),
                pending: 0,
                background_running: 0,
                closed: false,
            }),
            wake: Condvar::new(),
            cancelled: AtomicBool::new(false),
            events,
            dirs_scanned: AtomicU64::new(0),
            items_seen: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            started: Instant::now(),
            workers,
            preferred_path: Mutex::new(None),
        }
    }

    fn enqueue(&self, job: Job) {
        if self.cancelled.load(Ordering::Acquire) {
            return;
        }
        let mut queue = self.queue.lock().expect("queue lock poisoned");
        if queue.closed {
            return;
        }
        if let Some(priority) = queue.running.get_mut(&job.id) {
            *priority = (*priority).max(job.priority_depth);
            return;
        }
        if let Some(existing) = queue.jobs.get_mut(&job.id) {
            if job.priority_depth > existing.priority_depth {
                existing.priority_depth = job.priority_depth;
                existing.path = job.path;
                queue.urgent.push_back(job.id);
                self.wake.notify_one();
            }
            return;
        }
        let urgent = job.priority_depth > 0;
        let id = job.id;
        queue.jobs.insert(id, job);
        if urgent {
            queue.urgent.push_back(id);
        } else {
            queue.background.push_back(id);
        }
        queue.pending += 1;
        self.wake.notify_one();
    }

    fn next_job(&self) -> Option<JobLease> {
        let mut queue = self.queue.lock().expect("queue lock poisoned");
        loop {
            if queue.closed || self.cancelled.load(Ordering::Acquire) {
                return None;
            }
            while let Some(id) = queue.urgent.pop_front() {
                if let Some(job) = queue.jobs.remove(&id) {
                    queue.running.insert(id, job.priority_depth);
                    return Some(JobLease {
                        job,
                        background: false,
                    });
                }
            }
            if queue.background_running < self.workers.min(MAX_BACKGROUND_WORKERS) {
                while let Some(id) = queue.background.pop_front() {
                    if let Some(job) = queue.jobs.remove(&id) {
                        queue.background_running += 1;
                        queue.running.insert(id, job.priority_depth);
                        return Some(JobLease {
                            job,
                            background: true,
                        });
                    }
                }
            }
            queue = self.wake.wait(queue).expect("queue lock poisoned");
        }
    }

    fn running_priority(&self, id: NodeId) -> u8 {
        self.queue
            .lock()
            .expect("queue lock poisoned")
            .running
            .get(&id)
            .copied()
            .unwrap_or(0)
    }

    fn finish_job(&self, id: NodeId, background: bool) -> bool {
        let mut queue = self.queue.lock().expect("queue lock poisoned");
        queue.running.remove(&id);
        if background {
            queue.background_running = queue.background_running.saturating_sub(1);
        }
        if queue.pending > 0 {
            queue.pending -= 1;
        }
        if queue.pending == 0 && !self.cancelled.load(Ordering::Acquire) {
            queue.closed = true;
            self.wake.notify_all();
            true
        } else {
            self.wake.notify_all();
            false
        }
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        let mut queue = self.queue.lock().expect("queue lock poisoned");
        queue.closed = true;
        queue.jobs.clear();
        queue.running.clear();
        queue.urgent.clear();
        queue.background.clear();
        self.wake.notify_all();
    }

    fn prefer(&self, path: PathBuf) {
        *self
            .preferred_path
            .lock()
            .expect("preferred path lock poisoned") = Some(path);
    }

    fn priority_for_path(&self, path: &Path) -> u8 {
        if self
            .preferred_path
            .lock()
            .expect("preferred path lock poisoned")
            .as_ref()
            .is_some_and(|preferred| preferred.starts_with(path))
        {
            INITIAL_VISIBLE_DEPTH
        } else {
            0
        }
    }

    fn emit_progress(&self, force: bool) {
        let dirs = self.dirs_scanned.load(Ordering::Relaxed);
        if force || dirs.is_multiple_of(PROGRESS_EVERY_DIRS) {
            let _ = self.events.try_send(ScanEvent::Progress {
                dirs_scanned: dirs,
                items_seen: self.items_seen.load(Ordering::Relaxed),
                errors: self.errors.load(Ordering::Relaxed),
            });
        }
    }
}

fn available_workers() -> usize {
    thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(2)
        .clamp(1, MAX_WORKERS)
}

fn worker_loop(session: Arc<Session>) {
    while let Some(lease) = session.next_job() {
        let job = lease.job;
        if session.cancelled.load(Ordering::Acquire) {
            break;
        }
        let children = enumerate_directory(&session, &job);
        let next_priority = session
            .running_priority(job.id)
            .max(job.priority_depth)
            .saturating_sub(1);
        for (id, path) in children {
            session.enqueue(Job {
                id,
                priority_depth: next_priority.max(session.priority_for_path(&path)),
                path,
            });
        }

        if session.finish_job(job.id, lease.background) {
            session.emit_progress(true);
            let complete = session.tree.read().is_complete();
            let summary = ScanSummary {
                dirs_scanned: session.dirs_scanned.load(Ordering::Relaxed),
                items_seen: session.items_seen.load(Ordering::Relaxed),
                errors: session.errors.load(Ordering::Relaxed),
                elapsed: session.started.elapsed(),
                complete,
                workers: session.workers,
            };
            let _ = session.events.send_blocking(ScanEvent::ScanFinished {
                root: session.root,
                summary,
            });
            break;
        }
    }
}

fn enumerate_directory(session: &Session, job: &Job) -> Vec<(NodeId, PathBuf)> {
    {
        let mut tree = session.tree.write();
        let Some(node) = tree.get_mut(job.id) else {
            return Vec::new();
        };
        if !matches!(node.state, ScanState::Unscanned) {
            return Vec::new();
        }
        node.state = ScanState::Scanning { child_count: 0 };
    }

    let entries = match list_dir(&job.path) {
        Ok(entries) => entries,
        Err(error) => {
            log::debug!("scan {:?} failed: {error}", job.path);
            session.errors.fetch_add(1, Ordering::Relaxed);
            let mut tree = session.tree.write();
            if let Some(node) = tree.get_mut(job.id) {
                node.state = ScanState::Error;
            }
            session.emit_progress(true);
            return Vec::new();
        }
    };

    let item_count = entries.len() as u64;
    let mut child_directories = Vec::new();
    {
        let mut tree = session.tree.write();
        for entry in entries {
            let counts = entry.kind != Kind::File
                || entry.link_count <= 1
                || entry.dev == 0
                || entry.inode == 0
                || session.inodes.insert(entry.dev, entry.inode);
            let apparent = if counts { entry.size } else { 0 };
            let allocated = if counts { entry.alloc_size } else { 0 };
            let is_dir = entry.kind.is_dir();
            let name = arc_os(entry.name.clone());
            let child = Node {
                id: 0,
                parent: Some(job.id),
                name,
                kind: entry.kind,
                size: apparent,
                block_size: allocated,
                mtime: entry.mtime,
                inode: entry.inode,
                dev: entry.dev,
                state: if is_dir {
                    ScanState::Unscanned
                } else {
                    ScanState::Scanned { child_count: 0 }
                },
                complete: !is_dir,
                children: Vec::new(),
            };
            let child_id = tree.alloc(child);
            tree.get_mut(job.id)
                .expect("enumerated directory exists")
                .children
                .push(child_id);
            if is_dir {
                child_directories.push((child_id, job.path.join(entry.name)));
            }
        }
        if let Some(metadata) = dir_metadata(&job.path) {
            let node = tree.get_mut(job.id).expect("enumerated directory exists");
            node.mtime = metadata.mtime;
            node.inode = metadata.inode;
            node.dev = metadata.dev;
        }
        let child_count = tree
            .get(job.id)
            .map_or(0, |node| node.children.len() as u32);
        tree.get_mut(job.id)
            .expect("enumerated directory exists")
            .state = ScanState::Scanned { child_count };
        tree.recompute_upward(job.id);
        if child_directories.is_empty() {
            tree.mark_complete_upward(job.id);
        }
    }

    session.items_seen.fetch_add(item_count, Ordering::Relaxed);
    let dirs = session.dirs_scanned.fetch_add(1, Ordering::Relaxed) + 1;
    if job.id == session.root {
        let _ = session
            .events
            .try_send(ScanEvent::VisibleReady { root: session.root });
    }
    session.emit_progress(dirs == 1);
    child_directories
}

pub(crate) struct RawEntry {
    pub(crate) name: OsString,
    pub(crate) kind: Kind,
    pub(crate) size: u64,
    pub(crate) alloc_size: u64,
    pub(crate) mtime: i64,
    pub(crate) inode: u64,
    pub(crate) dev: u64,
    pub(crate) link_count: u32,
}

struct DirMetadata {
    mtime: i64,
    inode: u64,
    dev: u64,
}

#[cfg(target_os = "macos")]
fn list_dir(path: &Path) -> io::Result<Vec<RawEntry>> {
    match crate::macos_bulk::read_dir(path) {
        Ok(entries) => Ok(entries),
        Err(error) if should_fallback(&error) => {
            log::debug!("bulk enumeration unavailable for {:?}: {error}", path);
            list_dir_portable(path)
        }
        Err(error) => Err(error),
    }
}

#[cfg(target_os = "macos")]
fn should_fallback(error: &io::Error) -> bool {
    error
        .raw_os_error()
        .is_some_and(|code| matches!(code, libc::ENOTSUP | libc::EINVAL | libc::ENOSYS))
        || matches!(
            error.kind(),
            io::ErrorKind::Unsupported | io::ErrorKind::InvalidData
        )
}

#[cfg(not(target_os = "macos"))]
fn list_dir(path: &Path) -> io::Result<Vec<RawEntry>> {
    list_dir_portable(path)
}

fn list_dir_portable(path: &Path) -> io::Result<Vec<RawEntry>> {
    use std::os::unix::fs::MetadataExt;

    let mut entries = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let metadata = std::fs::symlink_metadata(entry.path())?;
        let file_type = metadata.file_type();
        let kind = if file_type.is_dir() {
            Kind::Dir
        } else if file_type.is_file() {
            Kind::File
        } else if file_type.is_symlink() {
            Kind::Symlink
        } else {
            Kind::Other
        };
        entries.push(RawEntry {
            name: entry.file_name(),
            kind,
            size: metadata.len(),
            alloc_size: metadata.blocks().saturating_mul(512),
            mtime: metadata.mtime(),
            inode: metadata.ino(),
            dev: metadata.dev(),
            link_count: metadata.nlink().min(u32::MAX as u64) as u32,
        });
    }
    Ok(entries)
}

fn dir_metadata(path: &Path) -> Option<DirMetadata> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path).ok()?;
    Some(DirMetadata {
        mtime: metadata.mtime(),
        inode: metadata.ino(),
        dev: metadata.dev(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "daggy-scan-{label}-{}-{}",
            std::process::id(),
            NEXT_GENERATION.load(Ordering::Relaxed)
        ))
    }

    fn wait(scanner: &Scanner, path: &Path) -> ScanSummary {
        let handle = scanner.scan_root(path.to_path_buf());
        while let Ok(event) = handle.events.recv_blocking() {
            if let ScanEvent::ScanFinished { summary, .. } = event {
                return summary;
            }
        }
        panic!("scan channel closed before completion")
    }

    #[test]
    fn full_background_scan_produces_exact_complete_tree() {
        let root = temp_root("complete");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("one"), vec![1; 2000]).unwrap();
        std::fs::write(root.join("a/two"), vec![2; 3000]).unwrap();
        std::fs::write(root.join("a/b/three"), vec![3; 4000]).unwrap();

        let scanner = Scanner::new();
        let summary = wait(&scanner, &root);
        let tree = scanner.tree_handle();
        let tree = tree.read();
        assert!(summary.complete);
        assert_eq!(summary.errors, 0);
        assert!(summary.workers <= MAX_WORKERS);
        assert_eq!(summary.items_seen, 5);
        assert!(tree.is_complete());
        assert_eq!(tree.len(), 6);
        assert!(tree.root().unwrap().size >= 9000);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn symlinked_directory_is_not_descended_into() {
        let root = temp_root("symlink");
        let outside = temp_root("outside");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("large"), vec![1; 8192]).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        let scanner = Scanner::new();
        let summary = wait(&scanner, &root);
        assert_eq!(summary.dirs_scanned, 1);
        assert_eq!(scanner.tree_handle().read().len(), 2);
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn hardlinks_contribute_allocated_bytes_once() {
        let root = temp_root("hardlink");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("first"), vec![7; 4096]).unwrap();
        std::fs::hard_link(root.join("first"), root.join("second")).unwrap();
        let scanner = Scanner::new();
        let summary = wait(&scanner, &root);
        let tree = scanner.tree_handle();
        let tree = tree.read();
        assert!(summary.complete);
        assert_eq!(summary.items_seen, 2);
        assert_eq!(tree.root().unwrap().size, 4096);
        assert_eq!(tree.root().unwrap().block_size, 4096);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn new_generation_is_isolated_from_cancelled_workers() {
        let first = temp_root("first");
        let second = temp_root("second");
        let _ = std::fs::remove_dir_all(&first);
        let _ = std::fs::remove_dir_all(&second);
        for index in 0..80 {
            std::fs::create_dir_all(first.join(format!("{index}/nested"))).unwrap();
        }
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(second.join("only"), b"ok").unwrap();
        let scanner = Scanner::new();
        let _old = scanner.scan_root(first.clone());
        let summary = wait(&scanner, &second);
        assert!(summary.complete);
        let tree = scanner.tree_handle();
        let tree = tree.read();
        assert_eq!(tree.root().unwrap().name.as_ref(), second.as_os_str());
        assert_eq!(tree.len(), 2);
        std::fs::remove_dir_all(first).unwrap();
        std::fs::remove_dir_all(second).unwrap();
    }
}

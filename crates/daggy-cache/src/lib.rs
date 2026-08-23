//! Validated, versioned snapshot cache for complete scan generations.
//!
//! Files are archived with `rkyv`, validated before access, keyed by a BLAKE3
//! digest of the exact path bytes, and replaced atomically. A partial or failed
//! refresh never overwrites the previous last-good snapshot. Opening a folder
//! reuses the closest ancestor snapshot that contains it.

#![deny(unsafe_code)]

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use daggy_core::{Kind, Node, NodeId, ScanState, Tree};
use memmap2::Mmap;
use rkyv::{Archive, Deserialize, Serialize};

const SCHEMA_VERSION: u32 = 2;
const CACHE_EXTENSION: &str = "daggy";
const MAX_CACHE_NODES: usize = u32::MAX as usize;
const MAX_NAME_BYTES: usize = 1024 * 1024;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

#[derive(Archive, Serialize, Deserialize)]
struct CacheEnvelope {
    schema: u32,
    written_at_millis: u64,
    root_path: Vec<u8>,
    root: Option<u32>,
    nodes: Vec<CacheNode>,
}

#[derive(Archive, Serialize, Deserialize)]
struct CacheNode {
    parent: Option<u32>,
    kind: u8,
    state: u8,
    complete: bool,
    size: u64,
    block_size: u64,
    mtime: i64,
    inode: u64,
    dev: u64,
    name: Vec<u8>,
    children: Vec<u32>,
}

pub struct CacheHit {
    pub tree: Tree,
    pub written_at_millis: u64,
}

/// A stable, collision-resistant file name for exact Unix path bytes.
pub fn cache_path(root: &Path) -> Option<PathBuf> {
    let directory = cache_directory()?;
    let label = root
        .file_name()
        .unwrap_or(root.as_os_str())
        .to_string_lossy()
        .chars()
        .filter(|character| character.is_alphanumeric() || matches!(character, '-' | '_'))
        .take(32)
        .collect::<String>();
    let label = if label.is_empty() { "volume" } else { &label };
    let digest = blake3::hash(root.as_os_str().as_bytes()).to_hex();
    Some(directory.join(format!("{label}-{}.{}", &digest[..20], CACHE_EXTENSION)))
}

/// Save only a fully accounted tree. The old cache remains untouched until
/// archive, file sync, and atomic rename all succeed.
pub fn save(tree: &Tree, root_path: &Path) -> Result<()> {
    if !tree.is_complete() {
        bail!("refusing to cache an incomplete scan");
    }
    let path = cache_path(root_path).context("no cache directory available")?;
    save_to_path(tree, root_path, &path)
}

/// Load a dated snapshot without pretending it is current. The caller can show
/// it immediately while a separate fresh generation runs in the background.
pub fn load(root_path: &Path) -> Result<Option<CacheHit>> {
    let path = cache_path(root_path).context("no cache directory available")?;
    load_from_path(root_path, &path)
}

/// Walk `path` then each parent. The first snapshot that contains `path` wins,
/// so a Downloads cache beats a Home cache. The returned tree is rooted at
/// `path`, extracted from an ancestor snapshot when needed.
pub fn load_covering(path: &Path) -> Result<Option<CacheHit>> {
    for candidate in covering_candidates(path) {
        match load(&candidate) {
            Ok(Some(hit)) => {
                if let Some(hit) = hit_for_path(hit, path) {
                    return Ok(Some(hit));
                }
            }
            Ok(None) => {}
            Err(error) => {
                log::warn!("skipping unreadable cache for {:?}: {error}", candidate);
            }
        }
    }
    Ok(None)
}

/// Exact path first, then each parent up to the volume root.
pub fn covering_candidates(path: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let mut current = Some(path);
    while let Some(next) = current {
        candidates.push(next.to_path_buf());
        current = next.parent().filter(|parent| {
            parent.as_os_str() != next.as_os_str() && !parent.as_os_str().is_empty()
        });
    }
    candidates
}

fn hit_for_path(hit: CacheHit, path: &Path) -> Option<CacheHit> {
    let id = hit.tree.find_path(path)?;
    if hit.tree.root == Some(id) {
        return Some(hit);
    }
    let tree = hit.tree.extract_subtree(id)?;
    Some(CacheHit {
        tree,
        written_at_millis: hit.written_at_millis,
    })
}

pub fn save_last_root(root: &Path) -> Result<()> {
    let path = cache_directory()
        .context("no cache directory available")?
        .join("last-root");
    atomic_write(&path, root.as_os_str().as_bytes())
}

pub fn load_last_root() -> Result<Option<PathBuf>> {
    let path = cache_directory()
        .context("no cache directory available")?
        .join("last-root");
    match fs::read(&path) {
        Ok(bytes) if bytes.is_empty() => Ok(None),
        Ok(bytes) => Ok(Some(PathBuf::from(OsString::from_vec(bytes)))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read recent root {path:?}")),
    }
}

fn cache_directory() -> Option<PathBuf> {
    dirs::cache_dir().map(|directory| directory.join("daggy").join("snapshots-v2"))
}

fn save_to_path(tree: &Tree, root_path: &Path, path: &Path) -> Result<()> {
    let envelope = envelope_from_tree(tree, root_path)?;
    let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&envelope).context("archive cache")?;
    atomic_write(path, bytes.as_ref())
}

#[allow(unsafe_code)]
fn load_from_path(root_path: &Path, path: &Path) -> Result<Option<CacheHit>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("open cache {path:?}")),
    };
    // SAFETY: the read-only mapping cannot outlive `mmap`, and rkyv validates
    // the complete byte region before an archived reference is returned.
    let mmap = unsafe { Mmap::map(&file) }.with_context(|| format!("map cache {path:?}"))?;
    let archived = rkyv::access::<ArchivedCacheEnvelope, rkyv::rancor::Error>(&mmap)
        .context("validate cache archive")?;
    let envelope: CacheEnvelope = rkyv::deserialize::<CacheEnvelope, rkyv::rancor::Error>(archived)
        .context("deserialize cache")?;
    validate_envelope(&envelope, root_path)?;
    let written_at_millis = envelope.written_at_millis;
    let tree = tree_from_envelope(envelope)?;
    Ok(Some(CacheHit {
        tree,
        written_at_millis,
    }))
}

fn envelope_from_tree(tree: &Tree, root_path: &Path) -> Result<CacheEnvelope> {
    let live: Vec<&Node> = tree.iter().collect();
    if live.len() > MAX_CACHE_NODES {
        bail!("tree exceeds cache node limit");
    }
    let mut dense_of = vec![u32::MAX; tree.cap() as usize];
    for (dense, node) in live.iter().enumerate() {
        dense_of[node.id as usize] = dense as u32;
    }
    let map_id = |id: NodeId| -> Result<u32> {
        dense_of
            .get(id as usize)
            .copied()
            .filter(|dense| *dense != u32::MAX)
            .context("tree contains a dangling node id")
    };
    let mut nodes = Vec::with_capacity(live.len());
    for node in live {
        nodes.push(CacheNode {
            parent: node.parent.map(&map_id).transpose()?,
            kind: node.kind as u8,
            state: encode_state(node.state),
            complete: node.complete,
            size: node.size,
            block_size: node.block_size,
            mtime: node.mtime,
            inode: node.inode,
            dev: node.dev,
            name: node.name.as_bytes().to_vec(),
            children: node
                .children
                .iter()
                .copied()
                .map(&map_id)
                .collect::<Result<Vec<_>>>()?,
        });
    }
    Ok(CacheEnvelope {
        schema: SCHEMA_VERSION,
        written_at_millis: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u64::MAX as u128) as u64,
        root_path: root_path.as_os_str().as_bytes().to_vec(),
        root: tree.root.map(map_id).transpose()?,
        nodes,
    })
}

fn validate_envelope(envelope: &CacheEnvelope, expected_root: &Path) -> Result<()> {
    if envelope.schema != SCHEMA_VERSION {
        bail!("unsupported cache schema {}", envelope.schema);
    }
    if envelope.root_path != expected_root.as_os_str().as_bytes() {
        bail!("cache root path does not match the requested path");
    }
    if envelope.nodes.len() > MAX_CACHE_NODES {
        bail!("cache exceeds node limit");
    }
    let Some(root) = envelope.root.map(|root| root as usize) else {
        bail!("cache has no root");
    };
    if root >= envelope.nodes.len() || envelope.nodes[root].parent.is_some() {
        bail!("cache root index is invalid");
    }
    for (index, node) in envelope.nodes.iter().enumerate() {
        if node.name.len() > MAX_NAME_BYTES {
            bail!("cache node {index} has an excessive name");
        }
        if let Some(parent) = node.parent {
            if parent as usize >= envelope.nodes.len() || parent as usize == index {
                bail!("cache node {index} has an invalid parent");
            }
        }
        for child in &node.children {
            let child = *child as usize;
            if child >= envelope.nodes.len() || child == index {
                bail!("cache node {index} has an invalid child");
            }
            if envelope.nodes[child].parent != Some(index as u32) {
                bail!("cache parent-child relationship is inconsistent");
            }
        }
        decode_kind(node.kind)?;
        decode_state(node.state, node.children.len() as u32)?;
    }
    let mut seen = vec![false; envelope.nodes.len()];
    let mut stack = vec![root];
    while let Some(index) = stack.pop() {
        if std::mem::replace(&mut seen[index], true) {
            bail!("cache graph contains a cycle or duplicate edge");
        }
        stack.extend(
            envelope.nodes[index]
                .children
                .iter()
                .map(|child| *child as usize),
        );
    }
    if seen.iter().any(|seen| !seen) {
        bail!("cache contains unreachable nodes");
    }
    Ok(())
}

fn tree_from_envelope(envelope: CacheEnvelope) -> Result<Tree> {
    let root = envelope.root.context("cache root missing")? as usize;
    let mut tree = Tree::new();
    let mut id_of = Vec::with_capacity(envelope.nodes.len());
    for record in &envelope.nodes {
        let node = Node {
            id: 0,
            parent: None,
            name: std::sync::Arc::from(OsString::from_vec(record.name.clone())),
            kind: decode_kind(record.kind)?,
            size: record.size,
            block_size: record.block_size,
            mtime: record.mtime,
            inode: record.inode,
            dev: record.dev,
            state: decode_state(record.state, record.children.len() as u32)?,
            complete: record.complete,
            children: Vec::new(),
        };
        id_of.push(tree.alloc(node));
    }
    for (index, record) in envelope.nodes.iter().enumerate() {
        let node = tree
            .get_mut(id_of[index])
            .context("allocated cache node missing")?;
        node.parent = record.parent.map(|parent| id_of[parent as usize]);
        node.children = record
            .children
            .iter()
            .map(|child| id_of[*child as usize])
            .collect();
    }
    tree.root = Some(id_of[root]);
    if !tree.is_complete() {
        bail!("cache is not a complete last-good snapshot");
    }
    Ok(tree)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("cache path has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("create cache directory {parent:?}"))?;
    let temp_id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("snapshot");
    let temp = parent.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        temp_id
    ));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .with_context(|| format!("create cache temporary file {temp:?}"))?;
        file.write_all(bytes).context("write cache snapshot")?;
        file.sync_all().context("sync cache snapshot")?;
        drop(file);
        fs::rename(&temp, path)
            .with_context(|| format!("replace cache snapshot {temp:?} -> {path:?}"))?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .ok();
        Ok(())
    })();
    if result.is_err() {
        fs::remove_file(&temp).ok();
    }
    result
}

fn encode_state(state: ScanState) -> u8 {
    match state {
        ScanState::Unscanned => 0,
        ScanState::Scanning { .. } => 1,
        ScanState::Scanned { .. } => 2,
        ScanState::Error => 3,
    }
}

fn decode_state(state: u8, child_count: u32) -> Result<ScanState> {
    Ok(match state {
        0 => ScanState::Unscanned,
        1 => ScanState::Scanning { child_count },
        2 => ScanState::Scanned { child_count },
        3 => ScanState::Error,
        value => bail!("invalid scan state {value}"),
    })
}

fn decode_kind(kind: u8) -> Result<Kind> {
    Ok(match kind {
        0 => Kind::File,
        1 => Kind::Dir,
        2 => Kind::Symlink,
        3 => Kind::Other,
        value => bail!("invalid node kind {value}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::sync::Arc;

    fn nested_home(home: &Path) -> Tree {
        let mut tree = Tree::new();
        let root = tree.alloc(Node::unscanned_dir(0, None, Arc::from(home.as_os_str())));
        tree.root = Some(root);
        let downloads = tree.alloc(Node::unscanned_dir(
            0,
            Some(root),
            Arc::from(OsStr::new("Downloads")),
        ));
        tree.get_mut(root).unwrap().children.push(downloads);
        let movie = tree.alloc(Node {
            id: 0,
            parent: Some(downloads),
            name: Arc::from(OsStr::new("movie.mp4")),
            kind: Kind::File,
            size: 8,
            block_size: 8192,
            mtime: 1,
            inode: 11,
            dev: 2,
            state: ScanState::Scanned { child_count: 0 },
            complete: true,
            children: Vec::new(),
        });
        tree.get_mut(downloads).unwrap().children.push(movie);
        tree.get_mut(downloads).unwrap().state = ScanState::Scanned { child_count: 1 };
        let docs = tree.alloc(Node::unscanned_dir(
            0,
            Some(root),
            Arc::from(OsStr::new("Documents")),
        ));
        tree.get_mut(root).unwrap().children.push(docs);
        let note = tree.alloc(Node {
            id: 0,
            parent: Some(docs),
            name: Arc::from(OsStr::new("note.txt")),
            kind: Kind::File,
            size: 2,
            block_size: 4096,
            mtime: 2,
            inode: 12,
            dev: 2,
            state: ScanState::Scanned { child_count: 0 },
            complete: true,
            children: Vec::new(),
        });
        tree.get_mut(docs).unwrap().children.push(note);
        tree.get_mut(docs).unwrap().state = ScanState::Scanned { child_count: 1 };
        tree.get_mut(root).unwrap().state = ScanState::Scanned { child_count: 2 };
        tree.recompute_upward(root);
        tree.mark_complete_upward(movie);
        tree.mark_complete_upward(note);
        tree
    }

    fn complete_tree(root_path: &Path) -> Tree {
        let mut tree = Tree::new();
        let root = tree.alloc(Node::unscanned_dir(
            0,
            None,
            Arc::from(root_path.as_os_str()),
        ));
        tree.root = Some(root);
        let file = tree.alloc(Node {
            id: 0,
            parent: Some(root),
            name: Arc::from(OsStr::new("file.bin")),
            kind: Kind::File,
            size: 3,
            block_size: 4096,
            mtime: 8,
            inode: 9,
            dev: 2,
            state: ScanState::Scanned { child_count: 0 },
            complete: true,
            children: Vec::new(),
        });
        tree.get_mut(root).unwrap().children.push(file);
        tree.get_mut(root).unwrap().state = ScanState::Scanned { child_count: 1 };
        tree.recompute_upward(root);
        tree.mark_complete_upward(root);
        tree
    }

    fn temp_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "daggy-cache-{label}-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn rkyv_round_trip_preserves_complete_tree() {
        let directory = temp_path("roundtrip");
        fs::create_dir_all(&directory).unwrap();
        let root = directory.join("root");
        let cache = directory.join("snapshot.daggy");
        let tree = complete_tree(&root);
        save_to_path(&tree, &root, &cache).unwrap();
        let hit = load_from_path(&root, &cache).unwrap().unwrap();
        assert!(hit.tree.is_complete());
        assert_eq!(hit.tree.root().unwrap().block_size, 4096);
        assert_eq!(hit.tree.len(), 2);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn truncated_archive_is_rejected() {
        let directory = temp_path("truncated");
        fs::create_dir_all(&directory).unwrap();
        let root = directory.join("root");
        let cache = directory.join("snapshot.daggy");
        save_to_path(&complete_tree(&root), &root, &cache).unwrap();
        let mut bytes = fs::read(&cache).unwrap();
        bytes.truncate(bytes.len() / 2);
        fs::write(&cache, bytes).unwrap();
        assert!(load_from_path(&root, &cache).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn wrong_root_is_rejected() {
        let directory = temp_path("wrong-root");
        fs::create_dir_all(&directory).unwrap();
        let root = directory.join("root");
        let cache = directory.join("snapshot.daggy");
        save_to_path(&complete_tree(&root), &root, &cache).unwrap();
        assert!(load_from_path(&directory.join("other"), &cache).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn incomplete_tree_never_replaces_last_good() {
        let directory = temp_path("last-good");
        fs::create_dir_all(&directory).unwrap();
        let root = directory.join("root");
        let cache = directory.join("snapshot.daggy");
        let good = complete_tree(&root);
        save_to_path(&good, &root, &cache).unwrap();
        let incomplete = Tree::new();
        assert!(envelope_from_tree(&incomplete, &root).is_ok());
        assert!(save(&incomplete, &root).is_err());
        let hit = load_from_path(&root, &cache).unwrap().unwrap();
        assert_eq!(hit.tree.len(), 2);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn live_scan_saves_and_reloads_through_shipped_api() {
        use daggy_core::{ScanEvent, Scanner};
        let root = temp_path("live-scan");
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(root.join("a.bin"), vec![1; 2048]).unwrap();
        fs::write(root.join("nested/b.bin"), vec![2; 4096]).unwrap();
        let scanner = Scanner::new();
        let handle = scanner.scan_root(root.clone());
        while let Ok(event) = handle.events.recv_blocking() {
            if matches!(event, ScanEvent::ScanFinished { .. }) {
                break;
            }
        }
        let tree = scanner.tree_handle().read().clone();
        assert!(tree.is_complete());
        let cache = root.join("snapshot.daggy");
        save_to_path(&tree, &root, &cache).unwrap();
        let hit = load_from_path(&root, &cache)
            .unwrap()
            .expect("cache should reload");
        assert_eq!(hit.tree.len(), tree.len());
        assert_eq!(
            hit.tree.root().unwrap().block_size,
            tree.root().unwrap().block_size
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn path_hash_disambiguates_sanitization_collisions() {
        let left = cache_path(Path::new("/a/b_c")).unwrap();
        let right = cache_path(Path::new("/a_b/c")).unwrap();
        assert_ne!(left, right);
    }

    #[test]
    fn covering_candidates_walk_from_leaf_to_volume() {
        let candidates = covering_candidates(Path::new("/Users/ada/Downloads"));
        assert_eq!(
            candidates,
            [
                PathBuf::from("/Users/ada/Downloads"),
                PathBuf::from("/Users/ada"),
                PathBuf::from("/Users"),
                PathBuf::from("/"),
            ]
        );
    }

    #[test]
    fn covering_hit_extracts_the_closest_ancestor_subtree() {
        let directory = temp_path("covering");
        fs::create_dir_all(&directory).unwrap();
        let home = directory.join("ada");
        let downloads = home.join("Downloads");
        let cache = directory.join("home.daggy");
        let tree = nested_home(&home);
        save_to_path(&tree, &home, &cache).unwrap();
        let home_hit = load_from_path(&home, &cache).unwrap().unwrap();
        let extracted = hit_for_path(home_hit, &downloads).expect("downloads lives under home");
        let root = extracted.tree.root().unwrap();
        assert_eq!(root.name.as_ref(), downloads.as_os_str());
        assert_eq!(extracted.tree.len(), 2);
        assert!(extracted
            .tree
            .find_path(&downloads.join("movie.mp4"))
            .is_some());
        assert!(extracted.tree.find_path(&home.join("Documents")).is_none());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn exact_snapshot_wins_over_an_ancestor() {
        let directory = temp_path("closest");
        fs::create_dir_all(&directory).unwrap();
        let home = directory.join("ada");
        let downloads = home.join("Downloads");
        let home_cache = directory.join("home.daggy");
        let downloads_cache = directory.join("downloads.daggy");
        save_to_path(&nested_home(&home), &home, &home_cache).unwrap();
        let mut downloads_tree = Tree::new();
        let root = downloads_tree.alloc(Node::unscanned_dir(
            0,
            None,
            Arc::from(downloads.as_os_str()),
        ));
        downloads_tree.root = Some(root);
        let other = downloads_tree.alloc(Node {
            id: 0,
            parent: Some(root),
            name: Arc::from(OsStr::new("other.bin")),
            kind: Kind::File,
            size: 1,
            block_size: 4096,
            mtime: 3,
            inode: 13,
            dev: 2,
            state: ScanState::Scanned { child_count: 0 },
            complete: true,
            children: Vec::new(),
        });
        downloads_tree.get_mut(root).unwrap().children.push(other);
        downloads_tree.get_mut(root).unwrap().state = ScanState::Scanned { child_count: 1 };
        downloads_tree.recompute_upward(root);
        downloads_tree.mark_complete_upward(other);
        save_to_path(&downloads_tree, &downloads, &downloads_cache).unwrap();

        let home_hit = load_from_path(&home, &home_cache).unwrap().unwrap();
        let downloads_hit = load_from_path(&downloads, &downloads_cache)
            .unwrap()
            .unwrap();
        let from_home = hit_for_path(home_hit, &downloads).unwrap();
        let from_exact = hit_for_path(downloads_hit, &downloads).unwrap();
        assert!(from_exact
            .tree
            .find_path(&downloads.join("other.bin"))
            .is_some());
        assert!(from_home
            .tree
            .find_path(&downloads.join("movie.mp4"))
            .is_some());
        assert!(from_exact
            .tree
            .find_path(&downloads.join("movie.mp4"))
            .is_none());
        fs::remove_dir_all(directory).unwrap();
    }
}

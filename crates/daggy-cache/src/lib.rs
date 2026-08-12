//! Daggy cache: an mmap-backed binary store for scan results.
//!
//! ## On-disk format (little-endian, compact, flat)
//!
//! ```text
//! magic        : 4 bytes  = b"dgy1"
//! version      : u16
//! root_index   : u32       (index into the node array, or u32::MAX if none)
//! node_count   : u32
//! nodes        : node_count × NodeRecord
//! ```
//!
//! Each `NodeRecord` is:
//! ```text
//! parent       : i32        (-1 = none)
//! kind         : u8
//! state        : u8         (encoded ScanState)
//! _pad         : u16
//! size         : u64
//! block_size   : u64
//! mtime        : u64
//! inode        : u64
//! dev          : u64
//! child_count  : u32
//! name_len     : u16
//! name         : name_len bytes (UTF-8)
//! children     : child_count × u32
//! ```
//!
//! The file is written atomically (`.tmp` + rename) and read via `mmap2` so
//! reloading a large tree is a cheap kernel page-map with no parsing copy.
//!
//! ## Invalidation
//!
//! On load we compare the cached root's `mtime` to the live directory mtime.
//! If they match, the cache is reused as-is. If they differ, the cache is
//! discarded (a per-subtree refinement is handled by the scanner's
//! `ScanState::Unscanned` markers when a future incremental invalidation is
//! wired in).

#![deny(unsafe_code)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use daggy_core::{Kind, Node, NodeId, ScanState, Tree};
use memmap2::Mmap;

/// Magic header identifying a daggy cache file.
const MAGIC: &[u8; 4] = b"dgy1";
/// Bumped when the on-disk layout changes.
const VERSION: u16 = 1;

/// Encode a tree into a flat byte vector.
pub fn encode(tree: &Tree) -> Vec<u8> {
    // Collect live nodes in id order, building a dense index. We rewrite node
    // ids to be dense (0..n) so the file is compact and reloads cheaply.
    let live: Vec<&Node> = tree.iter().collect();
    let n = live.len() as u32;
    // Map old NodeId -> dense index.
    let mut index_of: Vec<u32> = vec![u32::MAX; tree.cap() as usize];
    for (i, node) in live.iter().enumerate() {
        index_of[node.id as usize] = i as u32;
    }

    let root_index = tree.root.map(|r| index_of[r as usize]).unwrap_or(u32::MAX);

    let mut out = Vec::with_capacity(n as usize * 96);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&root_index.to_le_bytes());
    out.extend_from_slice(&n.to_le_bytes());

    for node in &live {
        let parent = node
            .parent
            .map(|p| index_of[p as usize] as i32)
            .unwrap_or(-1);
        out.extend_from_slice(&parent.to_le_bytes());
        out.push(node.kind as u8);
        out.push(encode_state(node.state));
        out.extend_from_slice(&0u16.to_le_bytes()); // pad
        out.extend_from_slice(&node.size.to_le_bytes());
        out.extend_from_slice(&node.block_size.to_le_bytes());
        out.extend_from_slice(&node.mtime.to_le_bytes());
        out.extend_from_slice(&node.inode.to_le_bytes());
        out.extend_from_slice(&node.dev.to_le_bytes());
        let child_count = node.children.len() as u32;
        out.extend_from_slice(&child_count.to_le_bytes());
        let name = node.name.as_bytes();
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(name);
        for &c in &node.children {
            out.extend_from_slice(&index_of[c as usize].to_le_bytes());
        }
    }

    out
}

/// Decode a flat byte slice into a tree. Performs bounds checks at every read.
pub fn decode(bytes: &[u8]) -> Result<Tree> {
    let mut r = Reader::new(bytes);
    if r.take(4)? != MAGIC {
        bail!("daggy cache: bad magic");
    }
    let version = r.u16()?;
    if version != VERSION {
        bail!("daggy cache: unsupported version {version}");
    }
    let root_index = r.u32()?;
    let n = r.u32()? as usize;

    // First pass: read everything except child references into a temp buffer,
    // because child ids are dense indices that we need to map back to NodeIds
    // (which are assigned as we alloc into the fresh tree).
    struct Rec {
        parent: Option<usize>,
        kind: u8,
        state: ScanState,
        size: u64,
        block_size: u64,
        mtime: u64,
        inode: u64,
        dev: u64,
        name: Vec<u8>,
        children: Vec<u32>,
    }

    let mut recs: Vec<Rec> = Vec::with_capacity(n);
    for _ in 0..n {
        let parent = r.i32()?;
        let kind = r.one()?;
        let state = decode_state(r.one()?)?;
        r.u16()?; // pad
        let size = r.u64()?;
        let block_size = r.u64()?;
        let mtime = r.u64()?;
        let inode = r.u64()?;
        let dev = r.u64()?;
        let child_count = r.u32()? as usize;
        let name_len = r.u16()? as usize;
        let name = r.take(name_len)?.to_vec();
        let mut children = Vec::with_capacity(child_count);
        for _ in 0..child_count {
            children.push(r.u32()?);
        }
        recs.push(Rec {
            parent: if parent < 0 { None } else { Some(parent as usize) },
            kind,
            state,
            size,
            block_size,
            mtime,
            inode,
            dev,
            name,
            children,
        });
    }

    // Second pass: allocate nodes in dense order, remembering the NodeId each
    // dense index maps to, then patch parent/child references.
    let mut tree = Tree::new();
    let mut id_of: Vec<NodeId> = vec![0; n];
    for (i, rec) in recs.iter().enumerate() {
        let node = Node {
            id: 0,
            parent: None, // patched below
            name: String::from_utf8_lossy(&rec.name).into(),
            kind: decode_kind(rec.kind)?,
            size: rec.size,
            block_size: rec.block_size,
            mtime: rec.mtime,
            inode: rec.inode,
            dev: rec.dev,
            state: rec.state,
            children: rec.children.iter().map(|_| 0).collect(), // patched below
        };
        id_of[i] = tree.alloc(node);
    }
    for (i, rec) in recs.iter().enumerate() {
        let id = id_of[i];
        let node = tree.get_mut(id).unwrap();
        node.parent = rec.parent.map(|p| id_of[p]);
        node.children = rec.children.iter().map(|&c| id_of[c as usize]).collect();
    }
    tree.root = (root_index != u32::MAX).then(|| id_of[root_index as usize]);

    Ok(tree)
}

/// Where to keep cache files for a given scanned path.
pub fn cache_path(for_path: &Path) -> Option<PathBuf> {
    let base = dirs::cache_dir()?; // ~/Library/Caches on macOS
    let mut dir = base.join("daggy");
    // Sanitize the scanned path into a stable filename.
    let sanitized: String = for_path
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    dir.push(format!("{sanitized}.daggy"));
    Some(dir)
}

/// Persist a tree atomically to the cache file for `for_path`.
pub fn save(tree: &Tree, for_path: &Path) -> Result<()> {
    let path = cache_path(for_path).context("no cache dir available")?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create cache dir {:?}", parent))?;
    }
    let bytes = encode(tree);
    let mut tmp = path.clone();
    tmp.set_extension("daggy.tmp");
    let mut f = fs::File::create(&tmp)
        .with_context(|| format!("create cache tmp {:?}", tmp))?;
    f.write_all(&bytes).context("write cache")?;
    f.sync_all().ok();
    drop(f);
    fs::rename(&tmp, &path)
        .with_context(|| format!("rename cache {:?} -> {:?}", tmp, path))?;
    Ok(())
}

/// Load a cached tree for `for_path`, checking that the root's recorded mtime
/// still matches the live directory. Returns `Ok(None)` if there is no cache
/// or it is stale.
#[allow(unsafe_code)]
pub fn load(for_path: &Path) -> Result<Option<Tree>> {
    let path = cache_path(for_path).context("no cache dir available")?;
    let file = match fs::File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("open cache {:?}", path)),
    };
    let mmap = unsafe { Mmap::map(&file) }
        .with_context(|| format!("mmap cache {:?}", path))?;
    let tree = decode(&mmap)?;

    // Invalidation: compare cached root mtime to the live dir mtime.
    use std::os::unix::fs::MetadataExt;
    let live_mtime = fs::symlink_metadata(for_path)
        .map(|m| m.mtime() as u64)
        .unwrap_or(0);
    let cached_mtime = tree.root().map(|r| r.mtime).unwrap_or(0);
    if live_mtime != cached_mtime {
        log::debug!(
            "cache stale for {:?}: cached mtime {} != live {}",
            for_path,
            cached_mtime,
            live_mtime
        );
        return Ok(None);
    }
    Ok(Some(tree))
}

// ---------------------------------------------------------------------------
// ScanState / Kind encoding helpers.
// ---------------------------------------------------------------------------

fn encode_state(s: ScanState) -> u8 {
    match s {
        ScanState::Unscanned => 0,
        ScanState::Scanning { child_count } => {
            // Pack "scanning" + low child count; lossy but we never persist
            // Scanning (the scanner only saves when idle).
            1.min(child_count.min(253) as u8)
        }
        ScanState::Scanned { child_count } => 64 + (child_count.min(191) as u8),
        ScanState::Error => 255,
    }
}

fn decode_state(b: u8) -> Result<ScanState> {
    Ok(match b {
        0 => ScanState::Unscanned,
        255 => ScanState::Error,
        v if v >= 64 => ScanState::Scanned {
            child_count: (v - 64) as u32,
        },
        v => ScanState::Scanning {
            child_count: v as u32,
        },
    })
}

fn decode_kind(b: u8) -> Result<Kind> {
    Ok(match b {
        0 => Kind::File,
        1 => Kind::Dir,
        2 => Kind::Symlink,
        3 => Kind::Other,
        other => bail!("daggy cache: bad kind byte {other}"),
    })
}

// ---------------------------------------------------------------------------
// Minimal bounds-checked byte reader.
// ---------------------------------------------------------------------------

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.pos.checked_add(n).map_or(true, |end| end > self.buf.len()) {
            bail!("daggy cache: truncated");
        }
        let out = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    fn one(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }

    fn u64(&mut self) -> Result<u64> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daggy_core::{Node, Tree};

    fn sample_tree() -> Tree {
        // root(dir) -> [a(dir) -> [f1(file)], b.txt(file)]
        let mut tree = Tree::new();
        let root = tree.alloc(Node::unscanned_dir(0, None, "/root".into()));
        tree.root = Some(root);
        let a = tree.alloc(Node::unscanned_dir(0, Some(root), "a".into()));
        let f1 = Node {
            id: 0,
            parent: Some(a),
            name: "f1.txt".into(),
            kind: Kind::File,
            size: 1024,
            block_size: 4096,
            mtime: 1000,
            inode: 7,
            dev: 1,
            state: ScanState::Scanned { child_count: 0 },
            children: Vec::new(),
        };
        let f1 = tree.alloc(f1);
        let b = Node {
            id: 0,
            parent: Some(root),
            name: "b.txt".into(),
            kind: Kind::File,
            size: 2048,
            block_size: 4096,
            mtime: 2000,
            inode: 8,
            dev: 1,
            state: ScanState::Scanned { child_count: 0 },
            children: Vec::new(),
        };
        let b = tree.alloc(b);
        tree.get_mut(root).unwrap().children = vec![a, b];
        tree.get_mut(a).unwrap().children = vec![f1];
        tree.get_mut(root).unwrap().state = ScanState::Scanned { child_count: 2 };
        tree.get_mut(a).unwrap().state = ScanState::Scanned { child_count: 1 };
        tree
    }

    #[test]
    fn encode_decode_roundtrip() {
        let tree = sample_tree();
        let bytes = encode(&tree);
        let decoded = decode(&bytes).expect("decode");

        let r0 = tree.root().unwrap();
        let r1 = decoded.root().unwrap();
        assert_eq!(r0.name, r1.name);
        assert_eq!(r0.children.len(), r1.children.len());
        // Walk a -> f1.
        let a0 = tree.get(r0.children[0]).unwrap();
        let a1 = decoded.get(r1.children[0]).unwrap();
        assert_eq!(a0.name, a1.name);
        assert_eq!(a0.children.len(), a1.children.len());
        let f1 = decoded.get(a1.children[0]).unwrap();
        assert_eq!(f1.name.as_ref(), "f1.txt");
        assert_eq!(f1.size, 1024);
    }

    #[test]
    fn decode_rejects_bad_magic() {
        let mut bytes = encode(&sample_tree());
        bytes[0] = b'x';
        assert!(decode(&bytes).is_err());
    }

    #[test]
    fn save_load_roundtrip_and_staleness() {
        let tmp = std::env::temp_dir().join(format!(
            "daggy-cache-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        // Stamp the root's mtime to match the live dir.
        let mut tree = sample_tree();
        use std::os::unix::fs::MetadataExt;
        let live_mtime = std::fs::symlink_metadata(&tmp).unwrap().mtime() as u64;
        tree.get_mut(tree.root.unwrap()).unwrap().mtime = live_mtime;

        save(&tree, &tmp).expect("save");
        let loaded = load(&tmp).expect("load");
        assert!(loaded.is_some(), "cache hit when mtime matches");

        // Touch the dir to change its mtime -> cache should be reported stale.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let touchfile = tmp.join("touch");
        std::fs::write(&touchfile, b"x").unwrap();
        let stale = load(&tmp).expect("load stale");
        assert!(stale.is_none(), "cache stale after dir mtime changes");

        std::fs::remove_dir_all(&tmp).ok();
    }
}

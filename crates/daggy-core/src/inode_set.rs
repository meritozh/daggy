//! Sharded inode set for hardlink deduplication.
//!
//! A file with N hardlinks should be counted once. We key on `(dev, inode)`
//! and shard across many mutexes so concurrent scanners almost never contend.
//! 128 shards matches the approach used by the fastest macOS disk scanners.

use parking_lot::Mutex;
use rustc_hash::FxHashSet;

const SHARD_COUNT: usize = 128;

type Key = (u64, u64);

#[derive(Default)]
struct Shard(Mutex<FxHashSet<Key>>);

/// A concurrency-friendly `(dev, inode)` membership set.
///
/// `insert` returns `true` if the key was newly added — i.e. this is the first
/// link we've seen and its size should be counted.
pub struct ShardedInodeSet {
    shards: Vec<Shard>,
}

impl ShardedInodeSet {
    pub fn new() -> Self {
        let mut shards = Vec::with_capacity(SHARD_COUNT);
        for _ in 0..SHARD_COUNT {
            shards.push(Shard::default());
        }
        Self { shards }
    }

    fn shard_for(&self, dev: u64, inode: u64) -> &Shard {
        // Mix the two halves so nearby inodes on the same device spread out.
        let hash = dev.wrapping_mul(0x9e3779b97f4a7c15).wrapping_add(inode);
        let idx = (hash as usize) & (SHARD_COUNT - 1);
        &self.shards[idx]
    }

    /// Returns `true` if this `(dev, inode)` was not already present (i.e. the
    /// caller should count the size). `false` means it's a hardlink we've
    /// already accounted for.
    pub fn insert(&self, dev: u64, inode: u64) -> bool {
        let mut set = self.shard_for(dev, inode).0.lock();
        set.insert((dev, inode))
    }

    pub fn clear(&self) {
        for shard in &self.shards {
            shard.0.lock().clear();
        }
    }
}

impl Default for ShardedInodeSet {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_insert_counts_duplicates_dont() {
        let set = ShardedInodeSet::new();
        assert!(set.insert(1, 100), "first link counts");
        assert!(!set.insert(1, 100), "second link (hardlink) does not");
        assert!(set.insert(1, 101), "different inode on same dev counts");
        assert!(set.insert(2, 100), "same inode on different dev counts");
    }

    #[test]
    fn clear_resets() {
        let set = ShardedInodeSet::new();
        assert!(set.insert(1, 100));
        set.clear();
        assert!(set.insert(1, 100), "after clear the link counts again");
    }
}

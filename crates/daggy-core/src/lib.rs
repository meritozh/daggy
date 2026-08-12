//! Daggy core: the platform-independent scanning engine and tree model.
//!
//! This crate has no UI dependencies, so it can be unit-tested headlessly and
//! reused by any frontend.

#![forbid(unsafe_code)]

pub mod fmt;
mod inode_set;
mod scanner;
mod tree;

pub use inode_set::ShardedInodeSet;
pub use scanner::{ScanEvent, Scanner, ScannerHandle};
pub use tree::{Kind, Node, NodeId, ScanState, Tree, TreeSnapshot};

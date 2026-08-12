//! Daggy core: the platform-independent scanning engine and tree model.
//!
//! This crate has no UI dependencies, so it can be unit-tested headlessly and
//! reused by any frontend.

#![deny(unsafe_code)]

pub mod fmt;
mod inode_set;
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod macos_bulk;
mod scanner;
mod tree;

pub use inode_set::ShardedInodeSet;
pub use scanner::{ScanEvent, ScanSummary, Scanner, ScannerHandle};
pub use tree::{ChartNode, Kind, Node, NodeId, NodeView, ScanState, Tree, TreeProjection};

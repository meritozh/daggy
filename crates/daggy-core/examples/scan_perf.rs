use std::path::PathBuf;
use std::time::Instant;

use daggy_core::{fmt, ScanEvent, Scanner};

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let workers = std::env::args()
        .nth(2)
        .and_then(|workers| workers.parse::<usize>().ok());
    let scanner = workers.map_or_else(Scanner::new, Scanner::with_worker_count);
    let started = Instant::now();
    let handle = scanner.scan_root(PathBuf::from(&path));
    let summary = loop {
        match handle.events.recv_blocking() {
            Ok(ScanEvent::ScanFinished { summary, .. }) => break summary,
            Ok(_) => {}
            Err(_) => panic!("scan stopped before completion"),
        }
    };
    let tree = scanner.tree_handle();
    let tree = tree.read();
    let root = tree.root().expect("scan root");
    println!(
        "full: {:?}  allocated={}  apparent={}  dirs={}  items={}  nodes={}  errors={}  workers={}",
        started.elapsed(),
        fmt::human_bytes(root.block_size),
        fmt::human_bytes(root.size),
        summary.dirs_scanned,
        summary.items_seen,
        tree.len(),
        summary.errors,
        summary.workers,
    );
}

use daggy_core::{Scanner, ScanEvent};
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "/Users".into());
    let scanner = Scanner::new();
    let t0 = Instant::now();
    let handle = scanner.scan_root(PathBuf::from(&path));
    let mut items = 0u64;
    let mut dirs = 0u64;
    while let Ok(ev) = handle.events.recv_blocking() {
        match ev {
            ScanEvent::Progress { dirs_scanned, items_total } => {
                dirs += dirs_scanned;
                items += items_total;
            }
            ScanEvent::ScanFinished { .. } => break,
            _ => {}
        }
    }
    let phase1 = t0.elapsed();
    let snap = scanner.snapshot();
    let root = snap.root().unwrap();
    println!(
        "phase1: {:?}  root={}  dirs(top2)={}  items={}",
        phase1,
        daggy_core::fmt::human_bytes(root.size),
        dirs,
        items
    );
}

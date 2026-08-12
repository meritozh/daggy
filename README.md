# daggy

> Your disk is daggy — let's analyze it.

A DaisyDisk-inspired disk usage analyzer for macOS, built in pure Rust with
[GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) (Metal-rendered)
for the UI and a high-performance, low-CPU scanning engine.

## Features

- **Sunburst + list visualization** — drill into your disk layer by layer, the
  way DaisyDisk does it.
- **Lazy per-layer scanning** — phase 1 gives you the top levels instantly;
  subtrees are scanned on demand when you expand them, so the app stays idle
  and cool.
- **Fast macOS enumeration** — uses the `getattrlistbulk` syscall (one bulk call
  returns many entries *and* their metadata), ~6× faster than per-file `stat`.
- **Low CPU** — a bounded 64-wide semaphore caps concurrent I/O; the app never
  spawns thousands of threads.
- **Hardlink-aware** — files with multiple hardlinks are counted once via a
  sharded inode set (128 shards → near-zero lock contention).
- **Cached results** — scan trees are persisted to a compact mmap'd binary file
  in `~/Library/Caches/daggy`, invalidated per-subtree by directory mtime.

## Crates

| Crate | Responsibility |
|-------|----------------|
| [`daggy-core`](crates/daggy-core) | Scanning engine + arena tree model (no UI deps). |
| [`daggy-cache`](crates/daggy-cache) | mmap'd binary cache, mtime invalidation. |
| [`daggy-app`](crates/daggy-app) | GPUI application binary. |

## Build

```sh
cargo run -p daggy-app
```

Requires the Metal toolchain (Xcode): `xcodebuild -downloadComponent MetalToolchain`.

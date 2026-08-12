# daggy

Daggy is a local macOS disk-usage analyzer built with a pure Rust application
stack and GPUI. It presents allocated storage as an interactive three-ring map
with a size-ranked inspector.

## What it does

- Shows the selected folder immediately from a dated last-good snapshot when
  one exists.
- Scans the visible layers first, then completes correct full-tree accounting
  in the background.
- Promotes a focused branch without spawning new recursive threads.
- Uses up to four workers for visible work and only two for deep background
  traversal, keeping sustained CPU and I/O modest.
- Enumerates macOS directories with an internal `getattrlistbulk(2)` adapter
  that preserves exact path bytes, does not follow symlinks, validates returned
  records, and falls back to `std::fs` on unsupported volumes.
- Counts multiply-linked files once using `(device, inode)` identity.
- Stores complete scans as validated rkyv archives under
  `~/Library/Caches/daggy/snapshots-v2`. Failed or partial refreshes never
  replace the previous good snapshot.
- Labels cached, calculating, complete, and unreadable states explicitly.

## Workspace

| Crate | Responsibility |
|---|---|
| `daggy-core` | Bounded scan coordinator, macOS enumeration, arena tree, compact UI projections |
| `daggy-cache` | Versioned rkyv archive, mmap validation, atomic last-good replacement |
| `daggy-app` | GPUI state, interactive sunburst, ranked inspector, keyboard navigation |

GPUI is pinned to commit `b13f6c7114e5e71f5e2048788fdc59bbfa2539d0`
because its API is still pre-1.0.

## Build and test

Install Apple's Metal compiler component once:

```sh
xcodebuild -downloadComponent MetalToolchain
```

Then run:

```sh
cargo test --workspace
cargo run -p daggy-app
```

Create the local application bundle:

```sh
cargo build --release -p daggy-app
./scripts/make-bundle.sh
open target/release/daggy.app
```

Benchmark a full traversal, optionally selecting one to four workers:

```sh
cargo run --release -p daggy-core --example scan_perf -- . 4
```

On the development tree measured on 2026-08-13, a warm complete traversal of
about 29,500 entries took roughly 57 ms with two workers. Treat that as a local
regression baseline, not a universal claim.

## Controls

- Click a branch or ranked folder to focus it.
- Click the center or press Escape to go up.
- Use Up and Down to select a row, then Return to open it.
- Press Command-O to choose another folder.

The first launch starts with an empty state. Later launches restore the most
recent complete snapshot and refresh it in the background.

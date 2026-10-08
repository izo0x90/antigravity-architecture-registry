# Performance: before and after

Measured by `crates/arch-service/tests/perf.rs` on 2026-10-08, in the dev
profile with dependencies and the arch crates optimized (opt-level 2), on
the sandbox (14 cores).

- **This repo**: 155 files, 2,958 components, 10,691 relations.
- **tokio 1.47.1**: 717 files, 159k lines, 7,977 components, 17,528 relations.

## Before (the old views)

| Step | This repo | tokio |
|---|---|---|
| Full index | 2.7 s | 6.1 s |
| One file changed | 1.3 s | 3.1 s |
| Search | 11 ms | 17 ms |

Views (server time / answer size; the browser then parsed and laid all of
it out on its one thread):

| View, depth | This repo | tokio |
|---|---|---|
| structure 1 | 30 ms / 40 KB, 39 nodes | 50 ms / 67 KB, 68 nodes |
| structure 4 | 55 ms / 2.7 MB, 1,784 nodes | 95 ms / 4.4 MB, 3,202 nodes |
| structure 12 | 75 ms / 4.9 MB, 2,958 nodes | 183 ms / 11.6 MB, 7,977 nodes |
| calls 1 | 35 ms / 2.8 MB, 1,154 nodes | 67 ms / 5.4 MB, 2,991 nodes |
| calls 4 | 41 ms / 3.9 MB, 1,724 nodes | 78 ms / 6.0 MB, 3,319 nodes |
| calls 12 | 39 ms / 4.0 MB, 1,763 nodes | 75 ms / 6.0 MB, 3,323 nodes |
| data_flow 1 | 32 ms / 2.6 MB, 1,074 nodes | 65 ms / 5.0 MB, 2,818 nodes |
| data_flow 4 | 44 ms / 4.5 MB, 1,815 nodes | 85 ms / 7.7 MB, 4,273 nodes |
| data_flow 12 | 41 ms / 4.6 MB, 1,820 nodes | 83 ms / 7.7 MB, 4,285 nodes |

Before this change the dev server also ran every dependency unoptimized
(tree-sitter, serde, the graph), which made all of the above several times
slower in practice.

Screenshots of the old layouts (the bar for spacing) are kept outside the
repo in `~/.cache/nah-bench/layout-before/`.

## After

Measured by `cargo test -p arch-service --test perf` (dev profile, the
same machine).

### Reading the code (slice P5)

Files are read on many threads at once; one whose size and time match what
was kept isn't read; the facts are kept in `.nah/cache/index.sqlite`.

| | this repo (171 files) | tokio (717 files) |
|---|---|---|
| Full index | 0.53 s | 0.92 s |
| One file changed | 0.36 s | 0.54 s |
| The same, found by re-checking the whole repo | 0.34 s | 0.54 s |
| Restart from the kept facts (no file read) | 0.23 s | 0.54 s |

Linking (tying calls and names across files) is now about 0.44 s on tokio,
down from 2.55 s, after its name lookups were indexed; it's redone whole
after a change, which is what a one-file change costs.

### Search (slice P6)

Symbols are matched in memory with neo_frizbee; files and lines with
fff-search (its file list is read once, ~0.1 s here, and kept up to date
from the watcher).

| | this repo | tokio |
|---|---|---|
| Symbols ("view") | <1 ms | <1 ms |
| Symbols, qualified ("servce::serch") | 2 ms | 4 ms |
| Files ("service") | 2 ms | 4 ms |
| Lines ("fn new") | 2 ms | 1 ms |

### Hot spots (slice P7, `cargo bench -p arch-service --bench hot`, release)

| On this repo | Time |
|---|---|
| Linking every file's facts | 127 ms |
| Contains tree, depth 4 | 2.5 ms |
| Calls tree, depth 4 | 4.8 ms |
| Data flow tree, depth 4 | 8.4 ms |
| Matching a name against every component | 10 µs |

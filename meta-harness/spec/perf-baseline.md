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

Filled in as the new system lands.

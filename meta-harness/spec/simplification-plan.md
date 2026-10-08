# The simplification plan

This is the plan to make the graph simple, correct and fast, on this repo
and on big ones. It's for discussion: nothing here is built yet.

## Part 1. What's wrong today

These come from reading the code and measuring the running server.

**Why it freezes.**
- When you open the calls view, the server sends the browser everything it
  found: about 1,700 functions, 4,000 lines and 3.7 MB of data. The combined
  view is 6.4 MB.
- The browser then does all the heavy work on its one and only thread:
  - it reads the data;
  - it copies it three times;
  - it regroups it by level;
  - it lays it out, with a step that gets eight times slower every time the
    number of boxes doubles;
  - it routes the lines;
  - it redraws every box and line on every frame, even off-screen ones.
- The story view takes 2 seconds even on the server.
- Nothing has a time limit, and nothing is capped except one level of the
  drawing.

**Why plan selection looks broken.** When you pick "code only", the server
actually shows the code with *every plan* mixed in. The code view's margin
does the same. An agent can't switch you back to code.

**Why focus doesn't trim.** Focus, levels, the 60-box limit and zoom all
change what's on screen, each in its own way, and they get in each other's
way.

**Indexing.**
- ast-grep is used properly as a library to *find* things. Almost
  everything after finding is hand-written string work:
  - counting brackets;
  - regexes over code text;
  - a home-made parser for `cfg` conditions.
- Files are read one at a time.
- The cache is 154 pretty-printed JSON files, 14 MB in total, 7 times the
  size of the code. The whole folder gets re-read after every change.
- Every change re-links the whole repo.
- Every request copies the entire graph, and re-reads the plan files.
- Adding a component scans every file and name, so building the graph slows
  down with the square of its size.
- Calls are matched by name, so some get linked to the wrong function.

**Search.** Search is a plain loop, written three separate times. The
cockpit's `/` only searches what's on screen. There's no search over file
contents.

**Code size.** The cockpit's graph file is 4,278 lines, and app.rs is
3,741.

## Part 2. What we're building

**Three views.** Each is a tree that starts from one root.

1. **Contains.** Boxes inside boxes, with no lines.
2. **Calls.**
   - Cards are functions, in the order they're called.
   - Direction is down (what it calls) or up (what calls it).
   - Pills on the selected card's lines show the data going in and coming
     back.
3. **Data flow.**
   - Cards are data types.
   - The functions that turn one into another are pills on the lines.
   - Direction is down (where it goes) or up (where it came from).

**One set of controls for all three:**
- **Select:** click, or arrow keys and `hjkl`. What's connected stays bright
  and the rest dims. The details card shows what's selected.
- **Open or close in place:** `z o` / `z c`.
- **Go in:** `Enter` or double-click. The card becomes the root.
- **Back:** `Backspace`.
- **Depth:** `+` / `-`. Changing it resets the branches you opened by hand.
- **Flip direction:** `r`.
- **Switch view:** `v`.
- **Source:** one picker at the top: **Code**, or one plan.
- **Load more:** on a card that says "calls 6 more", click the count to load
  that branch.

**Picking a plan focuses on the plan.**

When you pick a plan, the view starts from that plan's own things: what it
adds and what it changes.

- **Its roots:** the plan's top-level items, meaning the ones no other item
  in the same plan sits inside or calls.
  - **Contains:** the plan's items, inside the existing boxes they go into.
  - **Calls:** the plan's functions that nothing else in the plan calls,
    going down.
  - **Data flow:** the plan's data types that nothing else in the plan
    produces.
- **What else shows:** only the existing code the plan plugs into, meaning
  the parent a new item goes into and the existing functions it calls or is
  called by. These show as plain cards, so you can see where the plan
  connects.
- **What doesn't show:** nothing else in the codebase.
- **The other controls** work as usual inside this. Go into a plan item,
  load more, flip direction, or go out to see more of the surrounding code.
- **To see the plan inside the whole codebase,** use the one switch
  "show in full code". The plan's marks stay; the rest of the code appears
  around them.
- **Picking Code** shows the code only, with no plan anywhere.

**Honest trees.**
- A card at the edge says how much more is hidden ("calls 6 more").
- A function that appears twice is drawn once, with a line pointing to it.
- A call back up the tree is a loop line, not a copy.

**Going out** shows the parent, its other children, and the lines that
connect where you were to the rest. You always see where things plug in.

**Nothing automatic.** The screen changes only because you did something.
Zoom only zooms. Nothing hides on its own, and nothing regroups on its
own.

**Plan marks:**
- **New:** planned, not in the code yet.
- **Changed:** exists, but the plan changes it.
- **Done:** planned, and the code now matches.
- **Doesn't match:** the code differs from the plan. Both versions are
  shown: the planned line dashed, the real one solid.

A card that another plan also changes gets a badge naming that plan.

## Part 3. The architecture

### The one rule

**The server knows everything. The browser only knows what's on screen,
plus a little around it.**

- The server holds the whole graph in memory, as lookup tables.
- The browser asks for small pieces (chunks) and draws them.
- The answer to every question has a size limit and a time limit.

### The parts and what each one is for

Each part has one job, and talks to the others only through a small
interface (a Rust trait). That makes each part replaceable without
touching the rest.

```
 code files ──► [ Reader ] ──facts──► [ Store ] ◄──► SQLite file
                                         │
                                         ▼ loads
 plan files ──► [ Plans ] ──────────► [ Graph ] (in memory, read-only snapshot)
                                         │
                       ┌─────────────────┼──────────────────┐
                       ▼                 ▼                  ▼
                 [ Tree builder ]   [ Search ]        [ Details ]
                       │
                       ▼
                   [ Layout ]
                       │
                       ▼
 browser ◄──chunks── [ API ] (REST + MCP)
```

**1. Reader** (crate `arch-index`)
- **Does:** reads one file and returns its facts (definitions, calls,
  types, data flow, placeholders), using ast-grep. It also links calls
  across files.
- **Never does:** storage, search or drawing.
- **Interface:** `read(file) -> FileFacts`, plus `link(all facts) -> links`.
  Pure functions, which makes them easy to test and to run in parallel.

**2. Store** (new crate `arch-store`)
- **Does:** keeps facts on disk in one SQLite file, and gives them back.
- **Interface:** a `FactStore` trait:
  - `replace_file(path, facts)`: one transaction, deletes the old rows and
    writes the new ones;
  - `remove_file(path)`;
  - `fingerprints()`: which files we have, and their size, time and hash;
  - `load()`: everything, for building the graph at startup.
- **One implementation:** `SqliteStore`. If we ever switch to LadybugDB or
  anything else, only this crate changes.
- **Can always be thrown away.** The code is the source of truth, so a
  broken or outdated store file is deleted and rebuilt, never migrated by
  hand.

**3. Plans** (in `arch-service`)
- **Does:** loads the plan files from `.nah/plans/`, keeps them in memory,
  and reloads one only when its file changes.
- Plans stay as JSON files, because people and agents author them and they
  belong in git. The store doesn't own them.

**4. Graph** (crate `arch-graph`)
- **Does:** holds the whole linked graph in memory, as one read-only
  snapshot.
  - Ids are small numbers, not strings.
  - It answers lookups instantly: what's inside X, X's parent, who calls X,
    what X calls, where data goes from X, and how many there are of each.
- **Plan overlay:** a small layer per plan on top of the snapshot. It holds
  that plan's new and changed items and their marks. It's built once and
  rebuilt only when that plan or the code changes.
- **Interface:** a `GraphRead` trait (children, parent, out, in, counts,
  node). The tree builder and search use only this trait. If the graph
  someday gets too big for memory, a database-backed `GraphRead` can
  replace the in-memory one, and nothing above it changes.
- **Updates:** a new snapshot is built, then swapped in (`arc-swap`).
  Requests in progress finish on the old one. Nobody waits and nothing is
  copied per request.

**5. Tree builder** (in `arch-graph`)
- **Does:** answers "show me this view, from this root, this direction,
  this deep, with these branches opened", within the card budget.
- **Three builders:** contains, calls and data flow. They share one walk
  that handles the budget, the "N more" counts, repeats and loops.
- **Also answers chunk questions:**
  - "expand this card N more levels";
  - "the context around this card" (for going out).

**6. Layout** (in `arch-graph`)
- **Does:** gives every card a position and every line a path.
- Nested boxes use a squarified treemap. Trees use a tidy tree layout,
  whose time grows only in proportion to the number of cards.
- Repeat and loop lines are simple curves. There's no routing grid.
- Text sizes come from the theme's font settings, not hard-coded numbers.
- It runs on the server, with a time limit.

**7. Search** (new crate `arch-search`)
- **Does:** symbol search, file name search and file content search.
- **Interface:** a `Search` trait, `search(query, kind, limit)`.
- **Symbols:** `neo_frizbee`, fff's fuzzy matcher, over names held in
  memory.
- **Files and content:** `fff-search`, confined to the repo.

**8. Details** (in `arch-service`)
- **Does:** the full information on one card (signature, location, status,
  plan, and planned vs real). The browser asks for it only when you click.

**9. API** (the server, `src/`)
- **Does:** turns HTTP and MCP requests into calls on the parts above, and
  nothing else.
- **Chunk endpoints:**
  - `GET /api/graph/tree`: view, source, root, direction, depth, opened and
    closed branches. Returns laid-out cards and lines.
  - `GET /api/graph/expand`: a card, a direction and a number of levels.
    Returns only the new cards.
  - `GET /api/graph/context`: a card. Returns its parent, its siblings and
    the connecting lines (for going out).
  - `GET /api/graph/details/{id}`: one card's full information.
  - `GET /api/search`: query, plus `symbols`, `files` or `text`.
- The MCP tools `view`, `navigate` and `search` call the same functions.

**How the agent fits in.** The agent uses the same parts as the cockpit, so
it sees what you see.
- **`view`:** the same tree builder, with the same source, root, direction
  and depth. It gets text instead of positions, with the same "N more"
  counts and size notes. It can also load more and go out.
- **`navigate`:** moves *your* graph to a view, root and source. That
  includes switching you back to Code, or to a plan's own roots.
- **`search`:** the same search as the cockpit, now including file names and
  file contents.
- **`inspect`:** the same details you see when you click a card.
- **Plan edits:** when the agent edits a plan, that plan's layer is rebuilt.
  Your graph updates its marks without moving where you are.
- **Approvals** stay yours alone, in the cockpit, exactly as now.

**10. Cockpit** (crate `cockpit-ui`)
- **Does:** sends what you did, and draws what comes back. No layout, no
  search logic, no graph logic.
- **Keeps** only the cards it has been sent, up to its own cap. Past the
  cap, it drops the furthest closed branches first. They come back from the
  server in milliseconds if you return.
- **Draws** only what's on screen, and caches text measurements.
- The graph code is split into small files, one job each (see Phase 6).

### Updating after a code change

1. The watcher reports changed files. A debounced burst becomes one batch.
2. Each changed file is checked by size and time first. Only if those
   differ is it hashed.
3. The files that really changed are read in parallel (rayon).
4. Their facts are written to the store, one transaction per batch.
5. Linking is redone only for the changed files, plus the files that call
   names they define.
6. A new graph snapshot is swapped in. The plan overlays are rebuilt.
7. The cockpit is told that the graph changed, and it re-asks for what's on
   screen.

### The SQLite store in detail

- `rusqlite` with the `bundled` feature: no system SQLite needed, only a C
  compiler.
- WAL mode: readers never block the writer.
- One writer thread owns writing. Readers use a small pool.
- **Tables:**
  - `files`: path, size, time, hash, pack fingerprint;
  - `nodes`: id, file, kind, name, symbol, parent, span, signature;
  - `edges`: from, to, kind, file, sites, how sure we are;
  - `meta`: the schema version and the reader's version.
- **Indexes:** nodes by file, and by name. Edges by file, by `(from, kind)`
  and by `(to, kind)`.
- **Schema version** in `PRAGMA user_version`. If it doesn't match, or the
  reader's version changed, the file is rebuilt from the code.
- SQLite settings (cache size, sync mode) live in config, not in code.

## Part 4. Finding and fixing slowness

The goal is that we always know where time goes, slowness shows up before
you feel it, and nothing can freeze the UI even when something is slow.

**Limits that can't be broken.** All of these live in config:
- cards per answer (say 400);
- milliseconds per request;
- milliseconds per layout;
- the browser's card cap;
- the debounce window.

When a limit is hit, the answer is cut cleanly and says so ("showing 3 of
5 levels"), instead of freezing.

**Every stage is timed.** `tracing` spans cover:
- reading;
- storing;
- linking;
- building the snapshot;
- building the tree;
- layout;
- encoding;
- search.

Anything over its limit is logged, with what was asked and how big it was.

**A performance endpoint.** `GET /api/perf` shows:
- the last N requests, with time and size per stage;
- the graph's size and memory;
- the store's size;
- the last index run's times.

The cockpit's status line shows when an answer was cut or slow.

**Performance tests that fail on regressions:**
- A fixed set of repos: this one, plus one big open-source Rust project
  pinned to a commit.
- Each test asserts time and size limits for:
  - a full index;
  - a one-file update;
  - every view at depths 1, 4 and 12;
  - expand and context;
  - search.
- `criterion` benchmarks for the hot spots: linking, the tree walk, layout
  and fuzzy search.
- A browser check (a CDP script) that opens each view on the big repo and
  fails if any frame takes longer than the frame limit.

**Ways to fix slowness without redesigning:**
- **Slow tree walks:** tune the budget, or cache popular roots' trees per
  snapshot.
- **Slow linking:** narrow the "affected files" set, or add a name index to
  the store.
- **Too much memory:** move cold data (signatures, spans) out of the
  snapshot, and read it from SQLite on demand through the same `GraphRead`
  trait.
- **A slow store:** tune the SQLite settings in config, or replace
  `SqliteStore` behind `FactStore`.
- **A slow browser:** lower its card cap in config, or simplify the drawing
  at low zoom.

## Part 5. Crates

| Need | Crate | Replaces |
|---|---|---|
| Store on disk | `rusqlite` (bundled) | 154 pretty JSON files |
| Snapshot swap | `arc-swap` | copying the graph per request |
| Parallel reading | `rayon` | the one-file-at-a-time loop |
| Small ids | `lasso` (string interning) | string ids everywhere |
| Fuzzy matching of any list of strings | `neo_frizbee` (fff's matcher; also builds for the browser) | three hand-written loops |
| File name and content search | `fff-search` 0.11 | nothing (we don't have it) |
| File watching | `notify` + `notify-debouncer-full`, at fff's version | our hand-made debounce |
| Timing | `tracing` | nothing |
| Benchmarks | `criterion` | nothing |
| Accurate calls (later) | `scip` + `rust-analyzer scip` | guessing calls by name |

**What to stay away from:**
- Kuzu (archived), CozoDB (abandoned), IndraDB (dormant).
- SurrealDB: heavy, with a restrictive license.
- HelixDB: its API keeps changing.
- `bincode`: abandoned.
- `sled`: stuck in alpha.
- `stack-graphs`: archived.

**ast-grep:** we keep it, and use more of it:
- rule captures instead of re-reading text and counting brackets;
- `inside`, `has` and `stopBy` instead of hand-written ancestor walks;
- types from the syntax tree;
- each file parsed once.

## Part 6. The to-do list

Each phase ends with its tests passing and one commit. Old code goes in the
same commit as its replacement: no leftovers.

### Phase 0. Measure first
1. Pick the big repo (for example `tokio` at a fixed commit), and a script
   that fetches it into the cache folder.
2. Performance tests for today's code: full index, one-file update, every
   view at depths 1, 4 and 12, and search. Record time, size and memory.
3. The browser check script: open each view and record the longest frame.
4. Write the numbers into `spec/perf-baseline.md`.
4b. Screenshot today's layouts on this repo (each view, depths 1, 4 and
    8). These are the bar the new layout has to meet.

### Phase 1. Code vs plan: fix the source bug
5. Replace "plan or nothing" with `Source::Code` or `Source::Plan(id)`
   everywhere: arch-graph, service, REST, MCP and cockpit.
6. "Code" means code only, everywhere, including the code view's margin.
7. An agent can switch you back to Code.
8. If a plan is deleted, fall back to Code and say so.
9. Tests for each.

### Phase 2. The store (`arch-store`)
10. Create the crate, with the `FactStore` trait.
11. `SqliteStore`: schema, indexes, WAL, a writer thread and a reader pool,
    and settings from config.
12. Schema and reader versions: rebuild when they don't match.
13. `replace_file`, `remove_file`, `fingerprints` and `load`.
14. Move the service from the JSON cache to the store. Delete the JSON
    cache code and the files.
15. Tests:
    - storing and loading gives back the same facts;
    - one file's update only touches its rows;
    - a version mismatch triggers a rebuild;
    - a corrupt file triggers a rebuild.

### Phase 3. The graph snapshot and plan overlays
16. Interned ids (`lasso`) and adjacency tables per relation kind, both
    ways.
17. The `GraphRead` trait, and the in-memory implementation.
18. Build the snapshot from the store. Swap it with `arc-swap`.
19. Plan overlays, built once per plan and rebuilt when the plan or the
    code changes. Marks: new, changed, done, doesn't match.
20. "Also changed by plan X" badges.
21. Plans held in memory, reloaded per file change.
22. Delete:
    - the per-request graph copy;
    - `Graph::merge` per request;
    - the slow add (the one that scans every file and name);
    - the BTreeMap-of-strings graph wherever it's replaced.
23. Tests: lookups match the old graph on fixtures, requests make no
    copies, and overlay marks are right.

### Phase 4. Updates: incremental and parallel
24. Change detection: size and time first, hash only if they differ.
25. Parallel reading with rayon.
26. Linking only the affected files: the changed ones, plus the callers of
    names they define.
27. Watcher on `notify-debouncer-full`, at the same `notify` version as fff.
28. The update pipeline (watch → read → store → link → swap → tell the
    cockpit), with each step timed.
29. Delete the full re-link on every change, and the full cache-folder
    reads.
30. Tests:
    - one-file change gives the same graph as a full re-index;
    - an unchanged file is never re-read;
    - timing stays within the limits on the big repo.

### Phase 5. Tree builder, chunks and layout
31. The shared walk: budget, "N more" counts, repeats, loops, order of
    calls.
32. The Contains, Calls and Data flow builders.
33. Default roots:
    - Contains: the repo.
    - Calls: a list of entry points (main, routes, MCP tools) to pick from.
    - Data flow: the types those entry points take in.
34. Expand: N more levels under one card, only the new cards.
35. Context: parent, siblings and connecting lines, for going out.
36. Plan focus:
    - with a plan picked, start from the plan's own roots (its top-level
      items, per view);
    - show only the existing code it plugs into;
    - the "show in full code" switch.
37. Layout: a treemap for Contains, a tidy tree for Calls and Data flow,
    curves for repeats and loops, and pills placed on lines.
38. Text sizes from the theme's font settings.
39. Time limit on layout. A cut answer says so.
40. Endpoints: tree, expand, context and details. MCP `view` and
    `navigate` use the same code.
41. Delete:
    - `/api/arch/view` and its 700 KB to 1.6 MB `text` field;
    - the old general layout (`place()`);
    - the routing grid;
    - `grain.rs`;
    - the story layout;
    - the 60-box limit;
    - the views story, reached_by, data_path and combined;
    - the structure view's lines.
42. Tests:
    - each builder on fixtures;
    - every view on the big repo within the limits;
    - expand plus context gives the same cards as a deeper tree.
    - layout quality: the existing `problems()` check becomes a hard test
      on every view of both repos (no overlapping cards, no gaps under the
      minimum, no labels on cards, no lines through cards);
    - you compare the new layout with the screenshots side by side. The
      old layout code is deleted only after you agree it's as good or
      better.

### Phase 6. The cockpit graph, rebuilt and split up
43. Split `graph.rs` (4,278 lines) into:
    - `graph/request.rs`: what you asked for (view, source, root,
      direction, depth, open branches, back stack);
    - `graph/cards.rs`: the cards held, the cap, and dropping far branches;
    - `graph/draw.rs`: cards, lines and pills, only those on screen;
    - `graph/select.rs`: selection and highlighting;
    - `graph/details.rs`: the details card;
    - `graph/source.rs`: the Code/plan picker and the "show in full code"
      switch;
    - `graph/actions.rs`: the graph's actions and keys.
44. Actions: select, open, close, go in, back, depth up and down, flip
    direction, switch view, pick source. Nothing else.
45. Keys in `config/default.toml`, as in Part 2.
46. Highlighting on selection, exactly as it works today.
47. The path of roots at the top. Click one to go back to it.
48. "N more" on cut cards loads that branch.
49. The size note, with its fixes as buttons.
50. Zoom and pan only move the camera.
51. Work out the action list once per change, not twice per frame.
52. Delete:
    - zoom-driven levels and `<`/`>`;
    - focus and unfocus;
    - "show all";
    - off-screen markers;
    - "+N more" crowding;
    - story, reached by and follow;
    - `]e`/`[e`;
    - "up";
    - the automatic re-focus;
    - the gliding re-layout;
    - the cockpit's scene-building code.
53. Tests for each action, and for the card cap dropping far branches.
54. Browser check: every view on the big repo, no frame over the limit.

### Phase 7. Search (`arch-search`)
55. Create the crate, with the `Search` trait.
56. Symbols: `neo_frizbee` over the snapshot's names, rebuilt with each
    snapshot.
57. Files and content: `fff-search`, confined to the repo.
58. `GET /api/search`, and MCP `search`, on the same code.
59. The cockpit's `/` and the palette use `neo_frizbee`.
60. A "search everything" entry in the cockpit. Picking a result makes it
    the root.
61. Delete `rank_matches`, `fuzzy_score` and the service's search loop.
62. Tests: ranking, typos, confinement to the repo, and speed on the big
    repo.

### Phase 8. Performance visibility
63. `tracing` spans on every stage, and logs for anything over its limit.
64. `GET /api/perf`.
65. Cut or slow answers shown on the cockpit's status line.
66. `criterion` benchmarks for linking, the tree walk, layout and search.
67. Performance tests on both repos run in the normal test suite, and fail
    on going over a limit.

### Phase 9. Reading code with more ast-grep, less hand-written code
68. Parse each file once. Run every rule (definitions, calls, flow,
    placeholders, code rules) on that one parse.
69. Calls: capture the receiver and the method with ast-grep, instead of
    re-reading text and counting brackets.
70. Types: read from the syntax tree, instead of stripping strings.
71. Replace the ancestor walk (`directly_in`) with `inside` and `stopBy`
    rules.
72. Replace regexes over code text with captures where the grammar allows.
    Keep a regex only where the text really is unstructured (macro bodies),
    and say why.
73. Replace the home-made `cfg` condition parser with captures from the
    syntax tree.
74. Fix the known mislinks: a local variable named like a function, and
    nested functions with the same name.
75. Tests: the same facts as before on the ingest fixtures, plus the
    mislink cases.

Done (2026-10-08), with two decisions:
- Types stay text (item 70): plans write types as text, and code and plans
  must be matched the same way, so one text normalizer serves both.
- The cfg evaluator stays text (item 73): tree-sitter gives a `cfg(...)`
  attribute as a flat run of tokens, not an expression. It only calls code
  dead when every part is constant; anything unclear counts as live.
- The ancestor walk (item 71) went away entirely rather than becoming an
  `inside`/`stopBy` rule: the one traversal already knows the enclosing
  definition, so bases are collected there.

### Phase 10. Accurate calls (after the rest works)
76. Run `rust-analyzer scip` when it's installed, and read the result with
    the `scip` crate.
77. Join it to ast-grep call sites by position. A call SCIP confirms is
    "proven".
78. Without SCIP, keep name matching, and mark those calls "unconfirmed".
79. Tests on fixtures with same-name functions.

### Phase 11. Docs and proof
80. Update the explainer to the new model. Remove items for deleted things,
    and regenerate its data.
81. Update the README: graph section, keys, architecture, and the perf
    endpoint.
82. A requirements slice for this work, each item tied to its test.
83. Final numbers recorded next to the baseline.

## Part 7. Decisions for you

1. **SQLite as the store**, with the graph in memory on the server. I
   recommend yes.
2. **Layout on the server.** I recommend yes.
3. **fff-search** for files and content, and `neo_frizbee` for everything
   else. I recommend yes, pinned to fixed versions.
4. **The big test repo.** I suggest `tokio` at a fixed commit. Name another
   if you have one in mind.
5. **The limits.** I suggest:
   - 400 cards per answer;
   - 200 ms per request;
   - 1,000 cards held in the browser;
   - 16 ms per frame.
6. **The default source.** I suggest the plan under review if there is one,
   opened on its own roots. Otherwise, Code.
7. **The order.** I'd do phases 0 to 6 first, which fix the freezing and
   the navigation. Then 7 to 11.

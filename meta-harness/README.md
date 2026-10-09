# NAH (not a harness)

NAH sits between you and coding agents (Antigravity today, over ACP). It reads
your code into an architecture graph, lets an agent plan changes against it,
checks those plans before any code is written, runs the work one agent session
per component, and verifies that the code matches the plan. You steer and
approve from the cockpit, a keyboard-driven web UI.

The code is the source of truth. The graph is rebuilt from it (and cached);
only plans are stored.

## Run it

```sh
make ui                                   # build the cockpit (WASM)
cd meta-harness && NAH_REPO_ROOT=/path/to/repo cargo run --bin meta-harness
```

The server prints the cockpit's launch link:

```
Open the cockpit: http://127.0.0.1:8080/cockpit?nah=<token>
```

Open the cockpit through that link once per browser: it sets the session that
approvals and other user-only actions need. The token is kept in
`~/.config/nah/cockpit-token`; you can set your own with `NAH_COCKPIT_TOKEN`.
Agents connect to the MCP endpoint at `/api/mcp/sse`, which every session
NAH opens is given automatically.

The agent harness and each language's indexer are packs, installed by the
binary itself (see [Packs](#packs)). To build and install the ones you need:

```sh
scripts/bootstrap.sh antigravity-acp rust-analyzer
```

## How work flows

1. **Ingest.** Our analyzer reads the repo and, where the language's own
   indexer is installed, combines what it says (see [Analysis](#analysis)).
   NAH finds:
   - components (modules, types, interfaces, functions, operations);
   - containment;
   - calls, each marked proven (by the language's indexer) or matched by
     name;
   - implements;
   - typed data flow;
   - placeholders and code rule hits.

   After the first read, ingestion is incremental, and a file watcher keeps
   the graph current.
2. **Plan.** An agent writes a plan:
   - new or changed components, placed under real parents;
   - specs (logic steps, invariants, tasks);
   - data schemas (a shorthand DSL that compiles to JSON Schema);
   - calls with expectations;
   - data flows.

   A plan must connect to existing code unless it is marked greenfield.
3. **Check.** `plan_validate` runs every rule:
   - types and their allowed parents;
   - loops;
   - schema fit across implements, calls and data flow, including against
     existing code's real signatures;
   - spec rules and required details per type;
   - dependency gates;
   - proposed locations;
   - logic-step check scripts and project code rules.
4. **Approve.** Only you approve, in the cockpit: the architecture first, then
   the detailed plan. Agents have no approval path. Comments and change
   requests go to the agent as review points, and approval waits until you
   accept each answer.
5. **Build.** The work queue takes each component's next job in dependency
   order: design, plan, skeleton, then implement. Each job runs in its own
   session with a brief built from templates. Design work waits for your
   approval. Failed attempts run again with their findings, up to a limit.
6. **Verify.** Verification checks that:
   - the symbol is where it was planned and is the planned kind;
   - every planned call happens in its body (not in dead code);
   - the signature matches;
   - planned data flows show up;
   - no placeholders are left, including in helpers it calls;
   - project tools and reviewers pass.

   Only a clean verification moves a component to Implemented. A later
   change that breaks the code demotes it again.

## Analysis

There is one path from code to graph, the same for every language:

1. **Read.** Our analyzer (`code-analyzer`, built into the binary) parses
   each file once with ast-grep, by its language pack. From that parse it
   takes definitions, calls with the exact span of the called name, data
   flow, placeholders, code rule hits, and syntax colours (the pack's
   `highlights.scm`, run over the same tree). This always runs.
2. **Index.** If the language's indexer pack is installed (rust-analyzer,
   scip-typescript, scip-python), it runs in the background and writes a
   SCIP index. Each file's part is kept with the file's content hash at the
   time, so an edited file never uses an old index.
3. **Combine.** Both are joined by position, once. A call whose name the
   indexer resolved is **proven**, by that indexer and version, even where
   names alone couldn't choose. Otherwise names link it, and it stays
   **unconfirmed**. Data flows through a call follow the call's target, and
   are proven only when both ends are. Nothing read in step 1 is dropped.
4. **Store.** The result, and what each name in each file refers to, goes
   into SQLite (`.nah/cache/index.sqlite`) in one transaction. The graph,
   the views, the agent's tools and the code view only read that result.
   Nothing is filled in later.

Without an indexer everything still works; links are just unconfirmed. On
this repo, rust-analyzer takes about 76 s and proves about 7,600 links;
unproven calls go from about 2,850 to 9.

`meta-harness analyze --root <repo> --out <file.json>` runs the same steps
once and writes the result, for checking from outside.

## Packs

Everything NAH installs or reads a language with is a versioned pack: a
folder with a `pack.toml`.

| Kind | What it gives | Shipped |
|---|---|---|
| `harness` | an agent harness's program, for a driver | `antigravity-acp` |
| `language` | how to read a language (`syntax.toml`), its colours (`highlights.scm`), and its indexer | `rust`, `python`, `typescript` |
| `indexer` | a program that writes a SCIP index, and how to run it | `rust-analyzer`, `scip-typescript`, `scip-python` |

A manifest says:

- the pack's version and the tool versions it supports;
- how to check the installed tool's version;
- the install steps for each platform (downloads carry a checksum);
- how to run it: command, arguments, environment, output, time limit.

Manifests are checked strictly when loaded. The installer is generic: it
knows kinds of steps, not tools. Every URL, version, path, checksum and
platform name is in pack or config files. It installs into a staging
folder, swaps it in, and records it in a lock file.

Packs come from `packs/index.toml`, in order: the ones shipped in the
binary, then `<project>/.nah/packs/`, then `nah/packs/` in your config
folder (`~/.config` on Linux). Any folder holding a `pack.toml` is a pack. A
later pack of the same name replaces an earlier one. Platform names are in
`packs/platforms.toml`. Installed packs and their lock file live in
`nah/` under your data folder (`~/.local/share` on Linux).

```sh
meta-harness pack list            # every pack: kind, version, state
meta-harness pack check [name]    # installed, out of range, outdated or broken
meta-harness pack install <name>
meta-harness pack update [name]   # reinstall what's outdated or out of range
meta-harness pack which <name>    # where its program is
```

## The cockpit

The cockpit has six views: chat, code, graph, tree, spec and queue. Each
slot (column) shows one. Every view has a cursor, and the same keys drive
all of them:

- **Move:**
  - `j`/`k` (and `h`/`l` in the graph and tree), with counts (`3j`);
  - `ctrl+d`/`ctrl+u` for half a page;
  - `gg`/`G` for first and last;
  - `]]`/`[[` for siblings (sections in the spec, definitions in the code);
  - `Enter` to go in, `Backspace` to go out.
- **Find:**
  - `f` labels what's on screen (type a label to go there; `F` opens it);
  - `/` searches the view, `n`/`N` step through matches;
  - `g /` in the graph finds anything in the code (symbols, file names,
    lines) as you type; picking one makes it the root. The same search is
    `GET /api/search?q=&kind=symbols|files|text` and the agent's `search`;
    names match letters in order (neo_frizbee), files and lines come from
    fff-search;
  - `z o`/`z c`/`z a`/`z R`/`z M` fold.
- **Go to**, from whatever the cursor is on:
  - `g d` code, `g s` spec, `g m` graph, `g t` outline, `g q` queue job;
  - `g r` callers, `g c` callees, `g p` parent, `g n` members,
    `g i` implements, `g l` data flow.
- **Come back:** `ctrl+o`/`ctrl+i` walk one jump list across all views and
  slots; `m` plus a letter sets a mark and `'` plus the letter returns to it.

The key bar at the bottom shows the keys that matter for what the cursor
is on. Pausing on a prefix (`g`, `z`, `[`, `]`, `m`, `'`) lists what can
follow, and `?` lists every key of the focused view. `Space` opens the
leader menu (the focused view's entries first), and `:` opens the command
palette (`:42` goes to line 42).

Views also have their own keys:

- **Graph:** three views, each a tree from one root that the server
  builds and lays out (the browser only draws it):
  - `contains`: boxes inside boxes, no lines;
  - `calls`: functions in the order they're called; down is what it
    calls, up is what calls it;
  - `data_flow`: data types as cards, with the functions that turn one
    into another on the lines; down is where it goes, up is where it came from.

  The same actions in all three:
  - select (click, or `h`/`j`/`k`/`l`): what it's tied to stays bright,
    the rest dims, and the details card opens; the pills on its lines say
    what goes in and comes back;
  - open or close a card in place (`z o` / `z c`, or click its `+N`);
  - go in (`Enter`, double-click): the card becomes the root; back
    (`Backspace`) returns along the trail shown above the graph;
  - more or fewer levels (`+` / `-`), flip down and up (`r`), next view (`v`);
  - pick the code or one plan (the Source picker). A plan starts from its
    own items, shows only the code they plug into, and marks cards new,
    changed, done or doesn't match; "Full code" shows it in the whole codebase.

  Nothing changes on its own: zoom only zooms. A tree bigger than one
  answer holds is cut at a whole level and says so ("showing 2 of 3
  levels"); every cut card says how much is under it. A line matched only
  by name is drawn faded, and its details say so; a proven one names the
  indexer that confirmed it.
- **Code:** coloured by the language pack, from the stored analysis
  (`GET /api/code/analysis?path=`). Hovering a name says what it refers
  to. `g d` on a name, or ctrl/cmd-click, goes to its definition.
  `]f`/`[f` move between definitions. A file edited since it was analysed
  is marked stale until it's read again. Colours map to theme tokens in
  `[code.highlight]`.
- **Spec:**
  - `J`/`K` move between sections;
  - `c`/`C` comment on or ask for a change to the item under the cursor;
  - `r`/`a`/`x` reply to, accept or reopen a review point.
- **Queue:** `a` approve, `r` retry, `o` open the job's session.

All keys are configurable in `crates/cockpit-ui/config/default.toml`.

### The explainer

`/explainer/` on the same server explains the cockpit and its graph item by
item. Each item has:

- a live demo you drive with the real keys;
- what happens, step by step;
- what triggers it, for you and for the agent;
- what it ties to, and where it's built;
- an honest status.

It also has workflows as step-by-step lane diagrams, a comparison of the
ways to see more or less, and the words that mean more than one thing.

Its reference (every action and key, every agent tool, every view, the
look) is generated by tests into `static/explainer/data/`. Those tests fail
when the files are stale; regenerate them with
`UPDATE_EXPLAINER=1 cargo test`.

Questions and notes left on the page are kept in `.nah/explainer/notes.json`.
Anyone local can read them at `/api/explainer/notes`, the agent included.
Only your cockpit session can write them.

## Configuration

Built-in defaults live in each crate's `config/` folder. A project adds to or
overrides them in its own `.nah/` folder:

| File | What it sets |
|---|---|
| `.nah/types.toml` | component types (`extends` a built-in to specialize it) |
| `.nah/trees.toml` | the three views: default depth, starting points, layout sizes |
| `.nah/limits.toml` | cards per answer, time limits, the fact store, performance limits |
| `.nah/packs/**/pack.toml` | more packs, or replacements for shipped ones |
| `.nah/rules/*.toml` | forbidden or required ast-grep patterns, per type |
| `.nah/tools.toml` | validation tools, and the shell for logic-step checks |
| `.nah/reviewers.toml` | reviewers: rule-based, or run as agent sessions |
| `.nah/queue.toml` | queue settings and session brief templates |
| `.nah/schema_dsl.toml` | extra shorthand type names for schemas |

How long things take is visible: every answer is timed stage by stage
(tracing spans), anything over `slow_request_ms` is logged and marked "slow"
in the graph, and `GET /api/perf` lists the last answers by stage with the
graph, store and memory sizes. Benchmarks: `cargo bench -p arch-service
--bench hot`.

Plans live in `.nah/plans/`. The analysis is kept in
`.nah/cache/index.sqlite`, which isn't committed. It holds what was read
from each file (with its size and time), what the indexers said about
each file, the combined result, and each file's named places, all written together in one transaction. On
start, the stored result is loaded as it is unless the packs, the types or
the combine step changed; then it's combined again without reading a file.
Afterwards only files whose size or time changed are read again, in
parallel. It's thrown away and rebuilt when the reader changes.

## Agent tools (MCP)

| Area | Tools |
|---|---|
| Explore | `ingest`, `list_types`, `search`, `inspect`, `view`, `navigate` (moves your graph) |
| Plan | `plan_list`, `plan_create`, `plan_import` (old `system_architecture.json`), `plan_edit`, `plan_validate`, `contract` |
| Build | `verify`, `run_tools`, `review` |
| Review | `review_comments`, `answer_comment`, `reply_comment` |

Every result ends with next steps, given in dependency order.

## Code map

| Crate | What it holds |
|---|---|
| `arch-graph` | the domain: types, components, relations, plans, schemas, views, scenes (how a view is laid out and routed), queue shapes (no I/O; shared with the cockpit) |
| `code-analyzer` | our analyzer: reads each file with ast-grep by its language pack (with syntax colours and code rules), runs indexer packs and reads their SCIP, combines both by position |
| `nah-packs` | packs (harnesses, languages, indexers) and the one installer |
| `arch-store` | where the analysis is kept between runs (SQLite) |
| `arch-search` | one way of matching what's typed: names with neo_frizbee, files and lines with fff-search |
| `arch-service` | plans and cache, plan operations and checks, verification, tools, reviewers, review loop, queue state |
| `src/` (server) | REST (`/api/arch/...`), MCP, events over WebSocket, the queue runner, the cockpit session |
| `cockpit-ui` | the egui/WASM cockpit |
| `harness-*`, `provider-antigravity` | driving agent harnesses over ACP |
| `workspace-fs` | file access confined to the workspace (no path or symlink escapes) |

## Progress and what's left out

`spec/requirements.toml` lists every requirement and the test that proves
it. Check status with `python3 scripts/req_status.py --full`.
`spec/deferred.toml` lists what was deliberately left out, with why. For
example, it doesn't defend against an agent that edits plan files directly.

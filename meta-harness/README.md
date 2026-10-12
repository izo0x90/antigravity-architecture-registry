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

`make ui` is required: the cockpit's build (`static/wasm/`) isn't in git,
and the server won't start without it, saying to run `make ui`.

The server prints the cockpit's launch link:

```
Open the cockpit: http://127.0.0.1:8080/cockpit?nah=<token>
```

Open the cockpit through that link once per browser: it sets the session that
approvals and other user-only actions need. The token is kept in
`~/.config/nah/cockpit-token`; you can set your own with `NAH_COCKPIT_TOKEN`.
Agents connect to the MCP endpoint at `/api/mcp/sse`, which every session
NAH opens is given automatically.

The agent harness and each language's server are packs, installed by the
binary itself (see [Packs](#packs)). To build and install the ones you need:

```sh
scripts/bootstrap.sh antigravity-acp rust-analyzer
```

`make check` runs the lints and every test. The tests need `git` and
`python3` (stand-in language servers). A test that needs something more
(the network, `ast-grep`, Chrome, an installed harness) ends its name with
`_needs_` and what it needs, and without it prints `skipped:` and why. The
harness ones start the real agent (about a minute each) when its pack is
installed; the browser one runs when `NAH_FRAME_CHECK_URL` names a cockpit.

## How work flows

1. **Ingest.** Our analyzer reads the repo (see [Analysis](#analysis)).
   NAH finds:
   - components (modules, types, interfaces, functions, operations);
   - containment;
   - calls, each marked confirmed (by the language's server) or matched by
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
2. **Link.** Names link calls, implements and data flow across files,
   within one language. A link names alone can't settle stays
   **unconfirmed**. Nothing read in step 1 is dropped.
3. **Store.** The result, and what each name in each file refers to, goes
   into SQLite (`.nah/cache/index.sqlite`) in one transaction. The graph,
   the views, the agent's tools and the code view only read that result.

Each language's server (rust-analyzer, the TypeScript server, basedpyright),
when its pack is installed, is started in the background and told which
files change. Once it's ready it's asked about every name read in step 1
that a link rests on: `definition` for each call, each type named and each
field used, and `documentHighlight` for fields (whether a use reads or
writes it). What it says wins over names and is kept with its name and
version; a link it no longer backs is asked about again, not dropped. Only
the files a change reaches are asked about. Its state, and why when it
isn't ready, shows on the index light.

Every link says how it's known: what holds what is read from the code's
structure; calls, implements and data flow are confirmed by the language's
server (named, with its version) or matched by name only.

The calls view starts where the code is entered: functions a language
starts a program at (its pack's `entry_names` in `syntax.toml`, `main` for
Rust), then the receivers of the boundaries the pack marks as entries: a
route's handlers (`entry = "received"`) and the call that takes the MCP
tools the code lists (`entry = "declared"`). An event handled inside the
program isn't a way in. Data flow starts at the data types those take.

`meta-harness analyze --root <repo> --out <file.json>` runs the server's
pipeline once and writes what it stored, for checking from outside.

## Packs

Everything NAH installs or reads a language with is a versioned pack: a
folder with a `pack.toml`.

| Kind | What it gives | Shipped |
|---|---|---|
| `harness` | an agent harness, and how its driver starts it (program, arguments, environment) | `antigravity-acp` |
| `language` | how to read a language (`syntax.toml`), its colours (`highlights.scm`), and its server | `rust`, `python`, `typescript` |
| `server` | a language server: how to start it, what to send it, its time limits, where its caches go | `rust-analyzer`, `typescript-server`, `basedpyright` |

A manifest says:

- the pack's version and the tool versions it supports;
- how to check the installed tool's version;
- the install steps for each platform (downloads carry a checksum):
  download (and unpack a zip, tar.gz or Debian package, all of it or only
  some members), run a program, link inside the install folder, create a
  system group (as root only), take a certificate from the system's store,
  append a file to members of an archive. Downloads, programs run, checks
  and group creation have time limits (`packs/index.toml`, or the step's
  own); links, certificates and archive patches are local file work, with
  none. An install entry can also name files it leaves (`holds`) that its
  check can't reach: a missing one makes the pack broken;
- how to run it: command, arguments, environment, output, time limit. A
  harness's launch may differ per platform.

A harness pack's check can be `acp`: the harness is started and sent ACP
`initialize`, and the version it answers with must be one the pack
supports. A harness that isn't installed, fails its check or is out of
range gets no launch: its status (the cockpit's AGENT light) says why and
which command fixes it. An instance's own launch, from `harnesses.toml` in
your config folder's `nah/` or the project's `.nah/` (the project's wins),
replaces its pack's (a driver's default instance is named after it).
Re-probing the harness (in the cockpit) looks its launch up again, so a
pack installed or fixed meanwhile is used without a restart:

```toml
[antigravity]
program = "/opt/agy/agy_acp_server.par"
args = []
env = { SOME_VAR = "value" }
```

Manifests are checked strictly when loaded. The installer is generic: it
knows kinds of steps, not tools. Every URL, version, path, checksum and
platform name is in pack or config files. It installs into a staging
folder, swaps it in, and records it in a lock file.

Packs come from `packs/index.toml`, in order: the ones shipped in the
binary, then `<project>/.nah/packs/`, then `nah/packs/` in your config
folder (`~/.config` on Linux). Any folder holding a `pack.toml` is a pack. A
later pack of the same name replaces an earlier one. Platform names are in
`packs/platforms.toml`, matched on the operating system, the architecture
and, where only some machines can run a build, the memory page size; the
most specific match wins, and a pack without steps for it uses the general
one. `linux-arm64-16k` (Linux arm64 with 16 KiB pages, as in VMs on Apple
Silicon) is a developer platform: the sandbox this is built in, where the
Antigravity pack runs Google's x86_64 server under QEMU with an x86_64
glibc, all inside the pack's own folder. Installed packs and their lock file live in
`nah/` under your data folder (`~/.local/share` on Linux).

```sh
meta-harness pack list            # every pack: kind, version, what the lock records (runs nothing)
meta-harness pack check [name]    # runs the checks: installed, out of range, outdated or broken
meta-harness pack install <name>
meta-harness pack update [name]   # reinstall what's outdated or out of range (tools found on PATH too)
meta-harness pack which <name>    # how it's started (an instance's own launch, if set)
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
    names match letters in order (neo_frizbee); file paths and lines are
    found with fff-search, less what the one definition of ignored leaves
    out;
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
  - `contains`: boxes inside boxes, no lines; each row of boxes fills its
    parent's width, a box as big as what it holds needs;
  - `calls`: functions in the order they're called, starting from the
    entry points; down is what it calls, up is what calls it;
  - `data_flow`: data types as cards, with the functions that turn one
    into another on the lines; down is where it goes, up is where it came from.

  Calls and data flow are tidy trees, left to right: each level a column,
  each card centred on its children, a small branch tucked in beside a big
  one, nothing overlapping. Cards are as wide as their text, measured with
  the cockpit's own font. Laying out has its own time limit
  (`layout_ms`); what didn't fit in time is left out and said so.

  The same actions in all three:
  - select (click, or `h`/`j`/`k`/`l`): what it's tied to stays bright,
    the rest dims, and the details card opens; the pills on its lines say
    what goes in and comes back;
  - open or close a card in place (`z o` / `z c`, or click its `+N`);
  - go in (`Enter`, double-click): the card becomes the root; back
    (`Backspace`) returns along the trail shown above the graph;
  - more or fewer levels (`+` / `-`), flip down and up (`r`), pick the
    view (`v` lists them);
  - pick the code or one plan (the Source picker). A plan starts from its
    own items, shows only the code they plug into, and marks cards new,
    changed, done or doesn't match; "Show in full code" shows it in the
    whole codebase;
  - find anything in the code (`g /`), and go back to where you were
    before the agent moved the view.

  Nothing changes on its own: zoom only zooms. A tree bigger than one
  answer holds is cut at a whole level and says so ("showing 2 of 3
  levels"), with buttons for the fixes (show only the levels that fit, go
  into the selected card); every cut card says how much is under it. A
  line matched only by name is drawn faded, and its details say so; a
  confirmed one names the server that confirmed it. The legend (the
  button in the graph's corner) explains the cards, lines and how each
  link is known.

  The views are GETs, their request in the query: `GET
  /api/graph/tree|expand|context?view=calls&source=code&way=down&root=&depth=`
  (`opened` and `closed` as JSON lists, `full_code=true` to show a plan in
  the whole codebase; expand and context add `card`, expand `levels`). A
  source is text everywhere (the API, the agent's tools, the cockpit):
  `code`, or `plan:<id>`. An argument a read doesn't know is refused.

  The server keeps the trees it sends. Opening a card (`expand`, asked with
  the request the tree held answers) walks only that card's branch, merges
  it in, keeps the whole to `cards_held` (closing the opened branches
  furthest from it) and lays it out again; the cockpit gets the new cards,
  every card's place and the lines, and places nothing itself.
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

## Configuration

Built-in defaults live in each crate's `config/` folder. A project adds to or
overrides them in its own `.nah/` folder:

| File | What it sets |
|---|---|
| `.nah/types.toml` | component types (`extends` a built-in to specialize it) |
| `.nah/trees.toml` | the three views: default depth, starting points, layout sizes |
| `.nah/limits.toml` | cards per answer, time limits (laying out: `layout_ms`), the fact store, performance limits |
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
--bench hot`. Measured on this repo and on tokio (717 files): a full index
takes about 1.3 s and 1.5 s, a one-file change 0.25 s and 0.08 s, a restart
about 0.55 s (slower than before the rebuild's 0.36 s and 0.44 s), and a view 0
to 11 ms on the server; the real rust-analyzer
confirms a changed file's links in 0.1 to 0.35 s once warm (71 to 327 ms measured).

Plans live in `.nah/plans/`. The analysis is kept in
`.nah/cache/index.sqlite`, which isn't committed: a table per kind of row
(files, components, refs, relations, flows, placeholders, rule hits,
highlights, the servers' states), each row under its file. One thread
writes them; a change is one transaction, and reads use their own
connections. On start the rows are loaded as they are, unless the packs,
the types or the linker changed; then they're linked again without reading
a file. Afterwards only files whose size or time changed are read again, in
parallel, and linked with the files whose refs they can affect. It's thrown
away and rebuilt when the reader changes.

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
| `code-analyzer` | our analyzer: reads each file with ast-grep by its language pack (with syntax colours and code rules) and links names across files |
| `nah-packs` | packs (harnesses, languages, servers) and the one installer |
| `nah-lsp` | the one language server client: starts a server in the background, knows when it's ready, asks where names are defined, restarts it when it ends |
| `arch-store` | where the analysis is kept between runs (SQLite) |
| `arch-search` | one way of matching what's typed: names with neo_frizbee; files and lines with fff-search |
| `arch-service` | plans and cache, plan operations and checks, verification, tools, reviewers, review loop, queue state |
| `src/` (server) | REST (`/api/arch/...`), MCP, events over WebSocket, the queue runner, the cockpit session |
| `cockpit-ui` | the egui/WASM cockpit |
| `harness-*`, `provider-antigravity` | driving agent harnesses over ACP |
| `workspace-fs` | file access confined to the workspace (no path or symlink escapes) |

## Left out

It doesn't defend against an agent that edits plan files directly.

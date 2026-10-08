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

## How work flows

1. **Ingest.** The repo is read with ast-grep through language packs: Rust,
   Python and TypeScript are built in. Packs are pure data, so a new
   language needs no engine code. NAH finds:
   - components (modules, types, interfaces, functions, operations);
   - containment;
   - calls, resolved only on evidence;
   - implements;
   - typed data flow;
   - placeholders.

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
  levels"); every cut card says how much is under it.
- **Code:** `]f`/`[f` move between definitions.
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
| `.nah/packs/*.toml` | more language packs |
| `.nah/rules/*.toml` | forbidden or required ast-grep patterns, per type |
| `.nah/tools.toml` | validation tools, and the shell for logic-step checks |
| `.nah/reviewers.toml` | reviewers: rule-based, or run as agent sessions |
| `.nah/queue.toml` | queue settings and session brief templates |
| `.nah/schema_dsl.toml` | extra shorthand type names for schemas |

Plans live in `.nah/plans/`. What was read from the code is kept in
`.nah/cache/index.sqlite` (one row per file, with its size and time), which
isn't committed: on start the graph is linked from it without reading a file,
and afterwards only files whose size or time changed are read again, in
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
| `arch-index` | ingestion: language packs, extraction, resolution, code rules |
| `arch-store` | where the facts read from the code are kept between runs (SQLite) |
| `arch-service` | plans and cache, plan operations and checks, verification, tools, reviewers, review loop, queue state |
| `src/` (server) | REST (`/api/arch/...`), MCP, events over WebSocket, the queue runner, the cockpit session |
| `cockpit-ui` | the egui/WASM cockpit |
| `harness-*`, `provider-antigravity` | driving agent harnesses over ACP |

## Progress and what's left out

`spec/requirements.toml` lists every requirement and the test that proves
it. Check status with `python3 scripts/req_status.py --full`.
`spec/deferred.toml` lists what was deliberately left out, with why. For
example, it doesn't defend against an agent that edits plan files directly.

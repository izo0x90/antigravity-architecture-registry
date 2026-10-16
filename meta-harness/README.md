# NAH (not a harness)

NAH sits between you and coding agents (Antigravity and Claude Code, over ACP). It reads
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

Open the cockpit through that link once per browser: it sets the session
every route needs. Only `/health`, the launch link, the static files, the
agents' MCP endpoint and the sockets (which take a one-time ticket) are open;
anything else without the cockpit's session gets 403. Each route names the
capability it needs, as each agent tool does (see `.nah/access.toml`). The
token is kept in `~/.config/nah/cockpit-token`; you can set your own with
`NAH_COCKPIT_TOKEN`. It's also this server's secret: `meta-harness token`
prints it, and another server (a home) sends it as `Authorization: Bearer
<secret>`, never in a URL. Agents connect to the MCP endpoint at
`/api/mcp/sse`, which every session NAH opens is given automatically.

The server listens on `127.0.0.1:8080` unless told otherwise (`--listen
addr:port`, or `HOST` and `PORT`). It listens only where the internet
doesn't reach: loopback, a LAN (10/8, 172.16/12, 192.168/16), Tailscale's
100.64/10, IPv6 unique-local and link-local; anything else, `0.0.0.0`
included, is refused unless you pass `--listen-public`. `--no-cockpit`
serves the project without the cockpit's page; `--no-project` serves the
cockpit alone (its state in `~/.local/share/nah/home/`), reading no code.

The server that serves the cockpit is a home: it links to other servers
(one per project), listed in its state database with each one's address
and secret (`POST /api/home/servers` with `name`, `url`, `secret`; or
`meta-harness servers add|list|remove` while it's stopped, `--project` for
a project's server). It follows each one's events and sends them, by
name, on `/ws/home`, with the list whenever it changes. A server that
stops answering is tried again (waiting longer each time) and is a
notification until it's back; disconnecting one stops following it and
leaves it running. `/s/{server}/...` is that server's `/...` through home
(sockets too, at `/s/{server}/ws/...`), streamed both ways: home adds the
server's secret and drops the cookie, and the server checks what the call
needs. `POST /api/server/shutdown` stops a server the way Ctrl-C does
(its sockets close too); through home, `/s/{server}/api/server/shutdown`
stops that one, and home keeps it listed, as lost, until it's removed.

The agent harness and each language's server are packs, installed by the
binary itself (see [Packs](#packs)). To build and install the ones you need:

```sh
scripts/bootstrap.sh antigravity-acp claude-code rust-analyzer
```

`scripts/serve.sh [project]` starts the release build in the background
over a project (this repository by default), restarting it if it runs, and
prints the cockpit's link; its output is in `~/.cache/nah/serve.log`.

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
| `harness` | an agent harness: how it's started (program, arguments, environment, variables it doesn't get) and its settings (see [Harnesses](#harnesses)) | `antigravity-acp`, `claude-code` |
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
replaces its pack's (each harness pack is one instance, named in its
`[provides]`: `antigravity`, `claude-code`).
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

## Harnesses

Every harness is an ACP agent, run by the one ACP driver. What an agent
does its own way is data, in its pack's `[provides]`:

```toml
[provides]
launch = { program = "{dest}/node_modules/.bin/claude-agent-acp", env_remove = ["ANTHROPIC_API_KEY"] }
driver = "acp"
instance = "claude-code"
display_name = "Claude Code"

[provides.settings.modes]          # its mode for each of ours (one left out isn't offered)
approval_required = "default"
full_access = "bypassPermissions"
plan = "plan"
```

The settings (wrong or unknown ones stop the harness, saying why):

| Setting | What it says |
|---|---|
| `modes` | its mode for asking first, accepting edits, auto, full access, and planning |
| `approval_warnings` | words added to an approval option, by decision |
| `client_fs` | it reads and writes the project's files through us |
| `advertise` | what we tell it we do: run its login commands (`auth_terminal`), show its questions as forms (`elicitation_form`), more `meta` |
| `session_meta` | added to every session we open |
| `queueing`, `steering` | where its `initialize` answer says it queues prompts, and takes messages into the running turn (JSON pointers) |
| `sign_in` | the notification that says who it's signed in as, and where in it the status, name, email, plan and organization are |
| `login` | the link its login command prints, and what the page gives back to paste |
| `questions` | the suffix of a question's own "Other" box in its forms |
| `dirs` | its folders (`{name}` in paths), each from a variable, else a default |
| `skills` | where it finds skills, in its order, and how it names and orders them |
| `tool_config` | its configuration files that list tool servers (file, JSON pointer, scope) |

Paths and pointers can name `{home}`, `{project}` and the `dirs`. Every
shipped harness pack runs the same contract tests against a mock of its
agent made from its settings (`tests/harness_contract.rs`).

Sessions, checks, logins and saved sessions run in the project. Signing in
uses the agent's own login: Antigravity's opens a browser (paste the
address of a page that fails to load); Claude Code's prints a link (open
it, sign in, paste the code the page shows). Who it's signed in as shows
beside its name. An API key in the server's environment isn't passed to
Claude Code, so it uses the account.

While a turn runs, Enter queues the next message (the agent queues it, or
the server sends it when the turn ends; it shows as QUEUED);
`Ctrl+Enter` (or `Space s a`) adds it to what the agent is doing, when it
takes that. What an agent asks shows as one card, by what it asks: an
approval ("Wants to run a command", "change a file", "read a file", "use
a tool") with Allow and Deny (`y`/`Y`/`A`/`d`); a question ("Asks you")
with its answers numbered (`Space r 1`, `r 2`…), or its form (type your
own, then Answer or Skip); a page to open, then Done. Each answer shows
its key. Full access and Auto answer approvals only; a question always
waits for you. A harness that asks questions as permission requests says
how its pack marks them (`[provides.settings.questions]
permission_tool_prefix`, Antigravity's `interaction_`); a permission
request with no deny among its options is a question too. Requests belong
to their turn: ending the turn withdraws them.

In a chat, `J`/`K` show the next and previous agent slot: the lead, then
sub-slots, then queue workers, each by name (the slot picker, `Space w w`,
lists them so).

Tool servers: every session gets ours (`/api/mcp/sse`) and the project's,
kept in `.nah/mcp.toml` (`GET`/`PUT /api/mcp/servers`; `Space p a` adds
one, `Space p r` removes one, and the open session restarts with them).
Each harness also keeps its own (Claude Code's `.mcp.json` and
`.claude.json`), listed with the rest by `Space p l`; its skills are
listed by `Space p s`.

```toml
[[server]]
name = "docs"
url = "https://host/mcp"      # transport = "sse" for the older kind

[[server]]
name = "git"
command = "git-mcp"
```

## The cockpit

The cockpit has eight kinds of view: chat, code, graph, tree, spec, queue,
inbox and changes.
Each column has its own view: two graphs side by side (calls beside data
flow), several code files, several cards, several chats (each shows an
agent slot: the lead, or a sub-slot). There's one queue and one inbox
(showing either in another column moves it). Picking a kind of view
in a column (its tabs) gives it a new one, starting where the last one of
that kind was. Columns: `H`/`L` (or `Tab`/`Shift+Tab`) go to the column left and right; `+`/`-` in the top bar, `Space l 1`…`l 6` (or
`alt+1`…`alt+6`), `Space l a` adds one, up to `[columns] max` (12). Columns and views work like
vim's windows and buffers: closing a column (`Space l x`, or the × on its
header) or fewer columns only hide its view, with its state, and more
columns bring hidden views back where they were. `Space b b` lists every
open view (on screen or hidden; the top bar counts the hidden ones) to show
one in this column; `Space b n` then a kind starts a new view here; `Space
b d` deletes this column's view for good, the only way one is lost (the
queue, there's one, is never deleted). Keys, links and the agent act on the view
of each kind used last. Every view has a cursor, and the same keys drive
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
    `g i` implements, `g l` data flow;
  - a number first picks the column it opens in: `1 g d` in the first
    column, `3 g s` in the third, one past the last in a new column (not
    the column you're in). Without one: the nearest column showing that
    kind of view, else beside this one.
- **Come back:** `ctrl+o`/`ctrl+i` walk one jump list across all views and
  slots; `m` plus a letter sets a mark and `'` plus the letter returns to it.
- **Quick look:** `q` shows the full card of what the cursor is on (the
  spec view, floating over the column), with what the graph says about it
  on top, on the side away from that column (over it with one column).
  Hold `q` to peek (it closes when you let go); tap it to keep it
  open, and `q` again, `Escape` or a click outside closes it. Inside it,
  the spec's keys work (`j`/`k`, sections, links; following a link shows
  that card). `Space q c` (or its button) opens the same card in a column.

- **Changes (git):** each desk compares two points (a setting on the
  Changes view, kept with the cockpit's state): by default the uncommitted
  changes against the last commit, staged, unstaged and untracked kept
  apart as git status does; or the last commit, the staged files, the
  files, any commit or branch (`rev:main`) against each other. Only the
  project's folder is read, by its own paths. `Space b f` shows the
  Changes view: files then their hunks, run together (a few small files
  open by themselves; `z o`/`z c` open and close one), one column or side
  by side (a setting; auto: side by side when the column is wide enough).
  `Enter` opens the spot in the code; an action shows more lines around a
  hunk. The code view marks the lines changed against where the
  comparison starts (a bar: green added, blue changed) and shows deleted
  lines in place, struck through; `z d` hides and shows them. Changes
  land on the components whose lines they touch (and what holds them):
  the graph badges each with its lines added and removed, its "Only
  changed" switch shows just those (and `g /` then finds only in changed
  files), a card's Changes section (the spec, the quick look) shows its
  hunks, and `g x` goes to them in the Changes view. Files the index has
  out of date are left out of that until they're read again. The server
  reads git (`GET /api/changes?from=&to=`, `GET /api/changes/file`, `GET
  /api/changes/components`; `GET /api/search?changed=from..to`) and never
  changes the repository, its index or its files.

- **Talk to the agent:** `t` opens a box over whatever is on screen;
  Enter sends to the lead's session (opening one if there's none), the
  agent's reply shows above it, and `Escape` closes it. Nothing moves.

- **Servers:** the cockpit shows one server's project at a time and keeps
  every linked one live (each has its own desk: views, layout, marks, its
  agent-control link). The top bar's chip names the one shown (a dot: something
  waits on another); `Space S S` switches, `Space S a` links one (`name
  http://host:port secret`), `Space S c`/`S d` follow one again or stop
  following it, `Space S r` unlinks it, `Space S x` shuts one down (after
  you confirm). Each desk's calls and layout go to its own server through
  home; which one is shown is kept on home. A home with no project shows
  the first server it links to.

- **What needs you:** the bell counts what waits on you (approvals,
  questions, sign-ins), and the tray counts the updates (crashes, failed
  and finished turns, messages), every server's. The bell's look says
  whether system notifications (the browser's, or the OS's in a native
  build) reach you while the cockpit is out of sight: ringing, on; plain,
  off; a dot, on but not allowed yet or refused. Its list (click it, or
  `Space N`) starts with that switch, then what's waiting; the tray's is
  `Space u`. The inbox is a view like the others: one item at a time,
  where it's from, its request, what the agent said last. `Space n` opens
  the oldest waiting one in the quick look, which holds the inbox and
  gives it the keyboard: `j`/`k` move over its parts, `Ctrl+d`/`Ctrl+u`
  scroll, `f` labels its answers, `Enter` answers with the one under the
  cursor; `y`/`Y`/`A`/`d` answer as in the chat (several answers of one
  kind are `Space r 1`, `r 2`…); `J`/`K` go to the next and previous item
  (on whichever server), `o` opens its chat, `x` puts it away. A chat on
  screen puts its slot's updates away.

The cockpit's state is kept on the server (`GET`/`PUT /api/ui/state`, in
the project's state database, `.nah/state/state.sqlite`): the columns and
the view in each, where every view is (each chat's agent slot; which are
pinned, and which was used last), the marks and the jump list, and what
each chat had typed and not sent. A reload, or another browser, comes back to
it. It's saved half a second after it last changed (`[net] save_after_ms`).

The key bar at the bottom shows the keys that matter for what the cursor
is on. Pausing on a prefix (`g`, `z`, `[`, `]`, `m`, `'`) lists what can
follow, and `?` lists every key where you are: a section for what you're
on, this panel, what holds it (the quick look, the column) and
everywhere, each action with every way to run it (keys, leader path,
gesture), the mode's keys first while typing or in the palette, and the
pointer and touch grammar. `Space` opens the
leader menu (the focused view's entries first), and `:` opens the command
palette (`:42` goes to line 42). The palette and its pickers move with
`Ctrl+J`/`Ctrl+K` (or `Ctrl+N`/`Ctrl+P`, or the arrows) and pick with
`Enter`.

Every view is drawn the same way by what holds it (a column, the quick
look): its top line (its name, what it's doing, and its buttons and
settings), then its content. What you're on shows its own buttons under
it, and only while you're on it: a review point's Reply, Accept and
Reopen; a job's Approve or Retry; a card's Go into and go-tos; a request's
answers (a list to pick from for a question, Allow and Deny for an
approval); Send under the box you're typing in. Every button is an
action: it shows its key beside it (on hover when there's no room) and
runs the way its key does. The key bar, the `?` sheet, the leader menu,
the palette and every button read one list of what can run where you are,
so a key is shown wherever it runs and runs what it shows. A panel that
isn't focused shows its buttons without keys (they run it once it is).

The mouse and fingers work the same way in every view: a click or tap on
an item puts the cursor there; a double click or tap opens it (Enter); a
long press lists what can be done with it (the palette). A swipe runs
what it's bound to in the view it starts on (`gestures = ["swipe_left"]`
in `[keymap.bind]`, like keys): in a chat, the next and previous agent
slot; elsewhere, back and forward. The graph's canvas reads its own (a
tap selects, a double tap goes in, two fingers pan and zoom), and so does
the code's text (`[keymap.pointer] go_to` with a click goes to a name).
Thresholds are in `[touch]`.

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
    the rest dims; the pills on its lines say what goes in and comes back,
    and `q` shows its full card;
  - open or close a card in place (`z o` / `z c`, or click its `+N`);
  - go in (`Enter`, double-click): the card becomes the root; back
    (`Backspace`) returns along the trail shown above the graph;
  - more or fewer levels (`+` / `-`), flip down and up (`r`), pick the
    view (`v` lists them);
  - pick the code, one plan or one walkthrough (the Source picker). A plan
    starts from its own items, shows only the code they plug into, and
    marks cards new, changed, done or doesn't match; "Show in full code"
    shows it in the whole codebase. A walkthrough shows only its own
    components the same way, each with what the agent wrote about it;
  - find anything in the code (`g /`), and go back to where you were
    before the agent moved the view;
  - zoom in and out (`z +` / `z -`, the wheel or a pinch) and fit the
    drawing to the screen (`z z`, or the button in the toolbar).

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

All keys are configurable in `crates/cockpit-ui/config/default.toml`,
including those outside a view's bindings: `[keymap.list]` (moving in the
palette and pickers), `[keymap.input]` (`send`, `add` into a running
turn, and `newline`, in every typing box) and `[keymap.pointer]` (`go_to`,
held while clicking a name). A test fails the build when the cockpit
reads a key anywhere else, or writes a key's name into text instead of
the bound key's label, and when a view draws a button of its own instead
of placing an action for its host to draw.

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
| `.nah/mcp.toml` | the project's tool servers, given to every harness's sessions |
| `.nah/access.toml` | what each agent role (lead, sub, worker, external) may do with NAH's tools. A role named here replaces the shipped one. It covers NAH's tools only; a slot's own harness tools follow its mode and your approvals |

How long things take is visible: every answer is timed stage by stage
(tracing spans), anything over `slow_request_ms` is logged and marked "slow"
in the graph, and `GET /api/perf` lists the last answers by stage with the
graph, store and memory sizes. Benchmarks: `cargo bench -p arch-service
--bench hot`. Measured on this repo and on tokio (717 files): a full index
takes about 1.3 s and 1.5 s, a one-file change 0.25 s and 0.08 s, a restart
about 0.55 s (slower than before the rebuild's 0.36 s and 0.44 s), and a view 0
to 11 ms on the server; the real rust-analyzer
confirms a changed file's links in 0.1 to 0.35 s once warm (71 to 327 ms measured).

Plans live in `.nah/plans/`. What the server keeps across restarts (the
cockpit's saved views, dev feedback, an activity log, and as they arrive
slots and notifications) is in `.nah/state/state.sqlite`, which isn't
committed and is never thrown away: a second server on the same project, a
database from a newer NAH or one that can't be read is refused with what to
do. The analysis is kept in
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
| Explore | `ingest`, `list_types`, `search`, `inspect`, `view`, `navigate` (moves your graph: the one in the `column` it names, else the one you used last); `search` and `view` take `changed` (`HEAD..files`, `rev:main..files`) to keep to what changed |
| Plan | `plan_list`, `plan_create` (a change, or a walkthrough), `plan_import` (old `system_architecture.json`), `plan_edit`, `plan_validate`, `contract` |
| Build | `verify`, `run_tools`, `review` |
| Review | `review_comments`, `answer_comment`, `reply_comment` |
| Cockpit | `cockpit_read`, `cockpit_actions`, `cockpit_run`, `cockpit_click`, `cockpit_type`, `cockpit_keys` (only while you let agents drive it) |
| Slots | the lead's: `slot_list`, `slot_read`, `slot_start`, `slot_close`, `slot_send` (optionally waiting for the answer); a sub-slot's: `tell_lead` (the user sees it too); any slot's: `tell_user` |

Every result ends with next steps, given in dependency order.

Each session NAH opens sits in a slot and is given these tools at its own
address, with a key that says which slot is calling. What a slot's agent
may use comes from its role (`.nah/access.toml`): the lead drives the IDE,
plans, builds and runs its sub-slots; a sub-slot reads and reports to the
lead; a work-queue job plans and builds its component; an agent NAH didn't
start (no key) only reads. Tools a role may not use aren't listed to it,
and a call to one is refused saying which capability it needs.

Whatever an agent waits on the user for (an approval, a question, a
sign-in) is a notification until it's answered, and so is what happened
that the user would want to know: a session that stopped unasked, a failed
or finished turn, a sub-slot's report, an agent's `tell_user`. They're kept
in the state database (`GET /api/notifications`, `?state=recent` for the
latest in any state; `POST /api/notifications/seen` with a slot puts its
updates away; `POST /api/notifications/{id}/dismiss` one). Answering a
request stays the request's own route. What waited when the server
stopped expires at the next start: that agent session is gone.

### Driving the cockpit

The cockpit can be driven from outside the way you drive it: list what can
be run now (the actions the Space menu, keys and palette offer), run one by
id, read what's on screen (the mode, each column's view, and every piece of
text drawn, with where), click drawn text, type, and press keys. It's one
set of calls with two ways in:

- **`window.nah`** in the page: `nah.read()`, `nah.actions()`,
  `nah.run(id, value)`, `nah.click("Connect")`, `nah.type(text)`,
  `nah.keys("space s n")`, each a promise. For scripts in the page and
  browser drivers (DevTools, Playwright).
- **The `cockpit_*` tools** for agents, over the cockpit's own socket
  (`/ws/cockpit`, opened with a one-time ticket only the cockpit's session gets). They work only while
  you let agents drive it: `Space a d` ("Let agents drive the cockpit"),
  kept with the cockpit's state. Off, an agent can't click its own
  approvals.

Calls run inside a frame, on the same paths keys and clicks take. A
cockpit in a background tab draws rarely, so it answers slowly.

A walkthrough (`plan_create` with `kind: "walkthrough"`) is a plan the other
way round: it explains code that exists. Asked to show how something works,
the agent brings in each component the behavior goes through (`plan_edit`
`modify`), writes its description and spec (logic steps, invariants) from
the code, and moves your graph to the walkthrough (`navigate` with
`source: plan:<id>`): only those components, from the entry point, each
with its card in the quick look. A walkthrough changes nothing: adding
components, relations or signature changes are refused, and nothing in it
is approved, verified or built. It's saved in `.nah/plans/` like any plan.

## Code map

| Crate | What it holds |
|---|---|
| `arch-graph` | the domain: types, components, relations, plans, schemas, views, scenes (how a view is laid out and routed), queue shapes (no I/O; shared with the cockpit) |
| `code-analyzer` | our analyzer: reads each file with ast-grep by its language pack (with syntax colours and code rules) and links names across files |
| `nah-packs` | packs (harnesses, languages, servers) and the one installer |
| `nah-lsp` | the one language server client: starts a server in the background, knows when it's ready, asks where names are defined, restarts it when it ends |
| `nah-access` | what each caller may do: capabilities, the roles that hold them (configured), and the one check every tool and route goes through |
| `nah-notify` | notifications: what waits on the user (approvals, questions, sign-ins) and what happened (crashes, failed and finished turns, messages), from the event stream; kept in the state database, published, handed to adapters (Slack, email, a phone: none yet) |
| `nah-changes` | the project's changes, from git: what a comparison (two of the last commit, the staged files, the files, any commit, a snapshot) changes, one file's changes, where they are on the files now; snapshots of the files kept as commits of their own (`refs/nah/snapshots/`). Reads the repository and never changes it |
| `nah-state` | the project's state database (`.nah/state/state.sqlite`): what the server keeps across restarts that isn't the analysis. One server per project (a lock), explicit layouts, never thrown away |
| `arch-store` | where the analysis is kept between runs (SQLite) |
| `arch-search` | one way of matching what's typed: names with neo_frizbee; files and lines with fff-search |
| `arch-service` | plans and cache, plan operations and checks, verification, tools, reviewers, review loop, queue state |
| `src/` (server) | REST (`/api/arch/...`), MCP, events over WebSocket, the queue runner, the cockpit session, agents driving the cockpit (`remote`) |
| `cockpit-ui` | the egui/WASM cockpit |
| `harness-*` | agent harnesses: the contracts, the core (drivers, instances, sessions, the skill and tool-config readers), and the ACP driver every agent runs on, configured by its pack |
| `workspace-fs` | file access confined to the workspace (no path or symlink escapes) |

## Left out

It doesn't defend against an agent that edits plan files directly.

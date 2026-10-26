# Plan: terminals — shells on the project server, shown in the cockpit

Written 2026-10-17 from a discussion with the user and a read of the code;
revised the same day after checking the code (home's socket relay, the
event hub, cockpit routes' permissions) and egui/eframe 0.33's sources.
Step 0 is built; nothing else is. Each step is tested (behaviour, not
wording), checked in the running cockpit and committed on its own.
Decisions marked **★** are the user's; each has a recommendation, and
none is built until it's settled.

Words: a **terminal** is a shell the project server runs; a **terminal
buffer** shows one in a panel or the quick look. **Read** is seeing its
screen; **drive** is typing into it.


## What it is

- A project server runs shells for the user: `$SHELL`, in the project
  folder, in a pseudo-terminal (`portable-pty`, from WezTerm). The server
  keeps each one's screen and scrollback (`alacritty_terminal`, Alacritty's
  core): text, colours (the 16 named ones, 256, true colour), the cursor.
- The cockpit shows a terminal in a buffer like any other, in a panel or
  in the quick look. It draws the screen it's sent and sends keys; it
  understands no escape codes.
- A terminal runs until it's closed. Closing a panel hides the buffer;
  reloading, or opening the cockpit elsewhere, shows the same screen.
- The user can always type. Agents read a screen with `view.read` and
  type with `shell.drive`, which no agent role has by default.


## Rules it keeps (each checked by a test or a lint)

1. **A buffer has the keyboard only when its host says** (`Drawing::typing`,
   built in step 0). The terminal buffer is no exception: it takes keys
   while typing, never on its own.
2. **Which keys leave typing is the buffer's declared kind of field, not
   its type.** A text field leaves on `back` (Escape); a field that takes
   every key leaves only on `[keymap.input] leave`. The shell reads what
   the buffer declares; it never asks "is this a terminal".
3. **The terminal crate knows nothing of the cockpit, the hub or HTTP**: a
   PTY, an emulator, screens, input. Its only workspace dependency is
   `harness-contracts`, where the screen, cell and input types live (they
   are the interface between server and cockpit; written once). A test
   reads its manifest and fails on any other.
4. **The shared event bus never carries screens**: only that terminals
   exist, their title and status. A terminal's screen and its input go on
   its own socket. (`ClientEvent` has no screen variant; nothing to check
   but the type.)
5. **Keys reach the shell in the order typed**: one socket per terminal,
   in order by construction; a test types a burst and reads it back.
6. **Colours are the cockpit's**: the server sends a cell's colour as it
   was asked for (named 0–15, indexed, RGB); the cockpit's theme resolves
   named and indexed ones. The type has no resolved colour to send.
7. **What differs by platform is handed in, never checked for**: each
   platform capability is a small trait (alerts' `Backend` today), and the
   start-up code gives the app its implementations; nothing picks one with
   `cfg`, and none has a "does nothing" stand-in. Every `cfg(target_arch)`
   lives in the web start-up module; a lint fails on any other. Buffers,
   the desk and the shell go through egui and the capabilities only. A
   desktop app can't be built until it has every capability: its start-up
   doesn't compile without them.
8. **Agents type only with `shell.drive`**: the MCP tool checks it; a test
   with a role without it is refused. Cockpit routes and the terminal
   socket are the user's only (as every cockpit route is today).
9. **The shell names no buffer type** (the existing lint) and **a buffer
   reaches past itself only by asking** (the existing lint): the pop-up
   terminal is "a terminal buffer in the quick look", as talking is "a chat
   buffer in the quick look".


## What's there to build on (checked)

- **Typing and the quick look** (step 0, done): `Host` (a panel or the
  quick look), `InputMode::Insert { at }`, `Drawing::typing`, the quick
  look holding any `BufferId`, one opener (`look_at`).
- **Home relays any server's socket both ways**: `/s/{server}/ws/{*rest}`
  (`src/routes/home.rs`, `src/home/pass.rs`). A terminal's socket on the
  project server is reachable from the cockpit with no new relay code.
- **The event bus** (`harness-core/src/hub.rs`): 4096 events behind, then
  `Resync` and a full replay for that listener. Fine for terminals' list
  and status; wrong for screens (rule 4).
- **Cockpit routes are user-only**: `Needs<...>` admits the cockpit's
  cookie (`src/cockpit.rs`). Agents' permissions are on MCP tools
  (`ToolDef { needs }`, `src/mcp/sets/`), roles in `nah-access`.
- **egui/eframe 0.33 in the browser**:
  - A widget that sets `PlatformOutput::ime` gets the browser's text
    input focused, which is what brings up a phone's keyboard (the chat's
    field does the same). No hidden field of our own.
  - Ctrl+C/X/V arrive as a key event *and* as `Event::Copy`/`Cut`/
    `Paste` (eframe never blocks them, so the browser's clipboard events
    fire).
  - Tab and the arrows are kept from the browser; egui uses Tab to move
    between widgets unless the focused widget sets a focus lock filter
    (`Memory::set_focus_lock_filter`, `EventFilter`).
  - The browser keeps some Ctrl keys whatever we do (Ctrl+N, Ctrl+W,
    Ctrl+T in Chrome).
- **Agent sessions in the cockpit**: `Agents` on the desk (`agents.rs`),
  read through `AgentsView`, changed by `Ask::Agents`; the chat is a
  buffer over it. Terminals get their own beside it (not inside it).
- **Browser-only code today** (`cfg(target_arch)`), three kinds:
  - system alerts: a trait already (`alerts::Backend`), but chosen by
    `cfg` in `Alerts::default()`, with a desktop stand-in that says
    "unavailable" (so a desktop app would build without alerts);
  - the clock and logging, by hand in `attention.rs`, `net.rs`,
    `app/desks.rs`, `remote.rs` (`js_sys::Date` vs `SystemTime`,
    `console.warn` vs `eprintln`): not platform pieces, crates do them;
  - start-up: the canvas, keeping Ctrl keys from the browser
    (`keymap::keep_browser_keys`), `window.nah` (`remote::install_page_door`),
    the page's address (`lib.rs`).
- **No native build of the cockpit** yet (`lib.rs` has only the web entry).


## How it fits together

```
project server                                  cockpit (desk)
──────────────                                  ──────────────
nah-terminal (crate)                            Terminals (desk mirror)
  Terminal = PTY + emulator                       list + status  ◄── bus
  screen diffs, input, resize, scrollback         a socket per terminal shown
        ▲                                                ▲
terminals manager (server)                       TerminalBuffer
  open/list/close, DB, lifecycle → bus             draws cells, scrollback
  /api/terminals (user)                            asks: input, resize, open
  /ws/terminals/{id} (user): screen ⇄ keys         declares: takes every key
  MCP: list, read (view.read), type (shell.drive)
```

1. **`nah-terminal`** (new crate): a `Terminal` owns a PTY and an
   emulator; a reader thread feeds the emulator and marks what changed;
   `screen_since(seq)` gives the rows changed since a point (or the whole
   screen); `input(Input)` turns keys into the bytes the terminal expects
   in its current modes (application cursor keys, bracketed paste);
   `resize`, `lines(range)` (scrollback), `search`. No async runtime, no
   server types (rule 3).
2. **Terminals manager** (server): opens, lists and closes terminals,
   saves which exist (the state DB), publishes `TerminalUpdated` /
   `TerminalClosed` on the bus, serves `/api/terminals` and
   `/ws/terminals/{id}`, and the MCP tools. It's the only glue between the
   crate and the server.
3. **The socket** (one per terminal shown): the server sends the whole
   screen on connect, then changes (merged, at most ~60 a second); the
   cockpit sends input, size, and asks for scrollback lines or a search.
   Several viewers of one terminal each have a socket.
4. **`Terminals` on the desk** (cockpit): the list and status from the bus,
   and the sockets of the terminals shown; buffers read it through a
   reader and act on it by asking (`Ask::Terminals { id, act }`), as
   the chat does with `Agents`.
5. **`TerminalBuffer`**: draws the grid; keys while typing become input
   asks; its size follows its host's rect; scrollback by the shared
   viewport rules; declares that its field takes every key (rule 2).


## Decisions (★, with a recommendation)

1. **Leave keys** — settled: Escape twice within 300 ms (the wait a setting), and
   `Ctrl+\` then `n` (vim's chord, its second key without Ctrl, since the
   browser keeps Ctrl+N). Both in `[keymap.input] leave`, configurable.
2. **Panel keys while typing in a terminal** — not now: leave typing,
   then the panel keys work as everywhere.
3. **Server restart** — settled: shells end with the server; the buffer
   says so and offers to start a new one in the same folder. (The project
   server plays tmux's part: shells outlive the cockpit, not the server.)
   Surviving a restart, later if wanted.
4. **One terminal shown at two sizes** — settled: the smallest of those shown wins (as tmux does), so no viewer
   gets lines cut.
5. **Turning shells off on a server** — settled: `--no-shells` beside
   `--no-cockpit`.
6. **Notifications** — settled: through the notification system
   (`nah-notify` rules: the inbox, the bell, system alerts), for a
   terminal not on screen: its bell; its shell ending (with the code); a
   command finishing that ran longer than a setting. Commands are known
   by the shell marking them (OSC 133, as VS Code and iTerm do): the
   server turns that on when it starts bash or zsh (a hook in the shell's
   start-up); other shells get the bell and the end only.
7. **Platforms** — settled: no desktop build now. What differs by
   platform is handed in at start-up (rule 7), so a desktop app fails to
   compile until it has every piece; nothing desktop is written yet.


## Steps

### 0. One way to type; the quick look holds any buffer — done (`f3cd675`)

### 1. Spike: check the browser before building on it (thrown away)
A scratch buffer in the running cockpit that logs what it gets while
typing, in Chrome on a desktop and in Chrome's phone emulation:
1.1 Tab, Escape, arrows, Ctrl+C/V/X, Ctrl+D, Ctrl+\, Alt+keys: which egui
    events, in what order; with a focus lock filter set.
1.2 Paste from the system clipboard; copy to it (`OutputCommand::CopyText`).
1.3 Setting `ime` brings up the phone keyboard; its keys and backspace
    arrive.
1.4 Escape twice: both arrive, timing readable.
Results written into this plan (they settle details of steps 6–7); the
spike isn't committed.

**Results** (2026-10-17, Chrome 1706×1322, keys sent as real browser key
events through CDP; a scratch field in the quick look's place, typing):
- In Normal the shell takes the keyboard back from any focused widget
  that isn't a text field (`app/keys.rs`): the terminal field has the
  keyboard only while typing, as rule 1 says.
- With the focus lock filter set (tab, arrows, escape), Tab and the arrows
  arrive as keys and the field keeps the focus.
- Printable keys: a `Key` (pressed, released) and a `Text`. Enter and
  Backspace: `Key` only.
- Ctrl+D, Ctrl+\: `Key` with `ctrl` (and `command`). Ctrl+C: `Key` and
  `Event::Copy`; Ctrl+V: `Key` and `Event::Paste(clipboard text)`.
  Encoding: Ctrl+letter from the `Key`; `Copy`/`Cut` ignored unless
  something is selected (then copy it and send nothing); `Paste` sends its
  text (bracketed when asked); the Ctrl+V `Key` itself sends nothing.
- Alt+letter: `Key` with `alt`, and (as CDP sends it) a `Text` too:
  encode Alt+key as ESC + the key, and drop a `Text` that follows a key
  pressed with Alt or Ctrl in the same frame.
- While the field sets `ime`, the page's focused element is eframe's
  text `INPUT` (the canvas otherwise): what a phone shows its keyboard
  for. A real phone wasn't available; how its keys arrive (composition,
  Backspace) is eframe's text agent's, as for the chat's field.
- Escape in typing reaches the shell's key handling first, which leaves
  typing before the field sees it: a field that takes every key needs the
  shell to leave only on `[keymap.input] leave` (rule 2). Event times are
  in `i.time`, enough for "Escape twice within 300 ms".

### 2. The terminal crate
2.1 `nah-terminal`: `Terminal` (PTY + emulator), reader thread, screen diffs
    by sequence number, input encoding by mode, resize, scrollback lines,
    search; a cell type (text, colour as asked, attributes) and input type.
2.2 Closing hangs up, as closing a terminal window does: the shell's
    process group gets SIGHUP and the PTY closes, so the shell and the
    foreground job end; bash ends its background jobs itself (dash's
    survive, as in any terminal).
2.3 Rule 3's test (no workspace dependencies).
2.4 Tests against a real `sh`: output reaches the screen; input runs; a
    burst of keys arrives in order; resize; exit code; Ctrl+C interrupts;
    bracketed paste when the program asks; colours as asked.

### 3. The server: manager, routes, socket, permissions, tools
3.1 Shared types in `harness-contracts`: `TerminalId`, `Terminal` (id,
    title, status: running / ended with code, size), the socket's messages
    (screen, changes, lines, input, resize, search), bus events
    `TerminalUpdated`, `TerminalClosed`.
3.2 The manager: open, list, close; which exist saved (a migration); the
    bus events; one terminal shown at several sizes (decision 4).
3.3 `/api/terminals` (list, open, close) and `/ws/terminals/{id}`, both the
    user's only; `--no-shells` (decision 5).
3.4 `shell.drive` in `nah-access`, in no default role; MCP tools: list and
    read a screen (`view.read`), type (`shell.drive`).
3.5 Tests: the socket's first message is the whole screen, then changes;
    two sockets on one terminal see the same; reconnecting shows it as it
    is; the bus has the list and status only; an agent without
    `shell.drive` is refused; through home's relay end to end.

### 4. The desk's terminals (cockpit)
Revised while building (2026-10-17): each terminal buffer holds its own
socket, not the desk. Asks are taken from the buffer of each kind in use
only, so a terminal in an unfocused panel couldn't send its size; and one
socket per desk would make two panels showing a terminal one reader to
the server, so "the smallest wins" couldn't see both. Every drawn buffer
is ticked with the desk's network client each frame, so a buffer keeping
its own socket works in every panel, and the server sees one reader per
panel (a buffer no longer drawn closes its socket: a lifecycle hook).
4.1 `Terminals` on the desk: the list and status from the bus; open,
    restart, close (HTTP) through `Ask::Terminals { id, act }`; read by
    buffers through a reader (the same cell and reader the agents use,
    made generic: `Held<T>`, `Reader<T>`).
4.2 `Target::Terminal(id)`; the network client's terminal socket (with
    home's ticket, through home for another server).
4.3 Where the chat and the terminal buffer pick and step through the
    session they show, one helper if the code turns out the same (not
    forced).
4.4 Tests, commit.

### 5. Platform pieces handed in (before the buffer, so it's born clean)
5.1 The clock and logging through crates (`web-time`, `log` with eframe's
    web logger): their `cfg`s go.
5.2 Alerts' backend handed in by the start-up code (`CockpitApp::new`
    takes it); `Alerts::default()`'s `cfg` and the desktop stand-in are
    deleted; tests hand in a test backend that records what's raised.
5.3 The browser-only start-up (the canvas, `keep_browser_keys`,
    `install_page_door`, the page's address) in one web start-up module.
5.4 The lint for rule 7.
5.5 Tests, commit.

### 6. The terminal buffer
6.1 `BufferKind::Terminal`, made by `buffer::new`; draws the grid
    (monospace, theme colours, the cursor); its size from its host's rect
    (asked as a resize).
6.2 Typing: a buffer declares what its field takes (text, or every key);
    the shell's typing keys follow it (rule 2, decision 1). The
    terminal buffer, while typing: sets the focus lock filter (Tab, arrows,
    Escape are its), sets `ime` (phone keyboard), turns key, text and
    paste events into input asks; Ctrl+C is ^C unless something's selected.
6.3 Not typing: the cockpit's keys as everywhere (moves through the
    scrollback, selecting, the leader); the wheel scrolls back while
    typing without leaving it.
6.4 Copy a selection (`OutputCommand::CopyText`); paste (`Event::Paste`,
    bracketed when the program asks).
6.5 Place (which terminal, how far back), describe, keep; an ended shell
    says so and offers a new one (decision 3).
6.6 Tests (driving keys, reading what it draws, both kinds of field
    leaving typing as declared), live in Chrome and phone emulation,
    commit.

### 7. The pop-up terminal
7.1 A key shows the desk's pop-up terminal in the quick look, typing (made
    the first time, then the same one); the key again, or leaving and
    Escape, hides it. Through `look_at`, as talking is.
7.2 Tests, live, commit.

### 8. What every buffer does
8.1 `/` searches the scrollback (the server searches; the buffer moves to
    each match); `f` labels paths and URLs on screen; `g d` on
    `file:line` goes to it in the code view.
8.2 Notifications (decision 6): the bell, the shell ending, a long
    command finishing, for a terminal not on screen (rules in
    `nah-notify`); the start-up hook that marks commands (bash, zsh) and
    the crate reading the marks (`nah-terminal`); a setting for "long".
8.3 Tests, live, commit.

How step 8 was built (2026-10-17):
- `/`: the server searches the whole scrollback as it's typed; each match
  carries its line's text (`Found::text`), so the buffer's items are its
  lines plus the matches, and the shell's search, `n` and `N` work as in
  any buffer.
- `f`: files (`src/a.rs:12:5`, an absolute path in the project) and web
  links on screen (`terminal/links.rs`). On a file the terminal's target is
  that place (`Target::Span`): `g d` opens it in the code (the code go-to
  now opens a place in a file as it opens a link or a change); Enter opens
  a file or a web link.
- Notifications: the server's terminals publish `TerminalNews` (a bell,
  the shell's end, a command that ran `long_command_secs` or longer
  finishing; `.nah/terminals.toml`) on the event stream, not kept;
  `nah-notify` makes them updates about the terminal (`Notification::terminal`;
  one open bell per terminal). A drawn terminal says it was seen (as a
  drawn chat does), which puts its updates away; the inbox opens a
  terminal's notification in a terminal buffer.

### 9. README (terminals, keys, permissions, `--no-shells`), commit.


## Checklist

- [x] 0 one way to type; the quick look holds any buffer
- [x] ★ decisions 1–7 settled
- [x] 1 spike (results written here)
- [x] 2 the terminal crate
- [x] 3 the server: manager, routes, socket, permissions, tools
- [x] 4 the desk's terminals
- [x] 5 platform pieces handed in
- [x] 6 the terminal buffer
- [x] 7 the pop-up terminal
- [x] 8 search, hints, go-tos, notifications
- [x] 9 README

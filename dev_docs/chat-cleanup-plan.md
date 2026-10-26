# Chat cleanup: rebuilding what was built wrong, and finishing what was left

## Status (2026-10-26)

- **Built** (the user said go, 2026-10-25; finished 2026-10-26). Done: 1. Part 2c (commit
  3e1a805); 2. Part 1 (fca4ad8) and its stopping point (811c413). 3. Part 2e
  (f5770b0). 4. Part 2b
  (e1a2167); Part 2 item 13 (ff7e269). 5. Parts 2f and 2g
  (12660e6, 29b5dde) and stopping point 2 (46ab4c5). 6. Part 2 rest
  (113e2a6 … 182025f). 7. The end review (seven reviewers) and its fixes
  (e43de96, 8bc4bca, 516ef76); the two-reviewer re-read and its fixes
  (db1cde7); the live run (fix 840cd11). **Done.** Left for you: the test
  list under "What you test", and the Decide items in
  `chat-cleanup-findings.md`, where everything found is recorded.
- **Read first:** `process/decision-heuristics.md`, then this plan.
- **Probe recordings and scripts** (they become test fixtures; scrub the
  account email from `live-claude.json`'s auth message before
  committing any of it): `chat-cleanup-probes/`.
  - `acp_probe.py`: an embedded piece and a replay; `acp_live.py`: one live
    prompt, every raw message (`CAPS='{"subagents": {}}'` adds client
    capabilities); `acp_replay.py`: list and load a session, no prompt;
    `acp_commands.py`: an agent's slash commands.
  - `acp_cancel_child.py`: cancels one sub-agent; `acp_combined.py`: one
    prompt checking an attached piece, a sub-agent and the replay (for
    Antigravity's scarce quota).
  - Launch Claude Code: `env -u ANTHROPIC_API_KEY …/packs/claude-code/1.1.0/node_modules/.bin/claude-agent-acp`;
    Antigravity: `D=~/.local/share/nah/packs/antigravity-acp/1.2.0; $D/qemu/usr/bin/qemu-x86_64 -L $D/sysroot -E LD_LIBRARY_PATH=$D/sysroot/lib/x86_64-linux-gnu $D/libexec/agy_acp_server.par --uid=root`
    (slow: minutes). The user's lead session (Antigravity) is native
    `171da13e-…`, cwd `meta-harness`.
- **Live in the user's tab:** scratchpad scripts `pump.py` (js, shot),
  `reload.py` (after a server restart), `demo.py` (paced key steps),
  `restart.sh`; tab `F7F3C7977EE81D6605BD0B230D7628D2`.

## The rule for this plan

Everything here is built the correct way, or it isn't built. That means:

- **No side copies of data that already lives somewhere.** Tags, files,
  the harness's own session history and the cockpit's state already
  persist. We read from them; we don't keep second copies.
- **No matching things up by comparing or hashing whole texts.** If two
  pieces of data belong together, the data itself says so: an id, a label,
  a separate piece.
- **One owner per format.** If text has to carry meaning, the code that
  writes it and the code that reads it live in one module, with one test
  of the round trip.
- **Cost proportional to what's on screen.** Nothing scans a whole session
  when it's replayed; work happens only for what's shown, once, and is
  remembered.
- **Checked at scale.** Tests use sessions with thousands of messages and
  long attachments, not one message with one link.

A shortcut that works on a small example but is wrong at scale, or that
patches over a design problem instead of fixing it, is not acceptable.

## How it runs

The user is away; this plan runs end to end without stopping.
- Where something is under-specified, read
  `dev_docs/process/decision-heuristics.md` first (again after any
  compaction), decide the way it leans, keep it reversible, and record it
  under "Decide" for the end.
- Anything not done as planned (a deviation, something new, something
  that can't be done properly) is recorded, not worked around, and the
  rest goes on. It all goes in the end summary with the other open
  questions.
- The end summary is realistic: what was done and checked, what wasn't,
  what's for the user to test, what's for the user to decide.

## Why

The chat plan's end review and re-read found problems that were fixed,
and some that were left or put to the user (`chat-findings.md`). The worst
came up afterwards, from the user:

**Your messages are shown "as typed" by a lookup table.** When you send a
message with links (`#flaky`, `@src/a.rs`), the server pastes each linked
thing's content under your words, and the agent receives that. To show
your message again as you wrote it after a replay, the server saved a copy
of what you typed. Lately that copy included what was pasted. Every
replayed message of yours is hashed in full and looked up in that table.
That's:
- a second copy of content that already persists (tags, files);
- whole-text hashing of every message on every replay, a cost that grows
  with the session for no reason;
- a design patched twice by reviews instead of replaced.

## Part 1: your messages, rebuilt from what the agent was sent

### The approach

Send your message as separate pieces, which the agent protocol (ACP)
supports:
- one piece is your words, exactly as you wrote them;
- each linked thing is its own piece, carrying what it is (an id or
  address such as "tag x1" or "file src/a.rs lines 3-9") and its content.

When the harness replays the session, the pieces come back separately.
The chat shows your words, makes each `#name` / `@path` in them a chip by
matching it to the piece that names it, and folds the pieces' content
under the message. There's no table, no hashing and no copy.

### What to check first, before building anything

For **Claude Code** and for **Antigravity**, each separately, and written
down in the findings with what was seen:

1. **Does it accept attached pieces?** The agent says so when a session
   starts (`promptCapabilities.embeddedContext`). The adapter already
   reads this (`harness-acp/src/control.rs`). Record what each one says.
2. **Does the agent read them?** Send a message with one embedded piece
   and ask the agent what it was given. It has to see the content, not
   just its name.
3. **Does it replay them as separate pieces?** Resume that session and
   record exactly what the replay sends for your message: how many
   chunks, of what kind, and whether each piece's id/address and content
   come back as sent.
4. **Does a piece's address survive?** Check whether an address of our
   own (for example a `nah:` address for a tag) comes back unchanged, or
   whether only `file://` addresses do.
5. **The size limit.** Check how large an embedded piece can be before the
   harness truncates or refuses it, and compare with today's budget for
   what's pasted (`ATTACHED_MOST`).

Checks 2 and 3 run a real agent turn on the user's account, so they're
the user's to run or to approve first.

### What the checks found (2026-10-25, real sessions on the user's accounts)

The probe: `chat-cleanup-probes/acp_probe.py`. It sent one message
made of two pieces: the words "Look at #probe-note. What is the secret
word in it?", and an attached piece labeled `nah://tag/probe-note`
holding "The secret word is PELICAN-42." Then it reopened the session in
a fresh process and recorded what came back.

| | Claude Code (claude-agent-acp 0.89.1) | Antigravity (agy_acp_server 1.1.1) |
|---|---|---|
| Says it accepts attached pieces | yes | yes |
| Read the attached piece | yes, answered "PELICAN-42" | yes, answered "PELICAN-42" |
| Our own label (`nah://…`) came back unchanged | yes | yes |
| Replay: your words on their own | yes, its own piece | no: one text, your words run straight into the attachment with no space |
| Replay: how the attachment comes back | two more text pieces: the label, then `<context ref="nah://…">content</context>` | inside that one text: `Embedded resource (nah://…):` then the content |

What it means:
- Both harnesses take and read attached pieces, so sending them is right
  for both.
- Neither replays them as attached pieces. Each harness turns them back
  into text its own way. So reading a replayed message back depends on
  the harness.
- The harness's own way is a fact about that harness, like the other
  things each harness pack declares (how it marks questions, where its
  login lives). Each pack declares how its replay marks an attachment.
  The adapter reads that once per replayed message, using our labels
  (which both keep intact), and gives the chat your words and the
  attached pieces separately.
- The size limit (check 5): settled with the user, not tested. Our cap
  stays, about 16 KB a message, roughly 400–500 lines of code. Bigger
  things go as their first lines plus how to read the rest, and a link
  can already be set to hand on only its pointer, so the agent reads it
  itself. The agents' own limits are far above that.

### Building it

- **Sending.** The expansion (`nah-things` `attach`) returns the pieces
  instead of one pasted text. The session sends them as ACP pieces when
  the harness accepts them.
- **Replay.** Neither harness replays pieces as pieces (see the table
  above). Each harness pack declares how its replay marks an attachment
  (Claude Code: a label piece then `<context ref="…">`; Antigravity:
  `Embedded resource (…):` inside the text). The adapter reads a replayed
  message once by that rule and our labels. The `UserMessage` item then
  carries your words and its pieces separately. No table, no hashing.
- **Showing it.** The chat matches chips to pieces once, when the message
  is first drawn, and remembers the result for that message.
- **A harness that doesn't accept pieces.** It gets one text, in a fixed,
  labeled format written and read by one module (in `nah-things`, next to
  the code that pastes it). The reader only runs for a message on screen,
  once.
- **Removed, completely:**
  - the `typed_messages` table (a migration drops it);
  - `keep_typed`, `typed_kept`, `sent_key` and the in-memory map;
  - the lookup in the session event path (`read_typed_with`, `Typed`,
    `typed_wanted`, `with_typed`);
  - `AsTyped` and `TurnInput.typed`, replaced by the pieces.
- **Tests.**
  - A round trip: send, replay, show, for both the pieces path and the
    fallback text path.
  - A replay of a session with thousands of linked messages, which does no
    per-message work until a message is drawn.
  - The fallback format's writer and reader tested together.

## Part 2: what the reviews left (from `chat-findings.md`)

Each of these is either fixed properly here or written up for the user
with the reason it can't be.

1. **An update's time.** The cockpit stamps an event when it applies it,
   so in a tab that was hidden, old activity looks new. The server stamps
   each event when it happens, and the live line uses that stamp.
2. **Opened on a silent agent,** the "no word for N min" count starts when
   you opened the cockpit. With the server's stamps (1), it counts from the
   agent's last real update.
3. **Absolute paths in the agent's messages aren't links,** because the
   chat doesn't know the project's folder. The server tells the cockpit
   its project folder once, and the chat uses it as the terminal does.
4. **What counts as the agent's news** is decided in the store. Look at
   whether the server should mark events by who caused them (you or the
   agent) instead of the store inferring it.
5. **Other views don't say when their search items change,** so the key
   bar searches them again every frame. Each view says when its items
   change, as the chat now does.
6. **The chat walks every row each frame** (cheaply, but it's still the
   whole list). Look at keeping row positions so a frame only touches the
   rows near the screen.
7. **Gathering the chat's actions copies what's waiting on you every
   frame.** Return references, or cache it per change.
8. **The key bar cuts a long label at 40 characters** by hand. Look for a
   cut by width that doesn't shorten the wrong chips.
9. **Two SHA-256 helpers remain** (code-analyzer, nah-packs tests). Once
   Part 1 removes `sent_key`, check whether one shared helper is worth it.
10. **The session layer calling up into the slots** for typed messages is
    gone with Part 1.
11. **Menu entries are small, split targets** (raised by the user,
    2026-10-25). In the which-key menu each entry is a small key chip and
    its words, two separate click targets 22 points tall, and nothing
    shows that the whole entry is one thing. That's awkward to tap.
    - Each entry becomes one target: the whole cell, highlighted under the
      pointer and when pressed, like the other buttons.
    - It gets a touch-sized height on touch screens (about 40 points; the
      key bar's buttons already do this).
    - It's built once, as one entry piece shared with the other lists, so
      the panels' menu buttons and every list behave the same, not
      patched in one place.
12. **How an agent is doing is worked out separately in each place that
    names it** (raised by the user, 2026-10-25). The chat checks the agents'
    state (no session: "No session yet…", with New session and Resume). A
    discussion's "waiting on lead" reads only the discussion and never
    asks whether the lead is running. The inbox, the plan tree and the
    pickers word an agent their own ways.
    - One answer, in the shared agents service: an agent slot's state (has
      a session and is working, idle, no session, can't run and why), with
      its words and what to do about it (resume, new session).
    - Every place that names an agent reads it: the chat's banner, a
      discussion ("waiting on lead: no session running", with Resume), the
      inbox, the plan tree, the pickers.

13. **A request waiting on you shows no answers while you're typing**
    (found in the live demo, 2026-10-25). After you send, the chat stays
    in typing mode, and in that mode only the box's own buttons show: the
    request's card ("Run invoke_subagent?") had no Allow or Deny until
    Escape. What waits on you always shows its answers, typing or not.

## Part 2b: one layer over the panels (raised by the user, 2026-10-25)

What sits on top of the panels is built separately in each place today,
each as its own floating area with its own rules for closing:
- the which-key menu;
- the key hints after a pause;
- the action list and its pickers;
- the quick look card;
- the floating buttons under what you're on;
- the hint labels;
- the inspector, the issues window, the toasts.

Most close only with Escape (or a button that says so), which is poor on a
tablet or phone.

The approach:
- **One concept.** An overlay is a second layer stacked over the panels.
  There's one stack of them, owned by the shell; each kind of overlay
  says only what it draws and how it behaves.
- **One set of rules for all.**
  - A tap or click outside an overlay closes it (and isn't passed to what's
    under it).
  - Escape closes the top one.
  - Closing gives the keyboard back to where it was.
  - What's under an overlay doesn't take keys or taps while it's up,
    unless that overlay says it lets them through (the hint labels, a
    toast).
- **Room to grow.** A new kind (a splash screen, something stacked on
  another overlay) is a new entry in the stack, not new closing code.
- **Near zero cost.** The stack is a few entries, looked at once a frame;
  each overlay is drawn as today. Nothing is allocated per frame for it.
- **Tests.** Tapping outside closes each kind; Escape closes the top one
  of two stacked; keys and taps don't reach the panel under a modal one;
  the keyboard comes back where it was.

## Part 2c: the agents' connectors, updated (asked by the user, 2026-10-25)

Both harness packs move to the newest connector, so features such as
compaction reach the cockpit. The cockpit lists whatever commands an agent
offers. Checked 2026-10-25:
- **Claude Code:** we pin `claude-agent-acp` 0.89.1; the newest on npm is
  0.95.0. It already offers `/compact`, `/autocompact` and `/context`.
- **Antigravity:** we pin `agy_acp_server` 1.1.1. It offers only `/plan`
  and `/logout`: no compaction. No newer release was found at the address
  the pack downloads from (1.1.2 through 2.0.0 tried). The `agy` command
  line installed here is 1.4.5, but it's a separate program and has no
  connector command. First step: find where Google publishes newer
  connector releases (release notes, the `agy` changelog, the extensions
  page), and confirm a newer one has compaction.
- **For each:**
  - the new version and its checksums go in the pack;
  - install, then probe it the same way as before (it starts, signs in,
    takes a prompt with attached pieces, replays a session);
  - check its commands list in the chat;
  - the harness contract tests and the recorded Claude Code shapes are
    run again;
  - anything the connector changed (new fields, renamed ones) is handled
    where the adapter reads it, not worked around.

### Done (2026-10-25, commit 3e1a805)

- Claude Code: `claude-agent-acp` 0.95.0 (pack 1.1.0). Attached pieces
  read and replayed as before (each replayed piece of your message now
  has a message id). Commands include `/compact`, `/autocompact`,
  `/context`.
- Antigravity: `agy_acp_server` 1.4.3 (pack 1.2.0), found in the ACP
  agent registry (`cdn.agentclientprotocol.com/registry/v1/latest/registry.json`):
  Google renamed the download (`agy-acp-server-{v}-{asset}.zip`), and it
  answers its version as plain `1.4.3`. Commands: `/plan`, `/logout`,
  `/usage`. **Still no compaction** in its connector (the `agy` command
  line has `/compact`; the connector doesn't offer it).
- Found on the way: our saved-session listing asked for `<project>/.`,
  which 1.4.3 matches as text and found nothing in. Fixed, with the test
  comparing the text sent.

## Part 2d: what shows the agent at work is a look setting (asked by the user, 2026-10-25)

The heartbeat (or nothing) is picked on each chat's settings row today,
beside "Shows:". It's a look-and-theme choice made once, not something
changed while working.
- It leaves the chat's row, and isn't kept per chat any more.
- It's set in the UI config (`[chat] indicator`), with the theme's other
  settings, and stays reachable from the action list (`:`, `?`) as a
  setting of the cockpit, not of a chat.
- "Shows:" (how much a chat shows) stays on the row, kept per chat.

## Part 2e: full-screen programs in the terminal lose their lines (raised by the user, 2026-10-25)

A full-screen program (an editor, `htop`, a pager) runs in the cockpit's
terminal, but rows it doesn't redraw disappear: only the rows that just
changed show.

Likely cause, found reading the code: the reader's screen
(`harness-contracts` `ReaderScreen::take`) wipes the whole screen on every
update while a program has the full screen (`self.alternate ||
update.alternate`). Updates carry only the rows that changed, so every
other row is blanked. The wipe is meant for switching between the shell's
screen and the program's, not for every update while in it.
- Confirm it first: a test that sends a full screen, then an update with
  one changed row, with a full-screen program running, and checks the
  other rows are still there. It should fail today.
- Fix: wipe only when switching screens (entering or leaving the
  program's); every other update changes only the rows it carries.
- Check live in the user's tab with a full-screen program (e.g. `less`,
  `vim`, `htop`) in a terminal panel, not the chat's box.
- Small, if the cause is confirmed. If it isn't, what was found is
  recorded for the end summary and the plan goes on.

## Part 2f: Antigravity's sub-agents, as it really sends them (live demo and probe, 2026-10-25)

Recorded from a live run (one sub-agent prompt, every raw message kept,
`chat-cleanup-probes/live-agy.json`) and a replay of the demo
session:
- **The asking call ends "failed" by Antigravity's own doing.** It asks
  with one call ("Run invoke_subagent?", its own id). Once you allow it,
  the sub-agent runs under another call ("Running start_subagent"). At the
  end Antigravity marks the asking call `failed` with "Tool call was
  approved but never executed." The cockpit showed `[FAIL]`, faithfully
  but misleadingly.
- **The sub-agent's steps can be told apart by their call ids.** The main
  agent's calls are numbered `<session id>:1`, `:2`, …. The sub-agent's
  carry another id before the colon (its own conversation's). There's no
  `_meta` pointer as Claude Code has.
- **Messages carry nothing to tell whose they are,** and each piece is a
  whole message, not a fragment of one. The cockpit joins consecutive
  pieces into one message ("Waiting for its response.Done.There are 10").
  In the probe the answer "10" came three times: likely the sub-agent's and
  the agent's, which can't be told apart.
- **A replay drops the sub-agent's steps entirely,** and shows the asking
  call as completed.

**What changed in 1.4.3** (one prompt, `chat-cleanup-probes/combined-agy-143.json`):
- The asking call is no longer a stand-in that ends failed: "Run
  invoke_subagent?" becomes "Running invoke_subagent" and ends completed.
- Every call id is a plain random id now, the main agent's and the
  sub-agent's alike: the call-id rule below no longer exists. Nothing
  sent tells the sub-agent's steps from the main agent's.
- Live, the sub-agent's commands are sent, under the main session; the
  main agent's own checking-and-waiting calls (`manage_subagents`,
  `schedule`) aren't. The replay is the reverse: the main agent's calls,
  none of the sub-agent's.
- No duplicated answers this time.

So the 1.1.1 approach below is overtaken: what's built for Antigravity is
what 1.4.3 sends (a sub-agent started, with its role, type and task, from
the start call's input; its steps shown where they come, not attributed),
recorded as a deviation for the user.

The approach, as planned for 1.1.1 (its quirks are its pack's, as Claude
Code's sub-agent pointer is Claude Code's):
- **How a sub-agent's call is recognized:** for Claude Code, by its
  `_meta` pointer; for Antigravity, by a call id that isn't the session's
  own. Its parent is the sub-agent start call it belongs to (the first
  unclaimed `start_subagent` for a new id). One rule in the adapter, and
  each pack names its way.
- **An asking call that's replaced:** the pack says Antigravity's asking
  call is a stand-in. Once allowed it shows as answered and points to the
  call that ran, not as failed.
- **Whole-message pieces:** the pack says each piece is a whole message.
  Pieces are kept apart (a paragraph between them), not run together.
- **Duplicated answers** (the "10"s) are recorded as a finding to watch
  for. Sub-agent messages can't be attributed from what Antigravity sends.
- **Tests:** the recorded live messages and the replay become test
  fixtures for the adapter, as Claude Code's recorded shape is.

## Part 2g: sub-agents as things you can see and act on (asked by the user, 2026-10-25)

The aim: one consistent way to see and act on any agent's sub-agents
(check on them, open what each did, cancel), each harness giving as much
of it as it can, the rest shown as not available.

What each harness gives, probed live 2026-10-25 (`live-claude.json`,
`chat-cleanup-probes/live-agy.json`):

| | Claude Code (claude-agent-acp 0.89.1) | Antigravity (agy_acp_server 1.1.1) |
|---|---|---|
| Says a sub-agent started, with its name and task | yes, when the client declares `subagents: {}`: `subagent_spawned` (its own session id, name, task, prompt) | only its start call's input: role, type, prompt |
| Which steps are the sub-agent's | its updates come under its own session id (and `_meta` names the parent call) | by call id (the sub-agent's carry another id) |
| Its messages, apart from the main agent's | yes (its session id; each message has an id) | no |
| Its state (running, done, failed, cancelled) | `subagent_state_update` | only its start call's status |
| Cancel one sub-agent | no: cancelling stops the whole turn (each sub-agent then reported cancelled). Background shell tasks have a stop (`_session/async_task/stop`, behind another opt-in), not checked for sub-agents | no: the whole turn only |
| Message one sub-agent | no (only through the main agent) | no |
| After a reopen | not checked | its steps are dropped |

**Updated for the new connectors** (2026-10-25,
`chat-cleanup-probes/cancel-child-claude-095.json`,
`combined-agy-143.json`):
- **Claude Code 0.95.0**, with `subagents: {}` declared, sends the
  sub-agents form of the ACP draft ("RFD"), not 0.89.1's
  `subagent_spawned`:
  - `subagent_update` on the parent: the sub-agent's own session id,
    title (its type), description (its task), `capabilities: {cancel: {}}`,
    and `state` (`running`, `requires_action` while it waits on you,
    `idle` with a `stopReason` of `end_turn`/`cancelled`/`error`, or
    `unknown`);
  - `session_message`: the task sent to it and its answer, each with
    sender and recipient session ids (the launching Task call itself isn't
    shown);
  - its own steps under its own session id, each with `_meta`
    `parentToolUseId`.
  - **Cancel one sub-agent: yes.** `session/cancel` with its session id
    stopped it (`idle`, `cancelled`), and the main agent carried on and
    finished its turn.
- **Antigravity 1.4.3:** a sub-agent's start (role, type, task) only; its
  steps can't be told apart (see Part 2f); no cancel of one; no state but
  its start call's.

The approach:
- **The cockpit declares what it supports** (`subagents: {}` to Claude
  Code), so the harness sends the richer form.
- **One model for every harness:** a sub-agent has an identity (name,
  task, type), a state, and its own steps and messages, under the call
  that started it. The adapter fills it from whatever the harness sends
  (Claude Code's events, Antigravity's call ids and start-call input).
- **How they show** (a first design, agreed with the user 2026-10-25, to
  refine as it's built):
  1. **A sticky strip above the typing box,** where what waits on you
     sits. One chip per running sub-agent: its name, a state dot, what
     it's doing ("Reading parse.rs"), how long it's run. Several fold
     into "3 sub-agents working" (tap to open). A finished one shows done
     or failed a few seconds, then goes. None running: no strip.
  2. **A chip opens a pop-up** (the overlay layer of Part 2b: tapping
     outside closes it) with that sub-agent live: its steps and messages
     as they come, its instructions, its controls (cancel where
     supported; go to where it started in the chat).
  3. **The history stays calm:** one folded line where it started ("File
     Counter · done · 3 steps"); opened, what it did.
  4. **Signs elsewhere:** the live line ("Thinking… · 2 sub-agents
     working"); a count on the chat panel's top line, seen when the chat
     isn't focused.
  5. **Keys:** move between sub-agents as `] ]` moves between messages;
     Enter opens the one you're on.
  6. **Later, one way for every agent:** the same strip and pop-up for
     the agent slots we start ourselves (a reviewer, a worker), so
     harness sub-agents and ours are watched and acted on alike.
- **Controls, by what the harness offers:**
  - cancel the turn (all of them): both;
  - cancel one, or message one: offered where a harness supports it,
    shown as not available otherwise, never faked;
  - the stop for background tasks is probed for sub-agents before it's
    offered.
- **Tests:** the recorded live messages become fixtures for the adapter
  for both harnesses.

## Part 3: the user's decisions (from all the findings files)

Listed so nothing is lost. Each is built only once the user decides.

- **Fonts:** decided (2026-10-25): egui's default stays for now. The rest
  (JetBrains Mono as default, the jump on first load, compression) is in
  the look-and-theming review in TODO.md.
- **Chat:** accepted by the user (2026-10-25): one copy key `c` (and `e`
  for a change request), the pinned activity line, what quiet folds, long
  chats drawing only what's on screen.
- **Menus and lists:** decided (2026-10-25): all stay as they are.
- **Sign-in and security:** decided (2026-10-25): left as they are for now
  (the same exposure as running a harness natively); a TODO for later.

## What you test

Some checks need your accounts: signing in, real agent turns. Everything
is built and checked here as far as that allows. The end of this plan is a
list for the user: each item says what to do and what should happen, so a
check takes a minute. It includes what's still to be checked from the
chat plan (sub-agents in a real turn, a real Google sign-in).

### Your test list (2026-10-26)

Checked by the user (2026-10-26): 1 (Enter sends) and 2 (sub-agents).
The rest moved to TODO.md, "Things to test by hand"; touch waits for a
sitting on another device.

Each takes a minute. The server on 8080 runs the final build (840cd11).

1. **Enter sends.** In a chat, `i`, type a few words, press Enter on
   your keyboard. It's sent. (A synthetic Enter from the driver doesn't
   reach the box, so this wasn't checked with a real key.)
2. **Antigravity's sub-agents.** In the lead's chat (Antigravity), ask
   it to start one sub-agent for a short task. Its item and a strip chip
   show it at work; the live line says "1 sub-agent working"; a double
   tap on the chip opens it in the quick look, which says its harness
   doesn't say which steps are its, and offers no "Stop it". Close the
   session and resume it (Space s l): the sub-agent is there, ended, and
   nothing flashes as just ended.
3. **Touch, on a phone or with touch.** Tap a chat's sub-agent chip: the
   cursor goes to where it started. Tap twice: it opens. Open the leader
   menu and tap outside it: it closes, and nothing under it runs.
4. **Google sign-in.** Sign Antigravity out (Account, Space a), sign in
   again with Google from the sign-in card: the link opens, the code
   pastes back, and the card says signed in.
5. **A session that won't start.** Start one in a slot whose harness
   can't run (or with the network off): the chat and the slot's light
   say "couldn't start a session: …". Put the error away: "Resume its
   last session" shows again.
6. **A discussion's wake.** With the lead's session closed, open a
   discussion waiting on the lead: it says "waiting on lead: no session
   running", and `w` (Resume / Start) gets the lead going.
7. **Your messages with links.** Send the lead a message with a `#tag`
   and an `@file`, then resume its session: the message comes back with
   its chips, its pasted content folded under them.
8. **The decisions under "Decide"** in `chat-cleanup-findings.md`: five
   choices (replays read on arrival; Enter in normal mode; who caused an
   event; the chat's folder; the at-work setting's home).

## Order

Part 1's checks are done (above). Each step is built, tested, checked live
in the user's tab where it shows, and committed on its own.

1. **Part 2c, the connectors.** Claude Code to the newest; Antigravity's
   newest found and taken if it's newer. Everything after is built and
   probed on them.
2. **Part 1, your messages** rebuilt from what the agent was sent; the
   lookup table removed. Stopping point 1 (see Process).
3. **Part 2e, the terminal's full-screen programs** (small).
4. **Part 2b, one layer over the panels** (the sub-agent pop-up builds on
   it).
5. **Parts 2f and 2g, sub-agents:** Antigravity's way recognized, then
   one model for both, the strip, the pop-up, the controls each harness
   offers.
6. **Part 2, the rest,** item by item (items 11–13 included), and **Part
   2d**.
7. **The end review, fixes, the re-read; the test list for the user; the
   end summary.**

## Process (stopping points, per `process/build-and-validate.md`)

Every stopping point runs the full routine: linters (clippy desktop and
browser, the cockpit's lints, ast-grep), the whole suite one package at a
time, the live pass in the user's tab, the reviewers (fresh agents given
only this plan's goal and rule, the system description and the changed
code; briefs in `process/reviewers.toml`), fixes for what's real, and a
record. Reports are saved verbatim in `chat-cleanup-reviews/`; what was
found, fixed, left and put to the user goes in `chat-cleanup-findings.md`.

1. **After Part 1** (your messages): responsibility; performance, with
   sessions of thousands of messages; correctness; faithfulness to this
   plan's rule.
2. **After Parts 2b, 2f and 2g** (the overlay layer and sub-agents):
   responsibility, correctness, performance, consistency.
3. **At the end:** all eight reviewers on the whole change, then a
   two-reviewer re-read (responsibility, correctness) of the fixes, and a
   full "day in the life" live run.

Between stopping points each step is built with its tests, checked live
where it shows, and committed on its own.

# Chat cleanup: findings

What each stopping point found, and what happened to it. Reports are in
`chat-cleanup-reviews/`. The plan is `chat-cleanup-plan.md`.

## Along the way

### Step 1, the connectors (Part 2c, commit 3e1a805)

- Claude Code 0.95.0 and Antigravity 1.4.3 installed and probed (see the
  plan's Part 2c "Done"). Pack, contract and Claude Code end-to-end tests
  pass; the lead session replays on 1.4.3 in the user's tab, its commands
  listed.
- **Fixed:** the saved-session list was empty on 1.4.3: we asked for
  `<project>/.`. The old test compared paths, which ignore a trailing
  `.`; it now compares the text sent.
- **Left (Antigravity's):** its saved sessions have no titles and no
  dates ("Session 171da13e", 96 of them here): picking one means knowing
  its id. Same in 1.1.1.
- **Left (Antigravity's):** no compaction offered by its connector.

### Step 2, your messages (Part 1, commit fca4ad8)

- Built as planned: each linked tag or thing goes as its own ACP piece,
  labelled `nah://tag/<id>?at=4-9,20-25&include=auto` (or
  `nah://thing?...&subject=<json>`): the label says what it links and
  where the words link it. The item carries the words, the links, and
  what they handed on. Each pack declares how its replay writes a piece
  back (`replayed_piece`); `harness-acp/src/pieces.rs` writes and reads
  labels, pieces, and our own text form for a harness that takes no
  pieces. Removed: the table (migration 16 drops it), the in-memory
  copy, `sent_key`, the event-path lookup, `AsTyped`, `TurnInput.typed`.
- Checked live with Claude Code 0.95.0: a message linking a function
  showed its chip and "what went with it"; Claude answered from the
  piece; resumed (Claude's replay), the chip and the fold came back.
  Antigravity checked against its recorded replay in tests only (its
  quota is scarce).
- **Found and fixed on the way:** a discussion's message told to an agent
  had its links pointing into the wrong text (spans of the message,
  applied to the longer text around it). A message held for a slot now
  keeps its links (expanded on delivery) instead of the pasted text.
- **Left:** messages sent before this change, with links, replay as the
  agent got them (the pasted text in full): the table that showed them
  otherwise is gone, by design.
- **Left:** a piece's label is found in Antigravity's replay by its text
  (`Embedded resource (nah://…):`), with nothing marking where its content
  ends. A file whose own text contains that exact heading with one of our
  labels would split wrong. Labels must also fit the message's words, so
  a stray one is ignored.
- **Deviation:** a replayed message is read back by the adapter as it
  arrives (once each, linear in its length: 3,000 messages of 16 KB in
  about 0.05 s in the test), not deferred until it's drawn: only the
  adapter knows the harness's format.
- **Deviation:** the fallback format's owner is `harness-acp` (with the
  labels and the replay reader), not `nah-things` as the plan said: the
  reader is the adapter's, and `nah-things` only makes the content.
- ~~Left (old): the ast-grep test fails on a divider in `skills/…`~~
  Wrong: it failed on my own code (a panicking fallback in `pieces.rs`);
  the divider is only a warning. Fixed in 811c413.
- **Found (driving the tab):** Enter pressed by my tab driver while typing
  doesn't send in the browser, though the same key events send in the
  cockpit's tests. My live checks use the send action instead. On your
  test list: type in a chat and press Enter.

### Stopping point 1 (after Part 1): four reviewers, commit 811c413

Reports: `chat-cleanup-reviews/checkpoint1-*.md`. Suite: all green but
`lint_test` (mine, fixed). Live: a Claude Code message linking two
symbols showed both chips, Claude read both pieces, and its replay read
back the same.

**Fixed:**
- Claude Code lists every label, then every `<context>` (its
  `promptToClaude`): a message linking two things didn't read back
  (correctness 1, faithfulness 1). Its pack now says so (`listed`).
- Look-alike text in a piece's content or in your words could make fake
  pieces or cut real ones (correctness 3). Labels now carry the content's
  length; each piece's text is taken by its length, and the rest must be
  wholly pieces.
- Reading back could rescan the text at every `nah://` (performance 5,
  correctness 5): a label ends at its length's digits and is never looked
  for past a space.
- One include for all of a piece's links: chips changed after a replay
  (responsibility 1, correctness 4). Each link's own include is in the
  label now.
- Antigravity replays without message ids: two of your messages in a row
  merged (correctness 2). Its pack says each chunk is a whole message.
- Each replayed chunk re-sent the whole message so far (performance 1,
  responsibility 7): the item is made once, when the message has all come.
- The label format belonged with `Link`, not the ACP adapter
  (responsibility 2): it's `Piece::label`/`read_label` in
  harness-contracts now, using the types' own serde forms, tested there.
- `nah-things` carried links it never read (responsibility 3): `attach`
  returns a text per part; the server pairs them with their links.
- The item split pieces into two lists (responsibility 4, faithfulness 5):
  it carries its pieces.
- The packs' templates were copied into tests (responsibility 5): the
  tests read the shipped packs.
- The template was parsed twice, the second time quietly (responsibility
  6): parsed once, with the profile.
- `pieces::blocks` also built attachments (responsibility 8): back in
  `session.rs`.
- Two shapes for one result (responsibility 9): one `Expanded`, in
  harness-core (the cockpit never used it); a held message holds a
  `Message`.
- Joining held messages was quadratic (performance 4): linear.
- Every send copied the session's whole history (performance 3, older
  than this plan): a send listens to the session only when it waits.
- Small copies per send (performance 7): pieces moved, our template built
  once.
- The tests' "recorded replays" were hand-edited, and the new labels were
  never sent to Antigravity (faithfulness 7, correctness 7): one fresh
  prompt to each harness with two labelled pieces; both read both, both
  replays are fixtures (`harness-acp/fixtures/`), read back exactly.
- No send-replay-show round trip (faithfulness 6): added, for both ways of
  sending.
- The README described the removed table (faithfulness 8).

**Left:**
- Messages sent before this change (and the ones I sent this afternoon
  with the first label form) replay with their pasted text (correctness
  6): the table is gone by design.
- A message's size is its 16 KB budget plus about 300 bytes per link (each
  link past the budget is still a piece, so its chip survives a replay);
  links per message are bounded only by what you type (performance 2).
- An open "what went with it" copies each piece's text per frame
  (performance 6): only while open and on screen.
- A draft change made within the save delay before a reload is lost (found
  live: a cleared box came back).

**Deviations (with the plan):**
- Replay is read in the adapter as each message ends (once, linear), not
  deferred to drawing (faithfulness 2): only the adapter knows the
  harness's way. 3,000 messages of 16 KB: about 0.05 s.
- "Showing it": chips aren't matched by the chat any more; the labels say
  where each link is (faithfulness 3). The chat draws them as every
  message is drawn.
- The fallback format and labels aren't in `nah-things` (faithfulness 4):
  labels with `Link` (harness-contracts), the templates with the adapter.
- Built beyond the plan (faithfulness 9): `held_reports.links` (a held
  message keeps its links; it's a delivery queue, not a copy of something
  kept elsewhere: before, it kept the pasted text), `Message::after` and
  `joined`, the discussion links fix, merging a thing linked twice by what
  it is.

### Step 3, full-screen programs in the terminal (Part 2e, commit f5770b0)

- Cause confirmed as the plan read it: the reader's screen was wiped on
  every update while a program had the whole screen. Now only when a
  program takes the screen or gives it back (the emulator marks every row
  changed then, so nothing is lost). A test sends a full screen, then one
  changed row: the other rows stay; going out starts the screen anew.
- Live: `watch -n 1 "date; echo second line; echo third line"` in a
  terminal panel kept its other lines while the time changed each second;
  `less` scrolled and quit back to the shell's screen intact.
- The same reader is the server's record of what each reader has, so it's
  fixed there too.

### Part 2 item 13: a request's answers while typing (commit ff7e269)

- Cause: the answers had the "under the item" placement, shown only while
  not typing. They're now placed under what waits on you, shown typing or
  not (in a panel's bar, in its one row with the box's own).
- Live: Claude asked to run `touch …` (Ask first); the chat stayed typing;
  Yes and No showed beside Send; tapping Yes answered it and the file was
  made.

### Step 4, one layer over the panels (Part 2b, commit e1a2167)

- Built as planned: `overlay.rs` (kinds, their rules, where each was
  drawn, the tap-outside pass) and `app/overlays.rs` (which are up: each
  one's own state, read; closing one as its own way out does; Escape; a
  modal one keeps keys). The tap is taken before the frame (the raw-input
  hook), because panels are drawn before overlays: that's the only point
  where it can be kept from them.
- Kinds: the action list and pickers, the menus, the quick look (its own
  rule replaced; its floating controls count as part of it), the issues
  and feedback windows (close on a tap outside; Escape closes them, which
  they didn't do), the inspector (every tap is its pick: stays), the
  pending keys' popup and hint labels (close and let the tap through).
- Tests: a tap outside closes each and doesn't focus the panel under it;
  Escape closes the list over the quick look, then the look; under the
  issues, `j` moves nothing; the menus opened while typing, left by Escape
  or a tap outside, give typing back.
- Live: the action list, the menus and the issues window each closed on a
  tap on a panel, and the panel wasn't focused by it; the next tap was.
- **Left:** a setting's list of choices (egui's own popup) still closes
  itself on a tap outside and lets the tap through; the layer leaves taps
  alone while one is open.
- **Left:** toasts, and the controls floating under what you're on, aren't
  pop-ups: never closed by a tap.

### Step 5, sub-agents (Parts 2f and 2g, commits 12660e6 and 29b5dde)

- One model for both harnesses: a `Subagent` (name, kind, task, prompt,
  what it's doing, state with "waiting on you", whether it can be stopped
  alone) and an item where it started, its steps under it when the
  harness says which are its.
- Claude Code (0.95.0, told we show sub-agents): its announcements, its
  messages, and its steps under its own session, taken by a raw handler
  ahead of the typed one (the ACP library we build on, 2.2.0, pins a
  schema without these: a typed parse of them would fail the connection).
  The handshake is written from the typed one with `subagents: {}` added.
- Antigravity (1.4.3): a sub-agent from its start call's input (role,
  type, prompt), once the call goes ahead; it ends with the turn. Its
  steps can't be told from the agent's own (all call ids are random now),
  and it can't be stopped alone: the pop-up says so.
- Stop one alone: `POST /api/threads/{t}/subagents/{id}/cancel` (Claude
  Code's `session/cancel` on its session).
- The cockpit: the strip, the pop-up (an overlay), the line where it
  started (folded steps), `] a`/`[ a`, Enter, counts on the live line and
  the panel's top line.
- Tests: both harnesses' recordings as fixtures through the adapter; a
  refused start call; the strip, folding, the pop-up and its steps, Stop
  and its absence, the keys, the counts.
- Live (Claude Code): a sub-agent running `sleep 60` showed in the strip,
  on its line and in the live line; opened from its line, its steps and
  task; a second running `sleep 240`, opened from its chip and stopped
  alone: "stopped", its call failed, and the agent went on to "parent
  done".
- **Fixed on the way (live):** its answer showed twice (Claude Code sends
  it as its own message and as a message to its parent); the "●" dot has
  no glyph (an icon now); the live line said "No word for 1 min" while
  only its sub-agent worked (now it waits on it).
- **Deviation:** a chip is a place, as everything in a view is (the
  cockpit's input model: a tap puts the cursor there, a double tap or
  Enter opens); the plan said "a chip opens a pop-up". A tap shows its
  Open under it; the views draw no controls (the cockpit's rule).
- **Deviation:** 2f's "pieces are whole messages, kept apart" doesn't hold
  in 1.4.3: its live messages come in fragments of sentences; only your
  replayed messages are whole chunks. So nothing is put between its
  chunks.
- **Left:** a sub-agent's running time counts from when the server heard
  of it (stopping point 2 moved it there from the cockpit). A 60 s sleep
  still showed 41 s, because Claude Code announces it only after its first
  step begins.
- **Left (6, later):** the same strip for the agent slots we start
  ourselves.

### Stopping point 2 (after 2b, 2f, 2g and item 13): four reviewers, commit 46ab4c5

Reports: `chat-cleanup-reviews/checkpoint2-*.md` (responsibility,
correctness, performance, consistency).

**Fixed, the server:**
- A sub-agent's own session settings replaced the main agent's controls
  (all four reviewers). The fixture shows it: the child's three
  `config_option_update`s turned the lead's mode picker into "Manual" and
  its model list into one entry. Now only its steps (what it says,
  thinks and calls) are taken from its session. Its settings, mode,
  usage, commands, plan and title are dropped. Tests: the fixture's
  controls, and a child's config, mode and title sent on purpose.
- Two rules nested a sub-agent's steps (`parent_call` in the pack and in
  the state, beside the child-session rule). `parent_call` is gone: with
  sub-agents declared, Claude Code never sends it on the main session.
  The fake harness and the contract test now use each pack's own way.
  Claude Code's way is the announced, own-session form, and the test also
  checks that its session's mode isn't the agent's. Antigravity's way is
  its start call.
- Raw ACP JSON was read inside the session state. Reading is now
  `convert::session_update`, one handler takes every `session/update`
  (one lock, no copy of the update), and the tests go through the same
  path.
- A sub-agent's answer was lost when Claude Code hands back, and a second
  task sent to an idle sub-agent never showed. Messages are now read on
  any session: one to the sub-agent is its task (the latest), and one
  from it to its parent is its answer.
- Sub-agents could stay "working" forever:
  - the connection dropped (now they fail with it);
  - one announced without a state (now it isn't at work);
  - an Antigravity turn stopped or failed showed its sub-agent as done
    (now it ends as the turn did).
- A sub-agent of a sub-agent went to the top level without its task. Now
  it sits under its parent.
- An Antigravity session reopened showed no sub-agent: its replay sends
  the start call's input as text. That text is read now, and what a
  replay started ends with the replay. Its replay is now a fixture
  (`subagent-replayed-antigravity-1.4.3.json`, from the probe).
- A sub-agent still at work after its turn opened a new numbered turn
  with its steps. Its steps now go in its own item's turn.
- A start call whose input came again after it went ahead (or after it
  was refused) started the same sub-agents again. Once it goes ahead or
  is refused, it's done; it's dropped at the turn's end.
- Each step re-sent the whole sub-agent, task included, for "what it's
  doing". `doing` is gone from the contract; the cockpit takes it from
  the steps as they arrive. A change is now sent only when a field
  changed, and a start is sent once.
- Antigravity's task was a shortened copy of its prompt, cut on the
  server. That copy is gone; the cockpit shows the prompt's first line.
- A request the agent withdraws (a sub-agent stopped while it asks) stayed
  pending with live buttons. `$/cancel_request` now cancels it. Test:
  the fake harness withdraws one.
- Times come from the server now: `started_ms` and `ended_ms` on the
  sub-agent.
- `Capabilities.subagents` is now filled from the pack, and the cockpit
  reads it: "its harness doesn't say which steps are its".
- The two errors for one sub-agent's stop are now worded the same.

**Fixed, the cockpit:**
- The pop-up was drawn by the shell with hand-made buttons, beside the
  quick look (responsibility, consistency, performance, correctness):
  - Stop couldn't be reached by keyboard.
  - The typing box under it kept the keyboard, so Enter sent to the agent.
  - It rebuilt every step's text every frame.

  It's gone. A sub-agent opens in the quick look, as a discussion does:
  the chat shows that sub-agent alone (`Target::Subagent`). The header
  says who it is and how it's doing. Below it come its own rows, drawn
  as the chat draws them, only those on screen, and what waits on you in
  its work. Its controls are actions on the view's row:
  - "Stop it" (`chat.stop_subagent`, Space s x). When its harness can't
    stop it alone, it's unavailable and says why; while a stop is on its
    way, it's unavailable too.
  - "Where it started" (`chat.subagent_start`, g w).

  A failed stop is said in the chat.
- Opening the quick look stops typing in the panel under it. A typing box
  has the keyboard only where keys reach: nothing modal over it, and not
  under the quick look.
- What's on top of the stack was the last drawn, so a quick look's
  controls outranked the pop-up opened over it. Now the last one opened
  is on top.
- `before_input` built a list every frame. It now asks for the top one
  directly.
- A press whose release was lost (let go off the page) ate the next tap.
  A new press ends the taken tap now. Every finger put down in the
  closing tap is taken, not only the first.
- Dragging a window's resize edge counted as a tap outside and closed it.
  The edges count as inside now.
- The palette said where it was drawn through egui's memory. It returns
  that now, as the feedback windows do.
- `QuickLook.opened` was dead and is removed. The back key is read once.
  Hints end one way.
- Item 13 was half done (correctness 4): only the request the cursor was
  on (or the first) had answers. Now every request waiting on you shows
  its answers under its own card, typing or not (`Control.under`, one of
  the view's places). Floating answers no longer cover the next card or
  the box: the chat leaves room under each card (`Drawing`'s
  `room_under_waiting`). Test: two requests at once, each with its
  answers under it, the second answered while typing.
- The store:
  - stamped a sub-agent with the cockpit's clock, so after a reload every
    ended one showed as just ended, "0 s";
  - counted every sub-agent event as stream activity, which rehashed the
    whole stream;
  - kept a second, one-level copy of the nesting (`under`);
  - looked sub-agents up one by one.

  Now: times are the server's. One that ended shows in the strip only when
  it was seen ending live, for `[chat] subagent_ended_s`. Sub-agent events
  don't touch the stream's activity. Its steps are the stream's own
  nesting at every depth (`subagent_steps`, and `row.nested` on its
  line). It has an index by id, and the count at work is kept.
- The status vocabulary: `[status.subagent_*]` and `status::subagent`
  replace the hand-made words. `span_words`/`span_changes` replace the
  copied timing, and the strip repaints only when its words change.
- The strip:
  - chips are `status::pill`, filled like everything else the cursor is
    on;
  - the count says "1 sub-agent working";
  - it folds again once few are left;
  - its doing shows only while the sub-agent runs ("waiting on you"
    otherwise).
- Its line says its steps the way a call's sub-agent's are said ("2 steps
  by its sub-agent", folded).
- The actions: next and previous are in Navigate, and the ids follow one
  scheme. `At::under_something` is the one list of places under an item.
  `first_line` is one helper, in `conversation` (the tags kept theirs
  private to their module).
- A sub-agent target is named in words, not by its raw id.
- A new quick-look buffer reads what's shared before it's asked to show
  something.
- Tests now use the shared helpers.

**Live (Claude Code, the user's tab):**
- Three sub-agents ran one after another. The lead's mode picker kept its
  five modes and its model list its five models.
- One sleeping 240 s:
  - opened from its line with Enter, in the quick look: its task, what
    it's doing, its call as it ran, and "Stop it" and "Where it started"
    with their keys;
  - stopped with Space s x: it showed "stopped", its call failed, and the
    agent went on to "parent done";
  - g w closed the look and put the panel's cursor on its line.
- Sub-agents replayed from the session's history showed done or stopped,
  with no "0 s".
- Fixed on the way: the look's row showed the agent's own mode and model
  pickers as if they were the sub-agent's (now only its own controls, and
  "Shows"). A replayed sub-agent said "0 s" (its times are the replay's;
  under a second, ended, no time is shown).

**No change:**
- An item's status still follows its sub-agent's (responsibility 10). It
  is the stream's own word for every item, read by everything that reads
  items, and it's set in one place (`change_subagent`).
- The kinds are still a closed enum (responsibility 4). The list of every
  kind is gone, and adding one now means the enum and its rules
  (`overlay.rs`) and its open and close (`app/overlays.rs`).
- Menus and the palette read the back key themselves (responsibility 5):
  their `modal` isn't read because they take their keys in their own
  modes.
- A setting's choice list is still closed by egui's own rule.
- The overlay tests' `tap` stays: it needs set times, which `click`
  doesn't give.

**Left:**
- Three ways of giving the keyboard back (the palette's, the menus', the
  quick look's buffer). Merging them means reworking the keys with care.
  The cost now is low.
- Gathering the item controls still copies the bound actions and scans
  every request per frame. That is Part 2 items 7 and 12.
- "Waiting on a sub-agent isn't quiet" is still the chat's alone (item
  12).
- Shown alone, a sub-agent's rows still start with their turn's line,
  and the running turn's line carries the agent's live words.
- In normal Claude Code (not handing back), the sub-agent's answer is
  also its own last message. The view shows it folded under "What it
  answered" as well.

### Step 6, Part 2's rest

**Items 1–4 (commit 113e2a6):**
- **1. Event times.** The hub stamps each event when it's published.
  `Stamped` carries the event and its time, flattened on the wire. The
  hub's cache keeps each entity's last time for the replay. An event
  passed on from a linked server keeps that server's time. The cockpit
  applies each event as of when it happened. Test: an update that came
  90 s ago while the tab was hidden reads "No word for 1 min", not news.
- **2. A silent agent's count** starts at its latest news in the replay,
  not at opening. Test: a replay whose news is 5 minutes old reads 300 s.
- **3. Absolute paths in the agent's messages and calls** are links, read
  from the session's working folder. **Deviation:** the plan said the
  server would tell the cockpit its project folder once. The thread's
  policy already carries the folder its session works in, which is what
  the agent's paths are written from (as the terminal's folder is for
  its lines). So nothing new is sent. When a session works in a subfolder
  of the project, its paths are read from that subfolder.
- **4. Whose news it is.** Looked at, and decided: the event already says
  whose it is. A turn appearing is your prompt (queued, or started by
  it), and an item that's your message is yours. So the store reads that
  from the event; the server marks nothing. Before this, your own prompt
  counted as the agent's news (the AGENT light, the live line's
  updates). Test: your prompt isn't news; its answer and its turn's end
  are.
- **Left:** a linked server's times are compared with this machine's
  clock. Clocks on different machines can differ by their drift.

**Item 5 (commit 100abe6):** a view now says when its search items
change. It does this for the code view, the graph, the queue, the inbox,
the outline, the terminal, a discussion, the tags list and the chat. The
key bar works out its place among the matches again only on a change.
Test: every view but two says so.
- **Left:** the spec card and the changes view still don't say. Each
  reads several sources: the review's comments and findings, the compared
  files, and what's opened. Each would need its own signal. The spec
  card's list is short.

**Item 6 (commit 419169c):** a frame draws only the chat's rows near the
screen.
- The chat keeps where each row is, worked out once per change of the
  shape or of a row's height.
- The viewport takes those places as kept (`Measured::laid_out`) instead
  of rebuilding them each frame.
- The running turn's line is still placed when it isn't drawn, so it
  pins to the top.

Test: a chat of 2,000 messages measures only a few screens of rows, and
`gg` gets to the top. Live: a resumed session drew right at its top and
its bottom, and moved down correctly.

**Item 7 (commit 2da0933):** the store keeps which requests are pending.
The chat reads them in place, never copied and never by walking every
request. Test: the pending list is kept.

**Item 8 (commit 790657a):** the key bar cuts only a chip too wide for a
third of the bar, by width, whole on hover. Test: a long request's chip is
cut to its share and the rest show whole.

**Item 9 (commit ba04422):** one `sha256_hex` and one `hex` in nah-packs,
used by the engine (download checksum, cache key) and by both test files.

**Item 11 (commit ed75ca6):** one entry piece (`actions::entry`): a key's
pill and its words in one cell, tapped anywhere in it, lit under the
pointer and while pressed, a finger's height under a finger. The menus use
it, and so does what can follow a half-typed key or a mark key (tapping
there runs the action or goes to the mark).
- **Deviation:** the panels' menu buttons and the action list's rows keep
  their own drawing. Each was already one target, an egui button and a
  full-width row. The plan asked for every list to share the piece.
  Rebuilding the action list's rows was out of proportion.

**Item 12 (commit 9e63bf7):** `AgentState` in the agents service:
closed, can't run (why), starting, no session (its last to resume),
waiting on you, working (its sub-agents after its turn too), idle; with
its words and tone. These read it:
- the AGENT light;
- the chat's no-session banner, with "Resume the last";
- the live line's "can't run", and its waiting on a sub-agent;
- the slot pickers;
- a discussion: "waiting on lead: no session running", with "Start / Resume
  its session";
- the plan tree's discussions, whose rows are rebuilt when that changes;
- the inbox's items.

`SessionStatus` and the chat's own slot words are gone. Test: idle,
working, working with a sub-agent, no session with its last to resume,
and a discussion saying so.

**Part 2d (commit 182025f):** what shows the agent at work is a cockpit
setting: "What shows an agent at work", Space l w, saved with the layout,
else `[chat] indicator`. It's off the chat's row and isn't kept per chat.
Live: the chat's row shows only "Shows".

### End review: seven reviewers, commits e43de96, 8bc4bca, 516ef76

Reports: `chat-cleanup-reviews/end-*.md` (responsibility, performance,
correctness, faithfulness, permissions, tests, consistency).

**Fixed, security (e43de96):**
- An agent could crash a session's replay in a debug build: a label's
  `len=` digits big enough to overflow `pos + len`. The sum is checked
  now (permissions 1).
- An agent could stall the server for minutes on a replay: thousands of
  labels with no space between them, each read to the next space. A
  label is now read only to its length's digits, at most 16 KB, and a
  message with more than 256 pieces isn't read through (permissions 2).
  Test: `what_an_agent_writes_cant_break_the_reading`.
- An update under a session that's neither the agent's nor a sub-agent's
  it announced was taken as the agent's (its mode, its policy). It's
  dropped now; a resumed session knows its own id before the load
  (permissions 6).

**Fixed, the chat, the clock and the agents (8bc4bca):**
- The row layout cache was keyed by an `Rc` pointer, which can be reused
  after it's freed (correctness 1). It's keyed by a shape number now.
- A drawn row changing height laid out every row again (performance 2,
  faithfulness 8). The rest of the rows now move by the difference; the
  running turn's line is found from the end (performance 3).
- Server times were compared with the browser's clock (correctness 2,
  responsibility 4). The cockpit now takes the least gap seen between an
  event happening and arriving as the server's lead, reset on a resync.
- With the quick look open, a tap on the key bar closed it and was lost
  (correctness 3). The key bar now counts as inside the top overlay.
- In bar mode, a request showed no answers while you typed
  (correctness 4). The first waiting request's answers show in the bar.
- Any failed call marked the agent "can't run" everywhere
  (correctness 5). Only a session that couldn't start does now.
- An agent's state doubled its words when it couldn't run, and bypassed
  the status vocabulary (consistency 1, 2). It's `[status.agent_*]` now.
- "Resume the last session" / "Start a session" is one answer the chat
  and a discussion both offer (faithfulness 4, consistency 9); "N
  sub-agents working" is one phrase (consistency 10).
- A resumed session flashed its old sub-agents as just ended
  (correctness 10). A replay's sub-agents are told once, as they are.
- The hub kept one sub-agent per id across threads (correctness 8). It's
  keyed by thread too.
- An older server's events had no time and broke a newer home
  (correctness 7). A missing time is taken as now.
- A private `now_ms` copy and three stamp closures (consistency 6,
  responsibility 11): `harness_core::now_ms` and `stamped_now` are the
  one clock and stamp. A test's SHA-256 hex loop is gone (faithfulness
  13).

**Fixed, the rest (516ef76):**
- A kept search copied and re-ranked the whole session on every event,
  and its status reused another buffer's matches (performance 1, 6,
  correctness 6). Matches are kept per buffer and found again at most
  twice a second.
- "Talk" over an open sub-agent kept the pop-up (correctness 9). Talk
  always shows the lead.
- The plan tree rebuilt every discussion's words on every frame
  (performance 4, responsibility 6). Only when the agents change now.
- A sub-agent shown alone cloned its whole record each frame
  (performance 7); the strip scanned every sub-agent twice a frame when
  none was shown (performance 8, early exit).
- The shell named which buffer shows a sub-agent (responsibility 3). The
  target says what it's looked at over (`looked_at_over`).
- A sub-agent shown alone stripped settings by a list of action ids
  (responsibility 2, in part): its settings aren't made in the first
  place now.
- Messaging one sub-agent is offered, shown as not available
  (faithfulness 7). The "who answers" picker shows each agent's state
  (faithfulness 4).
- Keys: `session.resume_last` (s l), `discussion.wake` (w)
  (consistency 14).
- Doc comments on the wrong functions (consistency 12, responsibility
  11); `items_changed` built one way, `nav::changes_of` (consistency 11).
- Tests (tests A1, A2, E15, F20): the withdrawn request is asserted
  cancelled before the turn ends; the key bar test has a chip over 40
  characters that fits; the sub-agent test types `i` and double-taps the
  chip; a stale doc comment is gone.
- Found by that last test: while typing, the box's own controls floated
  over the sub-agent strip, so its chips couldn't be tapped. The box's
  place now includes the strip, so its controls clear both.

**No change needed:**
- The account email in the probe recordings (permissions 5): scrubbed
  in the working tree; `dev_docs/` isn't committed.
- Percent-encoding hand-rolled (consistency 5): `urlencoding` now.

**Left as it is** (recorded; each is a design call or a cost the
heuristics say to leave for now):
- The overlay stack isn't yet the one mechanism (responsibility 1,
  faithfulness 3, consistency 3): five kinds keep their own Escape code,
  a new kind needs new match arms, toasts and the choice list aren't
  entries, and egui's `Modal` isn't used. A rework of its own.
- The chat doubles as the sub-agent view through `alone`
  (responsibility 2). One buffer, one stream drawer; the flag stays.
- `room_under_waiting` lives in the shared `Drawing` for the chat alone
  (responsibility 8).
- The chat reads "sub-agents at work" by two paths (responsibility 9).
- Pieces are zipped by position across crates (responsibility 7).
- Pickers and the action list don't use the entry piece (faithfulness
  2).
- The Spec and Changes views don't say when their items change
  (faithfulness 5).
- Chips are read from the pieces each frame, not matched once and kept
  (faithfulness 9, performance 5).
- A sub-agent shown alone rebuilds its rows when anything in the thread
  changes (performance 9).
- The server resends a sub-agent's whole prompt and answer on every
  state change (performance 10).
- Small per-frame repeats: `pill` laying out twice, `CantRun` cloning
  (performance 11).
- A third `{name}` placeholder parser in the packs (consistency 4).
- Hand touch sizes and widths (40, 32, 200) instead of theme sizes
  (consistency 7); `Fingers` as an enum beside a bool (consistency 8).
- `sha2`'s own hex formatting not used; code-analyzer's own loop
  (consistency 13).
- `look.at_work` starts a one-member `look.` namespace;
  `chat.stop_subagent` sits under the Session menu (consistency 14).
- Duplicated test helpers (consistency 15).
- Security, as decided (heuristics: same trade-off as native, write it
  down): an agent can end its message with a well-formed piece that
  reads back on replay as content the server attached (permissions 3);
  a label carries a thing's raw subject even when the reader can't read
  it (permissions 4). Both go in TODO.md.
- Tests not yet written or cut (tests A3, B4, B5, B6, C7–C11, D12–D14,
  E16, E17, F18, F19): pieces through a real adapter, the sub-agents
  declaration, `cancel_subagent` on the server, the time kept across
  home, absolute paths, wake and resume, the at-work setting saved,
  migration 16 on an old database, tap-outside for every kind; the
  self-testing and duplicated replay tests; the time-bound test.

### End re-read: two reviewers on the fixes, commit db1cde7

Reports: `chat-cleanup-reviews/end-reread-{responsibility,correctness}.md`.

**Fixed:**
- Home mixed two clocks in a linked server's stream: its own marks
  (`Resync`, `Replayed`) on home's clock, the server's events on the
  server's. A server 90 s behind made every update look 90 s old ("No
  word for 1 min" while streaming). Home now puts a linked server's
  times on its own clock (the least gap seen on that connection). Test:
  a fake linked server a minute behind (`home_tests`).
- The lead was read when a frame took an event, not when it came: a
  hidden tab's queue (a server restarted meanwhile) all looked new. Each
  socket message keeps when it came. Test: events taken five minutes
  late.
- The server-clock correction was used only by the sub-agent strip.
  Notification ages, sign-in time left, change windows and tag ages now
  read the one `ServerClock` too.
- One failed start stuck as "can't run": over a session started from
  another device, over a closed slot, and after its error was put away
  (which hid "Resume its last session"). A running session, a closed
  slot and putting the error away each win now. Test: a session running
  over a failed start.
- The key bar counted as inside every overlay: under the leader menu a
  chip ran the panel's action and the menu stayed. It's inside the quick
  look only now (`keys_on_the_bar`). Test: the bar under a menu.
- A search's matches were kept per buffer but its search per kind: a
  search in the quick-look chat left the panel chat's count from the
  old one. Matches are kept with their search. Kept a moment while the
  items change, a frame is now asked for when they're due again (a
  phone's count stayed stale till the next tap).
- The replay reader counted every `nah://` in a message, pasted content
  included: a pasted log naming labels came back inline. Pieces are now
  counted in a row (at most 256). Test: content naming labels 300 times.
- `urlencoding` accepted malformed escapes in a label. Rejected again.
  Test.
- The replay's hold-back of sub-agent news (and the emit at its end)
  never reached anyone: a session's events are followed from its state
  only once it's open, and that state already holds each sub-agent once,
  as it ended. Removed; the test reads the state as it's followed. The
  end review's correctness 10 (a resume flashes old sub-agents as just
  ended) didn't happen in production.
- The latest sub-agent end seen could move back; a sub-agent being
  stopped was kept by its id alone. Both fixed.
- `Agents::changes()` is `version()`, as the other services name it;
  `typing_rect`'s doc says it holds the strip; `replays_by`'s doc.

**Left:**
- The chat stretches its typing rect over the strip so the host's
  controls clear it (responsibility 4): the rect is "the box and what's
  drawn on it", documented on `Hosted::typing_rect`.
- Messaging one sub-agent is always offered as unavailable, with no
  capability in the contract (responsibility 5): no harness can, so no
  field is invented for it yet.
- A label over 16 KB, or more than 256 pieces in a row, reads back as
  sent text (responsibility 6): the reading limits are the format's
  documented ones; nothing on the send side enforces them.
- Row places kept in the chat's `RowsAt` and the viewport's stops,
  grown together (responsibility 7).
- The agents service carries its state's words and colours
  (responsibility 9): the plan's "one answer, with its words".
- The search pacing uses the browser's clock, so its test sleeps.
- "Where it started" names the chat buffer (it opens in the panel's
  chat, not the quick look's); the shell still names `Target::Discussion`
  (from before this plan); Talk shows the lead each time it's asked.

### End live run (2026-10-26, your tab, commits db1cde7 and 840cd11)

Checked, Claude Code live (slot claude-code 1, Manual, Opus 5.5):
- Resuming its last session: ten turns replayed; the five old
  sub-agents show once each, ended ("done", "stopped"), and the strip
  stays empty (no "just ended" flash).
- A turn with one sub-agent: its item says "working"; the strip chip
  shows its name, what it's doing ("sleep 45") and how long, ticking;
  the live line says "1 sub-agent working". It ended, the chip went after
  its few seconds, and "parent done" followed.
- While typing, the box's Send control sits below the box, clear of the
  strip. A real double tap on the chip (mouse events in your tab) opened
  the sub-agent alone in the quick look: its head (name, state, time,
  task, "Doing: sleep 150"), its steps, "Stop it", "Where it started";
  typing stopped.
- Space s x in the pop-up stopped that sub-agent alone: it reads
  "stopped", its call failed, the turn went on and answered "parent
  done".
- The leader menu open, a tap on the key bar's "j Down" closed the menu
  and didn't move the chat's cursor.
- Search: `/parent done`, then `n`: the key bar says "/parent done 2/14".
- 400 px wide (emulated): the chat and the leader menu fit; the key bar
  wraps to two rows.
- "What shows an agent at work" (Space l w): its two choices, the
  current one marked.
- Agent slots, Show an agent slot: each slot with its state ("no session
  running", "idle"); picking the lead put panel 2 back as you had it.

Found and fixed: the double tap also selected the chip's word (egui
labels are selectable). Chips' words aren't selectable now (840cd11).

Not checked live:
- Touch: touch events can't be sent to your tab while it's in the
  background (the call waits for a frame that never comes). On your
  test list.
- Enter sending a typed message: a synthetic Enter doesn't reach the
  text box (Send was tapped instead). On your test list.
- Antigravity sub-agents, discussions' wake, a session that fails to
  start: on your test list.

### Decide (for the user at the end)

- **Reading replays as they arrive** (above): the plan said "until a
  message is drawn"; built in the adapter, once per message. Accept, or
  rework?

- **Enter in normal mode, with text typed.** The key bar says "Enter Send
  the prompt", but in normal mode Enter goes into what the cursor is on
  (nothing, when it's on nothing), so it does nothing there; Enter sends
  only while typing. Either Enter sends in normal mode when the cursor is
  on nothing, or the key bar stops showing it there. Left as it is.

- **Who caused an event** (plan item 4, faithfulness 6, responsibility
  5). The server doesn't mark it; the store infers it. A turn another
  agent starts (a lead's message, a discussion's news, a held report)
  counts as your prompt, so it isn't the agent's news. Have the server
  mark who caused each event, or leave it?

- **The chat's folder** (plan item 3, faithfulness 10, correctness 11).
  Absolute paths resolve against the thread's own folder, not one the
  server tells once. A slot started in a subfolder strips the wrong
  prefix. Keep it per thread, or have the server tell the project folder?

- **"What shows an agent at work"** (Part 2d, faithfulness 12,
  responsibility 10). Picked from the action list, it's saved with the
  layout and wins over `[chat] indicator` from then on; there's no "as
  the config says" choice. Keep, add that choice, or keep it in config
  only?

### Deviations from the plan

- **Part 2f (Antigravity's sub-agents) is overtaken by 1.4.3.** The
  asking call no longer ends failed (fixed by Google), and the call-id
  rule the plan relied on is gone: all ids are random, so the
  sub-agent's steps can't be told apart. Built instead: what 1.4.3 sends
  (its start, with role, type and task), steps not attributed.
- **Part 2g gains a real per-sub-agent cancel for Claude Code** (0.91.0 and
  later), probed live; the plan said "not available".
- **Built but not in the plan** (faithfulness): requests an agent
  withdraws are cancelled and taken off the screen; held reports keep
  their links (migration 16); a route to stop one sub-agent,
  `POST /api/threads/{thread}/subagents/{subagent}/cancel`.
- **Items 1 to 4 of Part 2's rest went into one commit** (113e2a6), not
  one each.
- **The end review had seven reviewers**, not eight: responsibility,
  performance, correctness, faithfulness, permissions, tests,
  consistency.

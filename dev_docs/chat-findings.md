# Chat: review findings and what was done

## Checkpoint after step 1 (d54286c)

Three reviewers looked at step 1: responsibility, correctness and
performance. Their reports are word for word in `chat-reviews/checkpoint-*.md`.
Where they found the same thing, it's listed once.

### Fixed

1. **A request kept in sight blinked every other frame, and the chat
   repainted without end** (all three).
   - The card below the stream was pushed into the list the next frame
     read to decide whether the stream had shown it.
   - Keeping requests in sight is now one place, not a copy chosen by
     last frame's view. A request waiting on you is drawn only in a
     "waiting" area above the typing box, which is always shown. The
     history keeps a line where it was asked ("… waiting on you, below"),
     then its outcome once answered.
   - That also ends:
     - the card vanishing when the cursor went to it (correctness 3);
     - "shown" meaning one pixel shown (correctness 5);
     - a question's text box laid out twice under one id (correctness 7);
     - the kept card being a second way to draw an item (responsibility 4).
   - One function draws an item's frame (the cursor's fill, the column's
     width), for the stream and the waiting area.
   - Tested: the same place for five frames running, and no repaint asked
     once settled.
2. **What waits could push the typing box out of the panel** (correctness
   2). The waiting area takes at most 0.4 of the chat's height and
   scrolls past that. It has that height from the frame its content is
   taller (it grew 40 points a frame before). Tested with a 120-line
   command.
3. **`y y` answered the first request twice** (correctness 4). An answer
   on its way is kept in the agents' state, read by the chat and the
   inbox, so that request no longer waits on you and the keys move on. A
   failed send makes it wait again, and says why. Tested: two `y` presses
   answer r1, then r2.
4. **A request whose item hadn't arrived couldn't be answered**
   (correctness 8). What waits is read from the requests themselves. Each
   has its item's place in the stream when there is one. Tested.
5. **The width fix was in the chat's loop, for every item, every frame**
   (responsibility 2; performance 3, 4).
   - It was a horizontal scroll area per item, with egui state never
     freed.
   - Only a message's markdown table widened its column. The shared
     message drawer (`conversation::message`, also used by discussions and
     the spec) now cuts a message's markdown around its tables. Each table
     scrolls sideways in a box of the width it's given, and the words keep
     wrapping.
   - Unit-tested (the cut) and app-tested (the words wrap; nothing starts
     left of the panel or shows past it).
6. **The clip was on the panel frame** (responsibility 3): too wide, and
   not on the quick look. It's in the host's `render_view` now, around the
   content area.
7. **A request's actions and who holds the keys were the chat's own**
   (responsibility 5). `request_card::offer` builds a request's actions
   (its page, its answers) for the chat and the inbox. `request_card::keyed`
   chooses which of several holds the keys: the one you're on, else the
   first.
8. **`pending_items` copied the whole stream several times a frame**
   (performance 2; responsibility 7). `pending` reads in place, returns
   the requests themselves, and reads nothing when none waits. `target()`
   reads the one item it needs, not a copy of the stream.
9. **The old 105-point guess** (responsibility 6). What's under the stream
   starts at nothing and settles in the first frame, asking for the next
   at once.
10. **The tests didn't protect what they claimed** (correctness 9):
    - the width test now checks that nothing starts left of the panel,
      that nothing shows past it, and that words wrap;
    - the request test checks several frames and both answers;
    - the internal place key is no longer used to steer.
11. **Key bar chips with a long title** (found while testing) made the bar
    two rows, over the panels' bottom. A chip says at most 40 characters
    of its words, whole on hover.

### Left (recorded, not changed)

- **A table's hidden columns can't be reached by keys** (correctness 6).
  A finger drags them, and a wheel or trackpad scrolls them; the views'
  sideways moves act on the stream, which doesn't scroll sideways.
- **What's under the stream is measured a frame late**, and the next frame
  is asked for at once. In immediate mode, laying out a box anchored at
  the bottom from its content comes to the same.
- **`render_stream` still copies the thread's view every frame** (outside
  this step, performance's "outside this commit"), and `stream()` with it.
  It belongs with step 2, which redraws the stream as rows.
- **The request builders in tests are still three** (`request_card`,
  attention, the chat's): one per module.

## Checkpoint after steps 2-3 (6dd0d2f, 32dc365)

Three reviewers looked at steps 2 and 3 and the indicator's interface:
responsibility, correctness and performance. Their reports are word for
word in `chat-reviews/checkpoint2-*.md`. Where they found the same thing,
it's listed once.

### Fixed

1. **Ten seconds after the last update, the live line reported the turn's
   age** (correctness 1, 7, 8; responsibility 3, 4; performance 8).
   - When the agent's updates came is now kept with its thread, in the
     store, as each event lands (`ThreadView::arrivals`). Before, the
     chat's view sampled a counter once a frame, only while drawn, and
     pruned it to the heartbeat's ten seconds.
   - An update is news from the agent: a turn, an item, its plan or usage
     changed, or a question asked. These aren't: the same thing again, an
     answer's echo, a turn queued (your prompt), the thread's own changes
     (a title), and what a resync replays as it was.
   - Kept each tenth of a second once, with a count, at most 600 per
     thread.
   - So switching slots, opening a chat mid-turn, two panels on one chat,
     and a hidden chat all read the same, true record.
   - The live line describes the turn running (`ThreadView::running`), not
     one queued after it.
   - Tested: the store keeps the order and the news (unit); the live line
     says "15 s since anything" 15 s after an update in a turn that began
     35 s earlier, with a turn queued meanwhile (app).
2. **A fold you opened snapped shut when a call failed** (correctness 2,
   13). Folds are kept as set (open or folded) by item id, not as toggles
   against a default that can flip.
   - Fold everything and unfold everything (`z M`, `z R`) are one setting
     over the defaults, not one entry per item (performance 9).
   - A run is open while you've opened one of its items, so a lone call
     you opened stays open when the next call makes it a run.
   - The folds start afresh when the chat shows another thread.
   - Unit-tested: each of the reviewers' sequences.
3. **The cursor could sit on an item that's no row** (correctness 3;
   responsibility 5, 6).
   - Nothing has to keep the cursor on a row's first item now. The row
     the cursor is on is the one holding it, worked out where it's needed:
     its fill, its spot key, its buttons.
   - One numbering: the cursor, places, `:N`, search and hints count
     messages. Only the scrolling counts rows, through `Shape::row_of`.
   - Tested: switching to quiet with the cursor inside a run that folds;
     the cursor's row is drawn and keyed.
4. **A jump into a folded run scrolled by the old layout's stops**
   (correctness 4). Opening the run marks the stops stale
   (`Measured::relaid`), so the jump waits a frame for the new ones.
   Tested: a jump to a call in a run of twenty, above thirty tall
   messages, lands in sight.
5. **The live line widened the stream** (correctness 5). The words take
   the room left beside the indicator's spot, and the files label comes
   first. The width test has a running turn's line among its cases. It
   fails without the fix.
6. **No key folded an open run from a call with output** (correctness 6).
   The fold keys act on the innermost fold, as in vim:
   - Toggle acts on a folded run, else on the item's own fold, else on
     the run;
   - Open acts on the innermost folded;
   - Close acts on the innermost open, so `z c` twice folds the output,
     then the run.

   `nav::fold_here` takes the three targets (`FoldsHere`). Other views
   pass their one fold (`FoldsHere::one`).
7. **Repaints while the live line wasn't seen** (correctness 9, 10;
   performance 1, 4).
   - The indicator is drawn, and frames asked for, only while its spot
     is in sight.
   - In a long turn, while its own line is scrolled past and the turn is
     still in sight, the line is pinned at the stream's top.
   - The words ask for a frame only when they change: receiving ends, the
     next second, the next minute. Disconnected asks for nothing.
   - The heartbeat moves a pixel at a time (about 5 frames a second), not
     10. Its breathing dot is drawn inside its spot, where it shows.
   - Tested: pinned and moving in a long turn; nothing asked with the
     turn out of sight; quiet with no indicator, the next frame comes in
     tens of seconds. Each test fails without its fix.
8. **The indicator's stage was wrong** (correctness 11; responsibility
   2). The stage is built after the stream is laid out, from:
   - the panel's rect;
   - the items on screen;
   - a see-through layer over the panel, clipped to it.

   It's drawn once per frame, for the turn running only.
9. **Quiet didn't say whether the agent can be reached** (correctness
   12). "No word for 2 min · the server still reaches it" uses the
   slot's and harness's state: starting, needs sign-in, can't run (why),
   failed (why).
10. **Each frame copied the whole thread and laid out the whole chat**
    (performance 2, 3, 5, 6, 7).
    - The stream's order is kept in the store, so rows, items and turns
      are found by index, not by scanning every item per turn.
    - `render_stream` reads the thread in place (no copy).
    - The rows, and whether anything is folded or open, are worked out
      once per change of the thread or the folds, and kept.
    - A folded run's words are made only when it's drawn.
    - A row more than a window out of sight is not drawn when it is drawn
      as last time (same items, unchanged, same width and text size). It
      takes its measured height instead.
11. **Enter was decided twice** (responsibility 1). `moves()` and
    `enter()` read the same fold. A folded run offers "Unfold the 3
    calls". Its target has no file of its own.
12. **The settings were offered two ways, with code defaults**
    (responsibility 7).
    - "Shows:" and "At work:" are both on the chat's settings row.
    - Their defaults, and when the agent is said to be receiving and
      quiet too long, are in the UI config's `[chat]`.
    - An unknown indicator name there fails the config load.
13. **The indicators' list was written twice** (responsibility 8). There
    is one table (`indicator::ALL`), and one stored pick
    (`indicator::Shown`).
14. **Per-kind tables and checks repeated** (responsibility 10).
    - One table (`chat/words.rs`) has each kind's icon, what it did and
      what it's doing.
    - "Waits on you" is one function.
    - The fold caret is `nav::fold_mark`, for every view.
15. **Smaller:**
    - the stream's order was defined six times (responsibility 11);
    - the socket was read in the chat's draw: the agents note it per
      frame (responsibility 12);
    - `rows` was `pub(crate)` (responsibility 13);
    - picking the verbosity it already has wiped the hand folds
      (correctness 15);
    - "1 calls, folded", and reasoning counted as a call (correctness
      15);
    - the tests didn't protect what they claimed (correctness 14).

### Left (recorded, not changed)

- **Quiet decides how items group, not only their defaults**
  (responsibility 9). That's what quiet is: everything between two
  messages is one line, so reasoning joins and one call is a run. A run's
  identity therefore differs between quiet and normal. Switching starts
  the hand folds afresh, as decided in step 3.
- **Updates that land in one frame share its time.** The server sends no
  times. The record is from when this cockpit got them. Updates in the
  same tenth of a second are counted together.
- **A page load counts what it loads as news.** Opening the cockpit
  mid-turn, the line says receiving, then counts from the load.
- **The live line hasn't been checked live in a real turn.** The chat
  showing has no session, and starting one means sending a prompt on your
  Antigravity account (or typing into the lead's chat, which holds your
  draft). The app tests cover it. The settings row was checked live:
  "Shows: normal" and "At work: heartbeat" are in place.

### Decide (when you're back)

- **The live line pins at the stream's top** while its turn's line is
  scrolled past and the turn is in sight. Without this it's hidden for
  most of a long turn. This is new: the plan put the line only on the
  turn's line. To undo, drop `pin_live`.
- **`:N` goes to message N**, the same number places and search use.
  Before, it counted rows, which change with folds. Step 2 had said "the
  cursor, the moves, the stops and the places all go by rows".
- **New `[chat]` settings in the UI config:** `shows`, `indicator`,
  `receiving_s` and `quiet_after_s`.
- **Step 4's live check is yours to run**:
  - Start a Claude Code session and ask it to use a sub-agent (the Task
    tool).
  - The sub-agent's calls should fold under its Task call.
  - The pack's setting (`parent_call` in
    `packs/harness/claude-code/pack.toml`) is compiled into the server, so
    no reinstall is needed.

## Found in step 4's live check

- **The settings row pushed the chat panel past the window** while a
  harness was starting. "Antigravity (Starting)" made the row longer, and
  the "Shows" picker was cut to "…" and drawn past the panel's edge.
  - A setting's check that its caption and picker fit read the row's
    whole width, not the room left on the current line.
  - It reads the room left now, and the caption no longer wraps on its
    own.
  - Tested: two harnesses, one starting, at screen widths from 520 to
    1400; no panel reaches past the next or the screen. The test fails
    without the fix.
  - Checked live in your tab while Antigravity was starting.
  - The code is the menus-and-lists plan's (the controls). The second
    setting on the chat's row is what exposed it.

## Step 5: Decide (when you're back)

- **A message's `file:line` references are links under the message**, not
  links inside its text. The markdown renderer draws the text and doesn't
  say where each word went, so links inside the text couldn't be places
  for `f` or Enter. Under the message they work as a call's places do.
- ~~A new dependency: `similar`~~ (since the end review: gone; the server
  compares a call's texts with git, as the Changes view's files are).
- ~~`c` copies the message you're on, or the code block `f` picked (the
  chat's own key)~~ (since the end review: `c` is one copy action in every
  view; see the end review's Decide).
- **Code blocks are coloured only for languages a pack knows** (rust,
  python, typescript now). `sh`, `json` and the rest are plain until
  language packs exist for them.

## Step 6: Decide (when you're back)

- ~~Chips in the history are all one colour~~ (since the end review: each
  link's colour comes from the shell, as the typing box's does).
- **What you typed is found by the exact text the agent was handed.** If
  a harness replays your message with different whitespace, the record
  isn't found and the message shows the words before what went with it,
  without chips. Matching by turn number instead would break when turns
  are numbered anew.
- ~~The typed records are never pruned~~ (since the end review: the newest
  10,000 are kept).

## End review (d54286c..50a125f): all 8 reviewers

The reports, word for word: `chat-reviews/end-*.md` (responsibility,
performance, correctness, faithfulness, permissions, tests, consistency,
anti-patterns). Fixed in 7c94928, a60a598, c82b2b0, 08c5a2e, 6212c41,
cceed69, de79c94, ad7668c, ec9d830, bb963b3 (and the one below).

### Fixed

- **Whose an item is** (correctness 1, 8; responsibility 5; permissions 1):
  - an item's parent is the update's own, not left from the last one;
  - each sender streams apart: two sub-agents' chunks no longer split
    each other's messages.
- **Where an item is** (correctness 2, 3; permissions 1):
  - a message the agent took mid-turn shows in that turn;
  - the cursor stays on its item as others are put in before it (every
    move of it records the item);
  - what waits is in the order it was asked.
- **What's news** (correctness 5, 13; responsibility 12; anti-patterns 8, 9;
  performance 12):
  - the server says when its replay is done (`Replayed`), so opening the
    cockpit on a stuck agent reads as quiet, not as a burst;
  - a reconnect's new view clears heights and changes worked out before
    it and keeps the folds you set;
  - the AGENT light reads the same news as the live line.
- **What waits on you, one rule** (responsibility 6; faithfulness 2, 7): the
  inbox answers only what still waits, as the chat does; two asks at once
  are tested with the mock harness.
- **What you typed** (responsibility 1; anti-patterns 1, 2; performance 10,
  13; tests 6):
  - it goes on the live item when the turn begins;
  - it's kept when the message has links;
  - what the links handed on is kept apart (`AsTyped { message,
    handed_on }`), so nothing parses the prompt's wording;
  - the lookup for a replayed message runs off the async threads;
  - the records are pruned.
- **One diff** (responsibility 2; consistency 1, 7; faithfulness 1;
  anti-patterns 3; correctness 7):
  - a call's change is compared on the server with git, as the Changes
    view's files are (`POST /api/changes/texts`); `similar` is gone;
  - in the chat the hunks are unnumbered (the texts may be a part of the
    file);
  - a change with no old text says it was written whole;
  - "+N −N" are counted and coloured as the Changes view does it (shared
    `diff::counts`, `diff::draw_counts`).
- **One way of asking the server to work things out** (responsibility 3;
  consistency 4; performance 1, 3; permissions 2; correctness 11): code
  blocks' colours and calls' changes both go through `asked::Asked`:
  - each is asked once, through `Ask`, in bounded calls;
  - the longest unused go first;
  - a failure is asked again after 30 s;
  - only closed code blocks are asked.
- **One markdown parse, one path parser** (consistency 2, 3; anti-patterns
  4, 17; responsibility 4; correctness 12):
  - pulldown-cmark finds code blocks and tables, with stable box ids;
  - one reading of `path:line` serves the terminal, the chat and the `@`
    picker (`places::path_and_lines`).
- **Settings** (correctness 9; responsibility 8, 11; faithfulness 3;
  consistency 11, 12, 16; anti-patterns 10; tests 5, 19):
  - each chat keeps what's picked for it through a reload;
  - the config is the default for a chat not picked for, and a change of
    another setting no longer resets it;
  - they're bound `m v` and `m a`;
  - the choices are named one way (`config::Named`), with words for the
    screen, and an unknown name is refused in the config's form.
- **One copy action** (responsibility 7; consistency 8, 10; faithfulness 8,
  11; anti-patterns 6): see Decide.
- **The live line** (correctness 4, 6; faithfulness 6; anti-patterns 11,
  13):
  - its words keep moving in a narrow panel;
  - waiting on you reads "Waiting on you";
  - under a minute it counts seconds;
  - the heartbeat blips where updates came, as tall as how many;
  - its sizes come from the look.
- **Search** (correctness 10): a hit on a folded run's first call opens the
  run; a folded run's row is a place of its own (`3:run`), so a hint keeps
  it folded.
- **What a run is** (faithfulness 5):
  - reasoning between calls is in their run;
  - quiet folds everything between two of the agent's messages, except a
    request.
- **Words** (consistency 13, 15, 18; anti-patterns 5, 14, 15; responsibility
  10):
  - one table of doing-words, with `None` for "no object";
  - one wording of a harness's state;
  - one capitalising;
  - message moves are offered only when there's a message to go to;
  - a pick's keys are made in one place;
  - the socket's state is the shell's, not the agents service's.
- **What's drawn each frame** (performance 2, 4, 5, 6, 7, 8, 9, 14):
  - only rows in sight or near it are drawn, plus the last window's while
    following; one out of sight takes its last height, or a line's;
  - a request's row is no exception;
  - a change's lines out of sight are skipped;
  - the key bar's search line reads what the search found;
  - what waits first is found without copies;
  - messages are numbered in one pass.
- **Tests** (tests 1-21):
  - the width test checks every text, whatever its clip, with every fold
    open and more kinds of item (a table now has its own box, as a code
    block does);
  - the keys go to the request you're on;
  - the live line's timing is checked by what moves, not by geometry;
  - "none" draws nothing;
  - the reach wording is checked;
  - the config's choices, and settings kept through a reload, are
    checked;
  - colours and changes come through the server's answers;
  - a chip's Enter lands on its tag, and a change's `g x` on its file;
  - search goes in order;
  - a long chat draws only what's near;
  - wording-only checks, checks of internals and duplicates are gone.
- Also fixed: the pack's sub-agent pointer is tied to a recorded Claude
  Code update; `for_fence` matches dialects and case; the doc comment on
  `highlight` and the blank line in convert.rs.
- Found while testing: the cursor followed its item only if a frame ran
  in between, and a long chat opened at the wrong place for a few frames
  without asking for them. Both are fixed.
- Found in the live check (b1d8998): the SRV light was red while events
  were arriving. Since "a desk per server", events come over home's
  socket, but the light read the desk's own client, which never opens
  one. The live line read the same, so during a real turn it would have
  said "The connection to the server is down". Both read home's socket
  now; checked live: SRV is green.

### Left (recorded, not changed)

- **An update's time** (correctness 5, sequence B): an event is stamped
  with the frame that applies it. In a tab hidden for 10 minutes, what
  came meanwhile counts as just now. Fixing it needs the server to stamp
  each event.
- **A replayed message is found by its sent text** (responsibility 1): the
  harness replays only text. A message whose replay differs in whitespace
  shows as sent.
- **Per drawn message, each frame** (performance 5, 6): the markdown is
  parsed again (egui_commonmark parses as it draws), and an opened
  output's text is copied into its label. Culling bounds this to what's
  near sight.
- **The rows are still walked each frame** (consistency 5; performance 7),
  each taking its room, but only those near sight are laid out. egui's
  `show_viewport` would need each row's top known ahead, which is the
  same height cache.
- **Gathering the chat's actions copies what waits each frame**
  (performance 9).
- **The key bar cuts a label at 40 characters** (consistency 14): egui's
  truncate cuts by the room left in the row, which in a wrapped bar would
  cut the last chips, not the long ones.
- **A third SHA-256 helper** (consistency 17): nah-state would need a
  dependency on code-analyzer for the shared one.
- **A one-field request body** (lint): `TextsAsk { pairs }`, as
  `HighlightAsk { blocks }` is: the JSON names the field.

### Decide (when you're back)

Accepted by the user (2026-10-25), as built: the copy key, the pinned
activity line, quiet, rows out of sight not drawn, tables in a box. Old
messages are redone by `chat-cleanup-plan.md` Part 1.

- **One copy action, built ahead of the selection TODO.** The plan said
  "the copy mechanism decided in the selection TODO (one)", and that's not
  decided. What I built:
  - one host action (`nav.copy`, `c` in every view);
  - it asks the view what's copyable where you are
    (`Navigable::copy_here`);
  - the note says "Copied {what} (N lines)";
  - the chat's, terminal's and code view's own copies are gone;
  - a discussion's "make it a change request" moved from `c` to `e`;
  - mouse selection and paste are untouched.
- **The live line pinned at the stream's top** (faithfulness 10): when the
  running turn's line scrolls away, a copy of it stays at the top. The
  plan didn't ask for it. It's one function (`pin_live`) to drop.
- **Rows out of sight aren't drawn** (faithfulness 10): no plan step asked
  for it, but without it a long chat lays out every row every frame.
- **A message sent before the typed records** shows as it was sent, with
  what went with it unfolded (nothing reads the prompt's wording any more).
- **Tables in a box of their own** (as code blocks are), so a wide table
  scrolls in its box.
- **Quiet now folds everything between two of the agent's messages**
  (to-do lists, notices, a sub-agent's work), except a request.

## Re-read after the end review's fixes: responsibility and correctness

The reports, word for word: `chat-reviews/reread-responsibility.md`,
`chat-reviews/reread-correctness.md`. Fixed in 065e7a2, 9ccb6de, 4460d54.

### Fixed

- **Turns in their order** (correctness 2): turns are shown by their
  number, so the turn the agent makes for a message it took comes before
  one queued after it.
- **When a thread was last heard of** (correctness 1, 9):
  - opened on a silent agent, the live line goes quiet in time;
  - answering a request that waited long doesn't read as quiet;
  - it shows "Waiting on you" till your answer is taken.
- **A restored place** (correctness 4): the cursor goes on what's there
  once the thread is all here, and it's scrolled into sight.
- **Whose rule the shape is** (responsibility 1; correctness 6, 12):
  - what the stream's shape reads is the chat's own (`rows::folds`,
    `rows::shape_facts`), out of the contract and the store;
  - the tag places are kept by which view of the thread they're of;
  - a tagged turn's area takes in its folded runs.
- **Quiet** (correctness 7, 8):
  - an error and a plan proposed stand out of runs;
  - a run says what it holds;
  - opened, a call's sub-agent's work stays folded under it.
- **Links in prose** (responsibility 2; correctness 3):
  - a file is a name with an extension and a folder or a line;
  - emphasis around it isn't part of it;
  - no absolute path while the project's folder isn't known.
- **What you typed, looked up in memory** (responsibility 3; correctness
  14): every message's lookup is a hash and a map's, not a database read
  awaited in the event path; pruning is by row number.
- **The indicator's window** (responsibility 4): it's the interface's
  (`RECENT_S`); the store keeps the last minute of updates on its own
  terms.
- **Bounds** (responsibility 6; correctness 5, 13):
  - each limit has one source on both sides;
  - what's too big is said, never re-sent;
  - the route takes a body big enough;
  - the answers kept are dropped least recently used.
- **Copying** (responsibility 7; correctness 15): what's copyable is said
  cheaply; the text is made only when copied.
- **Choices named one way** (responsibility 8): `by_name` serves every
  choice in the config and the saved layout; the indicator's choice is
  the config's.
- **Per chat** (correctness 10): what's picked is kept per slot.
- **The search count** (responsibility 9; correctness 11): it's found
  again when the view's items change.

### Left (recorded, not changed)

- **Absolute paths in an agent's words aren't links**: the chat doesn't
  know the project's folder. They were wrong links before.
- **Opened on a silent agent, the count is since it became known here**
  (you opened it), not since the agent last spoke. The server doesn't
  stamp its events.
- **The session layer still reaches the slot manager** for what was typed
  (a late-bound closure). The harness replays only text; what was typed
  is the slots'.
- **What counts as the agent's news is the store's** (responsibility 5):
  your queued prompt and your answer's echo are yours, not its. That's a
  fact of the event, which the store reads.
- **Views other than the chat** don't say when their items change, so the
  key bar finds their search again each frame, as before.

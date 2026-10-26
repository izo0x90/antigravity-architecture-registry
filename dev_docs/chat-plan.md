# The chat: never broken, less noise, richer, easier to move through

## Why

Talking to the agents is the main thing the cockpit does, and the chat:
- breaks its layout: after a long unbreakable line (a file path) every later
  item is drawn shifted left, off the panel, and a cut-off approval made
  the agent look stuck (seen 2026-10-24);
- shows every tool call and its output as it happens: long, noisy, hard to
  find the answer in;
- leaves out what it could show well (code, changes, its own messages as
  typed) and is slow to move through when long.

## Decided (with the user, 2026-10-24): all ten

1. **The layout never breaks.** Every item stays inside the panel's width;
   long unbreakable text wraps, or scrolls sideways in its own box. A test
   draws long lines in a narrow panel and fails if anything is outside it.
2. **What needs you is never hidden.** A pending approval or question stays
   in sight above the typing box, not only up in the history.
3. **Tool calls fold, as reasoning does.** A run of calls between two
   messages is one line ("Read 3 files, searched twice, ran 1 command");
   each call is one line (what it did, on what) with its output folded; a
   failed call stays open. Each folds and unfolds on its own, with the
   fold keys every view has (`z o`, `z c`, `z a`, `z R`).
4. **A verbosity setting per chat**: quiet (messages and what needs you),
   normal (the folded lines), everything (every call and its output).
5. **One live line while the agent works** ("Reading render.rs… 12 s"),
   not the history filling as it goes, where "running" is now (on the
   turn). It shows which of these it is, honestly:
   - *receiving*: something arrived in the last moment. A small activity
     trace (a heartbeat line over the last ~10 s) blips once for each
     update that comes in, so its rhythm is the real one;
   - *waiting*: nothing new, the connection fine. The trace lies flat, a
     slow breathing dot beside it, and the time since the last word ("12 s
     since anything");
   - *quiet too long*: no word for a long while (a minute, say) or the
     connection down: the trace dims, amber, "no word for 2 min", and
     whether the server can still reach the agent.
   The words say what it's doing, from its latest item ("Thinking",
   "Writing", "Running a command", "Asking you"). Drawn only while it's
   shown; it repaints at a gentle rate, not every frame.
6. **Code blocks** highlighted as the code view highlights, with Copy;
   `file:line` references open in the code view.
7. **File changes** as a folded "+12 −3"; open for the diff, or go to the
   Changes view.
8. **Your messages as typed**, links as chips, what was attached folded
   under them (from TODO: "Chat: polish the agent chat").
9. **Move by message**: the next or previous message with a key (check
   what `] ]`/`[ [` do in the chat now), copy one with a key; `f` (jump to
   anything shown) already covers the chat and stays the one mechanism.
10. **Search inside the chat**: the views' `/` search.

And: **a harness's own sub-agents.** In ACP they live inside the one
session: their tool calls and requests come over it like the main agent's.
- Several requests at once (sub-agents in parallel) are each seen and
  answerable, in the chat and the inbox, not only the first.
- Where the harness says which call a sub-agent's calls belong to, they
  fold under that call (its Task call); where it doesn't, they're shown in
  order as now. Check what Claude Code and Gemini (Argon) send first.
- NAH's own sub-slots stay separate sessions with their own chats.

## How it's built: the pieces there are, not new ones

- Folding: the views' fold (the `Move::Fold` actions, their keys, `fold_here`),
  not a chat-only toggle; the chat's items are folds like a tree's rows.
- Moving and jumping: the chat's spots already serve `f` and the motions;
  "next message" is a motion over them, not a key of its own.
- Verbosity: a chat setting like its others (the config actions,
  `m #`), deciding which items are folded by default. No second renderer.
- Code blocks: the code view's highlighter (the language packs), the copy
  mechanism decided in the selection TODO (one).
- Changes: the Changes view's diff, shown folded; no second diff.
- Search: the views' `/` search and its `n`/`N`.
- Pending requests: the request card and the inbox's list of what waits,
  not a new kind of card; the chat shows the waiting ones where they're
  seen.
- The running indicator is a module behind one small interface, so others
  can be added (the user's idea: pixel lemmings walking along the messages):
  - the chat hands it, each frame, plain data: the activity (receiving,
    waiting, quiet, disconnected; the times of recent updates; time since
    the last; the word for what it's doing) and the stage (the panel's area
    and the rectangles of the items on screen). Shapes, never the chat's
    internals;
  - it draws where it's given: its spot on the turn's line, and a
    see-through layer over the panel (clipped to it); it takes no clicks
    or keys;
  - it says when it next wants to move ("in 100 ms", "not until something
    changes"); the chat schedules that, capped (about 30 a second), and
    not at all while the panel or the tab is hidden;
  - a chat setting picks one by name. Built now: the heartbeat (the
    default, the first written against the interface). Others later.
- The live line: from what's already known (each update's arrival, the
  event socket's state, the turn's latest item); no new message from the
  server for "waiting" (the socket being up says the connection is).
- Width: one rule in the chat's item drawing (every item laid out in the
  column's width), tested once for all items.

## Steps

1. The layout fixed (find why items drift: the wide line's column) and its
   test; pending requests kept in sight; several at once answerable (a
   mock test with two at once).
2. Tool calls folded (runs and each call), fold keys; the live line.
3. The verbosity setting.
4. Sub-agents: what each harness sends, checked live; nested where marked.
5. Code blocks, `file:line`, changes folded.
6. Your messages as typed (needs the turns' own record of their links).
7. Move by message, copy one, `/` search.
8. Each step: tests, lints, live in the user's tab (a narrow panel, a
   long agent turn); reviewers at the end; findings in
   `chat-findings.md`.

## Status

- Step 1: built.
  - **The drift was a markdown table.** A long cell widened its message,
    and with it the stream's whole column. The panel was also clipped to
    the screen's width, not to itself, so what overflowed drew over the
    panels beside it.
    - Each chat item is laid out in a box of the column's width: its
      text wraps there, and what can't wrap (a table's row) scrolls
      sideways inside it.
    - Every panel clips what it draws to itself.
    - Tested item by item: a long unbreakable line in every kind of item,
      in a narrow panel, keeps the panel's width and shows nothing outside
      it.
  - **Requests kept in sight.** A waiting request the stream doesn't show
    (scrolled out) is drawn above the typing box, as the same card. It's
    a place of the chat's, so the cursor, `f` and its answers' buttons
    reach it there. The stream takes the height left above what's under
    it (measured, not the old 105-point guess).
  - **Several at once.** Every waiting request's answers are offered. The
    keys (`y`, `d`…) go to the one you're on, or the first while you
    follow the latest. Tested with two at once.
  - The live check of an agent's long turn isn't done: it means typing
    into this chat, which holds your draft. The app tests cover it.
  - Its checkpoint review's fixes: f2ec072 (in `chat-findings.md`).
- Step 2: built.
  - **Rows.** The chat's stream is rows: each item, except a run of calls
    (two or more tool calls, commands or file changes in a row in one
    turn), which is one row while folded: "Read 3 files, searched twice,
    ran 1 command (1 failed)". The cursor, the moves, the stops and the
    places all go by rows. A folded run is its first call's place, and
    moving onto a call inside one opens it.
  - **Folds.** A call's output and reasoning fold too. They use the fold
    keys every view has (`z o`, `z c`, `z a`, `z R`, `z M`, through
    `fold_here`, with its button under what you're on). Enter on a folded
    run opens it. Folded by default, except a failed call and a run
    holding one. What's set by hand is kept by item id, as set (open or
    folded).
  - **The live line.** It replaces RUNNING on the running turn's line:
    what the agent's doing ("Reading render.rs", "Thinking", "Asking
    you"), and how live it is ("…", "5 s since anything", "No word for 1
    min", "The connection to the server is down"). It's worked out from
    the thread's update count, the socket's state and the turn's latest
    item.
  - **The indicator** (`crate::indicator`). One interface: plain data in
    (the activity, the stage as shapes), drawing, and when it next wants
    to move (at most about 30 a second, only while drawn). The heartbeat
    is the first written against it: a ten-second trace with a blip per
    update, a breathing dot while waiting, dimmed amber when quiet or cut
    off. The chat's "Show the agent at work as…" picks one by name
    (heartbeat, none).
- Step 3: built. "Shows:" on the chat's settings row is how much the chat
  shows. It decides the folds' defaults, with no second renderer.
  - **Quiet:** what the agent did between two messages, even one call
    (reasoning too), is one folded line, and a failed call doesn't open
    it.
  - **Normal:** calls fold into runs; a failed call and its run are open.
  - **Everything:** every run and output open.

  Switching starts what was folded by hand afresh. While here, the rows
  are found in one pass (they were quadratic over a long chat).
- Steps 2-3's checkpoint review's fixes: 2adccef (in `chat-findings.md`).
  What changed from the above:
  - the cursor, places, `:N` and search count messages; only the
    scrolling counts rows;
  - the live line is worked out from when the agent's updates came, kept
    with the thread in the store;
  - the live line is pinned at the stream's top while its turn's line is
    scrolled past (under Decide).

- Step 4: built.
  - **What each harness sends.** This was read from the adapters' code
    (installed versions), not from a live run.
    - Claude Code (claude-agent-acp 0.89.1) runs a sub-agent inside the
      one session. Its updates (messages, thinking, tool calls) carry
      `_meta.claudeCode.parentToolUseId`: the id of the Task call that
      started it.
    - Antigravity runs a sub-agent as a conversation of its own
      (`invoke_subagent`), so its session has nothing to nest.
  - **The adapter.** A harness pack says where an update's `_meta` names
    the call whose sub-agent sent it (`parent_call`, a JSON pointer, as
    packs say where sign-in status is). Claude Code's pack declares it.
    Each item the sub-agent makes gets its call's item as `parent_id`.
    - A sub-agent's streamed text doesn't run into the agent's own.
    - Its prompt isn't a turn of yours.
    - Tested in the state, and in every pack's contract test against a
      mock that sends what its settings look for.
  - **The chat.** The store keeps a sub-agent's items right after its
    call (and their own after theirs), whatever came between. The call's
    row folds them ("2 steps by its sub-agent"), folded by default as
    other folds are.
    - Enter or the fold keys open it. Its items are a step in, its calls
      in runs of their own.
    - A call whose sub-agent did anything doesn't join a run.
    - A jump to one of its items opens what hides it.
  - The live check needs a real Claude Code run with a sub-agent, on your
    account: recorded for you (in `chat-findings.md`, Decide).

- Step 5: built in three parts (f036d44, c868b4e, b0d93b4).
  - **Code blocks.** The server colours a code block with the language
    pack its fence names (`POST /api/code/highlight`: the language's name
    or an extension), as a file of that language is coloured. Fences no
    pack knows are plain (now: rust, python, typescript).
    - The cockpit asks once per block and keeps what comes back (the
      highlighter, held by the desk). The code view and code blocks are
      coloured by one function.
    - Every view draws messages through one drawer (the chat,
      discussions, the spec). A code block scrolls sideways in the
      column's width.
    - In the chat a code block is a place of its own (`f`). `c`, or Copy
      under the message, copies the one picked, else the first, and the
      status line says so.
  - **`file:line`.** The files an agent's words name (a path with a folder
    in it, or a line after it) are links under its message, drawn and
    opened as a call's places are: Enter opens the one picked in the code
    view, at its line.
  - **Changes.** A call's diff output and a change item are compared into
    the Changes view's hunks, and folded as "+12 −3 src/a.rs". Opened,
    they're drawn as the Changes view draws them. `g x` on a message
    shows its turn's changes on that file.
  - Live: the server's new route answered from your tab (a rust block,
    coloured). The chat had no session, so blocks in a real message
    weren't seen; the app tests cover them.

- Step 6: built in two parts (38079a6, 447ed82).
  - **The turns' own record.** When a message of yours is sent with what
    it links, the server keeps what you typed (its text and links) by the
    text the agent was handed (its hash, in the state database). The
    sessions forward each of your messages with what you typed for it,
    live and replayed alike. The lookup is only done for a message that
    was handed things.
  - **In the chat** your message shows as typed. What it links are chips,
    each a place of its own; Enter goes to what one links (a tag: the
    tags' list, unfolded on it). What was handed on with it is folded
    under it ("what went with it"). A message whose typing wasn't kept
    shows the words before what went with it.
  - Live: not seen in a real session (none open). The app tests cover it,
    and so does the server's test of a linked prompt.

- Step 7: built (50a125f).
  - `] ]` and `[ [` (the views' next and previous at this level) go to
    the next and previous message, past the calls between.
  - `c` copies what you're on: the code block `f` picked, else the
    message (yours as typed), else a proposed plan.
  - `/` already searched the chat. It's now tested as used, finding in a
    folded run (opening it), with `n` and `N`.

- End review (all 8 reviewers): done. Fixed in 7c94928..c2b5abc; what's
  left and what's yours to decide are under "End review" in
  `chat-findings.md`. Since then:
  - `c` is one copy action in every view;
  - the settings are kept per chat (`m v`, `m a`);
  - a call's change is compared on the server with git;
  - what was handed on with your message is kept apart from what you
    typed;
  - quiet folds everything between two of the agent's messages.
  - Live: the settings row checked in your tab. A real turn (sub-agents,
    the live line) is yours to run.

## Order among the plans

After `sign-in-flow-plan.md` and `menus-and-lists-plan.md` (the copy
mechanism and the one list come first and are reused here).

## Reviews (agreed 2026-10-24)

Reviewers: `process/reviewers.toml` (seven) and anti-patterns (its brief in
`TODO.md`). Checkpoint reviews: three reviewers on that step's changes only.
End reviews: all eight on the whole plan. After fixes, two reviewers re-read
the fixes once; what they still find goes in the findings file, not another
round. Reports word for word in `<plan>-reviews/`.

- Checkpoint after step 1 (the layout rule, requests in sight):
  responsibility, correctness, performance.
- Checkpoint after steps 2-3 (the folding model, the verbosity setting) and
  the indicator's interface: responsibility, correctness, performance.
- End: all eight.

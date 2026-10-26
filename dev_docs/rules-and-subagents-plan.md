# Rules that hold, and sub-agents brought under them

## Status (2026-10-26)

- **Written, not started.** Waits for the user's go and the answers under
  "Questions for you".
- Comes from the talk after the chat cleanup (2026-10-26) and from
  `look-at-list.md`.
- **Read first:** `process/decision-heuristics.md`, then this plan.

## Why

The chat cleanup left a sub-agent view that breaks how the cockpit works.
Its header is drawn with click-only widgets that keys can't reach. It's
the chat switched into a mode by a flag. And sub-agents can be reached
two ways (the strip's chips and their lines in the chat).

The reviewers did flag the mode flag. The builder chose to leave it. No
reviewer checks how things behave for the user, and nothing in the code
stops a one-off like this.

This plan makes the system's rules hold by construction, then rebuilds
the sub-agent work on them. It also takes the user's three notes from the
look-at list.

## The rule for this plan

A rule is enforced by a type or a lint wherever one can enforce it.
Tests are kept for behaviour that neither can check, and a shared check
replaces per-feature tests rather than adding to them. A test or lint
that replaces another deletes the old one in the same commit.

## Part 1: the rules, in one place, enforced

1. **The rules live in `process/decision-heuristics.md`.** It gets no new
   list beside it. Add the missing rules, one line each:
   - views are made of the kit's pieces and never handle input themselves;
   - one server clock for every server time shown.

   Each rule says what enforces it: a type, a lint, a test, a reviewer, or
   "not enforced yet".
2. **Reviewers:**
   - The consistency brief gets: "every new interactive piece follows the
     input model; check the rules in decision-heuristics.md".
   - A finding that breaks a rule can't be "left" by the builder. It's
     fixed, or it goes to the user.
   - Written into `process/build-and-validate.md` and `process/reviewers.toml`.
3. **Lints:**
   - Clippy's `disallowed-methods` and `disallowed-types` (in
     `clippy.toml`) ban, in view code:
     - egui's interactive widgets (fold-outs, buttons, clickable labels);
     - hand-written colours and sizes;
     - the browser clock next to server times.
   - Each is allowed only in its one module: the kit, the theme, the
     clock.
   - ast-grep covers what clippy can't say.
4. **The gaps the end review found**, each given its enforcement first,
   then fixed where it flags:
   - overlay kinds with their own closing code;
   - hand-written sizes;
   - two views that don't say when their items change;
   - three template parsers.

## Part 1b: what's offered and shown, one system

Today every action is published by its view through one list. It says
where it belongs (what you're on, the panel, its host, everywhere), when
it applies, and how important it is (1 to 3). The key bar keeps what
applies, ranks it, and shows the first N. Three parts of that aren't a
system:

1. **Importance is a hand-picked number per action.** Replace it with
   written ranking rules the shell applies. For example: what acts on
   the thing you're on comes first, then moves, then the panel's own,
   then settings. An action says what kind of action it is, not a
   number.
2. **Some conditions are `if` statements in view code.** For example,
   "Clear the prompt" is only offered when text is typed. Every condition
   becomes a declared one (`When`) that the shell checks, so nothing
   outside the shell decides what's offered. A lint flags actions
   pushed conditionally.
3. **The key bar cuts by count.** It should keep what the ranking rules
   say matters here and make room by rule, not by how many happen to
   apply.
4. **One place to see why.** The action list shows, for any action,
   whether it's offered here and why, and where it ranks.

Part 5's "show Clear the prompt when text is typed" is then decided by
this system, not by hand.

## Part 2: the kit

A small set of pieces every view is built from:

- a row;
- a foldable section;
- a labelled field;
- a list entry;
- a chip.

Each piece registers itself as a place, so it gets keys, folds, copy,
search, `f` labels and taps from the shell, the same way everywhere. A
view draws through a wrapper that offers only the kit, so it can't make a
click-only widget at all.

New functionality that needs a new kind of piece adds it to the kit
first, with its behaviour defined once.

There are two list components, and the kit makes their rows one piece
(end review, faithfulness 2; corrected with the user, 2026-10-26):

- **The menu** (which-key): the Space menus, and the pop-up after a
  half-typed key like `g` or `]`, which is the same menu with narrower
  contents. Today they're built in two places in the code
  (`app/draw/which_key.rs` and the key bar's pop-up in
  `app/draw/bars.rs`), sharing only the row. They become one component.
- **The fuzzy picker** (`pick_list.rs`): the action list and every
  picker (slot, model, mode, settings). It's already one component, with
  only the contents filtered differently.
- **The gap:** the two draw their rows differently. The finger-sized
  height and tap behaviour live only in the menu's rows
  (`actions::entry`), so the picker's rows are too small for a finger.
  Both draw rows with the kit's list entry: defined once, so nothing
  differs between them but what they show and how it's filtered.

One shared check, run over every view: everything clickable is reachable
by keys, every action has a key or menu entry, and folds work. It
replaces the per-feature tap and key tests it covers, and those are
deleted.

## Part 3: one conversation view

- Pull the chat's stream out of the chat into a shared conversation view:
  rows, folds, cursor, keys, search, copy, and drawing only what's on
  screen.
- It draws a source. Each source gives its items and publishes its own
  actions:
  - **an agent's chat:** its turns (send, resume, settings);
  - **a sub-agent:** what it was told, its steps, what it answered, all
    as ordinary items (stop it; where it started);
  - **a discussion:** its messages (reply, settle, reopen). Whether this
    moves now or later is a question for you.
- The chat's `alone` flag goes, and so does everything that branches on
  it.
- Link chips in your messages are worked out once per message, the
  first time it's drawn, and kept with it, not again on every frame (end
  review, performance 5). Decided with the user (2026-10-26): a sent
  message never changes, so redoing it per frame is pure waste. The cost
  is a small list per message, far less than its text.

## Part 4: sub-agents

- **Their lines in the chat stay.** They're where each sub-agent came
  from, and they say how it is now.
  - `] a` / `[ a` move between them, and Enter opens one.
  - These keys appear in the Chat actions menu, not only once you're on
    one.
- **One Sub-agents list replaces the strip of chips.**
  - It's a list view, like the Inbox or Tags: one row per sub-agent in the
    session, working ones first, each with its state, name, what it's
    doing and how long it's run.
  - It uses the list's usual keys: `j`/`k`, search, Enter opens, `c`
    copies.
  - It publishes its actions (stop it, where it started).
  - It opens with one key. The live line's "2 sub-agents working" opens it
    too.
- **A sub-agent is the conversation view over its own source.**
  - Its name, kind, state, time and task go in its top line.
  - No typing box or settings: they aren't part of it.
- The server already sends each sub-agent's task, answer and steps. One
  change: today it resends a sub-agent's whole prompt and answer on every
  state change (running, waiting, ended, renamed), to every browser and
  through home (end review, performance 10). Decided with the user
  (2026-10-26):
  1. The server sends the long texts (task, answer) only when they
     change. Other updates carry just the state.
  2. The cockpit keeps the texts it already holds when an update leaves
     them out. That's one rule, in the one place that stores sub-agents,
     with no new structures.
  3. A missed update heals itself: a reconnect resends every sub-agent
     in full.
  4. Browser memory doesn't change: the cockpit holds the texts today
     too. Not keeping them, and fetching them only when a sub-agent is
     opened, is the next step only if real numbers show memory matters,
     since it brings a request, a loading state and its failures.

## Part 5: the user's notes (from the look-at list)

1. **The typing box hides long text.**
   - Cause: the box grows with its text and has no scrolling of its own,
     so what's past its room is cut off below.
   - Fix: give it a maximum height (a share of the panel) and let it
     scroll inside, keeping the caret in view.
   - It's the shared conversation typing box, so one fix covers the chat
     and discussions.
2. **Clearing the typing box.**
   - It exists: `x` in normal mode ("Clear the prompt"). But it's marked
     minor, so the key bar rarely shows it.
   - Fix: show it in the key bar whenever text is typed.
   - Your idea, Escape again in normal mode, is a question for you
     (below), because Escape means "go back" everywhere.
3. **Resuming a session is slow.**
   - Measured: Claude Code alone starts and loads your test session
     (`2c4167fc`, 11 turns) in about 0.8 s. The rest is ours.
   - First step: time the server's open path (process start, the cockpit
     tool server's connection, the replay) and the cockpit taking the
     replay in. Then fix what's slow.

## Questions for you

1. Escape again in normal mode to clear the typing box, or keep `x` and
   just show it? (Escape is "go back" in every view.)
2. The Sub-agents list: in a panel, or in the quick look? And which key?
3. Discussions onto the conversation view in this plan, or later?
4. Does the bottom strip go entirely (the live line's count opens the
   list), or stay as one compact count?

## Not in this plan

- **Reconsidering the tests as a whole** (look-at list). This plan only
  deletes the tests its checks replace. The rest is its own piece of
  work.

## Order

1. Part 1: the rules, reviewer changes and lints. The lints will flag
   today's one-offs, and that list is the work for Parts 2 to 4. Then
   Part 1b: what's offered and shown.
2. Part 2: the kit and the shared check.
3. Part 5's resume measurement (it may change what Part 3 does with the
   replay).
4. Part 3: the conversation view, the chat moved onto it.
5. Part 4: sub-agents.
6. Part 5's typing box fixes.
7. End review (with the new brief), the re-read, and a live run in the
   user's tab.

Built per `process/build-and-validate.md`. Findings go in
`rules-and-subagents-findings.md`.

# Plan: discussions on anything, and review lists in plans

Written 2026-10-20 from a discussion with the user. Built as
`dev_docs/process/build-and-validate.md` says. Nothing here is built.
**★** marks what the user decides before or while it's built. Anything
this plan says that turns out costly or wrong is asked about first, never
swapped for something cheaper.

## Why

Going through 66 findings as a long list, and answering them in one long
reply, loses track: which item, what was asked, what was decided. The user
wants to go through such a list in the cockpit, item by item, and talk
about each one where it is: a question, or a change asked for. The same is
wanted on code (a comment on lines that should change), on a plan's parts
(as today), on a terminal command, on a chat message.

Most of this exists, in pieces that don't join:
- **Naming any thing durably** (`Subject`), picking it at the cursor (a
  line, the command it's in; `Tab`), keeping what doesn't last (the logs),
  showing it (the gutter), going to it: built for tags.
- **A discussion's whole life** exists for plans only (`arch_graph::Comment`):
  a comment (a question or remark) or a change request (its answer links
  what changed); open, answered by an agent, accepted or reopened by the
  user; replies; approvals blocked until resolved; agent tools
  (`review_comments`, `answer_comment`, `reply_comment`). It's anchored to
  a plan's parts only (`CommentAnchor`), kept inside the plan's file, and
  drawn by the spec view's own thread and composer.
- **A tree buffer** that draws whatever its source gives it: only the code
  outline is a source.
- **A conversation** (messages, a typing box): the chat's, and a second one
  in the spec view.

## Agreed (with the user, 2026-10-20)

- **One pointer.** Tags, discussions (and marks, later) are notes on a
  thing, and they all point at it the same way (`Subject`). What they hold
  stays apart: a tag is a name on many things; a discussion is a whole
  back-and-forth on one thing. Nothing about a discussion is flattened to
  fit a tag.
- **One discussion engine**, for every kind of thing. Plans keep no
  discussions of their own; they ask the engine (is this component's
  discussion settled, before an approval).
- **A discussion is a thing too**: it can be tagged, gone to, named from
  another discussion.
- **Questions and change requests** both, as today: a question can be
  answered without anything changing; a change request's answer says what
  changed. Only the user settles or reopens one.
- **Review lists are plans**: a plan holds nested items (findings,
  decisions, to-dos), each with a state, each a thing that can be
  discussed and tagged. No separate kind of document.
- **Not another chat.** The thread is drawn by the chat's own parts (its
  message list and typing box, made one component both use); it opens in
  the quick look over what you're looking at. No new list view: the inbox
  says what's waiting on you, the plan tree shows each item's discussions,
  the gutter shows a thing has some. Agents answer through tools, not a
  chat of their own.

## Rules (each checked by a test or a lint)

1. Every note (tag, discussion) is on a `Subject`; there's no other kind of
   anchor (`CommentAnchor` goes).
2. Naming, keeping and moving things is one layer, shared; tags and
   discussions know no owner.
3. One discussion engine; no other part keeps discussions.
4. One conversation component in the cockpit: the chat and discussions
   draw their messages and typing box with it.
5. Only the user settles or reopens a discussion; a change request's answer
   names what changed.
6. Views add no keys, buttons or pickers of their own; every action says
   what it does (as before).

## Part A: things, one layer

A.1 The parts of `nah-tags` that aren't about tags move to a crate of their
    own (`nah-things`): the owners (`Resolve`) and their registry, naming
    back, where a thing is now (`moved`, `refresh`), whether it's there,
    settling it as it's noted (`settle`), the logs of what doesn't last
    (`Logs`, following, the cap), reading a thing (start, line count,
    windows, search). `nah-tags` keeps tags, on top of it. One registry,
    used by tags and discussions alike.
A.2 Cockpit: "notes on the thing at the cursor" is one: the things at the
    cursor (`at_cursor`), the gutter (bars for tags, a second mark for
    discussions), the names on the panel's top line, go-to.
A.3 Tests: tags behave exactly as before (their tests move with them, none
    rewritten to pass).
A.4 **Closes from the tags findings** (`tags-everywhere-findings.md`, by
    number):
    - #1 searching what notes are on uses FFF (its walk made to reach the
      logs' folder), not a hand-written scan.
    - #2 decided: events send the whole tag or discussion (no change).
    - #5 the cap on a growing log is set per project
      (`.nah/terminals.toml`), 64 MB by default.
    - #7 the branch-only log conversion (layout 10): dropped.
    - #4 a tagged command read back whole after a real server restart.
    - The logs, which discussions also point into: #19 logs of notes a
      build can't read aren't deleted; #21 text no note is on doesn't stay
      on disk; #22 a command ending as it's noted keeps its end; #32 a log's
      lines counted without reading it all; #34 noting a finished command
      doesn't hold everything up while its output is built; #60 headers
      can't be imitated by what's logged.
    - Owners and names: #18 a deleted definition still reads back (kept at
      a snapshot when noted); #20 a line keeps the command it's in in its
      name; #25 one name for a thing (the picker's words and the stored
      name the same, and named back); #30 snapshots no note points to are
      cleaned up; #31 no git run to find a thing's owner or name.
    - #16 a chat's turns keep their numbers after a reload (needed before
      discussions sit on chat messages).
    - #29, #50 what's left out of a read (a turn's tool output) is marked
      so nothing else can look like it, and every read of it folds the same.
    - #35 the cockpit doesn't copy whole notes every frame; #40 a view says
      where it draws things in one place, not two that must agree.
    - Tests (#51 code following a saved file through the index, a definition
      moving, a cleaned-up snapshot; #52 a chat replayed; #53 a read in
      full; #57, #58 no duplicates, no tests of wording or internals).

A.5 `.nah/` laid out as "Where things are kept" says (Decided): the cockpit's
    layout into `state/ui.json`, the terminals' start-up hooks into
    `cache/`, moved once; `.nah/.gitignore` written for it.

## Part B: a plan's parts are things

B.1 Subjects: `Plan { plan }`, `PlanComponent { plan, id }`,
    `PlanSection { plan, id, section }`, `PlanRelation { plan, kind, source,
    target }` (what `CommentAnchor` names today), and `PlanItem { plan,
    item }` (Part C).
B.2 Their owner (the server, over the plans): names them, gives what they
    say (a component's card, a section's text, an item's text), says
    whether they're there; plans last (files in `.nah/plans`), so nothing
    is logged.
B.3 Their targets: going to one opens the plan where it is (the spec card,
    the plan tree's row).

## Part C: review items in plans

C.1 A plan holds `items`: each an id, the item it's under (none: at the
    top), a title, a text, and a state: **open, decided, done, dropped**. Any kind of plan can have items (a change plan's
    to-dos); a new plan kind, **review**, is a plan of items only
    (findings, decisions, questions to settle).
C.2 Agents add, edit, nest and set items with the plan tools (`plan_edit`
    gains them); the user sets an item's state in the cockpit.
C.3 The plan tree: a tree source over a plan (the tree buffer, as the
    outline is): its items, nested, and a change plan's components and
    their sections. Each row shows its state and its discussions' state
    (waiting on you, on the agent, settled). Enter opens the item's card
    (the spec view); its discussions are rows under it.
C.4 Tests: a review plan's items round-trip through the file and the
    tools; the tree shows them nested with their states.

## Part D: the discussion engine

D.1 A crate (`nah-discussions`): a discussion is an id, the thing it's on
    (a `Subject`), its kind (question, change request), who started it,
    its messages (each: who, when, the text; an answer also the things it
    changed, as subjects), its status (open, answered, settled; reopened is
    open again with why), and whose move it is (derived: yours when it's
    answered, the agent's when it's open).
D.2 Kept as "Where things are kept" says (Decided): the database holds
    each discussion's structure and where its messages' text is; the text
    is in files under `.nah/state/discussions/`. Told on the event stream
    (`DiscussionUpdated`, `DiscussionRemoved`: the whole discussion), each
    change with who made it in the activity record.
D.3 A thing that doesn't last (terminal output, a chat turn) is kept in the
    logs when a discussion starts on it (Part A), as tags keep it.
D.4 The rules of today's plan review carry over: only the user settles or
    reopens; a change request is settled only with an answer naming what
    changed; a plan's component can't be approved while a discussion on it,
    its sections or its relations isn't settled (plans ask the engine).
D.5 A discussion is a thing (`Subject::Discussion { id }`, owned by the
    engine): it can be tagged and gone to.
D.6 Tests: the lifecycle (start, reply, answer, settle, reopen), who may do
    what, approvals blocked, a discussion on lines that move, one on output
    that's gone from the terminal, a restart.

## Part E: plans' discussions move over

E.1 A plan's `comments` move into the engine once, as each plan is read:
    each anchor becomes its subject, each comment a discussion with its
    replies and answer; the plan's file is written without them. The
    field and `CommentAnchor` go from the code; nothing reads the old
    shape after.
E.2 What the user last reviewed (`last_review`, the review diff) stays the
    plan's.
E.3 Tests: a plan file with comments of every anchor and status, read once:
    every discussion there, the file without them, approvals blocked the
    same.

## Part F: agents

F.1 Tools (replacing `review_comments`, `answer_comment`, `reply_comment`):
    `discussions` (what's waiting on the agent first; by plan, by thing),
    `discussion_read` (one, whole, with what it's on as it is now and as it
    was), `discuss` (start one on a thing by its name: a question or a
    change request), `discussion_reply`, `discussion_answer` (with what
    changed, by name).
F.2 Who may: reading with `view.read`; starting, replying and answering
    with a new capability, `discuss` (the lead's, sub-slots' and workers',
    as `tags.edit`); a chat's things only as `slot_read` allows (its own
    slot's; a sub-slot's or a closed slot's with `slots.observe`).
    Settling and reopening: the user only.
F.3 Who's asked: a discussion goes to the lead unless it names an agent
    slot. The slot asked is told when it's started, replied to or
    reopened (a message to it, as a sub-slot's report is).
F.4 The agent packs' instructions and skills that name the old tools are
    rewritten for the new ones.
F.5 **Closes from the tags findings:** #26 a chat's text put together one
    way for every reader; #44 one date format for what's written in the
    logs and in messages. Not widened by the new tools (they stay in
    `TODO.md`'s access notes): #63 an agent driving the cockpit acts as the
    user; #64 agents' own tools read `.nah/state`.

## Part G: the cockpit

G.1 **One conversation component**, taken out of the chat: a list of
    messages (who, when, text, links) and the typing box (`#` for tags,
    Enter sends). The chat draws its turns with it; the spec view's own
    thread and composer go, replaced by it.
G.2 **The discussion buffer** (a buffer kind, opened in the quick look by
    default, or in a panel): at the top, the thing it's on as it is now
    (code with its lines highlighted; a finding's text; the output kept),
    saying when it changed since; then the messages; then the typing box.
    Its actions (the host draws them, keys from the keymap): reply, settle,
    reopen (with why), make it a change request.
G.3 **Starting one** is a shell action on the thing at the cursor, in any
    view, with the same picker as tagging: its top line says what, `Tab`
    picks the line or the command, then question or change request, then
    the first message.
G.4 **Showing them:** the gutter marks a thing with discussions (a mark
    apart from the tag bars: a small speech-bubble icon), whose move it is in the
    mark; the panel's top line says "2 discussions, 1 waiting on you";
    hover or a long press says them; going to one opens it.
G.5 **The inbox** lists discussions waiting on you (answered, a change
    request done, a question asked of you): Enter opens the discussion.
G.6 **The plan tree** (C.3) and the spec card show a thing's discussions;
    opening one opens G.2.
G.7 Tests: start one from a code line, a terminal command, a chat message
    and a plan item; reply, settle, reopen; it's in the inbox when it's
    your move; the spec view's review works through it; nothing drawn over
    content; phone width and touch.
G.8 **Closes from the tags findings:**
    - Starting a discussion uses the tagging picker (G.3): #13 several code
      lines selected and noted (the lines a comment is about, exactly);
      #11 a whole chat session, #12 a whole code file as levels; #28 no
      letter lost after `Tab`; #42 the picker for an edit opened as other
      two-step pickers are; #46 the `Tab` hint with the other key hints;
      #6 the keys for the tag edits (decided: `Space k …`).
    - The gutter gains discussion marks (G.4): #23 two notes of one tag in
      one place told apart, columns kept as things scroll; #24 the code
      view's notes in its own gutter, not mixed up with its change bars;
      #36 the gutter not walking every note every frame; #47 its sizes from
      the theme; #38 the solid icon's font found once; #14 the graph's
      cards show their notes.
    - The conversation component (G.1): #10 `#` in a typing box opens the
      tag picker (decided); #27 a note written in the
      same many-line box; #43 the typing box, not the chat, says where a
      `#` starts a name; #45 one way to say who wrote something; #56 `#`
      after Backspace, decided and tested.
    - Tests: #54 buttons clicked, not found by their text; #55 hover, a long
      press on a shared column, where bars sit in the code view; #15 `#` in
      the chat and a chat's things checked live (H's run).

## Part H: the first review

H.1 `dev_docs/tags-everywhere-findings.md` becomes a review plan: its
    sections and items as nested items, each with its state (the "Decide"
    ones open, the fixed ones done, the kept ones decided with their
    reason as the item's text), the reviewers' reports linked from each.
H.2 The user goes through it in the cockpit; that run is this plan's "day
    in the life".

## Part I: README

The README's plans, review and tags parts, said as they now are; the
agents' tools table.

## Process

Built as `dev_docs/process/build-and-validate.md` says: each step tested
and checked live as a user, then committed. Learned from the tags plan,
where the plan said one thing and something else was built, recorded
afterwards as "as built":

- **Every step ends with a check against this plan** before it's
  committed: each thing this plan says about the step, listed and marked
  built or not. Anything not built as written stops the work and comes to
  the user as a question (in `discussions-findings.md`, marked "Decide");
  it's never swapped for something cheaper, never recorded as "as built"
  without the user's OK.
- **Every step is tried live in the cockpit**, as a user would, in the
  browser (`dev_docs/process/build-and-validate.md`): what was tried and
  what was seen is written down with the step. Nothing is taken as working
  because its tests pass. What can't be tried live here (it needs the
  user's own session, say) is said, and the user is asked how to try it;
  it's not skipped quietly.
- **Findings are kept as the work goes** in `discussions-findings.md`:
  every reviewer's point and every problem found while building, each
  with its status (decide, open, kept with a reason, fixed with its
  commit); the reviewers' reports, word for word, beside it.
- **A step is ticked** only when its check against the plan is clean, it's
  been tried live, and the tags findings this plan lists for its part are
  fixed or decided by the user.

Stopping points (linters, the whole suite, the live pass, the reviewers in
`dev_docs/process/reviewers.toml`, fixes, findings recorded), each with
the faithfulness reviewer (what was built against what this plan says):

1. After the shared layer and a plan's parts as things (steps 1–2):
   responsibility, correctness, faithfulness.
2. After items, the engine and the move (steps 3–5): every reviewer.
3. After the agents and the cockpit (steps 6–8): every reviewer; touch and
   a phone-width window.
4. At the end (steps 9–10): every reviewer on the whole change, and the
   review of H as the live run.

## Steps

1. Things, one layer (A).
2. A plan's parts as things (B).
3. Review items in plans, and the plan tree (C).
4. The discussion engine (D).
5. Plans' discussions move over (E).
6. Agents (F).
7. One conversation component; the spec view on it (G.1, G.6).
8. The discussion buffer, starting one, showing them, the inbox (G.2–G.5,
   G.7).
9. The first review (H).
10. README (I).

## Tags findings outside this plan

Left for their own fix (not touched by this plan's work): #3 the two old
tags' lost text; #8 Escape as a jump in a terminal; #9 what a buffer keeps
across a reload in two places; #17 a jump's line after a terminal's width
changes; #33 one lock for every followed command; #37 the Tags view
drawing every row; #39 the key sheet without descriptions; #48 a settings
drop-down ignoring colours; #49 the palette's search over descriptions;
#59 one flaky terminal test; #61, #62, #65, #66 (kept, with reasons).

## Decided (with the user, 2026-10-20)

- **Who answers:** the lead, unless you pick another agent slot when you
  start a discussion.
- **Agent tools:** `discussions`, `discussion_read`, `discuss`,
  `discussion_reply`, `discussion_answer`; the capability `discuss`.
- **`#` in a typing box opens the searchable tag picker** (finding #10).
  The same picker will name files and code symbols later: one way to pick
  things everywhere.
- **Updates send the whole tag or discussion** (finding #2): simpler;
  sending only what changed is an optimization for when it's needed.
- **A running command's log has a size limit, set per project**
  (`.nah/terminals.toml`), 64 MB by default (finding #5).
- **Item states:** open, decided, done, dropped.
- **Search uses FFF** (finding #1); the branch-only log conversion is
  dropped (finding #7, no shims).
- **Where things are kept** (the project's `.nah/` folder):
  - Settings files you write (`ignore`, `mcp.toml`, `terminals.toml`, …)
    and `plans/` stay where they are, committed by default.
  - `cache/`: anything built from something else, safe to delete: the code
    index (`index.sqlite`) and the terminals' start-up hooks (moved from
    `state/terminal-hooks/`). Ignored by git.
  - `state/`: what can't be rebuilt: the database (`state.sqlite`, `lock`),
    the cockpit's saved layout (moved from `ui/state.json` to
    `state/ui.json`), the tags' logs (`tagged/`) and discussions' text
    (`discussions/`). Ignored by git, all of it; nothing in it is
    committable (keeping state beyond this machine is in `TODO.md`).
  - Discussions: the database holds who, what, where and status, and where
    each message's text is; the text is in files under `state/discussions/`,
    through the same log layer as the tags' logs, searched with FFF.
  - The moves (`ui/` and `terminal-hooks/`) happen once; the code then
    reads only the new places.
- **Keys and looks:** tag edits on `Space k r` rename, `c` colour, `n`
  note, `x` clear note, `d` delete, `o` take off (finding #6); `C` starts a
  discussion on what the cursor is on (the spec view's "ask for a change"
  key, made everyone's); a thing with discussions is marked in the gutter
  with a small speech-bubble icon beside the tag bars.

## Open (★)

- Marks becoming notes on things (settled before; not in this plan).

## Checklist

A step is ticked as Process says; `tags-everywhere-findings.md` is
updated as each of its findings closes (its status, the commit).

- [ ] 1 things, one layer
- [x] 2 a plan's parts as things
- [x] 3 review items, the plan tree
- [x] 4 the discussion engine
- [x] 5 plans' discussions move over
- [x] 6 agents
- [x] 7 one conversation component, the spec view
- [ ] 8 the discussion buffer, starting, showing, the inbox (built, d537e31; ticked once Decide 5 is answered)
- [ ] 9 the first review
- [x] 10 README

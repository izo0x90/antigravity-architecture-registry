# Discussions plan: findings, kept as the work goes

What `dev_docs/discussions-plan.md`'s build found: every reviewer's
point and every problem found while building, each with its status. The
reviewers' reports, word for word, are in `dev_docs/discussions-reviews/`.

Status:
- **Decide**: the plan can't be built as written, or I'd build it
  otherwise; you decide. Nothing is built for it until you do.
- **Open**: a real problem, not fixed yet.
- **Kept**: left as it is, for a reason given; you may disagree.
- **Fixed**: fixed, with the commit.

---

## Decide

1. **[Decided 2026-10-21: the database. Delete `.nah/ui/`, write `.nah/.gitignore` as the layout is.]** **The cockpit's layout isn't in `.nah/ui/state.json`** (plan A.5 and
   "Where things are kept" say to move it from there to
   `state/ui.json`). It's been in the state database
   (`.nah/state/state.sqlite`, the `ui_state` table) since 8ba1b6f (Oct
   14), which is already under `state/` and ignored. What's in `.nah/ui/`
   is a file from before that, which nothing reads, and `.nah/.gitignore`
   still lists `ui/` from then. I said otherwise when we discussed where
   things are kept; I hadn't checked.
   My suggestion: leave the layout in the database, delete the old
   `.nah/ui/` folder here by hand (only this project has it), and have
   `.nah/.gitignore` written as NAH's layout is (`cache/`, `state/`),
   not only added to. The other way (taking it out of the database into
   `state/ui.json`) gains nothing. (SP1 faithfulness 2 and correctness
   raised it too.)

2. **Deleting a discussion** isn't in the plan (D and G.2 list reply,
   answer, settle, reopen, make it a change request). I built it for the
   user alone, and it takes the discussion's message files and logs with
   it; deleting an open one lifts what it held on an approval, as
   settling does. Keep it, or take it out? (SP2 faithfulness 6)
3. **A change request on a thing whose owner can't say it changed.** The
   answer has to name what changed, and a plan's component, section,
   relation and the plan itself are checked against the review's diff (as
   before). Code lines, a terminal's output, a chat's turn and a review
   plan's items can't be checked that way: naming the thing is accepted
   as it is. Items aren't in the review's diff; code lines could be
   checked against their snapshot. Accept naming them, check code lines
   only, or add items to the review's snapshot? (SP2 faithfulness 3;
   responsibility 9; correctness 3)
4. **A key for setting an item's state.** `plan.item_state` is reached
   only through the palette (`:`); every tag edit has a key. My
   suggestion: `s` in the plan tree and on an item's card. (SP2
   consistency 3)
5. **What the discussion buffer shows of code.** G.2 says "code with its
   lines highlighted". It shows the discussed lines alone, as plain
   monospace text, the same as any other thing's text. The other way is
   the code around them, with those lines highlighted, as the code view
   draws them. My suggestion: the code around them, highlighted (a few
   lines either side), since a comment on lines often needs what's around
   them. Or keep it as built. (Step 8)
6. **An agent driving the cockpit settles, reopens, makes change requests
   as you** (part of #63). Suggestion: the cockpit refuses the user's own
   actions to an agent driving it. (SP3 permissions 5; details under
   "Stopping point 3")
7. **Settling and the rest need `plans.approve`.** Suggestion: a
   capability of their own, the user's only. (SP3 responsibility 7)
8. **A discussion asked of a slot that's closed** waits on it for good.
   Suggestion: it goes to the lead, told so; or you ask another slot.
   (SP3 correctness 6)
9.–14. From stopping point 4 (details there): whose move after the agent
   replies; who may ask or reply (`slots.message`, `user.notify`); the
   reserved slot names; an agent can't ask itself; searching some logs
   without FFF; a key for `discuss.open` (suggested `g D`).

## Open

1. **The plan read twice, the discussions in three copies in the
   cockpit** (the plan tree and an item's card each ask for the plan;
   the desk, the review and a card each hold the plan's discussions). To
   be made one desk service and one reader in step 7, where the card is
   rebuilt on the conversation component. (SP2 responsibility 6;
   consistency 1)

## Kept

1. **#16 with an agent that gives no message ids:** two messages of yours
   one after the other (a prompt stopped before any answer, then the next)
   are told apart on replay only by the ids the agent gives them; without
   ids they're one message, and later turns are numbered one less. The
   Claude Code agent gives ids. (SP1 faithfulness 4)
2. **#16, a message added while a question waits on you:** the agent
   takes it at once but reads it after the question's answered; what comes
   between (the tool's result) is the new turn's live and the old turn's
   once replayed. The agent says nothing when it reads it. (found checking
   SP1 correctness 1)
3. **Counting a growing log's lines after a restart** reads it through
   once (then only what it grew by); a whole thing's count is kept with
   it. (SP1 faithfulness 10)
4. **What a card's sections say is read now** (findings checked again,
   the changes since the last review), not what the card showed: reads
   give things as they are now. (SP1 responsibility 4, in part)
5. **C.3's discussions in the plan tree** (each row's discussions' state,
   its discussions as rows) come with G.6 in step 7, which the plan puts
   there; the step-3 check said so. (SP2 faithfulness 2; correctness 7;
   tests 5)
6. **What a plan part touches is the contract's** (`Subject::touches`:
   a section, a relation and the plan touch a component), not each
   owner's: subjects are the one shared protocol, and every reader needs
   the same answer. (SP2 responsibility 3, in part)
7. **"The user" is `user` in discussions** (only the user settles, as
   the engine checks), while approvals and reviews carry the cockpit's
   name; the routes that are the user's alone now need `plans.approve`,
   as before. (SP2 responsibility 7, 8)
8. **A plan file is still agent-writable**: an agent writing comments
   into one (signed as the user) has them brought in at the next start,
   as one writing approvals into one has them read. (SP2 permissions 1, in
   part)
9. **Approvals without discussions** (the service embedded with no
   engine, as its own tests run it) aren't held by any; the server
   always sets them up, and a closed engine holds every approval. (SP2
   faithfulness 8)
10. **The current item state isn't marked in the item-state choice**: the
    shell has only the item's name at the cursor. (SP2 consistency 3, in
    part)
11. **Hand-written date reading and writing** (`arch_graph::when`), one
    pair for everything a person reads, tested both ways; old comments
    were only ever written in that form. (SP2 consistency 7)
12. **Every message in its own file, read at start-up; the hub holds every
    discussion** (as decided for tags and discussions). (SP2 performance
    7, 9)

## Fixed

Stopping point 1 (steps 1–2), the reviewers' points (reports in
`discussions-reviews/sp1-*.md`), fixed in 4e4faf7:
- A session numbering its turns anew moved tags onto other turns; a
  followed turn waited for the wrong number: a noted turn keeps its own id
  and is found by it, the chats tell the notes when turns are numbered
  anew. A turn's end between looking and following was missed: watched
  first now. (SP1 correctness 1, 5; responsibility 2)
- An added message's new turn started where it was sent, not where the
  agent took it: it starts where the agent says it took it. (SP1
  correctness 1)
- Owners were told what's noted before every kind of note was there, and
  when tags this build can't read were left out: code snapshots could go.
  Now only once start-up is through, and never when a kind of note can't
  say all it's on. (SP1 correctness 6; responsibility 1)
- The logs held one folder for tags only: the log layer (`Logs`) is
  reusable for another folder and prefix, its own FFF search with it, for
  discussions' text in `state/discussions/`. (SP1 faithfulness 1;
  responsibility 3)
- The shared layer said "tag_read's `full`" and "a running command's":
  the reader's words are given by the reader; the cap's are neutral.
  (SP1 responsibility 3; faithfulness 8)
- A database at layout 10 wouldn't open: layout 10 stays, converting
  nothing. (SP1 correctness 2)
- The same code lines tagged before the index caught up were kept twice:
  lines of the same file are compared where they are now. (SP1 correctness
  3)
- A thing past the running-log cap from the start was kept whole: what
  fits, then said so. (SP1 correctness 4)
- Things a lasting owner hasn't (a plan that isn't, lines past a file's
  end, a section no card has) could be tagged: refused. (SP1 correctness 7)
- Search: a heading used up a file's limit, growing files were read again
  every time, matching differed by where text was kept, and a search cut
  short said "nothing": every word in any order (as `tag_find` matched
  before), lines as they are, and said when it ran out of time. (SP1
  correctness 8; faithfulness 5)
- `chat session …` names couldn't be named back, and a closed slot's
  chats kept its name: named and named back as no slot's. Code paths with
  " line " in them split wrong. (SP1 correctness 9)
- Notices, errors, commands could pass for detail in a turn's text: only
  tool output is detail. (SP1 correctness 10)
- A noted thing kept before owners kept more with it (a line's command, a
  definition's place) lost its name's detail at refresh: it gains them.
  (SP1 faithfulness 6)
- A running command's output so far was built under the lock every
  terminal's output passes: built first now. (SP1 faithfulness 7)
- A card's sections were listed three times: one list
  (`CARD_SECTIONS`), checked against the cockpit's; a card and its logic
  section say the logic the same way. (SP1 responsibility 4)
- Plan "code" sections read files their own way: through the one checked
  read of the project's files. (SP1 responsibility 5)
- A plan's part was named one way in the cockpit's go-to and another in
  the picker: one name. (SP1 responsibility 6)
- `PlanItem` was owned but not named back, read or there: it is (step 3's
  items). (SP1 responsibility 7; faithfulness 3)
- Deleting a tag read every store once per thing: once. (SP1
  responsibility 8)
- The terminals' start-up hooks' old folder was removed at every start:
  the code's gone (the folder was only this branch's). A test-only
  accessor of the palette is test-only. (SP1 faithfulness 2, 9)

Stopping point 2 (steps 3–5), the reviewers' points (reports in
`discussions-reviews/sp2-*.md`), fixed in 02daf34 with step 6 (they share
files):
- Old comments could be brought in again and again (a file checked out
  again, a file that isn't a plan, a failed write, two reads at once):
  they're moved once, as the server starts, each remembered by where it
  came from (`plan p comment c1`) and never brought in twice; a file with
  comments not moved holds its plan's approvals ("they become discussions
  when the server starts again"); a failed write no longer stops the
  watcher. (SP2 correctness 1, 4, 6; faithfulness 1, 7; responsibility 1;
  permissions 1, 2; tests 2; performance 8)
- A question could be settled unanswered, "make it a change request"
  reopened with no why, and a whole-plan change answered a component's
  change request: settled once answered (as before); an answered one made
  a change request is reopened with why, a settled one reopened first;
  the plan "changed" only when its own fields did. (SP2 faithfulness 3;
  correctness 3; tests 12)
- The engine held the approval rule and its words, and depended on the
  plans' crate: it says what isn't settled on a thing (`unsettled`); the
  server words the refusal. (SP2 faithfulness 4; responsibility 3)
- A whole plan's discussion held every approval but no card showed it: on
  every card's overview. (SP2 responsibility 4)
- The user's routes to settle, reopen, change and delete needed only
  `discuss` (every agent role's): `plans.approve` (the user's) again; the
  user's answer route is gone (agents answer). (SP2 responsibility 7;
  faithfulness 6, in part)
- Who's asked wasn't checked (`"leed"` made a discussion no one
  answers), and an agent couldn't ask the user: an agent slot by id or
  name, or `user` (then it's the user's move). (SP2 faithfulness 9;
  correctness 8)
- A refresh could tell an older discussion after a newer one: told while
  held. (SP2 correctness 5)
- Reading discussions on a route held the async threads while a write
  synced: off them. (SP2 performance 3)
- Every move looked up each chat-turn note's whole session again: the
  chats keep each turn's number from the event stream (a missed stretch
  read again), and forget a closed session's. Following a finished turn
  copied the whole event cache: only a running one. (SP2 performance 1, 4;
  consistency 5)
- Every move worked out where each thing is once per kind of note (a
  diff each): once per pass, and once per tag edit. (SP2 performance 2, 5)
- An open item's card read its plan again on every change anywhere: on
  its plan's. (SP2 performance 6)
- The logs kept every file written for a search that never came: only
  once there is one. Search capped a file's lines before checking every
  word: after. (SP2 performance 7; tests 6; consistency 9)
- Moved comments named agents `agent lead`, new ones `lead`: by the slot
  name both. (SP2 consistency 8)
- The tree buffer knew plan items: its source says where a row opens and
  what holds its thing. The plan tree's section titles are the card's.
  Words: "Question on …", "discussions not settled", the chip
  `open_discussions`. (SP2 responsibility 5; consistency 2, 4, 6)
- Rules 1 and 3 are checked (`clean_cut`: no plan comments, one
  engine). (SP2 faithfulness 5)
- Tests: a change elsewhere doesn't answer a change request; a relation's
  discussion holds its ends, another plan's none; a deleted one is told;
  the cockpit's requests are the routes' (and found by it: the first key
  after sending from a card was lost, now fixed); the plan tree's
  nesting; a key built, not spelled; a vacuous and a self-checking test
  gone; a duplicate check gone; the slow slot test given ten seconds.
  (SP2 tests 1, 3, 4, 5, 7, 8, 9, 10, 11)
- Engine and things: `things()` gone; started at once, discussions are
  in their numbers' order. (SP2 responsibility 10; tests 12)

---

## Couldn't be tried live here (how would you like them tried?)

1. **The agents' tag tools on the running server** (`tag_find` with
   FFF, `tag_read` folding, tagging by name). MCP gives a caller its
   rights only through an agent session's key; the one agent slot here
   is your lead, with no session (starting one would use your chat).
   They're tried against a real server in the tests
   (`slots_tests`, `terminals_tests`, `changes_tests`).
2. **#16 with a real agent:** a message added to a running turn, then the
   session reloaded. Tried with the mock agent (which now keeps and
   replays its session's history, message ids and all); a real one needs
   your session.
3. **The agents' discussion tools, and an agent told of a discussion in
   its running session.** As 1: tried against a real server with the fake
   harness (`discussions_tests`: listing, reading, starting, replying,
   answering, the lead's session told; `slots_tests`: held for a slot
   until it runs). Live, the user's side only (the lead has no session
   here: what it's told was seen held for it).

## Step 1: things, one layer (A)

### Checked against the plan

- **A.1** built: `crates/nah-things`. Owners (`Resolve`, now with
  `owns`, `detailed`, `noted`) and their registry; naming back (`named`);
  where a thing is now (`current`, `renamed`; owners say `moved`); there;
  `settle`; the logs (`Logs`: a file per thing), following, the cap;
  reading (start and line count, windows, all of it) and search (FFF).
  `nah-tags` is tags on top of it; every kind of note registers
  (`Notes`: which logs it points into, the things it's on, refresh), so
  discussions use the same registry.
- **A.2** built: the cockpit's `app/notes.rs`: what's at the cursor
  (`NotesHere`), the gutter, the top line's names, all through it; go-to
  is the one path it was. The discussion mark comes with G.4 (step 8).
- **A.3** built, with these test changes, each from a planned change:
  the tags tests take the things layer in their set-up; a log's first
  line no longer starts with a blank one (a file per thing); `find_text`
  takes the text (FFF searches it) where it took a closure; the
  long-text test moved to `nah-things`. Server and cockpit tests expect
  the one name (#25), and the picker's words through `Subject::name`
  rather than written out (#58).
- **A.4**, by finding: #1 FFF searches the logs (its walk started in the
  logs' folder reaches it though git ignores it; tested in a git repo).
  #2 decided, no change. #5 `running_log_mb` in `.nah/terminals.toml`,
  64 by default (tested). #7 layout 10 dropped (this project's database
  set back to layout 9 when the server was restarted). #4 a test that
  kills the real server and starts it again on the project. #19 the
  store says which logs every tag points into, those it can't read too;
  nothing's removed when a kind of note can't say. #21 a file per thing,
  removed when the note comes off. #22 a command is looked at and taken
  in under the lock its end is told under, and told its end if it ended
  in between (no test: it's a race). #32 a log's lines counted once,
  then only what it grew by (after a restart, once in full). #34 a thing
  is written to the logs before the tag is kept, nothing waiting on it.
  #60 only a file's first line is a heading. #18 a definition is kept
  with where it was at a snapshot, read from there once it's gone. #20 a
  line keeps the command it's in (and a command what was typed). #25 one
  name, `Subject::name`, used by the server and the picker. #30
  snapshots no note points to are let go (not the last, nor one handed
  to a note not kept yet). #31 an owner is found by the thing's kind; a
  name needs no git. #16 see below. #29, #50 only a chat turn is folded
  (its owner says it has detail), one fold for every read, and a
  message's own lines can't pass for detail. #35 the cockpit keeps a
  tag's name, colour and note at the cursor, not the whole tag. #40 a
  view says where it draws things in one place (`draws_things`). Tests
  #51, #52, #53, #57, #58: added or cut as listed.
- **A.5** built for the terminals' start-up hooks (`.nah/cache/`, the old
  folder removed at start-up). The cockpit's layout: **Decide 1**.

### Built this way (worth knowing; say if you'd have it otherwise)

- A component's name is `component <id>` (`component
  rust:meta_harness::main::main`): the picker can't know the kind and
  place the server added, and one name means one. The id says what and
  where in the code.
- A chat's name always carries its session's start (`chat
  lead@4f2a91c0 turn 3`), so it stays the same when the slot moves on to
  another session.
- A command's name carries what was typed (`terminal t1 command 3
  (make)`); the picker no longer says "and its output", nor a message's
  first words.
- A thing read as it was says so the same way whichever way it went:
  "as it was when tagged: changed or gone since" (lines of code changed
  no longer say "(changed since tagged)" in their name).
- #16: live, a message the agent takes while it runs now starts the
  next turn (as a replay numbers it); replayed messages are told apart by
  the ids the agent gives them.

### Tried live (the cockpit, headless browser)

- A terminal: `printf 'step1-a\nstep1-b\n'`, `M` on an output line: the
  picker said "Tag terminal t1 command 1 (printf …)", `Tab` "Tag
  terminal t1 line 3 (of command 1)"; tagged both. The server kept them
  by exactly those names, each in its own log file (header, then the
  text); the gutter's bar beside the command, `live1` on the top line.
- The server killed and started again: the Tags view gave both from the
  logs, named with their shell's old start, the line still "(of command
  1)", read "as it was when tagged: changed or gone since".
- Taken off from its row in the Tags view: its log file went at once.
- The code view: line 21 (in no definition): "Tag src/main.rs line 21",
  no `Tab`; line 239: "Tag component rust:meta_harness::main::main",
  `Tab` the line. The component was kept with where it was, at the same
  snapshot as the line. At start-up the snapshots no note pointed to had
  gone; after deleting the test tags, only the last one stayed. The
  terminals' start-up hooks were written to `.nah/cache/`.
- Put back as it was: panel 1 the terminal, panel 2 the chat (your draft
  untouched); the test tags deleted (yours kept).

## Step 2: a plan's parts as things (B)

### Checked against the plan

- **B.1** built: `Subject::Plan`, `PlanComponent`, `PlanSection`,
  `PlanRelation` (its kind is the field `relation`: `kind` is taken by how
  a subject is written), `PlanItem` (owned from step 3, with items). Their
  one names: `plan quick-look`, `… component <id>`, `… component <id>
  section logic`, `… relation <source> calls <target>`, `… item <id>`. A
  plan has its parts in it, a card its sections (a tag on a card shows on
  its sections).
- **B.2** built: `src/plan_things.rs`, registered at start-up. Names them,
  names them back (a part the plan doesn't have is refused, saying why),
  gives what each says (the plan's title and description; a card; a
  section: overview, signature, logic, invariants, relations, the code,
  what changed since the last review, placeholders, findings; a
  relation), says whether it's there; nothing logged.
- **B.3** built: going to a card, a section or a relation opens the card
  read from that plan, its cursor on the section (the relation); the
  desk's source moves to the plan. Going to a whole plan, the graph shows
  it. The plan tree's row is step 3's (C.3).

### Built this way (worth knowing)

- The spec card says what's at its cursor as things (a relation the plan
  has, then the section, the card, the plan; a component the plan doesn't
  have: the component), and where it draws them, so its host keeps a
  gutter (the card's content moved right by its width). Not written in B,
  but noting a plan's parts from the card needs it (G.3 relies on it).
- A card is the plan's when it's read from the plan and isn't only
  observed in the code (a walkthrough's existing components are the
  plan's too).
- Going into a plan from elsewhere moves the desk's source, not any
  graph: each panel's graph keeps its own source, as it did (making
  graphs follow broke that, and was taken back).

### Tried live

- The quick-look plan in the graph; the card of `quick_look_to_column`;
  `M` on its logic section: "Tag plan quick-look component
  rust:cockpit_ui::app::CockpitApp::quick_look_to_column section logic",
  `Tab` the card, the plan. Tagged: the server read it back as the plan
  has it (the four steps); the gutter's bar beside the section's rows.
- From the terminal, `g T` to it: the card, its cursor on the section
  header, read from the plan, also after the desk was switched back to
  the code. The whole plan tagged and gone to: the graph showed the plan.
- A go-to opens in the other panel, over your chat (as every go-to does);
  your draft was kept. Put back: panel 1 the terminal, panel 2 the chat,
  the graph on the code; the test tags deleted.

## Step 3: review items in plans, and the plan tree (C)

### Checked against the plan

- **C.1** built: a plan holds items (id `i1`…, the item it's under, title,
  text, state: open, decided, done, dropped); any plan can have them; a
  new kind, review, is items only (anything else is refused, saying so).
  (In 4e4faf7, with the stopping point's fixes: they share files.)
- **C.2** built: `plan_edit` adds, edits, moves (nests) and sets items;
  `plan_create` makes a review; `plan_list` lists items nested with their
  states. You set an item's state in the cockpit: one shell action on the
  item the cursor's on, in any view ("Set i2's state": open, decided,
  done, dropped).
- **C.3** built: the plan tree (a buffer, `Space b p`): the plan the desk
  reads, its items nested, each showing its state, then a change plan's
  (or walkthrough's) components with their cards' sections. Enter on an
  item opens its card (the spec view: its title, state, the item it's
  under, its text, the items under it). Each row's discussions' state and
  its discussions as rows under it: with the discussions (G.6, step 7).
- **C.4** built: a review's items through the tools and the plan's file
  (server tests); the tree showing them nested with their states, an
  item's card, its state set, its card back after a reload (cockpit test).

### Built this way (worth knowing)

- A plan is opened in the plan tree by making it the desk's source (the
  graph's source picker, which now lists reviews too); the plan tree has no
  picker of its own.
- An item's card is the spec view's (as the plan says), with nothing of a
  component's card (no sections, no review controls): only Pin.
- The plan tree is one per desk, as the Tags view is (it shows the plan
  the desk reads).

### Tried live

- A throwaway review plan (made and deleted through the server's routes):
  shown as the graph's source, the plan tree showed Decide (open) with FFF
  skipped (decided) under it, and Open (open). Enter on FFF skipped: its
  card in the other panel (title, "decided", "i2 · plan live-review",
  "Under i1 Decide", its text). "Set i2's state" done: said "i2 is done
  now", the card read "done". Reloaded: the card came back. First seen
  live and fixed: the card offered a component card's sections and review
  controls, and its panel said "following what you're on".
- Put back: panel 1 the terminal, panel 2 the chat (your draft
  untouched), the graph on the code; the plan deleted.

## Step 4: the discussion engine (D)

### Checked against the plan

- **D.1** built: `nah-discussions`. A discussion has:
  - an id (`d1`, …);
  - the thing it's on (a `Subject` and its name);
  - its kind (question or change request);
  - who started it, and the agent slot asked (the lead unless another is named);
  - its messages: who, when and the text, and for an answer, the things it changed as subjects with their names;
  - its status (open, answered, settled; reopened is open again with why);
  - whose move it is, worked out from the status: the agent asked while it's open, yours once it's answered, no one's once settled.
- **D.2** built:
  - The state database (layout 11, `discussions`) holds each discussion's structure and where its messages' text is.
  - The text is in files under `.nah/state/discussions/`, written through the same log layer as the tags' logs and searched with FFF (`Discussions::find`).
  - Each change goes on the event stream (`DiscussionUpdated` carries the whole discussion; `DiscussionRemoved`) and into the activity record with who made it.
- **D.3** built: a thing that doesn't last (a terminal's output, a chat turn) is written to the logs when a discussion starts on it, as tags do. A discussion follows its thing when the thing moves.
- **D.4** built:
  - Only the user settles, reopens, deletes or makes a discussion a change request.
  - A change request is settled only once it's answered.
  - Its answer has to name a change to the thing it's on (or a part of it), and each named thing must have changed since the plan was last reviewed, where its owner can say (a plan's parts, from the review's diff).
  - A plan's component can't be approved while a discussion on it, its sections, its relations or the whole plan isn't settled. Plans ask the engine through a check set once at start-up. The old comments' check stays until step 5 moves them over.
- **D.5** built: a discussion is a thing (`Subject::Discussion`, `discussion d1`), owned by the engine. It can be tagged, named and read. Going to one from the cockpit comes in step 8.
- **D.6** built. Engine tests cover:
  - the whole lifecycle;
  - who may do what;
  - approvals held;
  - lines that move;
  - output gone from the terminal, read from the logs;
  - a deleted one taking its logs with it;
  - a restart;
  - a discussion on a discussion;
  - search.

  The state database's own test covers numbers never given twice, a later build's discussion left alone, and the logs those point into. A server test covers approval held until settled through the routes, with a change since the review needed.

### Built this way (worth knowing; say if you'd have it otherwise)

- **Routes:** the cockpit's routes for discussions are built now (`/api/discussions`: list, on a thing, read, start, reply, answer, settle, reopen, make a change request, delete). The engine can't be tried live without them, and step 8 uses them.
- **The `discuss` capability:** built now, as F.2 says (the lead's, sub-slots' and workers'). The routes need it. The agents' tools come in step 6.
- **Numbering:** a deleted discussion's number is never given again, so a tag or a discussion on `discussion d3` never ends up pointing at another one. A start that fails on its thing uses no number. That's why the line in the logs starts "a discussion · …" rather than with its number.
- **Settling a question:** you can settle a question that has no answer yet. Only a change request has to be answered first.
- **`on(thing)`:** gives the discussions that touch the thing. For a component, that includes its sections', its relations' and the whole plan's discussions.

### Tried live (the server's routes; the cockpit draws discussions from step 8)

- **A terminal command:**
  - Ran `echo discussion-live-check one two` in the cockpit's terminal. Tagging it there told me its shell's number, and the tag was then removed.
  - Started a question on its command: it was kept with its output in the logs.
  - Restarted the server: the discussion came back with its message. Its thing is now named for its ended shell (`terminal t1@… command 1 (…)`).
  - Then, in order:
    - a reply;
    - settled, with no answer needed for a question;
    - an answer, refused because it's settled;
    - a reply, refused for the same reason;
    - reopened with why;
    - made a change request;
    - an answer naming nothing changed, refused ("name what you changed there").
  - Tagged the discussion: the tag reads it as a thing ("question on plan quick-look (open)", then each message).
  - Deleted it: its message files and its log went.
- **Found live and fixed:** starts that failed on a thing already gone (I was guessing the shell's number) still used up numbers, so the first discussion was d25. Numbers are now given only once the thing is noted. I reset this project's numbering, so it starts at d1.
- **Not tried live:** approval held by a discussion. That would mean approving a component of one of your plans. The server test covers it.
- **Put back:** panel 1 the terminal (its shell started again, the probe command left in its scrollback), panel 2 the chat (your draft untouched), the graph on the code. No discussions or probe tags are left.

## Step 5: plans' discussions move over (E)

### Checked against the plan

- **E.1** built:
  - A plan's comments move into the engine once, as each plan is read. Each anchor becomes its subject (the plan, a component, a section, a relation). Each comment becomes a discussion with its replies and its answer, in the order said. The answer keeps what it changed, as the plan's parts it named.
  - Statuses carry over: open is open, answered is answered, resolved is settled.
  - The plan's file is written without the comments once the discussions hold them.
  - Gone from the code: the field, `Comment`, `CommentAnchor` and the rest, the comment routes, the service's comment calls, and the agents' `review_comments`, `answer_comment` and `reply_comment`.
- **E.2** built: what the user last reviewed (`last_review`) and the review's diff stay the plan's (`service/reviewed.rs`).
- **E.3** built: a server test reads a plan file with a comment of every anchor and status, once. It checks that every discussion is there as it stood (messages in order, when each was said, what the answer changed), the file has no comments, approvals are held the same way (open and answered hold, settled doesn't, the plan-wide one holds every component), and nothing comes in twice when the file is read again. The service's test covers comments waiting in a file until the engine takes them (a save keeps them in the file), a failed move leaving them, and a file that comes later with comments.

### Built this way (worth knowing; say if you'd have it otherwise)

- **The old shape stays readable:** a reader of it stays in the server (`src/plan_comments.rs`, private). "As each plan is read" also covers a plan file that comes later with comments, for example one checked out from another branch. Nothing else reads that shape.
- **A comment that names its slot:** an agent's comment named its session, not its slot, so it becomes `agent <session>`. A user's becomes `user`.
- **The spec view** now reads and writes discussions:
  - It reads them from the event stream, through one keeper on the desk (`Shared::discussions`) that the inbox and gutter will use in step 8.
  - It writes through the discussion routes.
  - Its own thread and typing box stay until step 7 replaces them with the conversation component.
  - Its labels now say discussions: "Ask about this item", "Settle this point (accept its answer)", and the chip reads "open discussions".
- **Agents between this step and the next:** in this commit agents have no review tools (the old three are gone). Step 6 adds the new ones.
- **Missed in step 4, added here:** the discussions' routes in the server's pinned list of routes.

### Tried live

- **Your plan `nav-audit`:**
  - It had one open comment (on `audit_helper`, with a reply). After the restart it's discussion d1, open, with both messages. Its file has no comments, and the rest of the plan is unchanged (compared field by field).
  - A copy of the file as it was is in this session's scratchpad.
  - In the cockpit (the plan tree, then "Go to its spec"), its card says "1 open discussions" and "Overview changed · 1 open". The thread reads "OPEN d1 · you", "audit comment one", "you: audit reply one".
- **A throwaway plan:**
  - Made one with one component and opened its card.
  - `J J` to Logic steps, then `C`. The box said "Change request on Logic steps". Typed and sent it: d2, a change request on the logic section, shown "OPEN change request d2 · you".
  - `j` onto it, `r`, typed a reply and sent it.
  - Changed the component and answered through the route, naming it. The card showed ANSWERED, the answer and "changed plan zz-live component …".
  - `a` on it: SETTLED, and the section's "1 open" went.
  - Deleted the discussion and the plan.
- **Not tried live:** an agent answering, since the agents' tools come in step 6.
- **Put back:**
  - panel 1 the terminal, panel 2 the chat (your draft untouched), the graph on the code;
  - the numbering, so your d1 is followed by d2.

## Step 6: agents (F)

### Checked against the plan

- **F.1** built: `discussions` (what's waiting on you first, then the
  rest; on a plan, or on a thing by its name; settled ones with `all`),
  `discussion_read` (one whole: what it's on now and, when kept, as it
  was; whose move it is; everything said, with when), `discuss` (a
  question or a change request on a thing by its name), `discussion_reply`,
  `discussion_answer` (with what changed, by name). They replace
  `review_comments`, `answer_comment`, `reply_comment` (gone in step 5).
  The plan's next steps name the discussions holding its approvals.
- **F.2** built: reading with `view.read`; starting, replying and
  answering with `discuss` (the lead's, sub-slots' and workers'); a
  chat's things as `slot_read` allows, a discussion as what it's on (one
  `Reach` for tags and discussions); settling and reopening the user's
  alone (no tool, and the routes need `plans.approve`).
- **F.3** built: a discussion asks the lead unless it names an agent slot
  (or, from an agent, the user). The slot asked is told when one is
  started, something's said in it or it's reopened, by anyone but
  itself: a message in its running session, else held until one of its
  sessions runs (the sub-slots' reports, made one way for any slot:
  `SlotManager::tell`; layout 12 keeps who it's for).
- **F.4**: no agent pack, skill or queue prompt named the old tools
  (checked: `agents/`, `plugin/`, the queue's templates); nothing to
  rewrite. The old names are now checked gone (`clean_cut`).
- **F.5** built: #26 a message's text is put together one way for every
  reader (`ItemBody::said`: the chat, `slot_read`, what's tagged and
  discussed); #44 one date form for everything a person reads
  (`arch_graph::when`: plans, the logs' headers, messages). #63 and #64
  aren't widened.

### Built this way (worth knowing; say if you'd have it otherwise)

- What a moved comment said is in its discussion, but its slot isn't
  told (moved ones wait in `discussions`, as before).
- An agent may answer a discussion asked of another slot (the engine
  doesn't check who answers; the one asked is who's told).

### Tried live (the cockpit, headless browser)

- A throwaway plan's card: `J J`, `C`, typed "Is step 2 needed?",
  Enter: d2, a change request on the logic section, "OPEN change request
  d2 · you". The lead has no session here: what it's told was held for
  it ("Discussion d2 on plan zz-live component … section logic: the user
  asks you for a change: …", from "the user (discussion d2)"). A reply
  (`j`, `r`, typed, Enter): "you: Second line.", held for the lead too.
- Put back: the discussion, the plan and what was held deleted; the
  numbering after your d1; panel 1 the terminal, panel 2 the chat (your
  draft untouched), the graph on the code.


## Step 7: one conversation component; the spec view on it (G.1, G.6)

### Checked against the plan

- **G.1** built: one conversation component (`cockpit-ui/src/conversation.rs`).
  - **A message** shows who said it (one way: "you" or the slot), when, what it is (answered, settled it, reopened it), the text (an agent's as markdown) and what's under it (what an answer changed).
  - **The typing box** has many lines; Enter sends, as the keymap says. A `#` typed where a word starts asks for a tag's name. The box says so (`Ask::NameTag`) and the shell opens its tag picker, putting the name in after it. Once sent, it gives the keyboard back.
  - **The chat** draws your messages and the agent's with it, and its prompt box is the typing box.
  - **The spec view's** own thread and composer are gone. Each discussion is drawn with the component: its heading, then everything said. It writes in the typing box.
- **G.6** built:
  - **The plan tree** shows each part's discussions as rows under it ("d1 question: …", whose move it is). Each row also says how many discussions there are and whose move they wait on ("decided · 1 on lead"). A whole plan's discussions are rows at the top.
  - **A discussion row's things** are the discussion itself, what it's on, then the plan.
  - **Cards:** a component's card shows its discussions in their sections. An item's card shows the item's.
  - **Opening a discussion:** for now, opening one from the tree opens the card of what it's on. Opening the discussion itself comes with its buffer (G.2, step 8).
- **G.8, the conversation component's closes:**
  - #10: `#` in every typing box opens the tag picker (the chat's and a card's).
  - #27: a tag's note is written in the same many-line box (the palette's writing, `Param::Writing`).
  - #43: the typing box says where a `#` starts a name, and the shell picks; the chat no longer names the shell's action.
  - #45: one way to say who wrote something (`conversation::who`: messages, tags, notes).
  - #56: `#` after Backspace: typed again, the picker opens again (decided that way, tested).
- **Open 1 (SP2), closed:** the plan the desk reads is read once (`Shared::review`: the plan, what changed, its discussions; the last read asked for wins). The plan tree and an item's card read it from there.

### Found while building, fixed

- **A key that opened a typing box was typed into it, or the first key after it opened was lost.** The new tests found this, depending on how focus was taken. Now a key that runs a command isn't typed anywhere else that frame, and a box takes the keyboard before it's drawn, so the first key typed is its.

### Built this way (worth knowing; say if you'd have it otherwise)

- **The chat's other items** (tool calls, thinking, plans, questions) stay the chat's own. Only messages go through the component.
- **What an answer changed** is listed under it. Going to each one comes with the discussion buffer's actions (step 8).
- **A card and the plan tree** each take what's shared when it changes, as every buffer does: a copy each, from one read.

### Tried live (the cockpit, headless browser)

- **A throwaway review plan** (two items, one under the other) in the plan tree. Started a question on the first item through the route (two lines). The tree showed:
  - "Flaky test · open · 1 on lead";
  - under it, "d2 question: Which test is it? · on lead";
  - the tree's hint on that row read "Open d2 question…'s card", since reworded to "Open the card d2 is on".
- **Enter on the discussion's row:** the item's card opened with its text, the item under it, and the discussion drawn as every conversation draws one: "OPEN · d2 · asks lead", "you · 2026-10-21 00:13", then both lines.
- **A tag's note in the many-line box:** tagged the item, then `Space k n`. The palette showed the box ("its note (as many lines as it takes)", "Enter sends · shift+Enter new line · Escape goes back"). Typed a line, shift+Enter, then a second line, then Enter. The note kept both lines.
- **Not seen live:** the chat's messages drawn with the component. The lead has no session here; the cockpit's tests draw them.
- **Put back:**
  - the tag, the discussion, the plan and what was held for the lead deleted;
  - the numbering after your d1;
  - panel 1 the terminal, panel 2 the chat (your draft untouched), the graph on the code.

## Step 8: the discussion buffer, starting, showing, the inbox (G.2–G.5, G.7, G.8) — d537e31

### Checked against the plan

- **G.2 built:** the discussion buffer (a buffer kind; opened in the quick look unless a panel shows one, or in a panel).
  - At the top, the thing it's on, as it is now or as it was ("as it was when tagged: changed or gone since"), read from the server (`GET /api/discussions/{id}/on`).
  - Then the messages, then the typing box.
  - Its actions: reply (`r`), settle (`a`), reopen with why (`x`), make it a change request, send, cancel.
  - **Not as written: see Decide 5.** The plan says "code with its lines highlighted". The buffer shows the discussed lines alone, in plain monospace, without the code around them.
- **G.3 built:** starting one is the shell's `C`, on the thing at the cursor, in any view, with the tagging picker:
  - its top line says what it's on, and `Tab` picks another level;
  - then question or change request;
  - then the first message;
  - once the server has it, it opens.
- **G.4 built:**
  - **The gutter** has a speech-bubble mark where a thing has discussions, apart from the tag bars. Its colour says whose move it is: yours first, then an agent's, then settled.
  - **The panel's top line** says "2 discussions, 1 waiting on you", in the colour of whose move it is.
  - **Hover or a long press** on the mark says each discussion; a click or a tap opens the one waiting on you.
- **G.5 built:** the inbox lists a discussion waiting on you (its own kind of notification, made by the server's rules when it's your move, ended otherwise). Enter opens it.
- **G.7 tests:**
  - Started from a code line, a terminal command, a chat message, and a plan section (the earlier test).
  - Reply, settle and reopen.
  - The inbox when it's your move.
  - The gutter mark: its colour, the top line, a long press, a click.
  - At a phone's width, by touch: a tap on the mark opens the discussion, and nothing in it is drawn over anything else.
  - On a graph's card.
- **G.8, the tags findings closed here:**
  - #11: a whole chat session (the picker's last level; `chat lead` for agents).
  - #12: a whole code file (`file src/a.rs`).
  - #13: lines selected in the code. Drag down the line numbers (touch scrolling on the code still scrolls); a click on a number drops the selection.
  - #14: a graph's cards show their notes on their top right corner: a dot per tag, and the speech bubble.
  - #23: a tag on a command and on a line in it are two bars: the command's faint, the line's solid on it. A tag keeps its column while it's on screen.
  - #24: the code view's change bars moved to the right of its line numbers, by the code. The tag bars stay in the panel's gutter on the left.
  - #28: no letter lost after `Tab` (cause below).
  - #36: the gutter works from a list made when the tags or discussions change, not from every tag each frame.
  - #38: the solid icon's font is checked once.
  - #42: the tag-edit picker keeps the picked tag in the desk and opens a bound action, as the other two-step pickers do.
  - #46: one line of key hints under every picker, with `Tab` in it.
  - #47: the gutter's sizes come from the theme (`gutter_bar`, `gutter_gap`, `gutter_mark`).
  - #54: the Tags view's Rename and Take off buttons are clicked in its test.
  - #55: covered by tests: hovering a bar, a long press on the shared muted column, and where the bars sit in the code view.
  - #6 was already in (`Space k …`).

### Found while building, fixed

- **A click that opened the quick look closed it again.** The quick look closes on a click outside it, and the click that opened it counted as one. It now closes only on clicks after it opened.
- **#28's cause:** two things together.
  - The palette asked for the keyboard every frame. That made egui forget that its field keeps `Tab`, so `Tab` moved the keyboard away and the next letters were lost.
  - The palette now takes the keyboard before drawing its field, and only when it doesn't have it. While it's open, egui's own moves on `Tab` and the arrows are cancelled.
- **The discussion buffer's own top line said "1 discussion" about itself.** A discussion that is itself one of the things at the cursor isn't counted.

### Open (found live)

- **Deleting a discussion leaves what it told the agent waiting.** With no lead session, d2's two messages stayed held for the lead after d2 was deleted, and the lead's next session would have been told about a discussion that's gone. This depends on Decide 2 (deleting discussions): if they stay deletable, a deletion should drop what's held for it.
- **The panel's top line wraps to a second row when notes appear** (narrow panels, long titles). That moves the content down as the cursor moves. Live, a click aimed at a mark landed on a line instead. The tags work had the same behaviour.
- **A tag's dot on a graph card is small** (5 points) for a finger. Hover works; a tap needs aim.

### Built this way (worth knowing; say if you'd have it otherwise)

- **Two new kinds of thing,** `ChatSession` and `CodeFile`, owned by the chats and the code as their other things are.
  - A whole session's text is each turn in order, under "turn N".
  - It's written to the logs when noted, as a turn is, since a session is the harness's.
  - Going to a whole session lands on its first turn; going to a whole file lands on its first line.
- **On a graph card,** the notes sit on the card's top right edge, half over its border.
- **The quick look's "click outside closes it"** now ignores the click that opened it. This applies to every way of opening it, not just discussions.

### Tried live (the cockpit, headless browser)

- **In the terminal:** ran `printf 'one\ntwo\nthree\n'`, put the cursor on a line of its output, and pressed `C`.
  - The picker's top line read "Discuss terminal t1 command 1 (printf …)".
  - Its key line read "Enter runs · ctrl+J/ctrl+K choose · Tab: Discuss terminal t1 line 3 (of command 1) instead · Escape goes back".
  - I picked "A question", wrote "Why three lines?" and pressed Enter. d2 opened in the quick look ("OPEN · d2 · asks lead", "waiting on lead"), with the command and my message.
- **The terminal's top line** said "1 discussion" while the cursor was in the command.
- **The gutter:** hovering the mark said "1 discussion / d2 question: Why three lines?", and a click on it opened d2.
- **Replying:** `r`, typed "Also: why printf?", Enter. The reply showed under the first message.
- **Tags in the terminal gutter:** tagged the command and a line in it with one tag (Tab for the line). In the screenshot, the command's bar is faint and the line's is solid in the same column, and the speech bubble sits left of the command's first line, over none of its text.
- **In the code:** dragged down line numbers 2 to 4, then pressed `M`. The picker said "Tag src/main.rs lines 2-4", with "Tab: Tag file src/main.rs instead".
- **In the graph:** tagged the "crates" box. A dot showed on its top right corner, and hovering it said "livetry".
- **Not tried live:**
  - The inbox entry when it's your move. It needs an agent to answer, and the lead has no session here.
  - The agents' tools.
  - Both are covered by tests.
- **Put back:**
  - the tag "livetry", d2, and the two messages held for the lead deleted;
  - the numbering after your d1;
  - panel 1 the terminal, panel 2 the chat (your draft untouched).
  - Two older tags, "jl" and "testtagforla" from 19 October, were left alone: I don't know they're mine.

## Stopping point 3 (steps 6–8)

### Live pass at a phone's width, by touch (400×850, touch emulated)

- **Setup:** started d2 on `src/main.rs lines 2-4` through the route, then showed the code in panel 1. The panel is narrow at this width, and the speech bubble showed beside line 2.
- **A tap on the bubble** opened d2 in the quick look, laid out to the screen. It showed "OPEN · d2 · asks lead", the two lines "as it is now", and the first message. Nothing was drawn over anything.
- **A tap in the typing box** put it in typing mode. Typed "Tapped reply from a phone" and pressed Enter: the reply showed under the first message.
- **A long press on the bubble** (1.5 s) said "1 discussion / d2 question: Phone check: why these imports?". A first try held for 0.9 s right after the emulation started, and nothing was said; held again, it was.
- **Seen, open:** on an open discussion, the Settle and Reopen buttons look the same as ones that work. Neither applies yet: there's no answer to accept, and it isn't closed. Tapping Settle did nothing and said nothing.
- **Put back:**
  - d2 and what it held for the lead deleted;
  - the numbering after your d1;
  - panel 1 the terminal, panel 2 the chat.

### The reviewers' points (reports word for word: `discussions-reviews/sp3-*.md`)

**Fixed (6f7b5b5):**
- **An agent's question to you couldn't be closed** (all seven reviewers). Agents asking you is in the plan: G.5 lists "a question asked of you". It's now closed this way:
  - You say your answer in it, then settle it; settling one asked of you no longer waits for an answer.
  - The agent that asked is told what you say.
- **The agent that starts a discussion wasn't told what's said in it** (responsibility 2, correctness 2, faithfulness 2). Both agents in it are now told: the one asked, and the one that started it. That includes the answer and the settling, to the one that started it.
- **An agent asking something back left it on the agent's move** (responsibility 3). Whose move it is now follows who spoke last: once the one asked has replied, it's the move of the one who started it. That's you, through the inbox.
- **Any agent could answer any discussion** (permissions 3, responsibility 4, correctness 3, faithfulness 11, tests). Now only the slot asked answers.
- **The inbox notified you once per discussion, ever** (correctness 1). There's now a notification each time it's your move anew, and the previous one is marked done.
- **`discuss` let any agent message any slot** (permissions 1). Asking a slot other than the lead (or you) now needs `slots.message`, which is the lead's.
- **A `#name` in what a slot is told handed on chats it can't read** (permissions 2). What an agent passes on names such a thing but doesn't hand it on ("not handed on: it's a chat this slot can't read"). Your own prompts are unchanged. Tags are now expanded when the message is told, so a held message keeps what it named then.
- **Slots were named two ways in a discussion** (responsibility 6, consistency 4, permissions 6).
  - A discussion now records who's asked by the slot's name, the same as who said what.
  - No two slots may share a name (refused when a slot is made).
  - One lookup by id or name (`SlotManager::named`) replaced two.
- **News lost when the event stream lagged** (correctness 4, performance 7): what's new since is now worked out from the discussions as they are, rather than dropped. Not fixed: a restart between keeping a message and telling it still loses that one (see Kept).
- **"Make it a change request" took two steps and told the agent the wrong thing** (correctness 5, faithfulness 12). It's now one step.
  - An answered question is reopened, with why, as a change request.
  - An open one gets a message saying what's asked now, so the agent is told.
- **The cockpit couldn't choose who's asked** (all but performance; a decided item). Starting a discussion now has a "Who answers?" step when there's another slot than the lead, with the lead listed first.
- **`#` did nothing in the palette's writing box** (responsibility 5, faithfulness 4, correctness 9, consistency 1). It now opens the tag picker, and the name comes back into what you're writing. This applies to a discussion's first message and to a tag's note.
- **A start let go of halfway stayed runnable** (responsibility 11, faithfulness 10, correctness 11, consistency 2). It's now gone once the palette closes without a pick. The same applies to a tag edit's picked tag.
- **The speech bubble was drawn as an outline** (responsibility 10, consistency 3). It's now in the solid icons' font (`theme::solid_font`), in the gutter and on the top line. Seen live: filled.
- **Typed text was lost when sending failed** (correctness 8). It comes back to its box, with what it was for.
- **"As it is now" could stay stuck reading** (correctness 10). A late read of another discussion is now ignored.
- **On a plan's graph, cards were the code's components** (faithfulness 5). A card there is now the plan's component, the one approval waits on: its notes are marked on it, and `C` starts a discussion on it.
- **Touch:**
  - A mark is tappable within a finger's reach.
  - A long press anywhere along a gutter row says every note on it.
  - A mark with several discussions asks which one to open.
  - On touch, a drag down the code's line numbers scrolls instead of selecting.
  - The palette's writing box has a Send button, for phone keyboards.
- **Smaller fixes:**
  - **Settle and Reopen** (seen live): a control that doesn't apply is drawn greyed and never runs.
  - **One wording each:**
    - who said something (`by_to_agent`, `by_to_user`);
    - what a message was and where a discussion stands (`SaidKind::words`, `DiscussionStatus::words`);
    - the naming grammar in the tools (`names_of_things`);
    - `capitalized` (consistency 7, 8);
    - "as it was when noted" (it was "when tagged", wrong for a discussion; faithfulness 9).
  - **The kind** is parsed from the wire, and labelled "A change request" (consistency 9).
  - **Key text:** the spec no longer names `C` in a message (consistency 5). The cancel action's Escape binding is gone: Escape already leaves typing (consistency 10).
  - **Stale docs** fixed in `net.rs` and the routes' header (faithfulness 16).
  - **One `LEAD` constant** (responsibility 6).
- **Performance:**
  - A whole session's text is built in one pass and capped at 4 MB (performance 1).
  - `discussion_read` reads only the start of what it shows (performance 6).
  - Reach reads the slots once per call (performance 9).
  - The review is read again only when its plan changed, and the plan tree is rebuilt only when the plan's own discussions did (performance 2).
  - The top line no longer copies discussions every frame (performance 4).
  - The discussion buffer reads its discussion where it's held instead of copying it several times a frame (performance 3).
  - The graph finds a component's cards from an index (performance 5).
  - The discussion buffer re-reads what it's on only for things the code or a plan says (performance 10).
- **Found while fixing:** after `Tab` in the discussion picker, its top line still showed the old thing. A picker whose action cycles is now read afresh each frame. Other pickers stay as they were asked, so an edit of a tag deleted meanwhile is still of that tag.
- **Tests added:**
  - the user answering and settling an agent's question, with the asker told;
  - a sub-slot asking: refused to ask another slot, refused to answer, told the lead's answer, a chat tag not handed on;
  - another slot's chat refused through every discussion tool;
  - names refused ("user", "you", one taken);
  - the inbox notifying each time;
  - the cockpit's start flow (who answers, `#`, Escape);
  - what it's on, as it is now;
  - a refused send kept;
  - both "change request" paths;
  - `Tab` while starting;
  - typing over a discussion panel, which replaces the Spec half that asserted nothing;
  - an item of another plan, gone to.
- **Tests removed or fixed:** a duplicate `#` assertion and a repeated date check are gone; the lead's report now waits 10 s.

**Decide (new):**
6. **An agent driving the cockpit can settle, reopen and make change requests as you** (permissions 5). This is part of #63 (deferred: an agent driving the cockpit acts as you), and approving a plan already works this way. My suggestion: the cockpit refuses the user's own actions (settle, reopen, change request, approve) when they come from an agent driving it.
7. **Settling, reopening, change requests and deletes need `plans.approve`** (responsibility 7; faithfulness "built but not asked for"). These are the user's alone either way. My suggestion: a capability of their own, `discussions.settle`, the user's only, so a discussion on a terminal isn't gated by a plan permission.
8. **A discussion asked of a slot that's closed** stays on that slot's move. On a plan component, that holds the approval until it's deleted (correctness 6). My suggestion: when a slot closes, its discussions go to the lead, which is told; or you may ask another slot from the discussion.

**Kept, with a reason (say if you disagree):**
- **A restart between keeping a message and telling it loses that message** (correctness 4, second half). Fixing it needs "told up to" kept per discussion: a store change.
- **Each discussion is drawn whole every frame** (performance 3). The chat draws its messages the same way. Drawing only what shows is for both, when a discussion gets long.
- **The chats copy the hub's replay when they lag** (performance 8). That's from step 6, and lag is rare.
- **Opening a discussion replaces what the quick look held, and doesn't add a jump** (responsibility 8, consistency 12). There's one quick look at a time, and closing it goes back to where you were.
- **An item card from another plan** (faithfulness 7): not reproduced. Going to one reads that plan (the desk too); a pinned card says to unpin it. Tested.
- **Whose move, in words:** "on lead" in the plan tree's compact rows, "waiting on lead" elsewhere (consistency 7, part).
- **Tests left as they are** (tests 4, 5): the route tests that repeat engine checks also check the routes' wiring. Wording asserts stay where the words are what an agent reads.
- **On touch, several lines can't be selected in the code.** Drag down the line numbers with a mouse.
- **A tag's dot on a graph card is 5 points.** It's now tappable within reach (6 points around it).

## Step 9: the first review (H)

### Checked against the plan

- **H.1 built:** `.nah/plans/tags-everywhere-review.json`, a review plan made from `tags-everywhere-findings.md`.
  - Its sections are the top items and its findings are nested under them, each with its state:
    - fixed: done;
    - decided, or kept with a reason: decided, with the reason as the item's text;
    - the rest: open.
  - Each item's text names the reviewers' reports it came from (`dev_docs/tags-everywhere-reviews/…`).
  - 73 items, 19 open.
  - It's a one-off conversion (a script kept outside the repository), not a tool. Like every plan file, it isn't committed (your rule: `.nah/plans/` stays out).
- **H.2 is yours:** going through it in the cockpit is this plan's "day in the life". Step 9 is ticked once you've done it.

### Found in the user's run (2026-10-21, ongoing)

Rough edges hit while going through the review plan. None is fixed yet.

1. **Picking a plan is only reachable from the graph.** `Space g P` is the
   graph's own menu, so it only exists while a graph panel has focus. To
   open a plan from the plan buffer or tree you first have to show the
   graph. The pick is on the graph because its source is the one setting
   every view reads; the user finds that odd and wants it discussed.
2. **The graph says "0 cards" for a review plan.** A review plan has items,
   not components, so the graph has nothing to draw and says so in its
   note. First impression is "the plan is empty".
3. **"filter…" on the plan buffer is a title search.** It matches titles
   and row ids only, not an item's state, its text, who raised it, or
   whether it has a discussion. The word "filter" promises more.
4. **No way to see what needs the user.** State is drawn but nothing reads
   it: no filter or grouping by state, no "needs me" view. The "Decide /
   Open / Done" top rows of this plan are hand-made nesting that doesn't
   follow state changes.
5. **"waiting on lead" with no sign the lead isn't running.** d2 (a
   question to the lead) showed "waiting on lead" while the lead had no
   session (ended by a server restart the day before). The message sat in
   the held queue. The discussion should say the slot has no session.
   The restart itself is a harness matter, in `TODO.md`.
6. **One saved layout for every browser.** The server keeps one `ui_state`
   row; the last browser to save wins, and a second browser (here, a
   headless one used for testing) overwrote the user's layout. Tied to
   Decide 1.

### Tried live

- **Opening it:** chose it as the graph's plan (`Space g P`), then the plan buffer. The plan tree shows the sections with their items nested under them, each with its state ("#1 Searching tagged text doesn't use FFF · done", "#3 Your existing tags lost their saved text · open"…).
- **An item's card:** Enter on #3 opened its card with its text. A first version showed the findings' `**` marks as they are, because an item's text is plain. They're taken out now, and the card changed live when the file did.
- **Put back:** panel 1 the terminal, panel 2 the chat (your draft untouched), the graph on the code. The plan file stays, for you.

## Step 10: README (I) — 0e24374

### Checked against the plan

- **The plans part:** review plans (items, their states, the plan buffer, an item's card), plus `plan_create` and `plan_edit` in the tools table.
- **The review part:** discussions as they are now, including:
  - starting one, and who answers;
  - the discussion buffer and its keys;
  - where they show;
  - the inbox;
  - the agents in it, and only the user settling.
- **The tags part:**
  - the new things: a whole session, a whole file, lines selected, a plan's card;
  - the gutter as it is now.
- **The agents' tools table:** `discuss` (who may be asked), `discussion_answer` (only the slot asked), and the tags' new names.
- **Also:** the buffer kinds (twelve). The spec's keys no longer list `c`, which is gone.
- **The docs and lint tests pass.**

## Stopping point 4 (the whole change, steps 1–10)

Reports word for word: `discussions-reviews/sp4-*.md`.

### Fixed (5d17878)

- **A question an agent asks you couldn't be settled from the cockpit** (consistency 1, faithfulness 1, correctness 1). The fix at stopping point 3 reached the engine but not the discussion buffer. Settle is now offered on a question asked of you while it's open; you settle it once you've said your answer. Tested in the cockpit.
- **A change request asked of you** settled without an answer naming what changed (faithfulness 5, correctness 1). Now a change request can only ask an agent slot, since its answer names what it changed. Making a question asked of you a change request is refused too.
- **Agents were named one way and checked another** (responsibility 1, permissions 3, correctness 2, faithfulness 2). A discussion now records agents by slot id.
  - Each agent's name, as it was when it was in the discussion, is kept with the discussion (`Discussion::names`). Every place that shows who said or is asked something uses that name, including a slot closed since.
  - So "Waiting on you" and "yours" now work for sub-slots and workers, not just the lead.
  - A new slot that reuses a closed one's name inherits nothing.
  - Tested with a sub-slot whose name differs from its id.
- **A slot could be asked about something it can't read**, and then could never answer (responsibility 2, faithfulness 3). Asking now checks that the slot asked can read the thing, in both the route and the tool.
- **Saying something in a discussion prompted its agents' sessions**, even from an agent not in it (permissions 1). Now only the one asked, the one that started it, or a slot with `slots.message` may.
- **Asking you needs `user.notify`** (permissions 5).
- **Queue briefs handed on what the worker can't read** (permissions 2). They're now expanded as the worker reads, since agents write what a brief is made of.
- **A slot named "the user" could pass for you** (permissions 4). "the user" and "someone" are now refused as slot names, as "user" and "you" already were.
- **A slot gone meanwhile:** the prompt fallback hands on nothing of what the slot would be refused (responsibility 5).
- **An agent could ask itself** (correctness 4). It's now refused with "ask the user, or another slot".
- **A change request was answered by naming the whole plan** (correctness 5). The plan only counts for a request on the plan itself. Tested.
- **Notes on a chat turn read as gone after the session reloaded** (correctness 3). A turn id the session doesn't know (its turns were replayed with new ids) now falls back to the turn's number.
- **Text that failed to send:**
  - It came back in another discussion's box (correctness 6); now it only comes back to the discussion it was written for.
  - A start the server refused lost the first message (correctness 7, tests 8). The palette now reopens with it, to send again. Tested.
- **`#` typed in the middle of what's written opened the tag picker** (correctness 9). Now only a `#` typed at the end does.
- **Comments in a plan file moved only at start-up** (responsibility 3, correctness 8, permissions 7, tests 2). They now move as each plan file is read, as E.1 says: a file a branch brings in is moved when it's read. A moved comment is recognised by its id and when it was written, so another branch's `c1` isn't taken for this one. Tested.
- **Searching discussions' text wasn't reachable** (responsibility 10, faithfulness 6). The `discussions` tool takes `find`, limited to what the caller may read. Tested.
- **No keys to open the discussions at the cursor** (consistency 2). `discuss.open` is offered wherever there are discussions on what you're on, and a click on a mark runs the same action. It has no key yet: see Decide 14.
- **The plan tree didn't count a component's relations' discussions** (faithfulness 9). It does now.
- **Whose move was said four ways** (responsibility 7). It's now said one way (`conversation::whose`): "waiting on you", "waiting on lead", "settled".
- **"user" was declared in several places.** There's now one constant (`harness_contracts::USER`), and the routes use it (consistency 7).
- **Performance:**
  - A note on a whole chat session no longer formats a string per item per frame (performance 3).
  - `touches` and `same_thing` no longer copy subjects (performance 2, felt every frame).
  - The discussion buffer's things, target, title, status and actions read their discussion in place (performance 4).
- **Tests:**
  - **Added:**
    - a discussion round-trips through the state database whole: messages, what changed, names, where it came from (tests 1);
    - a plan item started from its card (G.7);
    - a sub-slot sees what's waiting on it;
    - outsiders can't reply;
    - two slots sharing a name;
    - settling a question asked of you in the cockpit;
    - opening a discussion by its action.
  - **Restored:** the two assertions step 1 had dropped (tests 3).
  - **Fixed:**
    - the layout test, which now types into the box rather than asserting its own setup (tests 4);
    - the fold test's waits and its echo check (tests 7).
- **#6** is marked decided in the tags findings, and the review plan was made again (faithfulness 8).

### Decide (new)

9. **Whose move it is after the agent asked replies** (faithfulness 4). D.1 says "the agent's when it's open". At stopping point 3 I made it follow who spoke last: once the agent asked replies, it's the move of whoever started it. That's the only way an agent's question back reaches you. Keep it?
10. **Who may ask or reply** (faithfulness 4, permissions 1). F.2 says starting needs `discuss` only. As built:
    - asking a slot other than the lead needs `slots.message`;
    - saying something in a discussion you're not in needs it too;
    - asking you needs `user.notify`.

    Otherwise any agent could prompt any other's session. Keep it?
11. **Slot names** (faithfulness 4). At stopping point 3 I made names unique; that's undone now that agents are tracked by id. Still built: no slot may be named "user", "you", "the user" or "someone".
12. **An agent can't ask itself.** The lead, which is asked by default, must name the user or another slot.
13. **A search over some logs** (performance 1, consistency 3). FFF can't be told which files to search, so `tag_find` on one tag greps every log and filters afterwards. My suggestion: read only the asked logs directly when they're a few. That goes against "search uses FFF", hence this question.
14. **A key for opening the discussions at the cursor** (`discuss.open`). It's offered in the palette and runs from the gutter mark. Suggestion: `g D`.

### Kept, with a reason

- **The approval rule fails open when no blocker is set** (responsibility 4). The server always sets one; only arch-service's own tests run without discussions.
- **The tools' naming grammar is written once more beside the owners' parsers** (responsibility 6). Tool descriptions are text; one function now writes it.
- **The cockpit holds the review's discussions in more than one place** (responsibility 7). It's one reader feeding the review and the plan tree, which each keep a copy, as every buffer does.
- **`land` names the discussion buffer, and the gutter hands its ids on a desk field** (responsibility 8). It's one place each, cleared when the palette closes.
- **The discussion buffer decides which things the architecture changes** (responsibility 9).
- **The routes' error helpers look alike** (consistency 7). Each maps its own error kinds to statuses.
- **The palette's Send is its own button** (consistency 4). The palette is a host, as with its rows.
- **"Stop" in the discussion box** (consistency 5). A phone has no Escape.
- **The time parser for old comments** (consistency 6). Those times were written in UTC.
- **The cockpit acts on discussions by action name** (consistency 7). It's one call with the route's name.
- **The engine holds its lock across the database write** (performance 6). That's for correctness first.
- **Untagging re-reads every note** (performance 7). Low.
- **Finding opens files per line** (performance 8). Low.
- **Re-reading what a discussion is on** (performance 9). Only for code and plans.
- **Discussions never leave the desk** (performance 10). That's tied to Decide 2.
- **The gutter asks every view about every discussion** (performance 5). It's one lookup each now.
- **An agent's question can hold an approval until it's answered or deleted** (permissions 6). That's the plan's rule. Deleting is Decide 2.
- **Tests left:**
  - duplicate engine checks in the end-to-end tests (tests 6), which check the wiring;
  - wording asserts where an agent reads the words (tests 5);
  - `]c` stepping through discussions, a view.read-only role, code lines followed through the server, and a chat turn kept after its slot closes (tests 8);
  - the terminal test's 2 s sleep (tests 7).
- **`.nah/plans/` isn't committed** (faithfulness 8). That's your instruction for this work: plan files stay out of commits. That includes the review plan.

### Tried live

- **Started d2** on `src/main.rs lines 2-4` through the route. It records `asked: lead` by id, with names `{lead: lead}`.
- **The palette** offered "Open a discussion here" with d2. Running it opened d2: "OPEN · d2 · asks lead", "waiting on lead".
- **Put back:**
  - d2 and what it held for the lead deleted;
  - the numbering after your d1;
  - panel 1 the terminal, panel 2 the chat (your draft untouched).

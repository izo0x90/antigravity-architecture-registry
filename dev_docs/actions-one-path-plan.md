# Plan: one path for every action

Agreed 2026-10-16. Built 2026-10-16 (3673ff5, c9afbd9, eadd733, 3f4b3f1). Comes before the scrolling and Changes
fixes (`scrolling-and-changes-fixes-plan.md`), whose folding step builds on
it.


## What's wrong

Every action reaches the screen through one list per frame: gathered,
bound to its configurable keys, given a layer (what you're on, this panel,
what holds it, everywhere), and drawn by whichever surface shows that
layer. That part works.

But actions get into the list two ways:

- **A view's own actions** (Stage, More context, Compare…) are declared by
  the view: its label, when it applies, where it's shown. They work.
- **The moves every view shares** (up and down, Enter and back, folding,
  the go-tos) are made by the shell on the views' behalf, in
  `app/commands.rs`. The shell guesses what applies and what to call it,
  and tags where each belongs by hand. Untagged, it falls back to
  "everywhere", which the key bar never shows.

The second way breaks the separation the first one has. The shell ends up
knowing things only the views should know. Here's everything it decides
that isn't its to decide:

1. The fold actions were never tagged, so they count as "everywhere" and
   never show in the key bar, in any view.
2. Enter and back get a fixed label per kind of view from the shell, which
   each view can override through a separate question ("what's your Enter
   label?"), with two more questions for whether Enter and back apply.
   Folding never got its questions.
3. The labels for left and right and for next and previous sibling are a
   list in the shell, per kind of view.
4. Which go-tos are offered is decided by the shell looking into other
   views: it asks the queue whether the component has a job and the
   Changes view whether the changes touch it (I added that one in the
   timeline step: the same mistake again), and it hard-codes which views
   skip which go-tos ("not the graph's 'show in the graph'").
5. "Re-read <file>" is a shell action that reads the code view's file.

The layer itself is an optional field with a silent default. Nothing tests
that an action that applies is actually shown.


## One shape, stacked

Every layer of where you are is the same kind of thing: what you're on,
the panel (a view), what holds it (its column, or the quick look), and the
shell. Each one answers the same questions the same way: which actions do
I offer here, in my words, and which of them do I place as controls. They
stack, innermost first. An action's layer is simply which level of the
stack offered it, so there are no tags to set and nothing to forget. Keys
resolve closest first, the bars draw their level, and the contextual
actions you see come from stacking, not from special cases.

The parts below are just what each level knows.


## The parts and what each is responsible for

**A view** knows its content and its cursor. It's the only one that says
what can be done to what its cursor is on: its own actions *and* which
shared moves apply there, what each one does there in words ("Fold this
change", "Up to its file"), and whether it acts on the item or the whole
panel. It says this through the one action list, like everything else. It
never says keys.

**The shared moves** are a fixed vocabulary in `nav.rs` (down, up, left,
right, half page, sideways, first, last, next and previous sibling,
Enter, back, the folds, the go-tos). Each has a fixed id the keymap binds
(`nav.fold_toggle`, `goto.code`… as today, so your keys don't change). How
each one behaves is written once and shared: counts, the jump list, the
scrolling rules, calling into the view. A view offering a move doesn't
implement its bookkeeping; it only says it applies and what it does.

**The hosts** (a column, the quick look) are the level around a panel:
closing a column, its tabs, sending the quick look's card to a column. Same
shape as a view.

**The shell** is the outermost level, the same shape again, offering what
is really its own: views and layouts, the palette, servers, the inspector,
what needs you, the index. It no longer makes actions about a view's
content and never matches on the kind of view to decide labels or what
applies.

**Going somewhere else** (the go-tos) couples two views, and it couples
them in one place, through what the cursor is on (`Target`), which already
exists:

- The view you're in says what its cursor is on.
- Each view you can go to says whether it can show that: the queue, whether
  the component has a job; the Changes view, whether the changes touch it,
  or a message's turn was seen; the spec and the graph, whether it's a
  component they know. One question every destination answers the same
  way.
- A go-to is offered when there's a target and its destination can show
  it. "Not to yourself" is a general rule, not a list.

**Gathering** stays the one function that builds the list each frame: it
walks the stack, innermost first, and each action's layer is the level
that offered it.

**The surfaces** (the key bar, the `?` sheet, the leader menu, the
palette, the panel's top line, the controls on an item, the top bar) stay
as they are: they draw their layer from the list and don't know where an
action came from.


## Where correctness is guaranteed

At the points where the parts meet:

- **Views and the list.** A test runs every view in a set of states, with
  the cursor on each kind of thing it shows, and tries every shared move.
  Anything that would change something there must have been offered, with
  the view's own label, and must reach the key bar (if important enough),
  the `?` sheet and the palette. Anything offered must do something or say
  why not. That's exactly the test that would have caught folding.
- **The levels.** Each level is reached only through the shared shape; an
  action can't exist outside a level, so it can't land in the wrong one. A
  lint, like the one that keeps controls in the hosts, fails if the shell's
  or a host's code matches on the kind of view or reads a view's state to
  label or offer something.
- **Go-tos.** A test per destination: given each kind of target, it says
  whether it can show it, and the go-to is offered exactly then.
- **Keys.** The pinned list of actions and their keys stays, so this
  refactor can't move your keys by accident; the ids don't change.


## What gets deleted

As a clean cut, in the same commits as their replacements:

- The shell's shared-move actions and its per-kind label tables.
- The questions views answer one move at a time (can Enter, Enter's label,
  can leave, back's label) and the "Enter applies" / "back applies"
  conditions.
- The optional layer field, its "everywhere" default and every hand-set
  layer tag: the level that offers an action is its layer.
- The shell's look into the queue and the Changes view for go-tos, and the
  per-view go-to exclusions.
- "Re-read <file>" as a shell action (it becomes the code view's).


## Steps

Each tested and committed on its own:

1. **Destinations answer for go-tos.** Every view that can be gone to says
   whether it can show a target; the go-tos ask that instead of the shell
   looking inside views. Tests per destination.
2. **Views offer the shared moves.** The shared-move vocabulary, its
   shared behaviour, and every view offering its moves with its own labels
   (code, outline, spec, graph, Changes, chat, queue, inbox). The shell's
   versions, the per-move questions and the per-kind labels go in the same
   commit.
3. **Every level has the same shape.** The hosts and the shell offer their
   actions through it like the views; layers come from the stack and the
   tags go. "Re-read <file>" moves to the code view.
4. **The checks.** The test over every view and state, the lint on the
   shell, the go-to tests. Run in the cockpit: folding shows in the key bar
   and on items in every view that folds.

Then the scrolling and Changes plan continues from its step 2, with its
folding step reduced to the Changes view offering folds for files and
changes.

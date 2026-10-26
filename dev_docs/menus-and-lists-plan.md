# Menus and lists: tappable menus, one list, a calmer screen

## Why

- Buttons on every layer (the whole screen, the panel, the view) each show
  their full shortcut ("Space a c Finish signing in"): in narrow columns
  that's rows of controls, busy and hard to read. They're there so touch
  works and keys are learned; that has to stay.
- The action list (`:`) is almost right but built as a one-off: three
  copies of its list loop, its own small set of movement keys, a highlight
  the mouse and keyboard don't share, and flicker while typing.
- The `?` key sheet is a fourth list: a centered window that moves its
  filter box as you type, matches by exact text, has no keyboard movement,
  and can't run anything.

## Decided (with the user, 2026-10-24)

1. **Tappable menus from the which-key tree.** The keymap is already a tree
   (Space, a group letter, an action letter). The top of each panel shows
   only the groups as buttons (Buffer, Session, Account, Model, ...).
   Tapping one shows its actions: the same list that pressing Space and its
   letter shows. Keyboard, mouse and touch walk the same menus.
   - Inside a group, buttons show only the next key ("y Copy link").
   - A path shows where you are ("Account ›"); tap it or Escape to go back.
   - A group shows when something in it can be used here. Nothing decides
     what's "important".
   - What needs you now stays with what it's about (a request's answers on
     its card, a sign-in's buttons on its banner), not in the menus.
   - Long-press on touch shows what a button does and its key, as hover
     does with a mouse.
2. **One list for the action list, its pickers and `?`.** One list piece:
   rows (label, description, keys, colour mark), one highlight that hover
   and tap move too, the movement every view has (Ctrl+J/K, Ctrl+N/P and
   the arrows, Ctrl+D/U half a page, first and last), scrolling to the
   highlight, Enter or tap to run. The palette keeps only its stages
   (search, choose, write) and its box.
   - The shared fuzzy matcher everywhere (the action list already uses it).
3. **`?` is the action list showing everything.** `:` (or Space /) shows
   what can be used here; `?` shows everything: what can't be used now
   (faded, with why when known), the current mode's keys, mouse and touch
   gestures, grouped under the headings the sheet has now (on what you're
   on, this view, this panel, everywhere). A key in the list (Tab) switches
   between the two. The separate key-sheet window goes.
4. **A box that doesn't move or flicker.** The list's box has one size,
   whatever matches; rows are laid out at a fixed height, without egui's
   invisible measuring frame; only visible rows are drawn. Measured in the
   user's tab before and after (rapid screenshots while typing).
5. **The searchable list and the menus show the same actions the same
   way**: browsing (menus) and searching (the list) read one set.

## Status

- Step 1: done (eda7934), then its checkpoint review and re-read (a7f349c,
  c464f49).
- Step 2: done (856f36c).
- Step 3: done (17d4757).
- Step 4: done (03c4a3e): the README, a phone-sized test of the menus,
  the suite, then the end review (all eight).
- End review fixed in four parts (2834215, eb76087, dceb59c, 1694b22):
  step 3 as decided, the action list, the code search, tests and words.
  Then two reviewers re-read once. All of it is in
  `menus-and-lists-findings.md`, with what's left for the user under
  "Decide".

The live checks were in the user's tab, except at phone width. The relay
doesn't let the window be narrowed, and emulating a phone there isn't
faithful (the canvas keeps the old pixel ratio). The phone-sized app test
stands in for it; that's under "Decide" in the findings.

## Order

0. The sign-in flow first (`sign-in-flow-plan.md`): its keys go in the
   Account group, which the menus then show.
1. Measure the flicker live (findings below); the one list piece, used by
   the action list and its pickers; flicker fixed; measured again.
2. `?` folded into the list (everything, faded, mode keys, gestures,
   headings); the key-sheet window removed.
3. The tappable menus on each panel's top line; key labels inside a group
   only; long-press.
4. README, key sheet and layout snapshots, suite, lints; live in the
   user's tab (keyboard, mouse, a phone-width window); reviewers; fixes;
   findings in `menus-and-lists-findings.md`.

## Not in this

- Selecting, copying and pasting in general (TODO: to investigate).
- The chat's own layout (items drifting left) and the agent not driving
  the graph (TODO).
- The buffers rework (TODO): which buffers a panel offers.

## Flicker: what was measured (2026-10-24, the user's tab, 1278×1352)

Every frame recorded (CDP screencast) while opening the list and typing
"sign" then four backspaces, a key every quarter second.

- **The `?` key sheet jumps on every key.** It's an egui window anchored at
  the screen's center that sizes itself to what it held the frame before.
  Each key: the list's length changes, the window first draws at the old
  size and place, then moves and resizes over the next frames. Its top edge
  (and the filter box in it) went from near the top to the middle and back
  as matches came and went. That's the "accordion".
- **The action list (`:`) held still while typing**: same place, same
  size, its rows the same across a keystroke's frames (it keeps its tallest
  height and is anchored at the top). On opening it fades in over about
  four frames (egui's fade-in for pop-up areas), its rows showing a frame
  before its title and field.
- Not measured: a narrow window (under 600 px it's placed from the bottom),
  the pickers that search the server (their results arrive later and
  replace the list).

- **The `@` file picker flickers on every key** (the user saw it; the code
  says why, `app/linking.rs` `find_links`): each key empties the list at
  once and asks the server twice (files, symbols); the files' answer shows
  alone, then the symbols' is added. So each key draws: the old rows,
  nothing, the files, files and symbols. Fix: keep the rows on screen until
  both answers for the newest key are in, then replace them in one step
  (the waiting state shown quietly, not as an empty list).

So: the jumping is the `?` window's (step 2 removes it); the action list
needs a fixed size from the first frame and no fade (step 1), and the
server-searching pickers keep the list they have until new results come.

## Step 3: how it's built (2026-10-24, decided while the user was away; reversible)

- **A panel's top line** shows its groups as buttons, then (as now) the
  busy mark, its title, its status and its notes. The settings row
  (View, Source, Harness, Mode…) stays: each shows a state, and is in its
  group too.
  - Its groups are the leader's top-level groups that are about where you
    are (their closest action is what you're on, the panel's view, or its
    host; not everywhere's), with at least one action that applies now.
    The view's own menu (`Space ,`, "Graph actions") is one of them.
  - Each button is a control like any other: the group's name and the
    keys that open it ("Graph actions  Space ,"), the keys unsaid on a
    panel not focused.
  - The groups that act on the whole screen are the top bar's: its Menus
    button opens the leader's own menu. A view places top-line controls
    only as settings; the top bar has its own placements.
- **Off the top line, into their groups:**
  - the panel's buffer tabs and its close (Buffer `b`, Layout `l`);
  - the quick look's own (Quick look `q`);
  - a view's top-line buttons (the view's menu);
  - Stop (Session `s`, until the chat's live line, chat plan step 2, gives
    it a place on the turn).
- **What needs you now, on what it's about.** The chat's banners (a
  sign-in, its end, a notice, an error, "no session yet") carry their own
  buttons. This uses the mechanism that puts a request's answers on its
  card:
  - the banners are a place of the chat's ("banners"): `here()` names it
    when nothing else is waiting and the cursor follows the latest, and
    `drawn_at` says where it is (it isn't among the spots, which hints
    label);
  - their actions are item controls (Sign in, Copy link, Open the sign-in
    page, Finish, Stop signing in, Put away, Retry, New session, Resume).
  - The same holds in the other views: a discussion's Settle and Reopen
    under the latest said, the inbox's Open and Put away under what's
    said, the graph's "Show N levels" under its size note while no card
    is selected. Buttons with no such place (a queue's start and stop,
    the tree's clear filter, the chat's Stop) are in their menus.
- **Tapping a group button** focuses its panel and opens the which-key
  menu at that group, as Space and its letter do: one way in
  (`menu.open`), by key or tap.
- **The which-key menu.**
  - Its title is a path ("Space › Account"), the same as the top bar's
    mode; each part is tapped to go back there, and Escape goes back one.
  - Its entries show only their next key (as now); each entry, key and
    label, is one tap target.
  - A long press on an entry or a group button (a finger held still)
    shows what it does and its keys: egui's tooltip, as hover does.
- **Not in this step:** the top bar across the screen (its own buttons:
  index, what's waiting, panels, inspector). Recorded under Decide.

## Checkpoint after step 1: what changes (2026-10-24)

The three reviewers' reports are in `menus-and-lists-reviews/checkpoint-*.md`.
Step 1 (eda7934) is reworked as follows before step 2 builds on it.

- **The list moves and scrolls as every view does.**
  - Its moves are `nav::Motion`. The list's `Move` type goes, and a
    `viewport::Go::of(motion, count)` replaces the same match written out
    in five views.
  - Where it looks is a `viewport::Viewport` over stops of one height, as
    the tree does. That gives half a page that moves the view and the
    highlight together, room around the highlight, a scroll by hand
    respected by the next key, and the glide.
  - The list's own rule stays: one down from the last goes round to the
    first.
- **The list owns its whole job.** It takes the move with the one length
  it draws. Its empty note ("No matching actions", "Looking…") is drawn
  inside its fixed height, so the box keeps one size. Hovering moves the
  highlight before any row is painted, so two rows are never lit in one
  frame.
- **One search of the code index as it's typed**, for `@` and the
  graph's find:
  - One round per query, with at most one round out at a time; what's
    typed meanwhile is asked next.
  - A round counts as answered once every kind has answered, or once its
    time is up (said).
  - What's shown is the last round answered whole, in one order: symbols,
    files, lines.
  - The rows are worded one way, and a failure is said the same way.
  - The graph's find stops emptying its list on every key.
- **One contract for a search's rows.** `Param::Search` says which query
  its rows are of. The palette compares that with what's typed: rows of
  something else mean Enter waits and the key line says "Looking…".
  "Looking…", "Type to search" and "Nothing found" describe the rows
  shown. The palette's own guesses go.
- **Words and keys.**
  - Rows use `Choice`'s names (`colour`, `unavailable`).
  - The key line says the half-page and first/last keys.
  - The list keys' bare serde defaults go.
- **The panel's setting dropdowns** draw their choices with the same list
  (the tick, the description line), not a second way.
- **Not changed, and why.**
  - The list keys stay their own table (`[keymap.list]`). A list's box is
    a typing box, so its keys are the ones a box doesn't type, as
    `[keymap.input]` has its own. They now name the same moves
    (`nav::Motion`).
  - The action list re-sorts and hashes its actions each frame while open
    (from before step 1). That is recorded with a measurement, not
    reworked here.

## Reviews (agreed 2026-10-24)

Reviewers: `process/reviewers.toml` (seven) and anti-patterns (its brief in
`TODO.md`). Checkpoint reviews: three reviewers on that step's changes only.
End reviews: all eight on the whole plan. After fixes, two reviewers re-read
the fixes once; what they still find goes in the findings file, not another
round. Reports word for word in `<plan>-reviews/`.

- Checkpoint after the one list piece (order step 1): responsibility,
  consistency, performance. The action list, its pickers, `?` and the menus
  all build on it.
- End: all eight.

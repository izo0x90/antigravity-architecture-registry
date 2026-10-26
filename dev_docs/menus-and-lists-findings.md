# Menus and lists: review findings and what was done

## Checkpoint after step 1 (eda7934)

Three reviewers looked at step 1: responsibility, consistency and
performance. Their reports are word for word in
`menus-and-lists-reviews/checkpoint-*.md`. Several findings came from more
than one reviewer, so each is listed once below.

### Fixed

1. **The list had its own moves and its own scrolling** (responsibility 1,
   consistency 2 and 3).
   - The list's `Move` type is gone. Its keys give `nav::Motion`, the
     vocabulary every view uses.
   - `viewport::Go::of(motion, count)` now holds the motion-to-move mapping
     that eleven views each wrote out as six match arms. All eleven use it.
   - Where the list looks is a `viewport::Viewport` over stops of one
     height, as in the tree. So:
     - half a page moves the list and the highlight together;
     - a row of room is kept around the highlight;
     - a key after a wheel or finger scroll starts from what shows;
     - moves glide.
   - The list keeps its own rule: one down from the last goes round to the
     first.
2. **Two ways to tell whether a search's rows are current, and the graph's
   find said "Nothing found" while looking** (responsibility 2, consistency
   1).
   - `Param::Search` now says which query its rows are of (`of`). The
     palette compares that with what's typed. Its own guesses
     (`rows_of_nothing`, reading the ask) are gone.
   - "Looking…", "Type to search" and "Nothing found" now describe the rows
     shown.
3. **Two copies of the code search, and only one kept its rows**
   (responsibility 3 and 7, consistency 1). `code_search::CodeSearch` is
   now the one search of the code index as it's typed, for `@` and the
   graph's find:
   - one round per query, with at most one round out at a time; what's
     typed meanwhile is asked once it's back;
   - a round is answered when every kind has answered, or after 8 seconds
     (said);
   - what's shown is the last round answered, whole, symbols then files
     then lines;
   - the rows are worded one way, with an icon per kind;
   - a failure is said the same way in both pickers.

   The graph's find no longer empties its list on every key. Turning
   answers into rows happens in one place per picker.
4. **The empty note sat outside the list, so the box changed size**
   (responsibility 4, consistency 5). The note is `Listing::empty`, drawn
   inside the list's fixed height (tested: the same height with no rows).
   The list now takes the move with the one length it draws, so the copied
   glue in the palette is gone.
5. **Enter could wait with no end, and nothing said so** (performance 1).
   - A round counts as answered after 8 seconds, so a stalled request no
     longer holds Enter.
   - With one round out at a time, fast typing sends one round per answer,
     not one per key.
   - While an Enter waits, the key line starts with "Looking…".
6. **Two rows lit for a frame when the pointer moved down** (performance,
   outside its brief). Rows are laid out first, the highlight follows the
   pointer, and only then are rows painted.
7. **A late answer to a picked round was still taken** (performance,
   outside its brief). `CodeSearch::clear` drops the round out, so its
   answers are dropped.
8. **Allocations per row** (performance 2).
   - The keys are laid out from the owned string, not a copy.
   - The label is copied only when it gets a tick.
9. **The new list keys were never said** (consistency 4). The key line, the
   key bar's mode keys and the key sheet now say "ctrl+D/ctrl+U half a page"
   and "ctrl+Home/ctrl+End first, last".
10. **Bare serde defaults on the list keys** (consistency 6). Removed. The
    config is always laid over `default.toml`, and the built-in defaults
    give every move a key.
11. **`Row` renamed `Choice`'s fields** (consistency 7). It's now `colour`
    and `unavailable`. `Row::of_choice` is the one mapping.
12. **The panel's setting pickers drew choices a second way** (consistency
    8). Their popup is now the same list:
    - the tick on the current choice and a description line;
    - the highlight starts on the current choice;
    - it's sized to its rows.

    Tested: a choice is ticked, described, and picked by a tap.

### Not changed, and why

- **The list keys are their own table** (responsibility 6). A list's box is
  a typing box. Its keys must be ones a box doesn't type (no `j`, `g g`,
  `G`), just as `[keymap.input]` has its own. They now name the same moves
  (`nav::Motion`).
- **Headings and rows that don't apply in step 2** (responsibility 5). The
  list now takes the moves itself (`show` gets the motion), so step 2 can
  skip headings there without changing the callers. Headings will come with
  step 2, which needs them.
- **The action list's whole-list work each frame while it's open**
  (performance 3, from before step 1). There are 147 actions here. Each
  frame does:
  - one sort (about 1,000 comparisons);
  - one fingerprint;
  - one hash of their text (about 15 KB).

  Estimated at tens of microseconds; not timed. Worth keying on a
  generation number once actions carry one; not done here.
- **Choose and Search stages copy their rows a few times per frame**
  (performance 4, from before step 1). The lists are small: up to 24 found,
  and the tags there are. The tag picker's per-frame ranking is the
  largest. Recorded for the buffers rework (TODO), which reworks how
  actions are gathered.

### Live (the user's tab)

- **The action list while typing "sign" and four backspaces:** the box never
  moved, and each key changed the rows once.
- **The moves:**
  - Ctrl+D twice moved the list and the highlight together;
  - Ctrl+End went to the last row;
  - Ctrl+J from there went round to the first, scrolled back up.
- **The `@` picker and the graph's find, typing "tree" and back:**
  - "Looking…" until the first answer;
  - after that, no frame with an emptied list; each change was one step.
- **The graph's View picker:** opened with "calls" ticked and highlighted,
  each choice described.
- **The bottom key line:** names all the moves.

## The re-read of the checkpoint fixes (a7f349c)

Two reviewers, once: `menus-and-lists-reviews/checkpoint-reread-responsibility.md`
and `checkpoint-reread-correctness.md`. No further review round. What they
found that was real was fixed in the same pass. Where both found the same
thing it's listed once.

### Fixed

1. **A waiting Enter picked a row by position from rows never seen.**
   - A search's highlight starts again whenever its rows change.
   - Enter on a row chosen on the rows shown (by a key or the pointer) is
     taken at once: it's on screen, and it's a real result.
   - A plain Enter after typing waits for the rows of what's typed.
   - Typing after that Enter drops it.
   - Tested.
2. **A list came back where its scroll area was left** (one viewport for
   the palette's three areas; reopening after Ctrl+End). Every list now
   starts by showing its starting row:
   - the top for a new or reset list;
   - for a choice list, the current choice;
   - this frame's window and row count are given to the viewport before
     any move.

   So a key's move before the list was ever drawn lands right. Tested, and
   checked live: reopening after Ctrl+End shows the top.
3. **The graph's find from a view with no graph on screen never asked**
   (and Enter waited for good). A view searched from the action list is
   tended while its search is open, whether it's shown or not. Tested
   from a code-only layout.
4. **The setting popup.**
   - It's sized to its rows, so egui's own scroll area never engages: no
     nested scrolling.
   - It opens showing the current choice.
   - It takes the list keys and Enter, and the panel's keys stand aside
     while it's open.
   - Tested, and checked live: Ctrl+J moved within it and the graph
     didn't move.
5. **Rounds left behind.**
   - The `@` search is dropped when its picker closes.
   - The graph's find is cleared once a row is picked.
   - Rows from before a search was opened are never shown in it. Tested:
     reopening the graph's find shows nothing of the search before, from
     its first frame.
   - Clearing the field, or typing back to the query the rows are of, is
     answered at once and drops the round out.
6. **Going round at the ends ignored a scroll by hand.** It now goes from
   what shows, as every view does.
7. **A key's move could be overridden by a twitching pointer** in the same
   frame. The key wins.
8. **Choices started on row 0 in the action list but on the current one in
   the panel.** Both start on the current one.
9. **A pick in the frame the next round came found nothing.** The round
   shown before is kept for that.
10. **The graph listed its search kinds a second time.** It asks the kinds
    its search names.
11. **The "N of M" note could be lost with a row left out.** It goes on
    the last row listed.
12. **"file · file" for a file nothing stands for.** Now just "file".
13. **`Go::of`'s catch-all would hide a new motion.** It names every
    motion, so a new one has to be decided there, for every view.

### Recorded, not changed

- **Step 2 will extend the list's core** (responsibility 7). Today rows
  have one height, every row is a stop, and `Row` has no kind (a heading,
  a row that runs, a row that only informs). Step 2 needs headings and
  rows that don't apply. It will add a row kind, skip headings in moves,
  and say why when Enter lands on a row that doesn't apply. The callers
  won't change. The piece itself will, and that's step 2's work.
- **Per-frame costs left:**
  - the graph's find rows are built in every gather while a search is
    shown (at most 26 rows; cleared on a pick, or when it's next opened);
  - one small `Vec` per frame and two font lookups while a list is up;
  - the Search stage's two row copies (performance 4 above).

  All small at these sizes.

## End review (all eight): what changes (2026-10-24)

The reports are word for word in `menus-and-lists-reviews/end-*.md`. Permissions
found nothing. The others agree on one thing above all: step 3 was built
another way than its own design, without saying so. In particular, "what
needs you now" went to a new row (`At::Notice`) instead of onto what it's
about (the user's decision: "a sign-in's buttons on its banner").

The fix pass builds what was decided, in four parts:

1. **Step 3 as decided.**
   - The chat's banners become a spot, and their buttons are item controls
     (the mechanism a request's answers use). Other views' buttons go on
     their items, or into their menus. `At::Notice` goes.
   - A view can't place a top-line button no host draws: the top bar gets
     a placement of its own, and `header` takes settings only. The graph's
     chips and the other dead placements go.
   - **Every group with something usable is shown**, on the line of where
     it acts: the panel's on the panel, the whole screen's on the top bar.
   - Group buttons are controls like any other, saying the keys that run
     them ("Space a").
   - The menu is entered one way, by key or tap.
   - A focus change drops the frame's stale actions, and a click focuses
     only the panel it landed on.
   - The long press uses egui's tooltip while held, and the cockpit's one
     touch recognizer.
   - Faster: the key cache is kept per host, each view's situation is
     worked out once a frame, and the groups are found in one pass.
2. **The action list.**
   - Its scope and the go-to-line row belong to its stage and its listing.
   - The mode's keys come from where it was opened (`sheet_mode` goes).
   - A row's kind can be asked without building the row.
   - A faded row that takes a value says why.
   - One key line for the list and the key bar.
   - Nothing is built while it's closed.
3. **The code search.**
   - One driver.
   - A picked row carries what it stands for (no round lookup, no
     `before`).
   - Cleared when its picker closes (no `answered`).
   - A hidden view's search is tended alone.
4. **Tests and words.** What the tests review found, the stale comments,
   and "action list" as the one word on screen. Then live checks and
   measurements in the user's tab, and two reviewers re-read once.

### Part 1 done (2834215)

Built as listed above. Found while building it, and fixed in the same
commit:

- **The banners as a hint spot would get a dead hint label.** Hints label
  every spot, and "banners" is no item to move to. A place that's no item
  is now said apart (`Hosted::drawn_at`); the graph's size note is one too.
- **The graph's size note is where you are when no card is selected**,
  judged from the answer (it was cut), not from where the note was drawn
  last frame, so its button shows the frame the note does.
- **The top bar drew a long mode under its right side** (seen live: "Space
  › Graph actions" over "Panels: 2"). The right side is now laid out
  first, in the room the left didn't need last frame (the mode counted at
  its shortest), and the mode is cut short in its place, whole on hover.
  So the bar keeps its height and nothing on it moves when Space is
  pressed. Tested at 1300 points, checked live at the user's width.
- **Tests that read a removed top-line button** now read where it went:
  the spec's Re-read under its code, the size note's fix under the note.

Checked live in the user's tab:
- the chat's "no session yet" banner with New session and Resume… under it;
- a tap on the graph's "Graph actions" focused its panel and opened its
  menu, with the same path in the menu and the top bar.

After a reload, the saved layout lands with the first input, so a tap in
the first moments can hit the panel that moves there. That's the restore
order noted before, not this change.

### Part 2 done: the action list

Built as listed, plus two findings the list above didn't name:
- **The setting list's keys** (anti-patterns 9, responsibility 10). The
  shell stood aside for any egui popup. A setting's open list is now a mode
  of the cockpit's own (`InputMode::Choosing`, naming its combo box).
  - The key bar says its keys, and the top bar says CHOOSING.
  - A list gone without closing gives the keys back.
  - The control asks for a frame when the mode changes (the bars are drawn
    before it).
- **One rule for what can't run here** (anti-patterns 10, responsibility
  4). `Offered::refused` is asked by the keys, the controls (through
  `run_bound`) and the list:
  - an action that asks for a value isn't asked for it where it doesn't
    apply, and why not is said;
  - one without a value runs, and its view says why it can't.

  Refusing every action that doesn't apply was tried first. It broke the
  views' own, better reasons ("Nothing to go back to.", "No file"), so
  it's this rule.

What changed:
- **Where you are:**
  - `Stage::Actions { reach }`, and a value stage remembers the list it
    came from (Escape goes back to it, as it was);
  - `Listed` has sections in both reaches (the untitled one draws no
    heading), and the `:N` row is its entry (the go-to-line action found
    by its command, not its id).
- **What the list lists:**
  - `Sheet` has a title per `Layer`, named informs, the groups' names
    (the view's menu's too, so its name finds its actions), and no
    parallel `applies` (each action is `Offered` with its judgment, made
    by `applies` for its own view);
  - the mode's keys come from where the list was opened (`sheet_mode` is
    gone).
- **One key line.** The list's footer and the key bar both say
  `Palette::keys_said`; "Enter picks".
- **Less work:**
  - nothing is gathered or judged while the list is closed and not asked
    for (tested with a counter);
  - a row's kind is asked without building the row.

Tests: `app/tests/action_list.rs` (six), the setting list's mode (with a
key the panel has), and `everything()` opening `?` and typing as the user
does.

Live in the user's tab:
- `:` showed "What can be done here", and the same key line under it and
  in the key bar; Tab went to everything;
- a value picked from everything, then Escape, came back to everything;
- the graph's View list showed CHOOSING and "Escape closes".

### Part 3 done: the code search

- **One driver.** `CodeSearch::drive` takes its own answers and sends a
  round due through a closure for the net call (each kind, the caller's
  limits). `@` and the graph's find both call it; neither has an inbox or
  a reply variant of its own for it any more.
- **A picked row carries what it stands for.** The graph's row is its
  hit; the `@` row is the subject with its label. `before`, `hit()`, the
  round numbers in values, and the label lookup are gone.
- **Cleared when its picker leaves it.** The app sees the palette's search
  stage end (picked, gone back from, closed, or the list opened afresh)
  and tells its owner (`search_done`): a view clears its search; the shell
  drops its query, and the `@` search. The palette's `answered` is gone,
  and so are the tag pickers' own removals.
- **A hidden view's search is tended alone** (`Buffer::tend_search`), not
  through its whole tick.

Tests:
- the search's driver (each kind sent, answers taken, a failure said,
  what comes for a cleared round dropped);
- the graph's find rewritten through the driver and the net's test
  answers, a row picked after newer rows came, and its picker left.

Live: the graph's find through the action list listed what the index
found for "tree" (symbols first); reopened, it started at "Type to
search".

### Part 4 done: tests and words

Words:
- **The one name, "action list".** The top bar's button says it, `?`'s
  action is "Action list: everything", and the mode badge says ACTION
  LIST.
- **Comments and the README.** "Key sheet" is gone from the comments
  and the README. The leader menu has one name.
- **Stale docs fixed:** the hosts', `go`'s, `net.search`'s (doubled), and
  `ways`' (split by part 1).
- **`?`'s headings** are now the four the plan decided (faithfulness 6,
  consistency 7): what you're on, this view, this panel or the quick
  look, everywhere. Each section is in the order of the menus that hold
  its actions. `Group::title` went with the old per-group titles.
- **`?`'s pointer and touch section** says tapping a menu and a long
  press on a button.
- **Tips.** A menu button's tip is worded like every control's: "Name
  (keys)", then what it is.
- **The setting control** lets go of its list's state once, when its
  list closes, not with a lock every frame.
- **TODO** has the top bar's own buttons, to decide.

Tests (the tests review's items):
- **Done:**
  - exact half pages (2);
  - the pinch's pan equal to the fingers' travel (5);
  - the mock-only assertions replaced (6);
  - the key cache with two kinds drawn (8);
  - headings in the list (9);
  - a menu with nothing usable not offered (10);
  - `sheet`'s own tests: order, sections, kinds, rows, the refusal rule
    (11);
  - the which-key entry's tap, Escape and its button (12);
  - a finger held on a menu button shows its tip (13);
  - a wrap after a scroll by hand, and a key over a twitching pointer
    (14);
  - `?` and Tab in the box-keeps-its-size test (16);
  - the search rows' notes (17);
  - the shared helpers, the merged find tests (18, 20);
  - `?` opened by its key everywhere (21);
  - the groups wrapping on a phone (24);
  - ctrl+Home (25);
  - the stale test docs (26).
- **Item 4 was right in part only.** The list moves exactly half its
  window each time (asserted). The highlight moves the rows that come to,
  2 or 3 when rows are 28.8 points in a 120-point window. It keeps its
  place on the screen to within a row, by the viewport's rule for every
  view. So "the same step each time" can't hold, and the test asserts the
  exact parts.

Found while testing, and fixed:
- **The which-key menu jumped sideways as it opened.** Its first frame
  shown was 244 points right of where it settles, and it moved 2 points
  more the next frame. Its measuring pass sized it narrower than it is,
  and it's placed by its middle. It now has its width, and each entry's
  cell its size, from that pass. Tested: the same place from the first
  frame shown.

Live in the user's tab (`?` while typing "sign" and four backspaces, five
screenshots a key):
- the first frame is whole (title, field, headings, rows, key line), with
  no fade;
- each key changed the list once, inside the list, and nothing moved;
- the only other change was the SRV light's blink.

## The re-read of the fix pass (two reviewers, once)

The reports are word for word in `menus-and-lists-reviews/end-reread-responsibility.md`
and `end-reread-correctness.md`. They found most of the end review fixed,
and the faults below. There's no further review round.

### Fixed

1. **The top bar's Menus button opened the "Open a menu" picker,** not
   the leader's own menu. It now asks for the menu at its top. Tested,
   and checked live.
2. **A menu tapped while something else had the keys** dropped it
   without its leave path. A search being typed stayed applied (a tree
   left filtered); the action list's half-done start and its typed text
   were left behind. Now each leaves as its way back out does: the
   capture through its own Escape path, the list through
   `palette_closed`, now one function shared with closing it. Tested
   against plain Escape's result.
3. **A setting's list took over any mode** (and left a capture behind on
   close). It takes the keys only from Normal mode, where the panel's
   keys would run beside it. The configured way back out closes it (not
   only egui's Escape). Tested from a mark being asked for.
4. **Each view's situation was kept per kind,** so two graph panels were
   judged on one card. It's kept per buffer. Tested.
5. **The graph's find sent one more round after its picker closed.** A
   search left is told to its owner as the palette is drawn, before and
   after, so the next frame's tick has nothing to send.
6. **The key cache thrashed** past 8 hosts (8 panels and the quick look).
   It keeps as many as there are hosts on screen.
7. **The key bar showed the list's keys from the frame before.** The
   line is worked out when it's asked for, from the palette's state, not
   kept from its drawing. On a key that changes it, the palette asks for
   the next frame at once. Tested from the frame the list opens, and
   after Tab.
8. **A gesture on the quick look's top line reached the view under it.**
   A gesture counts only on the content of the host it's on. Tested.
9. **Controls didn't ask the one rule:** they ran through `run_by_id`. With
   no value, that now runs as the key does (`run_bound`). A view's
   request to open a picker goes the same way. `Offered` and its rule
   moved to the actions module.
10. **A refusal's line made the list's box taller** for the rest of the
    opening. It takes the key line's place until the next key.
11. **Tests that couldn't fail now can:**
    - a late answer for a gone round not completing the round out;
    - a cleared round's answers not answering the same query again;
    - the graph's find left with all three kinds answered;
    - the heading test starting off the heading.
12. **Comments:** three misplaced doc comments, and a "key sheet" in a
    test.

### Left (recorded, not changed)

- **A control's greying and the settings row are still their own
  judgments** (responsibility 2.1, 2.4). A control is greyed only when
  `When::Unavailable`, and the settings row isn't filtered by `applies`.
  Using the one judgment there means handing it to `actions::control`;
  not done here.
- **The widget sets `InputMode::Choosing` itself** (responsibility 2.2),
  and the list keys come from two sources (`keymap::local()` in the
  setting control, the config handed to the palette).
- **The top bar's Menus button is a group button, not a top-bar
  placement** (responsibility 2.3): a second way to put a button there.
- **`banners_shown` restates `render_banners`' conditions**
  (responsibility 2.6).
- **The shell's searches have no owner**: `search_done` checks `link.thing`
  by id (responsibility 2.8). `contents` is one more per-frame rect list
  (2.9).
- **The long press uses egui's hover delay, not `[touch] long_press_ms`;**
  the "Pointer and touch" section is a hand-written list.
- **Escape from a value goes back to the list on the same reach,** with
  the filter and highlight cleared (correctness 11: "as it was" said
  more than it does).
- **A `When::Unavailable` row in `?` says nothing until it's picked**
  (correctness 13).
- **Four wording assertions remain in the tests** (tests 22).

## Decide (for the user)

Decided by the user (2026-10-25): all five stay as they are.

From the end review's faithfulness report, not mine to settle:

1. **Step 4's phone-width check was done by an app test, not live**
   (faithfulness 5).
   - The plan says "live in the user's tab (… a phone-width window)".
   - The browser relay ignores window resizing, and Chrome's emulation
     doesn't draw the cockpit faithfully, so a 390×844 app test stands
     in. It now also checks that the groups wrap onto more than one row.
   - Options:
     - accept the test;
     - check on a real phone when you're back (the sign-in work made the
       cockpit reachable from any device).
2. **`?` runs a faded row that takes no value** (faithfulness 7). Decided
   3 says only "faded, with why when known". It now follows the one rule
   keys follow: an action without a value runs where it doesn't apply,
   and its view says why it can't ("Nothing to go back to."). One that
   asks for a value is refused, saying why.
   - Options:
     - keep this;
     - refuse every faded row in `?`, which would make `?` stricter than
       the keys.
3. **A panel not focused says no keys** on its menus and controls
   (faithfulness 7: "built but not asked for"). A long press there
   shows a tip with no keys too.
   - Options:
     - keep it (keys don't run that panel until it's focused);
     - show its keys greyed.
4. **Accepted by the user (2026-10-25): kept as the one exception.** **The action list reaches other views' own actions; the menus don't**
   (faithfulness 9). This predates the plan, which says "browsing and
   searching read one set".
   - Options:
     - record it as the one exception;
     - give the menus those actions too.
5. **The top bar's own buttons** stay on the top bar (Step 3 left them
   out). They're in TODO, to decide.

# Tags everywhere: every finding, and where each stands

Every problem the reviewers raised at the plan's four stopping points,
plus what I found myself, in one place. The reviewers' own reports, word
for word, are in `dev_docs/tags-everywhere-reviews/` (`sp1-consistency.md`
… `sp4-correctness.md`); each item below says which report it came from
(`SP3 tests` = stopping point 3, the tests reviewer). Where several
reviewers raised the same thing, it's one item with all of them.

Most of the open items are closed by `dev_docs/discussions-plan.md`
(its parts list which: A.4, F.5, G.8); the rest are listed there under
"Tags findings outside this plan".

Status:
- **Decide**: I changed the plan or chose something without asking; you decide.
- **Open**: a real problem, not fixed.
- **Kept**: I chose to leave it as it is, for a reason given; you may disagree.
- **Fixed**: fixed, with the commit.

---

## Decide (changed from the plan without asking you)

1. [Fixed, 3ecc985] **Searching tagged text doesn't use FFF.** `tag_find` opens each log
   file and reads it line by line. It gets slow as logs grow. I skipped FFF
   because its walk follows `.gitignore`, which hides `.nah/`, and didn't
   look for a way around it. (SP2 responsibility, SP4 consistency, SP4
   responsibility, SP4 faithfulness, SP4 correctness)
2. [Decided: kept as it is, sending the whole tag] **Changes still send the whole tag.** The plan said a change sends only
   what changed (a thing added or removed, a rename). Every change sends the
   full tag, with everything it's on, to every cockpit; the Tags view then
   rebuilds every row. (SP2 performance, SP4 tests, SP4 performance, SP4
   faithfulness, SP4 correctness)
3. **Your existing tags lost their saved text.** The plan said to move an
   old tag's saved text into the new logs. I deleted it instead: `jl` and
   `testtagforla` show their things as "gone". (SP2 correctness, SP4
   faithfulness, SP4 correctness)
4. [Fixed, 3ecc985] **The restart test doesn't restart the server.** The plan asked for a
   tagged command coming back whole after a server restart. The terminal
   test restarts the shell; a restart is only tested inside the tags crate
   with a stand-in owner. (SP4 tests, SP4 faithfulness)
5. [Fixed, 3ecc985] **A running command's log stops at 64 MB.** Not in the plan; the rest of
   its output isn't kept (the log says so).
6. [Decided 2026-10-21: Tags move to `Space t`, talk keeps plain `t` only. Done in the batch] **New keys:** `Space k r/c/n/x/d/o` (rename, colour, note, clear note,
   delete, take off) in a new "Tags" group. (SP3 consistency asked for keys;
   the letters are mine.)
7. [Fixed, 3ecc985] **A conversion for logs that existed only on this branch** (layout 10):
   it keeps only the first chunk of a command tagged while running by the
   branch's first build. Your rule is no compatibility layers. (SP4 tests,
   SP4 faithfulness)
8. **`Escape`, `G`, `gg`, `:N` in a terminal now count as jumps** (so
   `ctrl+o` goes back to the line you left), like `G` in vim. (SP1
   correctness)
9. **What a buffer keeps across a reload is in two places.** A view's
   `kept_place` (where it is, minus the terminal's cursor) sits beside the
   existing `keep`/`restore_kept`; the terminal's id is saved twice.
   Reviewer called it two mechanisms for one need. (SP1 consistency)
10. [Decided: the picker; every typing box since 27f5dea] **`#` in the chat opens the palette's tag picker**, while `/` shows
    matching commands as buttons under the box and keeps typing. The plan
    asked for the picker; a reviewer asked for it to work like `/`. (SP3
    consistency, SP4 responsibility)

## Open: not built

11. [Fixed, d537e31] **No whole-session chat tag**, only a turn or a message (the plan has
    session, turn, message). (SP4 faithfulness)
12. [Fixed, d537e31] **No whole-file code tag**, only a line or a definition (the plan has
    file, definition, lines). (SP4 faithfulness)
13. [Fixed, d537e31] **The code view can't select several lines to tag**, only the cursor's
    line; agents can name a range. (SP4 faithfulness)
14. [Fixed, d537e31] **The graph has no tag bars** beside its cards (cards aren't rows);
    a card's tags show only on the panel's top line.
15. **Not tried live in the browser:** `#` in the chat (your unsent draft
    is in the box) and tagging a chat (no agent session in the sandbox but
    yours). Covered only by automated tests.

## Open: wrong results

16. [Fixed, 3ecc985] **Chat turns can be numbered differently after a reload.** A message
    added to a running turn ("Add to what it's doing") is one turn live but
    two when the session is replayed; a prompt cancelled before any reply
    can do the same. Every later turn number shifts, so tags point at the
    wrong turn. (SP4 faithfulness, SP4 correctness; found when checking D.1)
17. **A jump's line after a width change can be off.** When a terminal's
    width changes, a line saved in the jump list or a mark is moved by how
    far the end moved; the server moves each line exactly. The shifts are
    kept per buffer, so a jump into a buffer that showed another terminal
    meanwhile isn't moved at all. (SP1 correctness, SP2 responsibility, SP2
    correctness)
18. [Fixed, 3ecc985] **A deleted definition's code comes back as nothing.** A tagged
    component isn't kept at a snapshot, so once it's gone from the code,
    nothing of it can be read. (SP4 faithfulness)
19. [Fixed, 3ecc985] **Logs of tags this build can't read are deleted.** Tags a newer build
    wrote are skipped when read; the start-up cleanup then removes their log
    files. (SP4 correctness)
20. [Fixed, 3ecc985] **The command a tagged line was in can be dropped from its name.** "(of
    command 3)" is worked out live; after a restart, the name is written
    again without it. (SP4 correctness)
21. [Fixed, 3ecc985] **Text you untagged can stay on disk.** A log file is removed only when
    no tag points into it; a secret printed by a command you untagged stays
    while any other tag uses that file. (SP4 correctness)
22. [Fixed, 3ecc985] **A command that ends just as it's tagged can lose its end.** If it
    ends between being read and being followed, the last lines and "exit
    code" line aren't written until more output comes (never, if the shell
    ends). (SP4 correctness)
23. [Fixed, d537e31] **Two bars of the same tag in one place look like one.** A tag on a
    command and on a line inside it draw in the same column. Bars also move
    between columns as other tags scroll in and out. (SP3 faithfulness, SP3
    correctness, SP3 tests)
24. [Fixed, d537e31] **Tag bars in the code view are easy to mix up with change bars.** The
    plan said the code view's own gutter takes the tag marks; they sit in a
    strip beside its change bars, and some tag colours look like the
    added/changed/removed colours. (SP4 consistency)
25. [Fixed, 3ecc985] **A thing has two names.** The picker says "command 3 and its output:
    make" or "line 12 of src/a.rs"; the stored tag says "terminal t1 command
    3" or "src/a.rs line 12". Copying the picker's "lines 100–1023" (an en
    dash) to an agent doesn't name it back. (SP2 responsibility, SP4
    consistency, SP4 responsibility)
26. [Fixed, 02daf34] **Chat text is put together two ways.** `slot_read` and `tag_read` give
    the same answer with different line breaks. (SP4 consistency)
27. [Fixed, 27f5dea] **A note is edited in a one-line field**: a note with several lines
    can't be edited properly. (SP3 faithfulness)
28. [Fixed, d537e31] **A letter typed right after `Tab` in the tag picker may be lost.** In a
    test the first letter in the same frame as `Tab` was dropped; I gave the
    test an extra frame instead of fixing it. (mine)
29. [Fixed, 3ecc985] **Prompts naming a tag show tool output unfolded**, while `tag_read`
    folds it: two renderings of the same thing. (SP4 responsibility)

## Open: slow, or grows without bound

30. [Fixed, 3ecc985] **Code tags take whole-project snapshots that are never cleaned up.**
    Each time tagged lines move, a snapshot of the project is taken (a scan
    of every file) while other tag changes wait; the snapshots pile up in
    the repo. (SP4 performance, SP4 faithfulness, SP4 correctness)
31. [Fixed, 3ecc985] **Code tags run git for simple questions.** Finding a thing's owner,
    its name and whether it's still there each run a git diff for code
    lines, also on every tag and untag over every thing the tag is on. (SP4
    performance, SP4 responsibility)
32. [Fixed, 3ecc985] **Counting a log's lines reads all of it.** Each read of a tag counts
    every line of each thing's log; prompts naming the tag do it too. (SP4
    performance)
33. **Following a running command uses one lock for all terminals**, and
    writes each chunk unbuffered. (SP2 performance, SP4 performance)
34. [Fixed, 3ecc985] **Tagging a finished command builds its whole output while other tag
    changes wait.** (SP4 performance)
35. [Fixed, 3ecc985] **The cockpit copies whole tags every frame** (the tags at the cursor,
    twice per panel). (SP3 performance, SP4 performance)
36. [Fixed, d537e31] **The tag gutter goes through every tag's things every frame.** (SP3
    performance, SP4 performance)
37. **The Tags view draws every row every frame**, and looks each thing up
    in its tag per row: slow with a big tag unfolded. (SP3 performance, SP4
    performance)
38. [Fixed, d537e31] **The solid tag icon checks the fonts each time it's drawn.** (SP3
    performance)

## Open: inconsistent, or awkward to maintain

39. **The key sheet doesn't show what actions do.** The key sheet
    (`nav.keys`) still shows labels only; the descriptions show in the
    palette, the leader menu's hover and button tips. (SP1 consistency)
40. [Fixed, 3ecc985] **A view says two things that must agree** (where a thing is drawn, and
    whether it has a tag gutter); a new view that forgets one shows no tags.
    (SP3 responsibility)
41. **The generic picker choice has a tag-specific colour mark**, and the
    "caret to the end" helper lives in the theme module. (SP3
    responsibility, SP4 responsibility)
42. [Fixed, d537e31] **The picker opened for an edit is built by hand**, and shell commands
    are no longer copyable so they can carry a tag; other two-step pickers
    keep the pick in the desk and open a bound action. (SP4 consistency)
43. [Fixed, 27f5dea] **The chat names the shell's `tag.insert` action by its text id** and
    decides itself where a `#` starts a name. (SP3 responsibility, SP4
    responsibility)
44. [Fixed, 02daf34] **A third date format** in the log headers. (SP4 consistency)
45. [Fixed, 27f5dea] **Three helpers say "tagged by …"**. (SP4 consistency)
46. [Fixed, d537e31] **The picker's `Tab:` hint sits apart from its other key hints.** (SP4
    consistency)
47. [Fixed, d537e31] **The tag gutter's sizes are fixed pixel numbers**, not the theme's. (SP3
    consistency)
48. **A settings drop-down ignores a choice's colour** (no tag choice
    reaches one yet). (SP3 consistency)
49. **The palette's search reads every action's description**, longer text
    ranked every frame while open. (SP2 performance; small)
50. [Fixed, 3ecc985] **The tool-output mark could still appear in a terminal's output**,
    which would then be folded away in `tag_read`. Made rarer, not removed.
    (SP4 responsibility)

## Open: tests

51. [Fixed, 3ecc985] Code tags following a saved file through the index isn't tested (the
    test calls the refresh itself); a tagged definition moving, and a
    cleaned-up snapshot, aren't tested. (SP4 tests)
52. [Fixed, 3ecc985] A chat session replayed, and its tags still pointing right, isn't
    tested. (SP4 tests)
53. [Fixed, 3ecc985] `tag_read` with `full` isn't tested. (SP4 tests)
54. [Fixed, d537e31] The Tags view's buttons are found by their text, never clicked. (SP3
    tests, SP4 tests)
55. [Fixed, d537e31] Hovering a gutter bar isn't tested; a long press on the shared muted
    column isn't; where the bars sit in the code view isn't. (SP3 tests, SP4
    tests)
56. [Fixed, 27f5dea] `#` reopening after Backspace isn't pinned either way. (SP3 tests)
57. [Fixed, 3ecc985] Duplicate tests left: "a bar spans its rows" twice; "a place without a
    line" in the terminal tests. (SP3 tests, SP4 tests)
58. [Fixed, 3ecc985] Tests check wording that the unit tests already check, and read
    internals (the chat's typed text, the palette's memory id). (SP3 tests,
    SP4 tests)
59. One older terminal test (`a_reader_is_sent_lines…`) failed once in the
    full run and passed alone: it checks timing. (mine)

## Kept (with a reason; say if you disagree)

60. [Fixed, 3ecc985] **Log headers can be imitated** by what a command prints: the tags in
    the database say what's tagged; headers are a reading aid. (SP2
    permissions)
61. **Progress bars that redraw lines above the cursor** are logged as first
    written. (SP2 correctness)
62. **The logs folder's permissions:** if they can't be set, it warns and
    carries on (the state database errors instead). (SP3 consistency, SP3
    correctness)
63. **An agent driving the cockpit acts as you**, beyond its role (older than
    this plan; in `TODO.md`). (SP3 permissions)
64. **Agents can read `.nah/state` with their own tools** (they run as the
    server's user; in `TODO.md`). (SP3 permissions, SP4 permissions)
65. **Agents can name output from earlier shell starts**: the same access as
    reading the terminal. (SP4 permissions)
66. **`Action` descriptions now always sent to remote tools as text**, ""
    where there was null. (SP1 correctness)

---

## Fixed

Discussions plan, step 1 (3ecc985): #1, #4, #5, #7, #16, #18, #19, #20,
#21, #22, #25, #29, #30, #31, #32, #34, #35, #40, #50, #51, #52, #53,
#57, #58, #60 (how each was done: `discussions-findings.md`, step 1).

Stopping point 1 (e1fc58f):
- `ctrl+o` to a terminal place without a line did nothing; now its live
  screen. (SP1 consistency)
- Going back to a closed terminal was silent; now said. (SP1 consistency,
  SP1 correctness)
- A go-to under way saved no line in the jump list; now its target's. (SP1
  correctness)
- A full-screen program put the cursor on its own rows; a go-to now waits.
  (SP1 correctness)
- Descriptions now in button tips, the leader menu's hover and the palette
  search. (SP1 consistency)
- Shared move descriptions that contradicted views' labels. (SP1 consistency)
- An empty description now fails in debug builds; harness commands and
  settings with none get words. (SP1 consistency, SP1 correctness)
- The palette row uses egui's `Sides`; the mark key's text no longer names a
  key. (SP1 consistency)
- The palette shrank between steps and its title wrapped. (SP1 correctness)

Stopping point 2 (d4c5b67, f67f315):
- Each chunk of a followed command rewrote and broadcast the whole tag;
  now appended to its own file. (SP2 all four)
- Untagging didn't stop following; tagging again logged twice. (SP2
  responsibility, permissions, performance, correctness)
- Tagging checked and wrote under different locks (a deleted tag coming
  back, doubles, a file removed under a new piece). (SP2 all four)
- A partly scrolled-away command read back cut short as "as it is now".
  (SP2 correctness)
- Following lost everything after `clear` or a small scrollback; blank lines
  dropped; wrapped lines split. (SP2 correctness, performance)
- A failed tag leaked memory and kept following. (SP2 performance,
  correctness)
- Files a crash left behind; logs readable by other users; a folder made
  before. (SP2 permissions, correctness)
- Reads loaded whole texts to show 4,000 characters. (SP2 all four)
- Line names cloned every command. (SP2 performance)
- The tag picker's pick reset by the palette's drawing and kept by list
  position. (SP2 responsibility)
- "Where it is" words in three places. (SP2 responsibility)
- `tag_read` past the last line. (SP2 correctness)

Stopping point 3 (6ab62ec):
- A confirmation could delete or rename another tag than it named. (SP3
  permissions, correctness)
- The shell laid out the Tags view's buttons; now the view places them.
  (SP3 responsibility)
- "Name a tag" everywhere, leaving a terminal typing; `#` with no tags,
  after Backspace, after `(`; the caret before the name; Escape left typing.
  (SP3 responsibility, faithfulness, correctness, consistency)
- Taking a tag off a Tags-view row took off an overlapping range. (SP3
  correctness)
- The gutter dropped a fourth tag; long press used its own timing. (SP3
  faithfulness, correctness, consistency)
- Search missed things under folded tags; unfolded state of deleted tags.
  (SP3 faithfulness, correctness, performance)
- Clearing a note with several tags at the cursor; delete offered in every
  view; colours shown as tags not swatches. (SP3 faithfulness, consistency)
- Edits failing silently; the field's caret; keys for the edits. (SP3
  consistency)
- The tags at the cursor worked out once per gather; picker rows only for
  the open picker; key lookups. (SP3 performance)
- The logs folder followed a link. (SP3 permissions)
- Tests: the top line checked for real, colours per tag, Backspace and folds
  in the Tags view, the folder's mode, a stale confirmation, overlapping
  ranges. (SP3 tests)

Stopping point 4 (41a3157, 772aa54):
- **An agent could read any file the server can** (the cockpit token
  included) by tagging it as code lines. (SP4 permissions, correctness)
- An agent could read any slot's chat through tags. (SP4 permissions)
- A running command's log had no limit; reading a few lines read it all.
  (SP4 permissions, performance, faithfulness, correctness)
- A chat turn tagged while running was kept cut short; turns left out
  thinking, plans, to-dos, diffs and tool inputs. (SP4 faithfulness,
  responsibility, correctness)
- Code lines read at their old numbers; changed or deleted lines gave
  today's text or nothing; the same lines tagged twice; refreshed on plan
  edits too. (SP4 faithfulness, responsibility, correctness)
- Names that didn't name back (chat sessions, components, a closed slot).
  (SP4 responsibility, correctness)
- The chat worked out where each item is for every item, every frame.
  (SP4 performance, correctness)
- Code read terminal names as its own. (mine)
- `tag_find` and `tag_read` numbered lines differently; `full` ignored in
  the list; agents not told how chat and code are named; a late-tagged
  command not saying what scrolled away; "Name a tag" without `#`;
  `scrollback_lines` raised silently; the empty Tags view's text. (SP4
  tests, faithfulness, consistency, correctness)
- Tests: a restart reading back what was kept, nothing untagged logged, a
  re-tagged command proven followed, more margin for the 50,000-line run, a
  real search in the Tags view, a running chat turn kept whole. (SP4 tests)

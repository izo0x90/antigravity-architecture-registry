# Links: review findings

Eight reviewers (the seven of `process/reviewers.toml` and anti-patterns,
from `TODO.md`) read `0dfa7ff..9dd240b`. Reports word for word:
`links-reviews/`. Each point: **Fixed**, **Decide** (the user's), or
**Kept** (with why).

## Fixed

1. **Pointers had no size limit; the budget held only for whole things**
   (performance 1, faithfulness 1, correctness 1). A pointer's lines are
   each cut to 200 characters, ten at most; every thing (whole, pointer,
   or a heading) counts against the 16 KB; past it, one closing line says
   how many more were left out and how to read them, and the user is told.
   The test that showed it: a tag's 2000 things came to 166 KB, then 25 KB
   (headings), now within the budget.
2. **The server didn't check the reader may read what's linked** (the
   plan's own words; responsibility 1, permissions 1, faithfulness 2,
   correctness 2). Expansion takes the slot it's for again (`Reach`): what
   it can't read is named, not handed on, and the user is told.
3. **Links went from the palette's writing through a side field**
   (responsibility 2, anti-patterns 3, faithfulness 4, consistency 2). A
   writing says whether it takes links (`Param::Writing { links }`); one
   that does sends its message (text and links) as its value; `#` and `@`
   open only there. A tag's note takes no links, so it offers none.
4. **`ctrl+i` never fired on a Mac** (consistency 1). It's read like every
   other key (`Chord::consume`: ctrl or command), and kept from the
   browser like the others. A test now presses it.
5. **`@` swallowed sentences** (correctness 3). A space always ends a
   trigger's picker (no name `#` or `@` picks has one), giving back what
   was typed.
6. **`gives_back` stayed set after a pick** (anti-patterns 2,
   faithfulness 6, correctness 9). Cleared on a pick and when the writing
   comes back.
7. **Clearing the prompt left its links** (anti-patterns 8, correctness 4).
   `TypingBox::clear`; the chat's clear uses it.
8. **Two warnings for one thing** (anti-patterns 5, faithfulness note). The
   after-send notice is gone; what links come to is said under the box,
   before sending, only.
9. **The check re-ran on every pause in typing, and a failed one was never
   retried; entries for closed boxes stayed** (performance 2/7,
   responsibility 6, anti-patterns 6, consistency 3, correctness 11).
   Checked by links (not the text), whenever a box has links (so a tag
   deleted since is said too), after the configured quiet time; a failed
   check is asked again; a box gone lets its entry go.
10. **A tag deleted after it was linked went silently** (responsibility 5,
    faithfulness 5, correctness 7). Said in what the agent gets and to the
    user, before sending.
11. **Spans weren't checked** (tests 10, faithfulness 7, correctness 10).
    `Message::valid` drops links that aren't spans of the text (server),
    `trimmed` too; the box draws only valid spans.
12. **Symbols picked over a plan resolved to nothing** (correctness 5).
    `@` searches the code itself.
13. **Two answer sets for the same query doubled the hits** (anti-patterns
    1). Answers are matched by round, not by the query's text.
14. **A text ending in `…` was taken as cut** (correctness 6). Cut is
    judged by length.
15. **The same thing linked twice kept the first's include** (correctness
    7). Whole if either asks for it.
16. **Chips said `@src/a.rs`, the agent read `@file src/a.rs`**
    (consistency 5). The agent's heading is what the message says at the
    link, with the thing's name beside it when they differ. The lines
    choice is named by `Subject::name`.
17. **Edits around a link were misjudged where characters repeat**
    (tests 5, correctness 8). The caret after the edit says where it
    began. Tests for both cases.
18. **The message's shape was rebuilt per route and per request**
    (anti-patterns 4, consistency 7). The routes flatten `Message`; the
    cockpit sends `json!(message)`; the mangled doc comment is gone.
19. **`thing_read` copied `tag_read`'s reading** (anti-patterns 9). One
    `some_lines` for both.
20. **Two predicates for "something's being written here"** (anti-patterns
    10, consistency 8). `Navigable::typing` (the box, read-only) replaces
    `takes_typed`; one `writing_here` gates `#` and `@`.
21. **No way to change a link on a phone** (correctness 12). An action,
    "The link here: whole, a pointer, or as it fits", in the palette.
22. **Per-frame costs** (performance 4/5, anti-patterns 7). Only boxes
    with links are tended; tag colours read only when a tag is linked; the
    box copies its text only when it has links; the draft is compared, not
    copied; the chips' byte offsets are found in one pass.
23. **`attach` read each thing up to three times** (performance 3). Once:
    its start serves both "is it small" and a pointer's lines; a second
    read only for a big thing linked whole. Links are resolved in one
    `things.pass`.
24. **Tests asked for** (tests 1, 2, 6, 7, 8, 9): `attach` tests (small,
    pointer, the budget for many, not the reader's, gone, `…`); the check
    route saying a gone tag; `thing_read` refusing another slot's chat;
    Escape after typing in the picker; `ctrl+i` pressed; the check's wait
    and a stale answer dropped; negative checks made positive (the
    content, not a heading's wording). Wording: "# links a tag, @ a file
    or a symbol"; the key's label in `@`'s description.

## Kept, with a reason

- **The `@` picker is a second client of the code index beside the
  graph's find** (responsibility 4, anti-patterns 1, consistency 4).
  It's the one picker-source shape in `TODO.md` (the search rework); its
  limit stays fixed until then.
- **The shell writes into every typing box** (responsibility 3). Buffers
  may not know tags (the lint), so link colours are the shell's to set;
  checks need the network. Fewer boxes now (only those with links).
- **The check goes through the slot manager's expansion**
  (responsibility 6). It's the one expansion a send uses; a second path
  for checking is what the reviewers otherwise ask against.
- **`@` asks per keystroke** (performance 6). As the graph's find does;
  the search rework.
- **`TypingBox.text` stays public** (anti-patterns 8). Its links follow
  edits by the box's own diff; `clear` closes the one bypass found.
- **A discussion brought in from a plan's comments has no links**
  (tests 4). Old comments had none.
- **No server test of a file or lines link through to the agent**
  (tests 3). The `@` picker's test stops at the request; the owner's
  `now_at` with an empty snapshot is the tags' path, already tested.

## Decide

(none)

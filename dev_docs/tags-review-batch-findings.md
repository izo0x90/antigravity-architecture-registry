# Tags review batch: findings

What was built for each decided item of `.nah/plans/tags-everywhere-review.json`
(decisions made 2026-10-21), checked against the decision, with what was
tried live and what the reviewers found. Plan and order:
`tags-review-batch-plan.md`. Reports word for word: `tags-review-batch-reviews/`.

Words: **Decide** is the user's call; **Built this way** is a choice made
building, said so it can be changed; **Kept** is a reviewer's point not
taken, with why.

## Decide

(none yet)

## Built this way (worth knowing; say if you'd have it otherwise)

1. **A key sheet row's description is on the row, after the label, in
   small muted text, wrapped** (#39). Under the label (as the palette's
   rows have it) doubled every row's height and the sheet became a long
   scroll; on the row keeps it a dense table. The sheet's filter matches
   descriptions too, as the palette's search does.
2. **A choice's swatch is a text glyph (`■`) in the colour** (#48), so one
   text job names a choice everywhere (the palette rows, a drop-down); it
   was a painted rounded rectangle in the palette only.
3. **The chat draft is saved by slot, not by view** (#9, as decided). Two
   chat views on the same slot share one draft; the one used last says.
4. **A go-to to a terminal line always asks the server where the line is
   now** (#17), even when no new width happened since: one path, no
   bookkeeping to know whether asking is needed. The cursor lands a
   round trip later (the socket's, local). The cursor a new width moved
   is found the same way.
5. **A growing log's chunks are written through, not held** (#33). The
   decision said "buffered writes flushed on a tick or at the chunk
   cap". Built first as a 64 KiB buffer written out on read; the full
   suite showed what that loses: the tags crate's restart test found
   the last chunk missing after the server came back (a crash or kill
   would lose up to 64 KiB of every growing log). Nothing in the server
   ticks, so a buffer can't be made safe without a timer thread. What
   stays of #33: the lock per terminal, and a lock per growing log. A
   chunk is one `write` to the file (the kernel holds it; no fsync), the
   same count of syscalls as the PTY reads that produce them.
6. **Items 2 and 3 (#48, #49) are one commit**: both are inside the
   palette's row drawing; splitting the hunks by hand wasn't worth it.

## Kept, with a reason

- **A second chat view's asks are taken only while it's the chat in use**
  (found writing #9's test). `take_asks` reads the used buffer of each
  kind, so a view in another panel that isn't the used one keeps its asks
  until it is. In use, every frame takes them, so typing in a focused
  view is reported at once. Not changed here: it's the asks mechanism's
  (every ask kind), not drafts'. Noted for the buffers rework (`TODO.md`).

## Per item

| # | Item | Commit | Plan item |
|---|------|--------|-----------|
| 1 | #41 + #60 trigger characters generalized | f3a619b | i46, i74 |
| 2 | #48 one way of naming a choice | 8460dfb | i53 |
| 3 | #49 palette ranks on change only | 8460dfb | i54 |
| 4 | #39 key sheet shows descriptions | b70dc8a | i44 |
| 5 | #37 Tags view draws visible rows | 1b072fb | i41 |
| 6 | #6 Tags group on `Space t` | 8880ee8 | i7 |
| 7 | #9 one place, a version per kind, draft out of the layout | 2757e65 | i10 |
| 8 | #17 jumps and marks name a line as a tag does | 81b9558 | i20 |
| 9 | #33 followed per terminal, buffered logs | c0319f0 | i37 |
| 10 | #59 the half line observed, not timed | d706bfb | i65 |
| 11 | #3 the two test tags deleted (`jl`, `testtagforla`) | (data) | i4 |

### Checked against the decisions

- **#41, #60:** `trigger.rs` is the one table (`#` → `tag.insert`). The
  typing box reports the character typed at a word start; the shell maps
  it and opens the action, with the character not typed anywhere else
  that pass (`typed_away`, the same rule a command key has). The palette
  holds what's written for any action (`hold_for`, `put_back`). `Choice`
  carries `Mark::Glyph(icon)`; the caret helper is in `conversation.rs`.
  Nothing in the palette, the typing box or the host names a tag.
  The `#` in the picker's box: not reproduced natively (the test asserts
  an empty box); to be checked live in the web build (step 13).
- **#48:** `actions::choice_text`; the drop-down and the palette row call it.
- **#49:** `Ranked`: the actions' or choices' ranking keyed by the text
  and a fingerprint of the list (ids, labels, details, groups).
- **#39:** the row draws the description from `action.detail`; the filter
  matches it.
- **#37:** `sync` keeps each row's thing index and height; `draw` lays
  rows out by height and draws those in the viewport (`show_viewport`),
  no per-row search; `Measured` is told every row's place.
- **#6:** `default.toml`: `t = "Tags"`, six bindings `t …`, `talk.open`
  keys `t` only; keys snapshot; README's six mentions.
- **#9:** terminal `keep`/`restore_kept` gone; `views` covers the quick
  look's buffers too (the pop-up terminal's place); `SavedUi.versions`
  (per kind, `BufferKind::saved_version`, all 1) with `serde(default)` so
  nothing resets now; a stale kind's views and kept are skipped and a
  note names them. `drafts` table (migration 13), `GET /api/ui/drafts`,
  `PUT /api/ui/drafts/{slot}`; `crate::drafts::Drafts` in `Shared`; the
  chat view takes its slot's draft when empty and reports changes as
  `Ask::Draft`; saved after the same quiet period as the layout.
- **#17:** `ToTerminal::LineNow { shell, first, last }` /
  `FromTerminal::LineNow { .., now }`; the server answers with
  `Open::lines_now` (the tags' exact moves) for the current start of the
  shell. `TermPlace` is `(terminal, (shell, line))`; `relaid`, `layouts`,
  `line_now` are gone from the cockpit. Server test
  `a_line_is_found_where_it_is_after_a_new_width`; cockpit tests answer
  the ask (`answer_line_now`).
- **#33:** `Open.followed` per terminal; `Logs::grow` hands a `Writer`
  (`Arc<Mutex<File>>`, its own lock); `done` lets the writer go when the
  follower drops (`Done` guard in `Things::log`). See Built this way 5.
- **#59:** `printf 'half'; read -s x; …`: the reader waits to see "half",
  sends Enter, then the rest prints in one write.
- **#3:** both tags deleted through `DELETE /api/tags/{id}`; none left.

## Lints

- Clippy (native, workspace, all targets): clean after each item.
- ast-grep: 0 errors; 2730 warnings against 2719 at 5d17878. The three
  new ones are of kinds the code has throughout: two public struct fields
  (`SavedUi.versions`, `Shared.drafts`) and one single-field struct (the
  `Draft { text }` request body in `routes/cockpit.rs`).
- Snapshots: keys (`Space t`), http routes (`/api/ui/drafts`,
  `/api/ui/drafts/{slot}`); the MCP tools' unchanged.

## Tried live (2026-10-23, the user's own browser, with the links work)

New build loaded in the user's tab (UI and server rebuilt, server
restarted; the lead's session reconnected for one agent turn).
- `#` with no tags: the picker opens and says "no tags yet". Typed
  `define` and a space in it: the picker closed and `#define ` was in
  the message. A paste (`fix #include`) opened nothing.
- `#` with a tag: picked `livetest`, drawn in its colour; `#include` and
  `#define` stayed plain. `ctrl+i` made it whole (filled).
- `@`: `Cargo` listed files (`Cargo.lock`, `Cargo.toml`, …); a symbol
  picked (a typed `j` went into the search, matching a test function),
  drawn in the accent.
- Sent to the lead: it was handed "What's linked above", with the tagged
  `Cargo.toml` whole; it answered "ok".
- `Space`: `t Tags…` (rename, colour, note, clear, take off under it); no
  talk entry. `?`: each key with its description ("Down  One down…").
- Tags view: the tag, unfolded with Enter: its file, "tagged by you".
- Put back: panel 1 the chat, panel 2 discussion d2, as they were; the
  test tag deleted; the chat box empty as it was (no draft to move).

## Reviewers

Done by me, reading the whole diff once from the angles in the plan
(2026-10-22; no sub-agents, at the user's ask).

**Fixed** (one commit after cd5f4ee):
1. **(Wrong, undone 2026-10-23.)** I took the trigger's ask to be handled
   a frame after the character and removed `typed_away`. It is handled in
   the same frame, after the box drew: the palette's field, drawn later
   that frame, takes the same typed text, so the `#` landed in the
   picker's box (the #60 the user saw). The links work's tests caught it;
   `typed_away` is back, with why in a comment.
2. **Correctness, #17:** when the server can't follow a line (laid out
   anew longer ago than it keeps), the go-to landed on the old number,
   which may be another line now. It says so and doesn't land.
3. **Correctness, #17:** a `LineNow` asked just before the terminal's
   socket dropped was never asked again on the new socket: the go-to
   never landed. The in-flight ask is cleared when the socket is reopened.
4. **Correctness, #9:** a draft whose save failed stayed unsaved until
   the text changed. It's put back as pending and tried again.

**Kept, with a reason:**
- **Tags view row heights only grow** (#37): a row whose note was
  cleared keeps its taller height until the list is rebuilt. Cosmetic;
  the alternative (defaults each rebuild) jumps the list for a frame.
- **A draft saved for a slot comes back into an emptied box after a
  reload** (#9): the box was empty when the drafts arrived, so it takes
  the saved text. Telling "emptied on purpose" from "not typed yet"
  needs state the layout doesn't have; the old layout-kept draft did the
  same.
- **Two views on one slot share a draft** (#9): as decided (by slot).
- **Lock order** (#33): `Things::log` holds no logs lock while the owner
  follows; `tell_followed` holds the terminal's `followed` while a
  follower writes behind its own log lock; `Logs::remove` takes writers
  then state; `search` takes search then state. No cycle.
- **ast-grep's three new warnings** are of kinds the code has throughout
  (public fields, a one-field request body). Not changed.

**Nothing found** in: #48, #49, #39, #6, #59, the per-kind layout
versions (an old record without `versions` reads as all 1, which is
every kind's version now, so nothing resets), the migration.

# Stopping point 3: correctness reviewer

I reviewed d4c5b67..HEAD by reading the code only. I built nothing and changed nothing. Most severe first:

1. **A tag edit can land on a different tag than the one the user picked.** Delete is the worst case. (`app/tagging.rs:45-55`, `:287-302`; `tag_list.rs:86`)
   - `TagEdit` works out its target again when it runs: `tag_to_edit(here)`. If the picked tag is no longer at the cursor, it falls back to "the one tag here".
   - Sequence A, any view: line 1005 carries tags A and B. The user runs "Delete a tag here", picks A, and gets "Delete A (from N things)?". Before they answer "yes", an agent or another cockpit takes A off the line or deletes it. Only B is left, so `picked` is None, `[one]` is B, and **B is deleted**. Rename, Recolour and Note fail the same way.
   - Sequence B, Tags view: the cursor is on row A (rows A, B) and the user presses Delete. A is deleted elsewhere while the confirm picker is open. `sync()` can't find `Row::Tag(A)`, so it keeps the old index, which is now B's row. "yes" then deletes B.
   - Severity: high. The user loses a tag they never chose. Fix: carry the tag id in the value or the invocation, and refuse when that tag isn't there.

2. **After picking a name with `#`, the next typing goes in the middle of the prompt.** (`chat/nav.rs:77`, `tagging.rs:271`)
   - `typed_in` appends to `input_buffer`, but the TextEdit's stored cursor doesn't move. In egui 0.33.3, `builder.rs:617`/`:915` reuse `state.cursor.range` when one exists, so the cursor stays just after `#`.
   - Sequence: type `look at #`, pick `flaky`, then type `x`. Expected `look at #flaky x`; actual `look at #xflaky `.
   - The test `a_hash_typed_in_a_prompt…` doesn't catch this. Its last check (`issue#` doesn't open the picker) passes whichever place the text lands.
   - Severity: high for F.4's main flow. Confirm by asserting the buffer after typing once the name is in.

3. **"Name a tag" is offered in every view and leaves that panel in typing mode.** (`tagging.rs:164,171`, `:273`)
   - `typing` is true whenever the palette is open, so the action shows in the terminal, Tags and code views alike.
   - Sequence in the terminal: open the palette, choose "Name a tag", pick `flaky`. `typed_in` does nothing there, but `input_mode = Insert{at}` is set. The terminal's field is `Field::Keys`, so every key after that goes to the shell: a `j` or `dd` runs in the shell.
   - In the Tags view (no text field), keys are swallowed until Esc.
   - Severity: medium-high.

4. **`#` opens the picker even when there are no tags, and again on unrelated edits.** (`chat/render.rs:327`; `draw/panels.rs:58`)
   - `take_palette_request` opens the action without checking `When`. With no tags, `tag.insert` is Unavailable, yet `fix #123` opens an empty picker that takes the `123`.
   - The check runs on any change while the buffer ends in a word-start `#`:
     - With `see #` typed, Esc, then any edit earlier in the text (or typing `a` and deleting it again), the picker reopens.
     - `## heading` opens it twice.
   - The client only triggers after whitespace. The server also treats `#` after `([{"'` as a tag (`nah-tags/src/lib.rs:187`), so `(#flaky` gets no picker.
   - Severity: medium.

5. **Taking a tag off a row in the Tags view can remove a different range of the same tag.** (`tagging.rs:254-266`, `tags.rs:122`)
   - The row's exact subject is thrown away. `tagged_here` → `tags.on(subject)` returns the first of the tag's own `on` entries that *contains* the row's subject. The old `TagList::run` used `thing_here()`, the exact subject.
   - Sequence: tag A is on lines 5-20 and also on 10-12. The server allows overlaps; it only dedupes identical subjects (`lib.rs:613`). Unfold A, put the cursor on the "lines 10-12" row, press Take off. Lines 5-20 are untagged and 10-12 stays.
   - Severity: medium. It changes the wrong thing silently.

6. **The gutter draws at most 3 tags; a thing whose only tags are past those gets no mark.** (`draw/host.rs:24`, `:99-101`)
   - Sequence: four tags, each on a different command on screen. The command under the 4th tag shows nothing.
   - Its name appears only when the cursor sits on it (top line), so rule 5 / F.1's "shows tagged things" fails for it.
   - Column order follows `tags.list()` among the tags visible now, so a tag's bar changes column as others scroll in and out.
   - Severity: medium.

7. **Search in the Tags view can't find things under folded tags.** (`tag_list.rs:84`, `items()` at `:369`)
   - The rows hold only unfolded things, and everything starts folded. A search for a command's text finds nothing until its tag is unfolded, even though the plan says the view "answers … search".
   - Severity: medium-low.

8. **The Tags view's `open` set is never pruned.** (`tag_list.rs:57`, `:504`)
   - Sequence: tags A and B. Unfold A, then delete A. Now `open={A}` with one tag, so `open.len() < tags` is false and "Unfold every tag" is unavailable while B is folded. "Fold every tag" is offered with nothing open.
   - Severity: low.

9. **The terminal's `drawn_at` returns a one-row rect when the window is 0 rows.** (`terminal/mod.rs:563`)
   - With `window == 0`, `to = top`, so a tag on the top line gets a one-row bar while no rows are drawn.
   - This needs a zero-height grid with `grid`/`drawn` still set. Low.

10. **`Logs::open` doesn't check what the folder is.** (`nah-tags/src/logs.rs:48-54`)
    - `set_permissions` follows a symlink, so it chmods the target. If it fails (folder owned by another user), it only warns, and tagged output goes on being written into a folder others can read.
    - Severity: low.

Areas with nothing found:
- The `TagEditWhich` → value picker hand-off (`opening + 1`) holds. A stale `tag_editing` left by Esc can't be reached, because none of the edits have a key and their buttons show only on tag rows, where there's only one tag.
- Rule 1 (no subject text in the database): nothing in this diff breaks it.
- Rule 6 (every action has a one-line description): every new action and move has one.
- The solid-font fallback (`tag_mark`): it guards the missing family, and `solid()` has no other callers.
- Folding the cursor's own tag, Enter and leave in the Tags view: nothing wrong found.

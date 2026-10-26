# Stopping point 3: faithfulness reviewer

## Review: Part F (cockpit) against the plan, d4c5b67..HEAD

Paths below are relative to `/Users/izo/code/antigravity_architecture_registry/meta-harness/crates/cockpit-ui/src/`. Ranked, most severe first.

1. **"Name a tag" (`tag.insert`) appears in every palette, and in the wrong view it breaks input** (medium). `app/tagging.rs:199` is `let typing = matches!(self.input_mode, InputMode::Insert{..}) || self.palette.is_open();`. Opening the palette makes `typing` true, so the action is offered in every view, not only "in the chat box" (F.4).
   - Trigger: in a terminal, `:` then "name a tag" then Enter, then pick a tag. `typed_in` is the default no-op (`nav.rs:767`), so nothing is inserted. Then `tagging.rs:273` sets `InputMode::Insert { at: here.at }`.
   - In a terminal that is type mode (`terminal/mod.rs:744`), so the next keys go to the shell. In the Tags view it is an insert mode with no field to type in.
   - The test only covers the chat (`app/tests/tags.rs:438`).

2. **The Tags view's search no longer finds tagged things** (medium). F.2 says the view "answers the shared moves, folds and search" and that "the one gone to [is] opened".
   - `tag_list.rs:369` `items()` lists only visible rows, and every tag is folded to start. So `/` only matches tag names and notes. A thing's name or the text it holds can't be found until its tag is unfolded by hand.
   - Before this change every thing was a row, so search found them.
   - The unfold-on-arrival code in `move_to` (`tag_list.rs:398-399`) is reached only through restore and the jump list, never through search, because search only returns keys taken from `items()`.

3. **The tag gutter drops a fourth tag silently** (low-medium). `app/draw/host.rs:24` sets `GUTTER_BARS = 3`, and `host.rs:100` skips any further tag (`columns == GUTTER_BARS → continue`). The plan says "several side by side".
   - Trigger: four tags, each on a different command visible on screen. The fourth command has no bar and nothing says one is missing. The panel's top line names it only when the cursor is on that line.
   - Also, a tag on a line inside a command that has the same tag draws in the same column, over the command's bar, so the two can't be told apart.

4. **The `#` picker reopens when it shouldn't** (low). `chat/render.rs:327` opens it on any change after which the buffer ends in a `#` that starts a word.
   - Trigger: type `see #x`, then Backspace. The buffer is `see #` and the picker opens again. The same happens when the user presses Escape to keep a literal `#` and then edits earlier text.
   - It also disagrees with how prompts read tags (`nah-tags/src/lib.rs:187`), which accepts `(#tag` and `"#tag`. Typing `(#` opens no picker, so there are two definitions of where a `#` starts a tag.

5. **A note can't be cleared when the cursor has several tags** (low). `tag.note_clear` (`app/tagging.rs:141`) is available only when there is a single target, and the 4-entry edits array never gives it a "which tag" route. The `TagEditing::ClearNote` arm at `tagging.rs:283` can't be reached.
   - The Text stage won't submit an empty value (`palette.rs`, Stage::Text: `!self.filter.trim().is_empty()`), so "Note" can't clear it either.
   - Trigger: in a terminal, the cursor on a line tagged `slow` inside a command tagged `flaky`. "Clear …'s note" is unavailable.

6. **A note is edited in a single-line field** (low; not confirmed). F.2 asks for "the note opens in its field to edit". The code fills the palette's `TextEdit::singleline` with it (`palette.rs:201`). Notes can be multi-line (agents write them; the Tags view shows only `first_line`).
   - What would confirm it: set a note with a newline, open "Write …'s note", and check whether egui shows or keeps the newline. Pressing Enter submits, so a second line can't be typed.

7. **A colour choice shows a tag icon, not a swatch** (cosmetic). F.2a: "A choice with a colour shows a swatch before its name, in every picker (the recolour picker first)". `palette.rs:369` draws `tag_mark` (a solid tag glyph) for every coloured choice, recolour included. This is a substitution, not what was specified.

8. **A tag picked for an edit stays picked after Escape** (low). `app/tagging.rs:75` accepts `o == opening`. After "Rename a tag here" → pick `slow` → Escape out of the name field, `tag_editing = (N+1, slow)` still matches while the palette is closed (`opening()` returns the last opening).
   - Until the palette opens again, the item buttons and labels act on `slow` rather than asking which tag.
   - The `opening + 1` pre-match is fragile; it depends on `run_by_id` opening the palette exactly once.

9. **Built but not asked for** (low):
   - F.3 asks for "rename, colour, note of the tags on it". The code also offers Delete from every view (`tagging.rs` edits array, `E::Delete`).
   - `nah-tags/src/logs.rs:47-54` changes the logs folder's permissions on every open. It is a follow-up to an earlier part, not Part F.

**Areas with nothing real found:**
- **Chips:** gone, and nothing is drawn over the content (rule 5 is tested for the terminal).
- **Gutter only on the terminal:** consistent, since only terminal `Subject`s exist so far. Chat and code owners aren't built yet, so "the code view's gutter takes it" doesn't apply yet.
- **Tag names on the panel's top line, on hover and on a long press:** built.
- **The Tags view:**
  - Folding: built as planned (`z` keys, folded to start, Enter unfolds).
  - Buttons: drawn by the host from declared actions (`tag_at` plus the shell's edits; Go to it and Take off on a thing's row).
  - Its own Space-comma actions are removed, and "take the tag off" is now only the shell's.
  - When each thing was tagged is shown.
- **The filled icon font:** loaded as a second family (`theme.rs`).
- **Descriptions:** every new action has one (rule 6).

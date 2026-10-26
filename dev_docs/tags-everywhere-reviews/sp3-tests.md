# Stopping point 3: tests reviewer

I reviewed the tests in `d4c5b67..HEAD` (meta-harness), worst first. All test paths below are under `crates/cockpit-ui/src/app/tests/tags.rs` unless stated. I read the code only; nothing was built or run.

1. **The top-line check is vacuous: tags.rs:60-61.** `assert!(texts(&output).contains("flaky"))` passes whether or not the panel's top line names the tag. At line 46 `app.note` is "Tagged with flaky.", and `bars.rs:70` draws it in the key bar. Notes last 4 s (`NOTE_SECONDS`), while untimed frames advance about 1/60 s, so the note is still drawn. If `hosted::header` stopped drawing the tags, the test would still pass.
   - **Unprotected:** F.1 and F.2a's naming on the top line, and the solid tag icon.
   - **Fix:** check the text inside the header's rect, or clear the note first. Also check that a tag not at the cursor is *not* named.
   - **Severity:** high (a core rule-5 replacement for the old chips is untested).

2. **The comment says Backspace but the test never presses it: tags.rs:364-366.** The comment reads "Backspace: up to its tag; z c folds it", but only `zc` is sent. `zc` moves the cursor to the tag through `set_open`, so `TagList::leave` and the new `nav.leave [Backspace]` binding are never exercised. Also untested in the Tags view:
   - `zR` / `zM` (`Fold::OpenAll` / `CloseAll`).
   - `move_to` unfolding a folded tag ("the one gone to opened", F.2).
   - Enter on an unfolded tag going to its first thing.

   Severity: medium.

3. **Every fixture tag is colour 2: tags.rs:25.** `bars()` matches on fill colour, so it can't tell tags apart.
   - In `next_and_previous…`, line 149, the `slow` vector also holds flaky's bars.
   - No test protects "several side by side" (separate columns per tag), "a bar in the tag's colour" (a bug using one colour for every bar passes), or the `GUTTER_BARS = 3` cap in host.rs. With the cap, a 4th tag whose things are on screen gets no bar at all.
   - **Fix:** give `tag_on` a colour argument, and assert distinct x-columns for two tags on one row.

   Severity: medium.

4. **The log-folder permission fix has no test: f67f315, `nah-tags/src/logs.rs:47-54`.** It narrows an existing folder to 0700. The only permission assertion (logs.rs:226) is on a file's 0600 mode. The tempdir in that test is already 0700, so it couldn't catch this anyway. If the fix regresses, a log folder made earlier stays readable to every user while it holds tagged terminal output.
   - **Fix:** create the dir at 0755, call `Logs::open`, assert 0700.

   Severity: medium (security-relevant).

5. **Naming a bar on hover or long press is untested: host.rs, `draw_tag_gutter`.** Neither `on_hover_text` nor `long_touched()` setting `self.note` is exercised; F.1 relies on both, and the phone case most. The existing long-press pattern at tags.rs:132-138 can be reused on a bar's rect. Severity: medium-low.

6. **Gaps in the `#` picker test: tags.rs:437-458.** It covers only the happy path. Untested:
   - Escape from the picker: is the `#` kept, and does typing resume? Only `TagInsert` restores `InputMode::Insert`.
   - `#` with no tags: `tag.insert` is Unavailable, so nothing should open or swallow input.
   - Backspacing back to "…#": `output.changed() && ends_with('#')` in `chat/render.rs:327` reopens the picker. Intended or not, nothing pins it.

   Severity: low-medium.

7. **A stale "which tag" pick is untested: `tag_to_edit` in `tagging.rs`.** The pick is kept for `opening == N || N+1`. After "Rename a tag here", picking `slow`, then Escape, the palette stays closed at opening N+1. Until it is next opened, the shell's tag edits (labels, and `tag.note_clear`, which has no parameter) act on `slow` without asking. Low severity today, since no keys are bound, but nothing protects the expiry.

8. **Duplicated tests (cut or merge):**
   - Taking a tag off from a line of the thing is asserted three times: lines 92-100, 176-184, and 357-358 (Tags view). Keep 176-184, which covers the harder lines-range case, and the Tags-view one. Drop lines 92-100.
   - The "bar spans the thing's rows" check appears at 55 and again at 149-150. Keep one, folded into the column/colour test from item 3.

9. **Tests reaching into internals:**
   - Line 452 reads `app.desk.chat().ui.input_buffer`, which is chat's private UI state, from an app test. Assert the drawn input text instead.
   - Line 385 depends on egui memory id `"command_palette"`.
   - Lines 195-199 dig `Param::Search { found }` out of `bound_actions()` instead of reading the drawn rows.
   - Medium-low: a rename of the id or field breaks the test, not behaviour.

10. **Wording-driven tests:**
    - Lines 344-347, 357, 360-362 and 430 invoke edits by fuzzy-typing labels ("rename the tag flaky", "give flaky another colour"). A label rewording fails them while behaviour is unchanged. Using `run_by_id` or the row's buttons would test behaviour.
    - Lines 324-326 and 353 check only that the words "Rename", "Colour", "Note", "Delete", "Go to it" and "Take off" appear somewhere in the frame. No button is clicked, so whether a row button runs its edit is unprotected.
    - Line 330 checks `" d ago"` with `at_ms: 0`. That only proves some age is shown, not the tagged time; a fixture at now minus 2 days with "2 d ago" would.

    Medium-low.

11. **Timing:** no problem found. The Tags rest check (lines 306-315) uses simulated time; READ_AFTER is 0.3 s and the check comes 0.5 s after resting, which is deterministic. The `d ago` check has years of headroom.

12. **Self-testing:** none found. Each assertion checks requests sent, the cursor or drawn output after real input, not values the test set itself.

13. **Layout test:** `the_palette_keeps_its_place_and_size…` (377-402) tests layout, but the plan explicitly asks to measure the palette, so keep it.

14. **Plan behaviour this change relies on that no test protects, beyond the above:**
    - The gutter is always reserved, so content doesn't shift as tags come and go (`tag_gutter()` is unconditional).
    - The swatch before a colour choice in every picker (`Choice::colour`, F.2a).
    - Delete from the cursor in a non-Tags view, and the "pick which tag, then delete" path.
    - Column order or stability when scrolling. Columns are assigned per frame among tags that are visible, so a tag's column moves as others scroll out. Nothing pins it, and the user would see bars jump.

**Net:** cut the duplicates in item 8. Fix the vacuous check in item 1 and the missing Backspace in item 2. Add four small tests: distinct-colour columns plus the 4th-tag cap, bar hover/long press, `#` Escape and no tags, and the logs folder 0700.

# End review: tests (menus and lists)

Scope: the tests changed or added in eda7934, a7f349c, c464f49, 856f36c, 17d4757 and 03c4a3e, and what the plan relies on that no test protects. Paths are under `meta-harness/crates/cockpit-ui/src/`. Ranked, most serious first.

## A. Assertions that can't fail, or were loosened

1. **The test-only gather counter was narrowed, and the cost it guarded is now invisible.** `app/commands.rs:180`: 17d4757 changed `#[cfg(test)] if keyed` to `if keyed && here == self.here()`. In the same commit, `shown_at` started a keyed gather for every panel that isn't focused (`gather(here, true)`). So `a_frame_gathers_the_actions_once_and_again_only_after_one_runs` (`app/tests/bindings.rs:157-168`) no longer counts those per-frame gathers, and nothing else does. The assertion stayed the same while what it measures shrank. Either count every keyed gather and assert "once per panel", or add a separate count for panels that aren't focused.

2. **The settings popup's "panel keys stand aside" check presses a key the panel doesn't have.** `app/tests/views.rs:316-320` presses ArrowUp, then Enter, and asserts the graph's `selection` didn't change. ArrowUp is bound only in `[keymap.list]` (`keymap/local.rs:43`, `config/default.toml:46`), never as a graph key, so it can't show the panel standing aside. `nav.enter` changes the root, not `selection`. The stand-aside in `app/keys.rs:83-84` (`InputMode::Normal if egui::Popup::is_any_open`) would be untested if removed. Press `j` (`nav.down`, which moves the graph's selection) while the popup is open, and assert the selection didn't move.

3. **The chat's sign-in, notice and error buttons can go back to the top line and no test fails.** `chat/tests.rs:296-298`: `on_top` now accepts `At::Header | At::Notice`. Every chat test that used it (lines 313-381) passes whichever row an action is placed on. Step 3's change (Connect, Resume, Stop, the sign-in steps, Dismiss and Retry moved to the notice row) isn't held by any test. Split it into `on_notice` (sign-in steps, Dismiss, Retry, Stop) and `on_top` (settings only).

4. **The half-page test was loosened from exact values to inequalities.** `pick_list.rs:336-337`: eda7934 asserted `[9, 0, 3, 6, 9, 9, 6, 0, 9]`. The test now checks only `went[2] > 0 && went[3] > went[2] && went[5] < 49`. Its doc comment (lines 319-322) says "half a window moves the list and the highlight together", but no scroll offset is read. "Half down at the end stops at the last" is no longer checked. Font-dependent sizes don't stop an exact check: assert that both half-downs are the same step, and that the list's scroll offset moved by that step × stride.

5. **The pinch-pan threshold dropped from 30 px to 5 px.** `app/tests/hand.rs:125` (17d4757): two fingers moved together by 40 px now only need to pan the graph 5 px. A pan scaled wrongly (for example, divided by zoom twice) would pass. The cause is the shorter graph area under the new top line. Give the pan room instead: zoom in first or move upward, then assert pan ≈ finger travel.

6. **Some assertions test the network mock, not the app.** `app/tests/links.rs:147` asserts that `net.answer` returns false for a call that was already answered, which is a property of the test mock. The app dropping a late answer isn't what it checks. `code_search.rs:232` ("an answer for a round gone is dropped") and `:246` ("its answers dropped") assert `answer(...) == None`, but `answer` returns `None` for every `Ok`, kept or not (see line 223). Line 245's `is_out() == false` does carry the meaning of `:246`. `:232` carries nothing: its `Files` answer is overwritten at 233. Cut these three assertions, or make `answer` say whether it kept the answer and assert that.

7. **The long-press test accepts either outcome.** `actions.rs:845` asserts `said || frame(1.7, ...)`, so "shown while held" is never checked on its own.

## B. Behaviour the plan relies on that no test protects

8. **The per-kind key cache.** `app/tests.rs:324-327` (`kept_keys`) reads only the focused kind's slot. `keys_are_given_out_again_only_when_the_actions_change` (`app/tests/bindings.rs:170-186`) calls `bound_actions()` twice with no frame between, so the other panel's gather never runs in between. Going back to a single-slot cache (thrashing between kinds every frame) would pass. Fix: frame between `first` and `again` with two panels of different kinds on screen, and assert that `kept_keys` is unchanged.

9. **Headings in the pick list.** `pick_list.rs`'s test harness (`Shown`, lines 262-313) only makes Pick and Inform rows. These are untested:
   - moves skipping a heading (line 153);
   - the pointer passing over one (line 181);
   - the highlight starting past a heading on row 0 (lines 136-138).

   Every `?` list starts with a heading. Only `app/tests/terminal.rs:536-540` covers the start skip, and only incidentally. Add one heading row to `Shown`, and assert that Down crosses it and that hovering it leaves the highlight where it was.

10. **Groups showing only when something in them applies, and only groups about where you are.** `app/draw/which_key.rs:22` (`is_primary`) and `:27` (`when.holds`) are untested. `app/tests/bindings.rs:56-58` and `app/tests/menus.rs:18-22` assert that some groups are present, never that one is absent. Add a case where a group's every action has a `When` that doesn't hold, and assert its button isn't drawn.

11. **The `?` list's own logic.** `sheet.rs` has no test module. These are untested:
    - section order by best match (`lines`, lines 120-146);
    - a faded action that takes a value being Inform, not picked (line 155);
    - a section for the keys of the mode `?` was opened from (`sheet_mode`, set at `app/commands.rs:448-451`, read at `app/draw/bars.rs:177-179`).

    `question_mark_lists_everything_by_section_faded_where_it_doesnt_apply` (`app/tests/bindings.rs:193-230`) only greps the drawn text. Its `everything()` helper (`app/tests.rs:230-236`) skips `ShellCommand::Keys`, so it can't reach `sheet_mode`. A unit test of `Listed` over three or four `Bound`s would cover all three, more strongly than the text greps.

12. **The which-key menu itself.** These are untested:
    - tapping an entry in the menu (`app/draw/which_key.rs:132-133`, then `151-152`);
    - the Escape button and Escape going back one level (`:91-94`, `:153`; `app/keys.rs:124`);
    - entries showing only their next key.

    Only tapping a path part is tested (`app/tests/bindings.rs:62-64`).

13. **Long press on the real controls.** `actions.rs:825-849` tests `tip_on` on a bare egui button. The group buttons (`hosted.rs:194`) and the which-key entries (`app/draw/which_key.rs:132`) are where the plan needs it. If either went back to `on_hover_text`, the test would still pass. One app-level long press on a group button, asserting its tip text, covers both paths through `tip_on`.

14. **The re-read's pick-list fixes.** No test covers either:
    - going round at the ends respects a scroll by hand (`pick_list.rs:213-214`, re-read fix 6);
    - a key's move wins over a twitching pointer (`pick_list.rs:177-178`, re-read fix 7).

    `Shown` can drive both: a wheel event, then Down at the last row; and a PointerMoved plus a move in the same frame.

15. **The notice row's behaviour.** The host draws it only while there's something (`hosted.rs:173-175`), between the top line and the settings row. Its drawing is reached only by clicking the graph's "Show 2 levels" (`app/tests/views.rs:23`), and that click passes on either row. Nothing asserts that the row is absent when empty, or that a notice button sits below the groups and above the settings.

16. **The box keeping its place and size, for `?` and the Tab switch.** `the_palette_keeps_its_place_and_size_while_typing_and_whatever_its_rows_say` (`app/tests/tags.rs:413-437`) covers `:`, `M` and `gT`. It doesn't cover `?`, where headings change the row count, or Tab between everything and what applies, the switch step 2 added. Add `("?", "sign")` and a Tab to its loop.

17. **Code search rows.** The "N of M" note stays on the last row listed when a row is left out (re-read fix 11, `code_search.rs:165` with `:172`), and a file's detail is "file", not "file · file" (fix 12, `:167`). Neither is tested. `graph/actions.rs:331-333` checks "1 of 40" only with no row left out.

## C. Duplicated tests, and tests reaching into internals

18. **`app/tests/links.rs:89-119`** re-declares the chat setup and a `typed` closure that match the file's own helpers `typing_in_the_chat` and `typed` (lines 7-21). Use the helpers.

19. **`graph/actions.rs:319-327`** repeats the body of `search_step` (lines 298-306) inline. `:355-366` re-tests CodeSearch's rules (rows kept while the next query is out, a late answer dropped) by calling `g.find.search.tend(...)` directly. Those rules are already tested in `code_search.rs:212-251`. Keep what is the graph's own: a pick making a root or view or asking the code, and a pick from the round before (`:367-372`).

20. **`app/tests/views.rs:103-117` and `:123-151`** both assert three searches asked for "run". `:110` reads `find_choices()`'s `of` value, which is internal. `:115-116` tests `arch_search::rank` on literal strings, not the palette. Merge the first test into the second and drop `:115-116`.

21. **`everything()` (`app/tests.rs:230-236`) and `app/tests/look.rs:59-60`** set `app.palette` and `app.input_mode` directly instead of pressing `?` and typing, so the `?` key path (`ShellCommand::Keys`) runs only once, at `bindings.rs:196`. Have the helper press `?`, then type the filter.

## D. Wording, layout, small things

22. **Wording assertions.** These assert wording where a behavioural assertion is beside them or could be:
    - `app/tests/links.rs:145`: "Looking… · Enter runs";
    - `app/tests/bindings.rs:199`: "Every action and key", "Tab: what applies here";
    - `app/tests/bindings.rs:207/209`: "Panel 2 · ", "Everywhere · ".

    Keep one that the waiting state is said. Drop the rest, or assert on `Listed`'s titles in a unit test (item 11).

23. **A click that depends on draw order.** `app/tests/bindings.rs:60`: `click_on("Buffer")` matches both "Buffer" and "Buffer  b" (`app/tests.rs:142`), so which panel gets the tap depends on which is drawn first. Target the panel that isn't focused by its rect, or match the exact text.

24. **A test name that promises more than it checks.** `app/tests/menus.rs:6`: the name says the groups "wrap inside the panel". With one panel the size of the screen, the test only asserts each group is within the screen (lines 18-22), not that they wrapped onto more than one line. Assert that at least two groups are on different rows. `drawn_containing(...)[0]` (line 23) panics on an index error rather than saying what's missing.

25. **The move to the first row is never asserted.** `keymap/local.rs:282-288` builds `first: ctrl+Home` but never asserts `Motion::First`.

26. **Misplaced doc comments.**
    - `app/tests.rs:229`: the doc of `sent` ("The calls sent to the server since last asked") now sits on top of `everything`, and `sent` (line 239) has none.
    - `chat/tests.rs:293-295`: two doc comments stacked; the first is stale.

## Cut or merge (fewer, stronger)

- Merge `views.rs` `find_in_the_code_asks_the_graph_as_its_typed` into `find_in_the_code_works_from_anywhere_and_starts_afresh`; drop its `arch_search::rank` lines.
- Cut `graph/actions.rs:355-366` (CodeSearch's rules, tested in `code_search.rs`) and the inline copy at 319-327.
- Cut `links.rs:147`, `code_search.rs:232` and `:246` (they test the mock, or nothing).
- `links.rs:89-119`: use the shared helpers.
- Replace the text greps for `?`'s structure (`bindings.rs:205-214`) with one `sheet.rs` unit test of `Listed` (item 11), keeping one app test that `?` opens, Tab switches, and Escape closes it and nothing else.
- Fold the long-press unit test into one app-level long press on a group button (item 13).

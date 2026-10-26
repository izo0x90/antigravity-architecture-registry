# End re-read: correctness and tests (03c4a3e..1694b22)

Paths are under `meta-harness/crates/cockpit-ui/src`. I only read the code; nothing was built or run. "Untested" means no test fails if the fix is undone.

## 1. The end review's findings

### Correctness (`end-correctness.md`)

1. **A tap on the which-key menu lands on the panel under it.** Fixed. `render_panel` focuses only when `layer_id_at(pos)` is the panel's layer (app/draw/panels.rs 31-39). Untested: no test taps the menu over a panel that isn't focused.
2. **The quick look's own menu by tap.** Fixed. A click on a Foreground layer no longer closes it (app/look.rs 155-158). Untested.
3. **First tap on a `.focused()` control in another panel does nothing.** Fixed. A focus change calls `forget_frame` before `run_asked`. The chat's banner buttons are now item controls, drawn on the focused panel only.
4. **The graph's chips took "View" away.** Fixed. The chips are gone, and `header` draws `At::Settings` only.
5. **A group tapped while the action list is open.** Partly fixed. `open_menu` closes the list, so keys no longer go to both. But closing it this way skips the list's own clean-up (new problem 4).
6. **A group tapped during a capture.** Partly fixed. `open_menu` now sets `capture = None`, but it doesn't end the capture (new problem 2). The new `Choosing` mode brings the stale capture back by another way (new problem 3).
7. **Keys stand aside for a setting's list only in Normal.** The double Enter is fixed: `InputMode::Choosing` has the keys. But `Choosing` replaces Capture and Leader (new problem 3).
8. **Dead top-line placements** (code's Re-read, `tree.clear_filter`, `terminal.restart`). Fixed: they are now an item control, a menu entry and an item control.
9. **A group tapped in another panel: one frame built from the old list.** Fixed (`forget_frame` in `render_panel`).
10. **A long press on a top-line control is also a gesture on the view.** Fixed for panels: `on_gesture` returns unless the press is on a content rect or a note mark. Not fixed on the quick look's own top line (new problem 9).
11. **A discussion's notice row with nothing to do.** Fixed. The row is gone, and Settle and Reopen are item controls filtered by `applies`.
12. **`?` never lists the mode's keys; `sheet_mode` left over.** Fixed: `mode_keys(self.palette_from)`, and `sheet_mode` is gone.
13. **A faded row that takes a value does nothing.** Fixed. `Offered::refused` says why when it's picked. A `When::Unavailable` row still gives no reason until it's picked.

### Tests (`end-tests.md`)

1. Fixed: every gather is counted, and the test expects one per panel.
2. Fixed: `j` is pressed while the list is open and moves nothing, then moves the selection after it closes.
3. Fixed at the action level: `on_banners` checks `At::Item`. Nothing at app level checks that the buttons are drawn under the banners.
4. Fixed: the list's offset is asserted exactly; the highlight is asserted to a row, with the reason given.
5. Fixed: the pan equals the fingers' travel.
6. Partly fixed. `links.rs:147` was cut. `code_search.rs` 243-246 still says "an answer for a round gone is dropped (never shown)", and nothing checks it: the late `Files` answer at 245 would be overwritten by 246 even if it had been kept. Same flaw as before, with the assertion removed and the claim left in the comment.
7. Fixed: replaced by an app-level test.
8. Fixed: two kinds are drawn between the reads.
9. Fixed, with a weak spot (problem T3).
10. Partly fixed. `a_menu_with_nothing_usable_here_isnt_offered` covers the `applies` filter. The other filter, groups that only act everywhere (`layer < Everywhere`, which_key.rs 39), still has no test for being left out.
11. Fixed: `sheet.rs` has its own tests, and the mode section has an app test.
12. Fixed.
13. Fixed. The test passes on hover alone, since `PointerMoved` gives egui's tip. That's acceptable because there is no separate long-press path.
14. Fixed.
15. Moot: the notice row is gone. The banners that replace it are tested only at the action level (see 3).
16. Fixed.
17. Fixed.
18. Fixed.
19. Partly fixed. The copies are gone, but the new "its picker left" part is vacuous (problem T2).
20. Fixed.
21. Fixed.
22. Partly fixed. These wording assertions remain: "Looking… · Enter picks", "Every action and key", "Typing: its keys", "This panel (2)".
23. Fixed.
24. Fixed.
25. Fixed.
26. Fixed.

## 2. New problems, and ones still there (most serious first)

### 1. The top bar's "Menus" button opens a value picker, not the leader's menu (app/draw/which_key.rs 62; app/draw/top_bar.rs 182-184; app/commands.rs 42, 323-331)

- `every_menu` asks for `("menu.open", None)`.
- `menu.open` is declared with `Param::Choice(choices)`.
- `run_asked` → `run_by_id` sees `value.is_none() && param != Param::None` and calls `open_palette`.

Sequence:
1. Tap "Menus" on the top bar.
2. The action list opens at "Open a menu", a list of group names. The leader's root menu doesn't open.

`ShellCommand::Menu`'s `None` → root branch is never reached from the button. The plan says this button "opens the leader's own menu", and it's the only touch way into the groups that act everywhere. No test taps it.

### 2. Tapping a group during a search drops the search without ending it (app/draw/which_key.rs 73)

`open_menu` sets `self.capture = None`. It doesn't do what Escape does (`on_capture` with `back`: `on_search(Cancelled)`, `restore`, `restore_view`).

Sequence:
1. Tree panel focused. Press `/` and type "foo". The tree filters to "foo".
2. Tap any panel's "Buffer" group, then press Escape.
3. The tree is still filtered by "foo", with `filter_before` kept and the view not restored.
4. Nothing says a search is on, `desk.searches` has no entry so `n` does nothing, and Escape can't undo it.

### 3. `InputMode::Choosing` takes over any mode, and on close leaves a capture behind (actions.rs 793-803)

`if open && *mode != InputMode::Palette { *mode = choosing }` replaces Capture, Leader and Insert. On close the mode becomes `Normal`, and `self.capture` is left as it was. This is end-review finding 6 again, by a click instead of a group tap.

Sequence:
1. Press `'`. The marks pop-up shows.
2. Click the graph's "View" setting and pick a view.
3. Now `input_mode == Normal` and `capture == Some(MarkJump)`:
   - `render_prefix_popup` (`marking` reads only `capture`) keeps drawing "marks";
   - the key bar keeps saying "jump to mark: press its letter";
   - letters run Normal commands;
   - Escape doesn't clear it.

With `/` (search) instead, the key bar keeps "/foo| N matches", and the view is left as in problem 2.

### 4. A group tapped while the action list is open closes it without its clean-up, and what was written is lost (app/draw/which_key.rs 70-72; app/draw/panels.rs 101-119)

`render_palette`'s clean-up runs only while `input_mode == Palette`, and `open_menu` has already set `Leader`. So:
- `desk.discussing`, `desk.tag_editing` and `desk.discussions_here` aren't cleared;
- `palette_from` isn't restored;
- `held` stays `Some`.

Sequence:
1. On a component, start a question. The list's write stage is "Question on X"; type a paragraph.
2. Tap "Buffer" on a panel's top line. The list closes.
3. The text is gone: `Palette::open` resets `writing`, and `Discussing::text` was never updated.
4. `desk.discussing` is still `Some`, so "Question on X" (`discuss.write`) and "Who answers…" stay offered in `:` and the menus. Picking one starts from an empty box.

`palette.holding()` stays true until the list next opens, so `link.thing` and `tag.insert` count as applying everywhere (`writing_here`) in the key bar and the menus meanwhile.

### 5. Each view's situation is kept per kind, not per buffer (app/views.rs 46-55; app/layout.rs 31-34; app/draw/which_key.rs 33)

`situation_of(kind)` keeps the first `Situation` worked out in the frame. That is the focused buffer's, from the key bar. `draw_as` then switches which buffer `desk.view(kind)` is for each panel, but the kept one is returned anyway. Before the fix, `groups_of` worked it out fresh each time.

Sequence (Code, Graph, Spec, Chat, Tree and Terminal can have several buffers):
1. Two graph panels. The focused one has a card selected; the other has nothing selected.
2. The other panel's top line shows "Quick look" (`When::Component`), judged on the focused panel's card. The plan's rule: a group shows only when something in it can be used there.
3. Tap it: the panel is focused, and its menu holds nothing that runs ("Nothing here to look at").

No test has two panels of one kind.

### 6. The graph's find sends one more round after its picker closed (graph/mod.rs 522; app/draw/panels.rs 45-53)

- `GraphState::tick` drives `tend_find` every frame with `find.query`, during the panels' draw.
- `search_done` is called later in the frame (start of `render_palette`), so the frame after the picker closes still drives the old query.

Sequence:
1. Type "ab", then "c" quickly.
2. Pick a row, or press Escape, while "ab"'s round is out.
3. Next frame, "ab"'s answers are in, so the tick sends a round for "abc": three searches, text included.
4. `search_done` then drops them.

### 7. The key cache thrashes at the configured panel maximum (app.rs 213; app/commands.rs 210-230)

- `KEPT_KEYS = 8` with FIFO eviction, and `[panels] max = 8`.
- With 8 panels and the quick look open, `here` is the look, so 9 hosts are gathered each frame.

Every lookup misses, so `bind_actions` runs for all 9 every frame. That breaks the rule that keys are given out again only when the actions change. It's also a cost.

### 8. The key bar shows the action list's keys from the frame before (app/draw/bars.rs 89-93; palette.rs 534; app.rs 584 vs 625)

`render_key_bar` runs before `render_palette`, which sets `keys`. Nothing asks for a repaint when they change.

Sequence:
1. In `:`, press Tab.
2. That frame, the list's line says "Tab: what applies here" while the key bar still says "Tab: everything".
3. They match only on a later frame (the caret's blink, or the next input).

On the frame the list opens, the key bar's line is empty: `open` resets `keys`. The claim "one key line" holds only a frame late.

### 9. A swipe or long press from the quick look's top line still reaches its view (app/draw/host.rs 252-255)

`on_gesture` checks the press against any host's content. The quick look floats over its panel's content, so a press on the quick look's top line passes the check.

Sequence:
1. Open the quick look.
2. Swipe starting on its "Quick look" group button. The swipe's binding runs on the quick look's view.

The check should be against the content of the host `view_at` returned.

### 10. The setting list's way out is said from the keymap but is egui's Escape (app/draw/bars.rs 44; actions.rs 781-784)

The key bar says "`<back>` closes". The list never reads `keys.back`: only egui's hard-coded Escape (popup.rs) closes it.

Sequence: rebind `back` to `ctrl+[`. The key bar says "Ctrl+[ closes", but pressing it does nothing. This breaks "keys come from the keymap config only".

### 11. Claims that don't hold as written (findings, Part 2)

- **"`Offered::refused` is asked by … the controls (through `run_bound`)."** Controls go `run_asked` → `run_by_id`, which never asks `refused`. It can't be reached today: no setting has a `When`, and item controls are filtered by `applies`. But a setting given one later would run where it doesn't apply.
- **"Escape goes back to it, as it was."** `back()` keeps only the reach. The filter typed and the highlight are cleared (palette.rs 300-305).
- **The box keeps one size.** A refusal's line (`said`) makes the box taller, and `tallest` keeps it that tall for the rest of the opening (palette.rs 531-540). The plan's rule 4 says one size whatever it shows.

### Tests that can't fail, or don't reach what they claim

- **T1.** `code_search.rs` 245-246: see tests item 6.
- **T2.** Two "dropped after it's cleared" checks can't fail:
  - `code_search.rs` 299-303 drives with `""` after `clear()`, and an empty query empties the rows whether or not the late answer was kept.
  - `graph/actions.rs` 373-377 answers only Symbols of three kinds, so the round wouldn't be shown anyway. The rows before were "run"'s, which were empty. "the find starts afresh" passes with `search_done` doing nothing.
  - Asserting `find_choices().1 == Some("")`, or answering every kind, would make them count. The app test in views.rs ("opened again: nothing of the search before") does protect the app's path.
- **T3.** `pick_list.rs` `a_heading_is_passed_by`: the highlight is already 2 before the pointer goes to "8 px above row 3's title". If that point lands on row 2 or the gap, the assertion `== 2` holds with the heading guard (line 181) removed. Start the highlight elsewhere (4, say) so that only "stayed where it was" passes.
- **T4.** Nothing tests:
  - the Menus button (problem 1);
  - a which-key tap over a panel that isn't focused (finding 1);
  - a menu tap with the quick look open (finding 2);
  - `Choosing` entered from a capture or the leader (problem 3);
  - `open_menu` during a capture or with the list open (problems 2 and 4);
  - two panels of one kind (problem 5);
  - the banners' buttons drawn under the banners at app level (checked live only).

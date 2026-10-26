# Checkpoint re-read: correctness, with performance (a7f349c)

Paths are under `meta-harness/crates/cockpit-ui/src`. Problems are ranked, most serious first. Each one gives the sequence that goes wrong.

## High

### 1. A pending Enter picks a row the user never saw (palette.rs 327-333, 405-410; pick_list.rs 105, 147)

The highlight is an index. It carries over from the old rows to the new rows, and a pending Enter fires on the new rows at that index. New rows arriving never call `reset()`; only a filter change does (palette.rs 405).

Sequence, in the graph's find or `@`:
1. Type "a" and wait for its rows.
2. Type "ab". The rows of "a" stay up, and `reset()` puts the highlight on 0.
3. Press Down twice to reach the row wanted among the rows still shown (index 2).
4. Press Enter. `of` is "a" and the filter is "ab", so Enter waits and the key line says "Looking…".
5. The rows of "ab" arrive. `enter_waiting` is taken (line 332) and `show(.., enter = true)` picks `found[2]` of the new rows, a row that was never highlighted on screen.

Resting the pointer over a row while waiting does the same, because hover sets the index.

Typing after the Enter doesn't cancel it either. Line 409 only ORs the flag in. Type "foo", press Enter, keep typing "bar": whatever row 0 of "foobar" turns out to be is picked when its rows land.

### 2. Back-to-top is skipped when the Viewport thinks it's already at 0; reopening is never sent back to the top (pick_list.rs 106-107; viewport.rs 232-234; palette.rs 131)

The palette has one `Viewport` but three scroll areas: `palette_actions`, `palette_choices` and `palette_found`. egui keeps each area's offset under its own id, persisted and across openings.

`to_top` calls `take(Moved { offset: 0 })`, which sets a glide only if `self.offset != 0 || self.at != 0`. Those values describe the last area drawn, not the one about to be drawn. When they're 0, nothing is asked, `wanted()` is `None`, and the new area comes back at its own stored offset.

- Sequence A (plain reopen):
  1. Open the palette and press ctrl+End.
  2. Press Escape (`back()` → `close()`).
  3. Open it again. `open()` resets to `PickList::default()`, so `to_top` is false and the offset is 0.
  4. `palette_actions` comes back scrolled to the bottom, with the highlight on row 0 off screen.
  5. Enter runs the first action without it being seen.
- Sequence B (Choose stage):
  1. Open "Go to something tagged" (many tags), scroll to the bottom and pick.
  2. Later, open the palette and type a filter. The action list is at the top, so offset and `at` are 0.
  3. Enter on any action with more choices than fit. `enter()` → `reset()` → `take` does nothing.
  4. `palette_choices` comes back at the old offset, with highlight 0 hidden. Enter picks it.
- Sequence C (setting popup, actions.rs 733):
  - `PickList::at(current)` sets the highlight but never brings it into view, and the ComboBox passes `moved: None`.
  - In `graph.path`, the current step ("here:…") comes after every `back:i` step. With a trail longer than about seven, the ticked, highlighted row opens below the fold.
  - On reopening, the popup's inner scroll area also comes back at its last offset.

### 3. "Graph · Find in the code" from another view, with the graph not on screen: Enter held forever, "Looking…" forever, and the 8 s limit never applies (app/commands.rs 281-286; graph/mod.rs 522; app/draw/host.rs 68)

- `palette_actions` offers every non-focused view's `Scope::Focused` actions, so graph.find shows up from every view.
- The query reaches `GraphState::search` → `typed()`.
- `tend()` runs only in `send_find`, from `GraphState::tick`. `tick` is called only from `host.rs` for a buffer being drawn.

Sequence:
1. In a layout with no graph panel, open the palette and pick "Graph · Find in the code".
2. Type "foo". `of` stays `None` or whatever was left from before. No round is ever asked, so the round limit (`ROUND_LIMIT_S`) never counts down.
3. The list says "Looking…" for good. Enter sets `enter_waiting`, and the key line shows "Looking…" until Escape.

The findings say "a stalled request no longer holds Enter". That doesn't hold here. The old `waiting` flag hung the same way.

## Medium

### 4. The setting popup's list sits inside egui's own 200 px ComboBox scroll area (actions.rs 735; egui combo_box.rs 397-408)

- `ComboBox::show_ui` wraps its contents in `ScrollArea::vertical().max_height(spacing.combo_height)`. That's 200 by default, and the code doesn't change it.
- `PickList::fitting(.., 8)` sizes the list to up to 8 rows:
  - with details: about 38 px per row plus 3 px spacing;
  - without: about 26 plus 3.
- So with 5-6 or more choices that have a description, or 7 or more without, the list is taller than 200. The popup grows an outer scrollbar and clips the list's last rows.
- With more than 8 choices there are two nested vertical scroll areas. The list's Viewport `window` (its own 8-row height) also isn't what's visible.
- Settings this hits: `graph.path` (details on, one row per trail step) and `graph.plan` (one row per plan). The test (app/tests/views.rs 267) uses 3 choices.

### 5. A round left behind is never dropped: Escape from `@`, and the graph's find has no `clear()` at all (app/linking.rs 184-189; code_search.rs 88; graph/actions.rs 225-252)

`clear()` is called only in `put_link`, the `@` pick (linking.rs 270). The graph never calls it. Findings item 7 ("CodeSearch::clear drops the round out") is wired only for `@`.

- Sequence A (`@`):
  1. Type "foo" in `@` with the index slow, then press Escape. The round stays out.
  2. `find_links` returns before `tend` whenever the picker isn't open, but `answer()` still runs every frame.
  3. Reopen `@` within 8 s and type "bar". `tend` returns early at line 88 because a round is out, so "bar" isn't asked until the abandoned "foo" round answers or times out.
- Sequence B (`@`, notes for a search already left):
  - If the abandoned round fails while the picker is closed, "Couldn't search the code: …" is said then (linking.rs 180-181).
  - If it never answers, the next opening after 8 s first says "The code index didn't answer in time: what came is shown." about "foo".
- Sequence C (graph):
  1. Type "a" (round 1 out), then "ab".
  2. Round 1 answers and round 2 ("ab") goes out.
  3. Backspace to "a". `of` equals the filter, so pick a row. The palette closes.
  4. Round 2's answers are still taken, and its rows become what's shown.
  5. `tend` then sees typed "a" ≠ shown "ab" and sends round 3 for a search nobody is looking at.
  6. The next graph.find opening shows those rows for two frames: the actions are gathered before the graph's tick takes the new "".

## Low

### 6. Going round at the ends ignores a scroll by hand (pick_list.rs 158-159)

The other moves go through `viewport.go`, which first brings the cursor onto the screen when `by_hand` is set. The wrap arms jump to `First` or `Last` before that.

Sequence:
1. Highlight on row 0.
2. Scroll the action list by wheel or finger to rows 20-26.
3. Press ctrl+K (Up). Every view's rule gives about row 25. The list goes to the last row, 146, scrolling away from what was being looked at.

The mirror case: highlight on the last row, scroll to the top by hand, ctrl+J lands on row 0 rather than one below the top row shown.

### 7. Moves use the last frame's window and content (pick_list.rs 109-110, 165-166; viewport.rs 195, 199-203)

- Before the list has ever been drawn, `window` is 0:
  - `HalfDown`: the step is 0 and `y == offset`, so the highlight goes to the last row;
  - `Last`: the offset is `max_offset()` = 0, so the last row is highlighted and not shown.
  - This needs a list key in the same frame the palette opens (e.g. `open_for` from a leader key followed at once, or batched remote input).
- When the list grows in the same frame as a move, `content` is still the old one. For example, the tag rows changing the frame after a key, or search rows landing:
  - ctrl+End puts the highlight on the new last row with the offset clamped to the old end;
  - ctrl+D from 3 rows to 30 is taken as "at the end" and goes to the last row.
  - Nothing later brings it into view.

### 8. Hover overrides a key's move made in the same frame (pick_list.rs 109-111, 132-134)

- `go()` sets the highlight and starts the glide to the key's row.
- Then, if the pointer moved at all that frame (trackpad jitter is enough), the hovered row takes the highlight.
- The view ends up scrolled to the key's target with the highlight on another row, which may be out of sight after the glide.

### 9. A graph pick can name a round that is no longer shown, and nothing happens or is said (graph/actions.rs 250; app.rs 528-530)

- `bound_actions` is cached for the frame. It's filled in `update_lifecycle` and the top bar, before the panels draw.
- The graph's tick takes answers and runs `tend` while drawing. So when the graph is focused, the palette in the same frame lists round N's rows, and `of` is N's.

Sequence:
1. Typed "a", round 1 shown, and round 2 ("ab") out from typing ahead, then back to "a".
2. Press Enter in the frame round 2's last kind lands.
3. `of` "a" equals the filter, so the pick is `1:symbols:0`. `hit(1, ..)` is `None` because shown is now 2.
4. The palette closes and nothing goes anywhere, with no note.

### 10. A file hit with no card reads "file · file" (code_search.rs 150; arch-service service/search.rs 66)

The server sets `detail: "file"` when no card stands for the file. `rows()` prefixes `what` ("file"), giving "file · file".

### 11. The same setting starts its highlight in two places (palette.rs 168; actions.rs 732-733)

The panel popup starts on the current choice. The palette's Choose stage for the same action (e.g. "Switch view") starts on row 0, because `enter()` → `reset()`.

## Per-frame costs

- **The graph's find rows rebuilt every frame for the app's life after one find** (graph/actions.rs 110 → `find_choices` → `CodeSearch::rows`):
  - `actions()` is gathered every frame: `bound_actions` when the graph is focused, and `render_palette` calls `palette_actions()` unconditionally (app/draw/panels.rs 55), even when the palette is closed.
  - The graph never clears its shown round, so after any find, every frame builds up to 26 `Choice`s (12 + 6 + 8 by default). Each is a `format!` value, a label clone, a `format!` detail, plus the per-kind tail `format!`s.
  - `link_actions` guards its rows with `picking`; the graph's don't.
- **`PickList::show`, while a list is up:**
  - one `laid` Vec per frame (line 126);
  - per visible row: `detail.to_owned()` for the detail's `LayoutJob` (line 189), a `format!` when the row is current (line 183), and in the action list a `key_hint()` String (palette.rs, the `row` closure);
  - two `fonts_mut` locks per frame in `row_height`.
- **Search stage:** `found.clone()` and `of.clone()` (palette.rs 466) on top of `self.stage.clone()`, which already clones the found rows. That's two full copies of the rows per frame. Already recorded as performance 4.
- **Not per frame:** the `stops` Vec (pick_list.rs 165) is built once per key move, O(len) (147 rows here). ComboBox: one `get_temp` clone and `insert_temp` per frame while open, one `remove` per drawn Setting control while closed. All negligible.

## Views on `Go::of`

There are ten views, not eleven: changes, chat, code, discussion, inbox, queue, spec, tag_list, terminal and tree. The pick list is the eleventh caller. No motion changed behaviour:
- code's and tree's "first Down lands on the first" is checked before `Go::of`;
- chat's "first Down/Up lands on the newest" and `Line` still come before the `_` arm;
- the new `_ => {}` catch-alls only take what used to be listed as no-ops.

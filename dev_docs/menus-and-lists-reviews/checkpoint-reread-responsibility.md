# Checkpoint re-read: responsibility, with consistency (commit a7f349c)

Paths are under `meta-harness/crates/cockpit-ui/src`. I checked each fixed finding against the code, then the rework itself. Ranked, most important first. Findings not listed here hold in the code as the findings file says.

## 1. The setting picker's list sits inside egui's own 200 px scroll area, and its starting highlight can be out of sight (fix 12 incomplete)

- `actions.rs:726-737`: the `PickList` is drawn inside `ComboBox::show_ui`. In egui 0.33.3, `ComboBox` wraps its contents in `ScrollArea::vertical().max_height(combo_height)` (`egui/src/containers/combo_box.rs:397-408`). `combo_height` defaults to 200 and nothing in cockpit-ui changes it.
- `PickList::fitting(ui, len, details, SETTING_LIST_ROWS)` (`actions.rs:735`, rows = 8) sizes the list to up to 8 rows.
  - With this theme (`text_label` 12, `text_small` 10, `space_sm` 6), a row with a description is about 44 px, and one without is about 31 px.
  - So a setting with more than about 4 described choices, or about 6 plain ones, is taller than 200 px.
- How it goes wrong:
  - Two scroll areas are nested. The bottom rows can be reached only by scrolling egui's outer one.
  - The list's `Viewport` measures its own window (up to 8 rows), not the 200 px that shows. So "room around the highlight" and the by-hand rule are computed against rows that are clipped.
  - This is the second scrolling engine the checkpoint removed, back in through the container.
- `PickList::at(current)` (`pick_list.rs:66-69`, used at `actions.rs:733`) sets only `highlight`. `show` never brings the highlight into sight unless a key moves it (`go`, `:109-111`).
  - When the current choice is below the first screenful (index 8 or more, or less given the clipping above), the popup opens with the ticked, highlighted row hidden.
  - The findings file says "the highlight starts on the current choice". It starts there off screen.
- Keys: `list.show(ui, listing, None, false, …)` (`actions.rs:737`). The popup takes no list keys and no Enter.
  - Consistency finding 8 named "no list keys and no filter" as part of the second way.
  - The findings file doesn't list this under "Not changed".

## 2. An Enter that waits is applied by index to rows that replaced the ones the highlight was moved over

- The contract now lets the user move on rows of an earlier query: `actions.rs:34-38` ("Rows of another query than what's typed are still shown").
- `palette.rs:404-411` resets the list only when the filter changes. Nothing resets it when the rows' `of` changes.
- Sequence:
  1. Type "ab". The rows of "a" stay.
  2. Ctrl+J twice: highlight 2, on a row of "a" the user is looking at.
  3. Enter. `waiting` is true (`palette.rs:327-330`), so `enter_waiting` is set.
  4. The rows of "ab" arrive. `enter` comes from `std::mem::take(&mut self.enter_waiting)` (`:332`), and `PickList::show` picks `self.highlight` (`pick_list.rs:147-148`). That is index 2 of the new list, a row that was never shown at that place.
- With the old flicker gone, this pick is now reachable by ordinary typing. Two options:
  - the list resets (or Enter is dropped) when the rows' `of` changes under a waiting Enter;
  - a move made while waiting also waits.

## 3. The same `Param::Choice` starts its highlight on the current choice in the panel, and on the first row in the palette

- The panel popup starts on the current choice: `actions.rs:732-733`, `PickList::at(current)`.
- The palette's Choose stage, for the same action (graph View, Source, …, picked from `:`), starts on row 0: `palette.rs:166-168`, where `enter` calls `self.list.reset()`.
  - Enter at once there picks the first choice, not the current one.
  - That row 0 is the same row the panel would show ticked as current only by chance.
- Fix 12 made the two drawings of one `Choice` list agree on tick and description, but they now disagree on where the highlight starts.

## 4. `CodeSearch::clear` is used by one of its two users; the graph's find outlives its opening

- `app/linking.rs:270`: the `@` picker clears its search on a pick. That is fix 7: the round out is dropped, so its late answers are dropped.
- `graph/actions.rs:246-252`: `pick_found` doesn't call `self.find.search.clear()`. After a pick from the graph's find:
  - the round still out is kept;
  - its late answers are taken into `shown`;
  - `typed` stays the old query.

  That is performance's outside finding 7, fixed in one picker only.
- `graph/actions.rs:110` builds `find_choices()` on every gather, whether or not the palette is on `graph.find`. `@` builds rows only while picking (`app/linking.rs:149`).
- How it goes wrong. On the next opening of the graph's find:
  - the palette's first frame(s) draw the previous search's rows, with `of` = the old query, under an empty field;
  - this lasts until the `""` from `take_search` (`app/draw/panels.rs:71-76`) reaches `find_query` and the graph's `tick` tends it.
  - If a round was out at the pick, those stale rows stay until it returns or 8 s pass (finding 5).
- The two users of one mechanism differ in lifecycle. Clearing on pick (and on leaving) belongs to one place, so neither user has to remember it.

## 5. Answers that need no server wait behind the round out

- `code_search.rs:88` returns while a round is out. The cases at `:91-96` are only reached after that:
  - the empty query, "answered at once";
  - the same query with other spaces, which keeps its rows.
- How it goes wrong:
  - Clearing the field while a round is out keeps that round's rows (of a query no longer typed) until it returns, up to `ROUND_LIMIT_S` = 8 s on a stall.
  - Meanwhile `of` ≠ filter, so Enter is held and the key line says "Looking…".
- `app/linking.rs:165`: the `@` picker's "lines of a file" row (`path:10-20`) is now derived from `rows_of()`, the answered query. The row is computed locally from the text, yet it appears only after the code index answers both kinds for that query. Before the rework it appeared with the query.
- The "one round out at a time" rule should apply to rounds sent to the server, not to answers decided locally.

## 6. The list's wrap rule ignores the by-hand rule it was meant to adopt

- `pick_list.rs:157-159`:
  - `Motion::Down if self.highlight + 1 == len => Go::First`
  - `Motion::Up if self.highlight == 0 => Go::Last`

  Both are decided from `self.highlight` before `Viewport::go` applies `by_hand` (`viewport.rs:182-186`). `Go::First` and `Go::Last` don't consult it.
- How it goes wrong:
  1. Ctrl+End: the highlight is on the last row.
  2. Wheel up to the middle.
  3. Ctrl+J: it jumps to row 0 at the top. Per the views' rule (fix 1: "a key after a wheel or finger scroll starts from what shows"), it should move one down from the nearest row that shows.
- The same happens with Up from row 0 after scrolling down.
- The wrap should be decided after the by-hand rebase. That is, only when the cursor `Viewport::go` would start from is the last (or first) stop.

## 7. Steps 2 and 3 can't build on `pick_list.rs` without changing its core

The findings file says step 2 "can skip headings there without changing the callers". That holds for the callers. These parts of the piece itself assume a flat list of rows that can all be picked:

- `pick_list.rs:123`: `area.show_rows(ui, row_h, len, …)`. One height for every row, so a heading of another height can't be drawn.
- `pick_list.rs:165`: `go` makes a stop of every row. Down and Up, half a page, first and last all land on headings and on unavailable rows. Step 2's `?` list is mostly unavailable rows plus mode-key and gesture rows that run nothing.
- `pick_list.rs:158-159`: the wrap test compares with the last and first index, not the last and first row that can be picked.
- `pick_list.rs:132-134`: hover makes any row the highlight, a heading included.
- `pick_list.rs:147`: Enter on an unavailable row does nothing and says nothing. In the `?` list that becomes the common case. Step 2's "with why when known" has no field other than `detail`.
- `Row` (`:24-35`) has no kind (heading, row that runs, row that only informs).
- What step 2 needs inside `pick_list.rs`:
  - a row kind;
  - stops only for rows that can be picked, mapped to row indexes, in `go`, in the wrap rule and in hover;
  - either headings at the row height or a replacement for `show_rows`.

  None of these is an addition at the edge. Step 3 (menus) reuses `show`/`fitting` as the setting popup does, so it inherits finding 1 if it is put in an egui `ComboBox` or `Popup` with its own scroll area.

## 8. The graph sends its kinds from a second list

- `graph/mod.rs:400` sends a hand-written `[(Symbols, …), (Files, …), (Text, …)]`. The round's kinds are `CodeSearch::new(&SearchKind::ALL)` (`graph/actions.rs:222`), and `@` sends `self.desk.finding.search.kinds()` (`app/linking.rs` `find_links`).
- If the two graph lists ever differ, `tend` (`code_search.rs:78`) never sees the round whole:
  - every round waits the full 8 s;
  - each says "The code index didn't answer in time".
- Fix: send `self.find.search.kinds()` and look up the limit per kind.

## 9. The "what wasn't listed" note is lost when a kind's last hit is left out

- `code_search.rs:149-153`: the note goes on the hit where `i + 1 == found.hits.len()`. That hit is skipped (`continue`) when `value` returns `None`, which the `@` picker does for a symbol without a `key` (`app/linking.rs:208`).
- Then neither "N of M symbols" nor "stopped at its time limit" is shown for that kind. The counts also include hits that were never listed.

## 10. `Go::of` and the views' matches dropped exhaustiveness

- `viewport.rs:63` (`_ => return None`), together with the views' new `_ => {}` arms:
  - `changes/mod.rs:1176`
  - `code.rs:1186`
  - `spec.rs:1147`
  - `tag_list.rs:348`
  - `terminal/mod.rs:635`
  - `tree/mod.rs:653`
  - `chat/nav.rs:222`
- These replace matches that listed every `Motion`.
- A list move added to `nav::Motion` (or to `nav::list_moves`, `nav.rs:459`) now compiles and does nothing in all eleven users, the pick list included, with no error to point at them.
- Listing the non-list motions in `Go::of`'s `None` arm keeps the one mapping and the compiler check.

# Checkpoint review: consistency (anti-patterns), commit eda7934

Paths are under `meta-harness/crates/cockpit-ui/`. Most important first.

## 1. Two code searches through one Search stage, with two contracts; the graph's now says "Nothing found" while still looking

- `src/app/linking.rs:20-86`: a new `Finding` (round, files/symbols slots, `settle`, `shown`, `current`) keeps the `@` picker's rows until both answers for the newest query are in.
- `src/graph/actions.rs:215-276`: `graph.find` is the same code-index search (rounds, a slot per `SearchKind`, stale answers dropped), shown in the same palette `Stage::Search`. It was left as it was: `find_query` (line 235) still clears `found` on every key, and `find_choices` still lists partial answers as they arrive. That is the empty-then-fill flicker the plan says the server-searching pickers (plural) lose.
- The palette now assumes every search keeps its rows (`src/palette.rs:493-503`). After the first answer, `rows_of_nothing` is false. With graph.find, each later key gives `found` empty with `waiting` true, which matches `(_, false) => "Nothing found"`. The graph's find now says "Nothing found" on every key while it is still looking. Before this commit it said "Looking…".
- The new meaning of `waiting` ("the rows shown aren't of the newest query; Enter waits") is not written on `Param::Search` (`src/actions.rs:34-36`), so the other provider had no way to know it.
- The two structures also disagree:
  - Order: `@` lists files before symbols; graph.find lists symbols, then files, then lines.
  - Errors: `@` puts "Couldn't search the code: …" in the note (`linking.rs:242`); graph.find turns a failure into an empty list without a word (`graph/actions.rs:245`).
  - Detail words: "a file" against "file · …".
- One "search the code index by kinds, keep the last whole round" piece should serve both. Building a second one beside `graph::actions::Find` breaks the "one mechanism per need" rule.

## 2. `pick_list` brings back its own scrolling and half-page rules instead of `viewport::Viewport`

- `src/viewport.rs:1-6` says: "One way of moving for every view that scrolls … none calls egui's own scrolling."
- The tree already does fixed-height rows in `show_rows` on that mechanism (`src/tree/mod.rs:338-347, 367-417`). It makes a `Stop::new(i * pitch, pitch)` per row and calls `viewport.go` / `take` / `wanted` / `drawn`.
- `src/pick_list.rs` writes its own version:
  - `moved` (72-87): half a page is `in_sight / 2` rows.
  - Scroll-into-sight arithmetic (109-117) with `vertical_scroll_offset`.
  - `in_sight` and `scrolled` bookkeeping (135-136).
- So the list moves differently from every view bound to the same idea:
  - Ctrl+D/U moves only the highlight. In views (`Viewport::go`, `Go::HalfDown`) the view and the cursor move half a window together.
  - No room is kept around the highlight (`Viewport::margin`; the tree keeps two rows).
  - No `by_hand` rule. After a wheel or finger scroll, the next key moves from the off-screen highlight and yanks the list back, instead of starting from what shows.
  - No glide.
- The row wrap-round (`Next`/`Prev` with `% len`) can stay as the list's own rule. Where to look should come from `Viewport` with stops of one height.

## 3. `pick_list::Move` duplicates `nav::Motion` and reuses the name `nav::Move`

- `src/pick_list.rs:15-22`: `enum Move { Next, Prev, HalfDown, HalfUp, First, Last }`.
- `src/nav.rs:327-345` already has `Motion::{Down, Up, HalfDown, HalfUp, First, Last}`, and `nav::list_moves()` (457) names exactly that set as "the moves through a list every list view offers".
- `nav::Move` (363) is a different type ("the moves every view shares"). Two `Move` types in the crate mean different things.
- `ListKeys::moved` (`src/keymap/local.rs:51-58`) should return `nav::Motion` (or `viewport::Go`), as the views' motions do.

## 4. The new list keys are never said

- `ListKeys::keys` (`src/keymap/local.rs:81-84`) still lists only pick, next/prev and back.
- It is the one place the list's keys are said:
  - the palette's footer (`src/palette.rs:524`);
  - the key bar's mode keys (`src/app/draw/bars.rs:39`);
  - the `?` sheet's mode section (`bars.rs:177, 204`).
- Ctrl+D/U, PageDown/PageUp and Ctrl+Home/End work, but no surface shows them. The module's rule (`local.rs:1-6`) is that views "say them by their labels".

## 5. Empty-list notes sit outside the fixed-height list, so the box changes size

- `src/palette.rs:447` ("No matching actions"), `471` ("Nothing to choose") and `497-503` ("Looking…" / "Type to search" / "Nothing found") are now `ui.label`s drawn above `list.show(...)`.
- `list.show` always takes its full `list_height` (`auto_shrink([false, false])`).
- So:
  - When a note appears, the box grows by one line and the footer moves down.
  - When the rows arrive and the note goes, the list jumps up a line. `tallest` pads at the bottom; it does not hold the list in place.
- The removed code drew these notes inside the scroll area under `set_min_height(list_height)` so nothing moved. Plan decision 4 asks for the same: "The list's box has one size, whatever matches".
- The note belongs inside `PickList::show`, for example as an `empty: &str` on `Listing`. Then every list has one empty state.

## 6. New `ListKeys` fields use bare `#[serde(default)]`, against this file's pattern

- `src/keymap/local.rs:31-38`: bare `#[serde(default)]` falls back to an empty `Vec`, not to `ListKeys::default()`.
- In the same file, `InputKeys` uses `#[serde(default = "default_leave")]` and similar, so a missing key means the built-in value.
- The other `ListKeys` fields have no serde default at all.
- `UiConfig::with_overrides` (`src/config.rs:150-153`) always lays the user's table over `config/default.toml`, so these attributes never act. They are dead and look as if they did something. Drop them, or use named defaults as `InputKeys` does.

## 7. `Row` renames `Choice`'s fields

- `src/pick_list.rs:28`: `mark: Option<(Color32, Mark)>` is `Choice::colour` (`src/actions.rs:59`) and `choice_text`'s `colour` parameter (645) under another name.
- `faded` (35) is what the rest of the code calls not applying: `When::Unavailable`, and the key sheet's `applies` (`bars.rs:185`).
- `choice_row` (`src/palette.rs:560`) maps them one to one. Use the existing names.

## 8. The panel's choice picker draws the same `Choice`s a second way

- `src/actions.rs:714-731` (`Param::Choice` with `Style::Setting`) lists choices through an egui `ComboBox` with `selectable_label`:
  - the current one is shown by fill, not the check the pick list puts before it (`pick_list.rs:154`);
  - the detail shows only on hover;
  - there are no list keys and no filter.
- The palette's `Stage::Choose` now draws the same `Param::Choice` data through `PickList`. A view's "View" or "Source" choice reads differently in the panel control and in the palette.
- Plan decision 2 is one list for the action list and its pickers.

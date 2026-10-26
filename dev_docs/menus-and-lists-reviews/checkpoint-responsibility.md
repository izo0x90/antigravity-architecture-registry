# Checkpoint review: responsibility (commit eda7934, order step 1)

Paths are under `meta-harness/crates/cockpit-ui/`. Ranked, most important first.

## 1. `pick_list` is a second scroll-and-move system beside `viewport` and `nav::Motion`

- `src/pick_list.rs:15-22` adds `enum Move { Next, Prev, HalfDown, HalfUp, First, Last }`. That is a copy of `nav::Motion` (`src/nav.rs:327`: Down, Up, HalfDown, HalfUp, First, Last). `nav.rs:455-460` (`list_moves`) already says these are "the moves through a list every list view offers".
- `src/pick_list.rs:72-87` (half a page from `in_sight`), `:104-117` (bring into sight, setting `ScrollArea::vertical_scroll_offset` itself) and `:135-136` (a shadow copy of egui's scroll offset in `scrolled`) re-implement `viewport::Viewport::go` and `viewport::Measured` (`src/viewport.rs:155`, `:281-335`).
  - `viewport.rs:1-6` states the rule: "One way of moving for every view that scrolls ... none calls egui's own scrolling".
  - `tag_list`, `inbox`, `queue`, `discussion`, `chat` and `spec` all use `Measured`.
- How it goes wrong: the same key behaves differently in the list than in every view.
  - Half a page in `PickList` moves only the highlight and scrolls as little as needed. `Viewport` moves the view and the cursor together.
  - `PickList` has no `by_hand` rule, so a key after a wheel or finger scroll doesn't bring the highlight back first.
  - It keeps no margin around the highlight and doesn't glide.
- Steps 2 and 3 build on this piece, so the split grows with them.
- Coupling direction: `src/keymap/local.rs:54-61` (`ListKeys::moved`) now returns `crate::pick_list::Move`. The keymap config layer depends on a widget's type, where `nav::Motion` (or `viewport::Go`) is the existing shared vocabulary.
- Fix: map list keys to `nav::Motion`/`viewport::Go`. Drive the list's offset through a `Viewport`. With fixed-height rows, its stops are `i * stride` for all `len`, with no layout needed.

## 2. Whether the rows are of what's typed is decided in two layers, and the palette's guess breaks for the graph's find

- The `@` provider knows exactly whether its rows are current (`src/app/linking.rs:77`, `Finding::waiting`/`current`). The palette doesn't get told that. It re-derives it from three flags of its own:
  - `waiting` from `Param::Search` (`src/palette.rs:332`)
  - `self.ask.is_none()`
  - `rows_of_nothing` (`src/palette.rs:49`, `:493-501`)
- The palette's "Looking…/Nothing found" choice assumes every search provider keeps its old rows while waiting. That is an unwritten change to the `Param::Search` contract (`src/actions.rs:34-36` still says only "lists what it found").
- `graph.find` doesn't keep its rows:
  - `src/graph/actions.rs:235` clears `found` on every query.
  - `:274` reports `waiting` until all three kinds are in.
- From the second query on, `rows_of_nothing` is false while `found` is empty and `waiting` is true. The match at `palette.rs:498-500` shows "Nothing found" while the graph is still looking, on every key, then the rows fill in.
- Fix: one contract. For example, `Param::Search` carries the query its `found` rows are of (or "pending, keeping rows"), and the palette compares that with its filter. No inferred flags.

## 3. Two copies of the "rounds of per-kind server search answers" logic; only one got the no-flicker fix

- `src/app/linking.rs:21-86` (`Finding`: `round`, `files`/`symbols` per `SearchKind`, `settle` when both are in, `waiting`) and `src/graph/actions.rs:217-275` (`Find`: `asked` round, `BTreeMap<SearchKind, Found>`, `waiting` until all kinds are in) do the same job:
  - a counter per query
  - answers to older rounds dropped
  - results collected by kind
  - "waiting" until every kind has answered
- This commit grew the `@` copy (the `kind` added to `Answers`, `ask`/`answer`/`settle`). The graph's copy still empties and refills on every key (`graph/actions.rs:235`).
- The plan's finding says "the server-searching pickers keep the list they have until new results come" (plural). The policy lives in one provider instead of in the shared piece, so each new search picker has to re-implement it.

## 4. The list's empty state is drawn by the host outside the list piece, so the box changes size; the glue is still copied three times

- `src/palette.rs:446-448`, `:470-472` and `:497-502` draw "No matching actions" / "Nothing to choose" / the search note as a label above `PickList::show`. The list always takes its full `height` (`pick_list.rs:106`, `auto_shrink([false,false])` + `min_scrolled_height`).
- How it goes wrong:
  - When a filter first matches nothing, the box grows by a row and the list's top edge moves down. `tallest` (`palette.rs:534`) then keeps the extra height.
  - When matches return, the rows move back up. This is the jump step 1 was meant to remove.
  - Before this commit the note sat inside the fixed-height scroll area.
- Each stage also repeats the same three steps:
  - `if let Some(by) = moved { self.list.moved(by, len) }` (`palette.rs:434-436`, `:467-469`, `:484-486`)
  - the empty note
  - `show(Listing { len, .. })`
- `len` is passed twice, to `moved` and in `Listing`, and the caller must keep them equal. The `?` list (step 2) and the menus (step 3) would copy this glue again.
- Fix: `PickList` owns its empty note (a field of `Listing`) and takes the move in `show` with the one `len`.

## 5. The row model can't carry what step 2 needs without changing the piece's core

- `src/pick_list.rs:26-36` (`Row`) has no heading or section kind and no "why" for a faded row.
- `show_rows` with one stride (`:120`) can't hold headings of another height.
- `moved` (`:72-87`) and Enter (`:137`) land on faded rows. The test at `:243` pins this: "a faded row is shown, highlighted, not picked".
- Step 2 puts the headings (on what you're on, this view, this panel, everywhere) and many faded rows into this list. Moves would then stop on headings and on rows that can't run. The headings would have to be a second kind of row, with skip rules added to `moved`.
- The base assumes a flat list of same-kind rows. That is not what the plan's next two steps build on it.

## 6. List keys are a second binding table for the same moves

- `config/default.toml:47-50` (`[keymap.list] half_down = ["ctrl+d", "PageDown"]`, `first = ["ctrl+Home"]`, ...) binds half a page, first and last a second time, beside `nav.half_down`/`nav.half_up`/`nav.first`/`nav.last` (`:174-183`).
- Rebinding `nav.half_down` leaves the list on Ctrl+D. Nothing ties the two.
- The plan calls these "the movement every view has". That fits the first point: the moves exist once (`nav::Motion`), but here they are bound twice and run by two engines.

## 7. `Finding` mixes search state with how its rows look; the empty-query rule is in two places

- `src/app/linking.rs:42-49`: `Finding::ask` builds the "lines of a file" row (wording, `LIST_NUMBERS` icon, `Subject::name`).
- The file and symbol rows are built in `CockpitApp::find_links` (`:228-238`, `FILE`/`CODE` icons, wording).
- Turning results into rows is therefore split between the state type and the app.
- "An empty query asks nothing" is decided in `Finding::ask` (`:51-54`, settles at once) and again in `find_links` (`:256`, returns before asking).

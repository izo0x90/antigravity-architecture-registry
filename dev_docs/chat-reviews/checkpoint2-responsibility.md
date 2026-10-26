# Chat checkpoint after steps 2-3: responsibility (6dd0d2f, 32dc365)

Paths are under `meta-harness/crates/cockpit-ui/src`. Ranked, most serious first.

## 1. What Enter does is decided in two places, and they disagree

- `chat/nav.rs:236-242`: `enter()` unfolds a folded run.
- `chat/nav.rs:40-43`: `moves()` builds the `Move::Enter` offer from `target()`. On a folded run that is the run's first call, so when that call has a location the offer reads "Open src/parse.rs" while Enter unfolds the run. The app test shows this (`app/tests/moves.rs:337-340`: the first Enter stays in the chat, the second opens the file).
- The tree keeps both in one place: Enter on a folded row is offered as "Unfold X" (`tree/mod.rs:443`).
- Beneath this, `target()` (`chat/nav.rs:207-213`) makes a folded run stand for its first call. So every action driven by the target (GoTo code, the tag things for key `row.first`) acts on one call while the row on screen is the whole run.

## 2. The indicator's stage and overlay are built inside one row, from the viewport's stop list

`chat/live.rs:91-93` fills the interface in a way that breaks its contract (`indicator.rs:100-108`, plan "How it's built"):
- **`over`**: it is `ui.painter()` of the turn line, inside the stream's `ScrollArea`. It paints in the stream's own layer, under every row drawn after the turn line, and is clipped to the scroll viewport. It is not a see-through layer over the panel.
- **`panel`**: it is `ui.clip_rect()` of that horizontal line, which is the scroll area's clip, not the panel's rect.
- **`items`**: it is `self.ui.item_rects.clone()`, the buffer `viewport::Measured` stops are fed from. It is cleared at `chat/render.rs:71` and filled row by row at `chat/render.rs:299`. When `render_turn_line` (`chat/render.rs:321`) runs, it holds only the rows above the running turn, and it includes rows scrolled off screen.

The heartbeat ignores `over` and `items`, so nothing shows wrong today. The first indicator that uses the stage (the lemmings) gets a wrong one. Drawing the overlay is the stream host's job, after the stream is laid out, with the panel rect and the visible rects. It is not the turn line's job.

## 3. The "plain data" is cut to the heartbeat's own constant

- `chat/live.rs:27` drops updates older than `crate::indicator::TRACE_S`.
- `TRACE_S` is the heartbeat's trace span (`indicator.rs:181-182`, documented as "The seconds the trace spans").
- So the chat's record of arrivals is as long as one implementation's drawing window. Another indicator can't get a longer history, and changing the heartbeat's width silently changes what every indicator is handed. The chat should own its retention window, or the interface should let the indicator ask for one.

## 4. The arrival record lives in the view, sampled per frame, not with the thread

- `chat.rs:89-93` adds four loose fields to `ChatView`: `updates`, `seen_activity`, `running_since`, `connected`. Only `chat/live.rs:18-31` reads and writes them.
- They rebuild "when each update arrived" by comparing `ThreadView::activity` once per frame, and only while the chat is drawn.
- The arrival is known where the event lands: `store.rs:95,103` bump the count, and `net.on_ws_event` has `now`. Keeping the record in the view has these effects:
  - **Slot switch** (`chat/nav.rs:162,183`): the new thread's count is compared with the old thread's `seen_activity`, which records a false blip. `running_since` carries over when both threads are running. Nothing resets these fields.
  - **Several events in one frame**: they make one blip. The plan says "blips once for each update".
  - **While hidden**: arrivals aren't recorded.
  - **Two panels**: two panels on the same chat keep two different records.
- No type owns the liveness record. It is fields on the view struct, managed from another file.

## 5. "The cursor is on a row's first item" is enforced by hand in some paths and skipped in others

The invariant is enforced by hand in three places:
- `chat/render.rs:346-359` (`put_cursor`) opens a run.
- `chat/nav.rs:279-284` (`fold`) snaps the cursor to the run.
- `chat/render.rs:363-370` (`go`) maps through `holds`.

Paths that change the rows don't enforce it:
- **Verbosity switch** (`chat/actions.rs:476-478`): quiet can fold the cursor's item into a run and leave the cursor inside it. Then `item_frame`'s `self.ui.cursor == index` (`chat/render.rs:130`) never matches `row.first`, and nothing is highlighted.
- **`restore`** (`chat/nav.rs:197`): it writes the raw saved index.
- **New items**: they can grow a run around the cursor.

Fold state is also changed from four places: `chat/nav.rs` `fold` and `enter`, `chat/render.rs` `put_cursor` (a cursor-move helper in the drawing file that toggles folds), and `chat/actions.rs` (verbosity). Working out the cursor's row in one place, when it's needed, would remove the hand fixes.

## 6. Two numberings, converted by hand at every call

- `scrolling` (the `Measured` viewport) and `Motion::Line` count rows: `chat/nav.rs:202,225-226,258`, `chat/render.rs:358,366`.
- The cursor, `ChatPlace.message`, `place_in_words` ("message N", `chat/nav.rs:188-189`), the search keys (`chat/nav.rs:288`), `cursor_key` and the spot keys count items.
- Every crossing does its own `rows().iter().position(|r| r.holds(..))`: five places.
- A mark's line says "message 9", but `9G` goes to row 9, which is a different item when a run above is folded.

## 7. The two new chat settings are offered two different ways, and neither is configured like other views' settings

- `chat/actions.rs:277`: `chat.verbosity` uses `.setting("Shows", None)`, with the default group (`Act`, `actions.rs:376`).
- `chat/actions.rs:280`: `chat.indicator` uses `.group(View)` with no `.setting(..)`. So "Show the agent at work as…" is not on the chat's settings row, although the plan calls it "a chat setting".
- The graph's settings use both (`graph/actions.rs:87-93`).
- Both defaults are hard-coded: `chat.rs:124-130` and `#[default] Normal` at `chat/rows.rs:36`. Other views take their setting defaults from `UiConfig` through `share()` (`code.rs:1101-1104`). The thresholds `RECEIVING_S` and `QUIET_S` (`chat/live.rs:11-13`) are also code constants.

## 8. The indicator registry is written twice, and the choice is stored twice

- `indicator.rs:166` (`NAMES`) and `indicator.rs:169-174` (`by_name`'s `match`) must be kept in step by hand. "none" exists only as the `_` fall-through, so a misspelled entry in `NAMES` also reads as "none".
- `ChatView` keeps both `indicator: String` and `live: Option<Box<dyn Indicator>>` (`chat.rs:94-95`). They are set together at `chat/actions.rs:481-483` and defaulted separately at `chat.rs:129-130`, so they can disagree.
- One table of (name, constructor) and one stored name would make this a single source.

## 9. The verbosity is stored inside the hand-toggle set, and it decides the structure, not only the defaults

- **Stored inside**: `chat/rows.rs:70-74` makes `verbosity` a public field of `Folds`, next to the private toggle sets. `Folds::all` has to carry it across (`chat/rows.rs:154`), and `at_verbosity` resets everything.
- **Decides the structure**: `chat/rows.rs:132-149` (`runs`) uses the verbosity to decide what a run is. In quiet, reasoning joins runs and a single item is a run. A run's identity (its first item's id, `chat/rows.rs:109,176`) therefore means different things at different verbosities, which is why switching has to wipe all hand folds (`chat/actions.rs:477`). The plan has verbosity "deciding which items are folded by default", not how they group.
- **Two folds on one item**: in quiet, a lone reasoning item or call gets a one-item run fold ("what it did here") on top of its own fold (`chat/rows.rs:171-180`).
- **Wrong hover text**: `render_run`'s hover still says "N calls" (`chat/render.rs:46-50`) when the run holds reasoning.

## 10. Per-kind tables and checks repeated across files

- **Words per `ToolKind`**:
  - `chat/rows.rs:251-268` (`summary`) gives the past tense.
  - `chat/live.rs:52-70` (`doing`) gives the progressive.
  - `tool_icon` at `chat.rs:153` gives the icon.

  These are three tables in three files, and a new `ToolKind` or `ItemBody` call variant has to be added to each.
- **"A request waits on you"**: spelled out again at `chat/live.rs:53` and `chat/render.rs:302`, beside `pending()` (`chat.rs:248-262`).
- **Fold caret**: `fold_mark` (`chat/render.rs:41-43`) is one more copy of the caret choice that `nav::fold_here` (`nav.rs:470`) and each view make for themselves.

## 11. The stream's order is written out again beside the new helper

`with_items` (`chat.rs:272-276`) was added to give the stream's items in order. Yet `render_stream` builds the same list again (`chat/render.rs:280`). So do `item_at` (`chat/render.rs:335`), `stream` (`chat/render.rs:341`), `pending` (`chat.rs:256`) and `tag_places` (`chat/things.rs:34`). That makes six definitions of "the stream". `ThreadView` (`store.rs:32`) is where one belongs.

## 12. The chat reads the socket's mutex directly in its draw

- `chat/render.rs:60` does `cx.net.ws_state.lock()`. Other buffers only call `NetClient` methods through `cx.net`.
- Whether the server is connected is a desk-wide fact: the top bar reads the same field, `app/draw/top_bar.rs:190`. It could reach buffers through `Shared`/`share()`, as the agents and drafts do.

## 13. `rows` widened to `pub(crate)` with no user outside `chat`

32dc365 makes `chat.rs:32` `pub(crate) mod rows` so that `chat/actions.rs:276,476-477` and `chat/nav.rs:238` can write `crate::chat::rows::…`. Those are submodules of `chat`, so `super::rows` would reach it, and nothing outside `chat` uses the module.

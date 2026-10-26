# Chat checkpoint after steps 2-3: performance (6dd0d2f, 32dc365)

These sizes are used throughout: a chat of 3000 items over about 150 turns, 3 panels on screen, and an agent turn streaming 20 to 50 updates a second. Costs are worked out from the code. Nothing was run or measured.

## 1. The indicator asks for 10 frames a second while it can't be seen

- **Where:** `chat/live.rs:77-98` (`render_live`), called from `chat/render.rs:321` (`render_turn_line`) inside the stream's `ScrollArea`.
- **What repeats:** egui runs the closure of the stream's scroll area for every row, including rows outside the visible window. So `render_live` runs every frame the chat is drawn. It calls `request_repaint_after_secs(0.1)` whenever the heartbeat returns `Next::In(0.1)` (`indicator.rs:209-210`), with no check of `ui.is_rect_visible(spot)`.
- **How often:** The running turn's line sits above that turn's items. In a long turn, while you follow the latest, it scrolls out of view after one screenful. From then on, for the rest of the turn, the chat asks for 10 frames a second for a heartbeat nobody can see. The heartbeat keeps moving in two cases:
  - an update came in the last 10 s;
  - the agent is "waiting", which is anywhere from 1.5 s to 60 s since the last update.

  In practice that is the whole turn, except stretches longer than a minute with no update.
- **Cost:** about 10 extra frames a second for the whole length of every turn. Each frame costs what items 2 and 3 describe. The plan says "drawn only while it's shown" (Decided 5) and "not at all while the panel ... is hidden". Both are unmet whenever the line is scrolled away.
- **Also:** the 1/30 s cap (`FASTEST_S`) never takes effect, because the heartbeat always asks for 0.1 s. The real rate is 10 a second, not 30.

## 2. Each of those frames copies and lays out the whole chat

- **Where:**
  - `chat/render.rs:277`: `self.thread().map(|t| t.clone())` copies the entire `ThreadView`: every `TurnItem`, every tool output, command output, reasoning text and request. This line predates the checkpoint.
  - `render.rs:280`: the item list is gathered by scanning all items once per turn (see item 3).
  - `render.rs:281`: `rows::rows`.
  - `render.rs:283-307`: every row is drawn, with no culling (`show`, not `show_rows` or a visibility check). Every assistant message goes through `CommonMarkViewer::show`, and egui_commonmark 0.22 parses the markdown again on each call.
- **How often:** every frame the chat is drawn. Before this checkpoint that meant on input and on events. Now it is also at least 10 a second for the length of every running turn (item 1), on top of the frames each streamed update already causes.
- **Cost at 3000 items:**
  - The copy alone is about 3000 × (3-4 id strings + title + body strings) ≈ 15-20k allocations. That is several MB copied per frame if outputs average a few KB, and tens of MB with a few large command outputs.
  - At 10 frames a second that is roughly 150-200k allocations a second, before layout.
  - Every frame also re-parses the markdown of every assistant message in the chat (hundreds) and hashes every shown text for egui's layout cache.
  - "Everything" verbosity (step 3) opens every output, so every output's full text is laid out and hashed each frame too.
- **Why it belongs to this checkpoint:** the copy and the full layout are older code. The indicator is what turns them from "on change" into a steady 10 a second while an agent works.

## 3. `with_items` scans every item once per turn, and runs 2P+1 times a frame, even with no chat on screen

- **Where:**
  - `chat.rs:272-276`: `view.turns.iter().flat_map(|t| view.items_of(&t.id))`. `items_of` (`store.rs:32`) filters all of `view.items` by turn, so one call is turns × items string comparisons (`TurnId` is a `String`).
  - It is called from `chat/nav.rs:46` (`folds.at`, while the cursor is set) and `nav.rs:48` (`folds.any`, always), both inside `moves()`. `moves()` is the first thing `actions()` does (`chat/actions.rs:57`).
  - `moves()` also calls `target()` (`nav.rs:40`), which calls `item_at` (`render.rs:333-336`). That is the same scan, plus a copy of the item under the cursor, output included. This part is older code.
- **How often:** `app/commands.rs:186,193-198`. `gather` calls every view's `actions()`, the chat's included, whichever panel it is gathering for. The global ones are filtered afterwards, so the chat's moves are built and then discarded. `gather` runs once for the focused panel and once for each other host on screen (`app/draw/host.rs:29`, `shown_at`). It runs again after any mid-frame `forget_frame`.
- **Count per frame:** with 3 panels that is 3 × `actions()`, so 6 `with_items` plus 3 `item_at` scans. Add 1 in `render_stream` when a chat is drawn. This happens every frame of the cockpit while any chat has a thread, even when no panel shows the chat.
- **Cost:** at 3000 items over 150 turns, one scan is about 450k string comparisons. 7-10 scans a frame is about 3-4.5M comparisons, likely several ms per frame in wasm. That cost lands on every frame item 1 adds and every frame caused by a streamed update.
- **Fix direction:** a single in-order item list (or per-turn index ranges) kept in `ThreadView` when events are applied would make each of these a slice borrow.

## 4. A repaint every second, forever, while a turn runs, even when nothing on screen changes

- **Where:** `chat/live.rs:90-97`: `let mut next = 1.0` is the floor whatever the indicator says, and whether there is an indicator at all.
- **How often:** once a second for as long as a turn is non-terminal:
  - when the heartbeat returns `OnChange` (quiet, more than 60 s with no update);
  - when the connection is down;
  - when the setting is "none", which skips the `if let Some(live)` block entirely.
- **Cost:** a full frame (items 2 and 3) every second. Only "waiting" shows seconds. "No word for N min" changes once a minute. "The connection to the server is down" never changes, and neither does "Doing…" while receiving. Picking "none" does not stop the stream being rebuilt every second. An agent stuck for an hour costs 3600 full-chat frames.

## 5. `rows()` builds every run's summary text on every call, and key moves call it 4 times

- **Where:**
  - `chat/rows.rs:223-240`. Each folded run allocates a `Vec<&TurnItem>` (`:229`) and runs `summary` (`:244-289`). `summary` costs one `format!` per kind of call, a `join`, an uppercase `format!` that copies the whole string again, and maybe a `push_str(&format!(..))`.
  - Each folded run that is drawn also costs two more `format!`s in `render_run` (`render.rs:47,49`).
- **How often:**
  - Once per frame from `render_stream`.
  - Per key:
    - `put_cursor` (`render.rs:346-359`) calls `self.rows()` three times, plus `item_at`, which copies one item.
    - `motion` (`nav.rs:218`) calls it once more before `put_cursor`, so a fresh `j`/`k`, or `:N`, makes 4 full scans (item 3) and 4 sets of every summary.
    - `move_to` (an `f` jump, a click) makes 3.
    - `fold` makes 2.
    - `go`, `leave` and `restore` make 1 each.
  - These callers only need `first`/`last`, never the words.
- **Cost:** in Normal at 3000 items, a few hundred runs means about 1-2k small allocations per `rows()`. In Quiet every stretch between two messages is a run, so up to about 1000 runs and about 5k allocations per call. Each call also pays the turns × items scan. A held `j` with key repeat (about 30 a second), on the paths that go through `put_cursor`, does that 4 times per key.

## 6. `Folds::any` and `Folds::at` allocate per run on every gather

- **Where:**
  - `rows.rs:206-211` (`any`): the full `runs()` list, a new `Vec<&TurnItem>` per run, a `Vec<bool>` per run and a `Vec<bool>` over every item that folds. All of it is computed before `.any()` is asked, so nothing short-circuits.
  - `rows.rs:171-181` (`at`): the full `runs()` list, then `find`, then a `Vec` of that run's calls.
- **How often:** once per `actions()` call (item 3): `any` always, `at` while the cursor is set. That is P times per frame with P panels, whether or not the chat is shown.
- **Cost:** at 3000 items in Normal, about 500 `Vec` allocations plus 2 vectors of up to 3000 entries per gather. With 3 panels that is about 1500 allocations a frame, only to set two booleans for "fold all" / "unfold all".

## 7. `Stage.items` copies every item rect each frame, and nothing reads it

- **Where:** `chat/live.rs:92`: `items: self.ui.item_rects.clone()`.
- **How often:** every frame the running turn's line is drawn (item 1: about 10 a second).
- **Cost:** `item_rects` holds every row drawn so far this frame. For the last turn that is nearly all of them: up to 3000 × 16 B ≈ 48 KB allocated and copied per frame. The heartbeat ignores `stage.items`. (Being filled only up to the turn line also makes it partial: the running turn's own items are never in it.)

## 8. The heartbeat's history is bounded by time, not count, and each blip allocates

- **Where:**
  - `chat/live.rs:23,27-29`: `updates` gets one entry per frame in which the update count changed, and entries are dropped after 10 s.
  - `live.rs:43`: copied into a new `Vec` every frame.
  - `indicator.rs:198-201`: one `blip.to_vec()` allocation and one line shape per entry per frame.
- **How often / size:** while streaming, each update causes a frame, so at 30-60 updates a second `updates` holds 300-600 entries. Each frame then copies them and allocates and tessellates 300-600 four-point lines into a 64-px spot, where they overlap almost completely (6.4 px per second of trace).
- **Cost:** a few hundred allocations and shapes per frame, many frames a second. It does not grow without bound, but it is not capped by what can be seen. Merging blips per pixel column, or capping the count, would bound it.

## 9. Fold toggles are never pruned and survive switching slot or session

- **Where:** `rows.rs:70-74` (`Folds.runs` / `Folds.items`, `BTreeSet<ItemId>`) and `rows.rs:153-167` (`all`). `ChatView.folds` is reset only by the verbosity setting (`actions.rs`, `ChatCommand::Verbosity`).
- **What grows:**
  - `z R` / `z M` (`Folds::all`) inserts one `ItemId` string for every run and foldable item whose default differs. In Normal, `z R` inserts nearly every item: about 2-3k strings.
  - Switching slot or session keeps the set. Ids from earlier threads stay and are looked up (`BTreeSet` string compares) for every item, every frame (`item_open` in `render_item`, and `run_open` in `rows`).
- **Cost:** small per lookup. Growth is limited only by how often the user folds everything across sessions; nothing ever removes the entries.

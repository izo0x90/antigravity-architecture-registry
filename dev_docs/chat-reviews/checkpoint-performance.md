# Chat checkpoint after step 1: performance (d54286c)

Scope: the step-1 diff (`chat/render.rs`, `chat.rs`, `chat/actions.rs`,
`app/draw/panels.rs`) and the paths it calls. I only read the code; nothing
was built or run. Ranked, most serious first.

## 1. A request waiting out of sight makes the chat repaint every frame, forever

`chat/render.rs:52`, `:69`, `:78-83`, `:91-95`.

- `shown` is last frame's `self.ui.spots`. Line 81 pushes the card drawn
  below the stream into `self.ui.spots`, under the same key (`index`) the
  stream's own item uses.
- **Frame N:** the request isn't in the stream's view, so its card is drawn
  below and its key goes into `spots`. `below` grows, and line 94 calls
  `request_repaint()`.
- **Frame N+1:** `shown` now has that key, so the filter on line 78 skips
  the card. `below` shrinks and the chat repaints again.
- **Frame N+2:** `shown` (from N+1) holds only what the stream showed, so the
  card is drawn again, and so on.
- **How often:** every frame, at the display's rate (60-120 Hz). It lasts as
  long as any waiting request is out of the stream's sight. That is exactly
  the case step 1 is for: you scrolled up, or a sub-agent's later items
  pushed the request up. An approval can wait for minutes or hours while
  you're away.
- **What each frame costs:** a full redraw of every item (items 2-4 below).
  The card also flickers, and the stream's height jumps by the card's height
  every other frame.
- **The test doesn't see it.** `two_requests_at_once_are_each_answerable_and_kept_in_sight`
  checks after 3+1 frames, which lands on a frame where the card is drawn. A
  fifth frame would fail it.
- **Fix:** decide on this frame's stream spots: the stream is drawn before
  the area below it, and line 69 already has them. Keep the cards below out
  of the list that the next frame's "shown" check reads.

## 2. `pending_items()` copies the whole chat several times a frame, even when nothing waits

`chat.rs:215-228` (`pending_item`, `pending_items`), `chat/render.rs:275-278`
(`stream`), `:438` (`here`), `:78`, `chat/actions.rs:71`, `:263-264`,
`app/draw/host.rs:140`, `app/commands.rs:186-194`.

- **What it does:** `pending_items()` calls `stream()`, which deep-clones
  every `TurnItem` in the thread into a new `Vec`. That includes message
  text, tool-call outputs (whole files for a Read), diffs (`old_text` +
  `new_text`) and command output. All of that is copied just to find the 0-2
  `Request` items that are waiting.
- **What changed:** the old `pending_item` returned early on
  `requests.iter().find(Pending)?`, so no copy happened when nothing waited,
  and the old answers block in `actions()` only ran when a request was
  waiting. Now both run every time.
- **Copies per frame**, in the common case (a long turn streaming, cursor
  `None`, following the latest):
  - `render.rs:78`, the draw: 1 per chat panel.
  - `host.rs:140`, `hosted.here()` in `render_item_controls`: 1 when the
    chat is focused and you're not typing (`here` → `pending_item` →
    `pending_items`).
  - `actions()`: 3 per call: `on_banners()` → `here()` (line 71),
    `here()` again (line 263), and `pending_items()` (line 264).
    - `actions()` runs in every `gather`. `gather` calls
      `self.desk.view(k).actions()` for every `BufferKind` (commands.rs:194),
      so the chat pays this even when the focused panel isn't a chat.
    - `gather` runs once per frame for the focused panel (cached), once per
      other panel (`host.rs:29` `shown_at` → `gather`), and again after each
      action that runs (`invoke_counted` → `forget_frame`).
  - **Total:** with 2-3 panels, about 8-11 full copies per frame where there
    used to be none.
- **The scan behind each copy is quadratic.** `stream()` is
  `turns.iter().flat_map(|t| view.items_of(&t.id))`, and `items_of`
  (`store.rs:32`) filters all items for each turn. That is turns × items
  `TurnId` comparisons per call: 150 turns × 3000 items is 450k per call, and
  about 4-5M per frame at the call count above.
- **At a realistic size:** a long session of about 3000 items, about 1000 of
  them tool calls with outputs averaging a few KB, is roughly 5-15 MB of
  strings in some 10k heap blocks.
  - That is about 10 copies of 5-15 MB per frame through wasm's allocator,
    estimated at tens of ms per frame.
  - A frame happens on every update while a turn streams (tens per second),
    and continuously under item 1.
  - The transient copies also raise the wasm heap's peak, which wasm linear
    memory never gives back.
- **Smaller costs in the same block:**
  - The `pending` closure (chat.rs:223) scans all of `thread.requests` for
    each `Request` item: (request items × requests), and resolved requests
    are kept.
  - `index.to_string()` is allocated per pending request per gather
    (actions.rs:265, :268).
- **Fix:**
  - Find what waits from `thread.requests` (only the pending ones, exiting
    early when there are none), and locate their items by reference, with no
    clone.
  - Work it out once per change of the thread. The chat already keeps one
    such cache: `tag_places`, keyed by thread id and activity count
    (chat.rs:63).
  - `here()`, `actions()` and the draw then read that one result.

## 3. A horizontal ScrollArea per item, for every item, every frame

`chat/render.rs:255-264`, with the stream drawn by `ScrollArea::show`
(`render.rs:59`). There is no row virtualization, so every item is laid out
each frame, off-screen ones included.

- **What each item's area costs per frame** (egui 0.33 `scroll_area.rs`):
  - `State::load`: a ctx lock, a hash lookup and a copy.
  - Two `animate_bool_responsive` (the `h` and `v` bars): a ctx lock and an
    animation-map lookup each.
  - A `new_child` `Ui`.
  - Two `pass_state_mut` calls that save and restore scroll targets.
  - `rect_contains_pointer`.
  - `State::store` → `insert_persisted`: a new boxed `Element`, so one heap
    allocation per item per frame.
- **At 3000 items:** about 3000 extra allocations and 15-20k ctx lock
  round-trips per frame. This adds to the existing full layout of every item
  (markdown is re-parsed by `CommonMarkViewer` each frame) and to the
  per-item `Spot` key `String`s.
- **Most items don't need it.** Plain messages, tool lines, notices and
  errors already wrap at the column's width. Only what can't wrap needs a
  sideways box: markdown tables, code blocks, long monospace output.
- **Fix:**
  - Wrap only the parts that can't wrap (the markdown viewer's tables and
    code blocks, monospace output), not every item.
  - Or draw only the visible rows: the stream already measures each item's
    rect (`scrolling.item(rect)`, render.rs:61-64), which is what
    `show_viewport` needs.

## 4. Per-item egui state that is never freed

`chat/render.rs:261`: `id_salt(("chat_item", item.id))`.

- **What's left behind for each item ever drawn, per panel it was drawn
  in:**
  - A persisted `scroll_area::State` in `ctx.data` (`insert_persisted`).
  - Two `BoolAnim` entries (`id.with("h")`, `id.with("v")`) in egui's
    `AnimationManager.bools`.
- **Why nothing frees them:**
  - The `AnimationManager` never prunes `bools`.
  - `IdTypeMap` only drops old entries when it serializes. eframe is built
    without `persistence` (cockpit-ui `Cargo.toml:10`), so that never
    happens.
- **How it grows:**
  - With every item of every session opened or resumed during the tab's
    life.
  - Again when a chat is shown in another panel, or after a layout change:
    the id goes through the parent `Ui`'s path, which includes the panel.
- **Size:** about 200-300 bytes per item and panel, so 10k items come to
  about 2-3 MB. The maps' lookups, done per item per frame (item 3), get
  slower as they grow.
- **Fix:** item 3's fix removes most of these entries, since only the parts
  that can't wrap would have state.

## 5. Small per-frame allocations added to the draw

- `render.rs:52`: `shown` clones the key `String` of every spot from last
  frame (every visible message and link), every frame, to be checked against
  0-2 pending requests. Each check also allocates `index.to_string()`
  (line 78).
- `render.rs:79`: each request kept in sight is cloned (`.cloned()`) every
  frame just to draw it. A reference would do.
- These are small next to items 1-4. Item 1's fix (using this frame's spots
  directly) removes the first.

## Outside this commit, on the same paths

These existing costs are multiplied by the calls above, and item 2's fix
applies to them too:

- `render.rs:231`: `render_stream` clones the whole `ThreadView` (items,
  requests, turns) every frame.
- `nav.rs:202`: `target()` clones the whole stream to take one item. It is
  called each frame from `situation_now` (`app/views.rs:71`) while the
  cursor is on a message.

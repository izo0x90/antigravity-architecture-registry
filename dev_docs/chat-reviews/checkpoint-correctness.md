# Chat checkpoint after step 1: correctness (d54286c)

Ranked, most serious first. Paths are under `meta-harness/crates/cockpit-ui/src/`.

## 1. A kept card flickers on and off every frame, and the chat repaints forever

`chat/render.rs:52`, `:69`, `:78-82`, `:91-95`.

`shown` is last frame's `self.ui.spots`. Line 69 rebuilds the spots from the stream, then line 81 pushes the kept card's spot under the same key (`index.to_string()`). On the next frame `shown` contains that key, so the filter on line 78 drops the card.

Sequence: a request at index 5 scrolls out of the stream.
- Frame A: `shown` has no "5", so the card is drawn below and spot "5" is pushed. `below` grows, so line 94 asks for a repaint.
- Frame B: `shown` has "5" (from the kept card), so no card is drawn. The spots now come only from the stream, so "5" is gone. `below` shrinks, so it repaints again.
- Frame C is the same as A, and so on.

What follows from this:
- The card blinks.
- The stream's height jumps by the card's height every frame; with stick-to-bottom on, the whole stream jitters.
- `request_repaint` fires every frame, with no end.
- `here()` points at "5", so the floating answer buttons have an anchor only every other frame (`app/draw/host.rs:141`).
- `f` takes a snapshot of the spots (`app/capture.rs:24`), so on half the frames the kept card gets no hint.

Related, from the same cause: when a request first arrives at the bottom while you follow the latest, last frame's spots lack its index. So for one frame it is drawn both in the stream and below it, and then it jumps. The same happens on the first frame after opening the chat, switching the thread, or a Resync.

The test `two_requests_at_once_are_each_answerable_and_kept_in_sight` (`app/tests/chat_view.rs:149-152`) checks a single frame after 3 + 1 frames. Whether it passes depends only on frame parity: one frame more or fewer flips it.

## 2. Requests kept in sight are never limited in height, and can push the typing box out of the panel

`chat/render.rs:51`, `:78-83`.

The cards below the stream are drawn in full, and nothing scrolls them. The stream's height is floored at 50 (`.max(50.0)`), so once the cards are taller than the space left, the cards and the typing box run past the bottom of the panel. The new panel clip (`app/draw/panels.rs:17`) then hides them. In bar mode they also paint into the item bar's area.

Sequences:
- A command approval whose `detail` is an 80-line heredoc, then a few messages that push it out of the stream. The card is taller than the panel, and the typing box and the card's lower part are not visible.
- Five sub-agents asking at once in a 500-pixel-tall panel: the same.

This breaks rule 2, "stays in sight above the typing box". Before this commit the card was only in the scrollable stream, and the typing box always showed.

Smaller, same lines (`render.rs:91-95`): `below` is last frame's height. Whenever what is under the stream grows (a card appears, or the typing box gains a line), that frame overflows the panel's bottom and the clip cuts it. The repaint fixes it on the next frame; with finding 1, never.

## 3. Putting the cursor on a kept card sends you to the history copy, and the card disappears under the pointer

`app/draw/host.rs:110` → `chat/nav.rs:264-268` → `chat/render.rs:280-286`.

A click, `f`, or a long press on a kept card finds its spot (key = the stream index) and calls `move_to`. That calls `put_cursor(index, Show::Middle)`, which scrolls the stream to the request's place in the history. Setting the cursor also turns stick-to-bottom off. On the next frame the request shows in the stream, so the kept card is removed.

Sequence: two requests are kept in sight. You click the second card to answer it there. The stream jumps up to the second request in the history, both kept cards move or vanish, and the answer buttons follow the history copy. Clicking a question's option in a kept card (`request_card/draw.rs:90`) picks it, and then the card vanishes the same way.

The plan's "the cursor, `f` and its answers' buttons reach it there" doesn't hold. The cursor never stays on the kept card.

## 4. With several waiting, the keys stay on the first until the server's reply arrives: `y y` answers the first twice

`chat/actions.rs:263-273`, `chat.rs:203-211`, `agents.rs:259-263`.

`keyed` is the first pending request while you follow the latest. Answering only POSTs the answer. Nothing marks the request answered locally; it stays `Pending` until `RequestUpdated` comes back.

Sequence: r1 and r2 are waiting. You press `y` (r1 is allowed, and the reply is in flight), then `y` again for r2 within the round trip. Both POSTs go to r1: the second one fails or is ignored, and r2 is never answered. `y` then `d` sends allow and deny to r1. Before this commit only one request was reachable, so this sequence couldn't arise.

## 5. "Shown in the stream" means one pixel shown

`chat/render.rs:69`, `:78`.

A request counts as shown when its rect `intersects(stream.inner_rect)`.

Sequence: you follow the latest, and new messages push a request up until a strip of 1-2 px of its bottom margin is left at the stream's top edge. It counts as shown, so it isn't kept below, and nothing of it is readable or clickable. The same applies at the bottom edge with the cursor set: only the title row shows, and the question's fields and options are cut off. Rule 2 doesn't hold for these cases.

## 6. A wide item's hidden part can't be reached by keys or by touch

`chat/render.rs:261`.

The item's box disables drag. `Go::Sideways` / `Go::Panels` act on the stream's viewport (`viewport.rs` `sideways`), and the stream scrolls vertically only, so they do nothing. What remains is the mouse wheel with shift, a trackpad's horizontal swipe, or the floating bar.

Sequence: on a phone, a markdown table's right-hand columns can only be reached by hitting the thin floating bar. A horizontal swipe over the table does nothing. With the keyboard only, nothing reaches them at all.

## 7. A question kept in sight is laid out twice with the same TextEdit id

`chat/render.rs:80` and `:362`, with `request_card/draw.rs:98`.

The stream lays out every item, so its copy of the request card is built even while scrolled out of view. The kept card adds a second `TextEdit` with the same absolute id (`("question", request, question)`).

The result is an id clash: egui's debug builds draw a warning over it. Focus is shared between the two, and each focused `TextEdit` reads the frame's text events, so while both exist and one has focus, a typed character is inserted twice into the same draft.

## 8. Answers now need the request's item to be in the stream

`chat.rs:221-228`, used by `chat/actions.rs:264`.

Before this commit the chat offered the first pending request's answers from `view.requests`. Now `pending_items` only finds requests that have an `ItemBody::Request` item, inside a turn listed in `view.turns` (`store.rs:32`, `items_of`).

Sequence: a `RequestUpdated` is applied while its `ItemUpdated` hasn't been, or the item's turn isn't known yet (an event before the state it refers to). For that time the chat offers no way to answer and keeps nothing in sight. Before this commit, the keys still worked.

## 9. The tests don't protect what they claim

1. **`long_unbreakable_lines_stay_inside_a_narrow_panel`: the "drawn outside" half can never fail** (`app/tests/chat_view.rs:89`).
   - Each text rect is intersected with its own clip rect before the check, and every clip rect now sits inside the panel (`panels.rs:17`).
   - For a text drawn wholly left of the panel (the reported bug: "later items drawn shifted left"), the intersection is an inverted rect with `left = clip.left` and `right < panel.left`. Both checks pass.
   - Only the "panel widened" check bites. Removing `ui.set_max_width(width)` (`render.rs:262`) passes the test: nothing wraps any more, and every long line scrolls sideways in its box. So "its text wraps there" isn't tested either.
   - "The item after it is drawn" accepts a shape scrolled out of sight or clipped away.
2. **`two_requests_at_once_are_each_answerable_and_kept_in_sight`:**
   - It samples one frame, and whether that frame shows the cards depends on parity (finding 1).
   - `drawn_containing` ignores clip rects, so "in sight" means "laid out within the panel rect", not visible.
   - It doesn't cover cards taller than the room left (2), the cursor or a click on a kept card (3), answering both in a row (4), or that `y` on the second actually sends r2's answer. It only checks that some key is bound.
3. **The kept card is outside the width rule and its test.** The "a request" case in the width test has only two items, so the request is always shown in the stream, never kept below. The kept card (`render.rs:80`) isn't inside any item's box, so the plan's "one rule in the chat's item drawing, tested once for all items" doesn't cover the one card this step added. A long unbreakable option label or title there is untested.

# Chat checkpoint after step 1: responsibility (d54286c)

Paths are under `meta-harness/crates/cockpit-ui/src`. Ranked, most serious first.

## 1. `ui.spots` now does two jobs, so the kept-in-sight card turns on and off every other frame

`chat/render.rs:52`, `:69`, `:78-82`; `chat.rs:86-88`.

`spots` is documented as "messages and links drawn last frame". The navigation and hints read it (`chat/nav.rs:256`, `chat/things.rs:92`). The commit gives it a second job: line 52 reads it to decide what "the stream showed last frame". Then line 81 pushes the cards drawn *below* the stream into the same list. The trace:

- Frame N: the request isn't among the stream's spots, so its card is drawn below and pushed into `spots` with key `index`.
- Frame N+1: `shown` (line 52) now contains `index`, because of that push. The filter at line 78 drops it and the card isn't drawn. Line 69 resets `spots` to the stream's spots only.
- Frame N+2: the card is drawn again, and so on.

Each flip changes the height of what's under the stream. Lines 91-94 then call `request_repaint()`, so the panel repaints every frame for as long as a request is out of sight. The test (`app/tests/chat_view.rs:143-153`) checks a single frame, so it can pass on parity.

The root cause is coupling: drawing reads the navigation's list of places to answer a drawing question ("did the stream show it"), and then writes into that list. The stream needs its own record of what it showed (`drawing` filtered at line 69, kept apart), with the cards below added only to what navigation sees.

## 2. The width fix sits in the chat's loop, not in the shared message drawer it fixes

`chat/render.rs:253-265`; `conversation.rs:50-71`.

The drift came from `CommonMarkViewer` inside `crate::conversation::message` (`conversation.rs:62`): a table widens its message. The cure (a horizontal `ScrollArea` with `set_max_width`) wraps the chat's per-item loop instead of the shared drawer. Three other callers of the same drawer still widen on a table:

- `discussion.rs:206`
- `spec.rs:345`
- `spec.rs:851`

There, the new panel clip (item 3) only cuts the table off; it doesn't scroll. The plan's "one rule in the chat's item drawing" ended up as a rule on one *caller* of the item drawing, not in the drawing itself. Anything else that draws an item (item 4) has to repeat it, and doesn't.

## 3. The clip is set in one host (the panel frame), not in the shared host drawing

`app/draw/panels.rs:14-17`; `app/draw/host.rs:23-80`; `app/look.rs:146-151`.

Both hosts draw a buffer through `render_view` (`host.rs:23`), which already works out the buffer's `content` rect and scopes it (`host.rs:76`, `scope_builder(...max_rect(content))`). The clip was added in `render_panel` instead, so:

- **The quick look isn't clipped.** `render_quick_look` calls `render_view` with no clip, and the chat can be held there.
- **The panel's clip is too wide.** It is the whole panel rect, not `content`. A buffer can still paint over its own host's top line, the notes gutter and the item-button bar (`host.rs:44-67`).

Keeping the buffer inside its area is the host's rule. It belongs once in `render_view`, clipped to `content`.

The comment at `panels.rs:14-16` also describes a view's internals ("the chat's items scroll sideways in a box of their own") in the shell.

## 4. The kept-in-sight card is a second way to draw a stream item

`chat/render.rs:76-83` against `chat/render.rs:250-266` and `:360-364`.

In the stream, a request item is drawn by `render_item` (`ItemBody::Request`, line 360) inside the item frame. That frame carries the cursor's selection fill (`on`/`fill`, lines 251-253) and the width box (lines 260-264). Below the stream, `draw()` calls `render_request` directly and records a `Spot` by hand. So the same item, keyed as the same place:

- gets no selection fill when the cursor is on it there, though the commit says it is a place the cursor reaches;
- skips the width box;
- looks its request up its own way (`self.thread().and_then(|t| t.request(..).cloned())`, line 79, while `render_item` uses `view.request`).

The per-item frame should be one function both places call.

## 5. Who gets the answer keys is decided in the chat, overriding `request_card`'s own decision; the answer actions are built twice

`chat/actions.rs:262-276`; `request_card/answers.rs:16-19, 80-108`; `inbox.rs:360-367`.

`request_card::answers` is documented as the one place that decides an answer's key alias ("only when its kind is the only one of it"). The chat now adds a second key rule beside it, "only the one you're on, else the first". It does this by overwriting the card's decision field by field: `crate::request_card::Answer { alias: None, ..answer }`, line 270. The rule for "several requests, no key is two's" concerns the set of requests, which is `request_card`'s domain. It lives only in the chat, so the inbox can't use it when it reaches several at once (the plan's "in the chat and the inbox").

The chat's rewritten block also builds the same actions the inbox builds, by hand:

- the same id formats (`request.{}.page`);
- the same labels ("Open the page: {}");
- the same detail ("Answers the agent's request this way").

Compare `chat/actions.rs:268-273` with `inbox.rs:362-366`. `request_card/mod.rs:3-5` says these are "the same in the chat and the inbox". Building the `Action` (and choosing who is keyed, given several requests and the one you're on) should be one `request_card` function that both callers use.

## 6. The stream's height depends on last frame's measurement, and the old guess is still in the state

`chat.rs:91-93`, `chat.rs:111`; `chat/render.rs:51`, `:89-95`.

The plan says the height is "measured, not the old 105-point guess". Yet `105.0` stays as `below`'s default (`chat.rs:111`), so the first frame of every chat view still uses the guess. The layout of the stream against what's under it is solved as a frame-late feedback loop kept in view state, with a repaint to settle it. It is not laid out in the frame: the bottom part drawn first, or bottom-up, and the stream given the rest. This loop is what turns item 1 into a constant repaint.

## 7. "Waiting on you" is worked out in several places

`chat.rs:221-228`; `chat/render.rs:79`; `chat/actions.rs:267`; `request_card/answers.rs:69-72, 83`.

- `pending_items` finds pending requests by scanning `thread.requests` for each `Request` item.
- It returns only ids, so both callers look each request up again: `render.rs:79` with `t.request(..).cloned()`, and `actions.rs:267` with `view.request(..)`.
- `request_card::answers` and `page` check `status == Pending` once more.

`pending_items` should return the requests themselves; that is the one answer to "what waits here".

## 8. Tests couple to internals and duplicate fixtures

`app/tests/chat_view.rs:18-34`, `:116-131`, `:139-140`.

- The same `RuntimeRequest` builder is written twice in this file. Others already exist: `request_card/tests.rs:7, 78` and `app/tests/attention.rs:35`.
- The second test steers through the chat's internal index-as-string place key: `app.desk.chat().pending_items()[1].0` and `move_to(&second.to_string())`. It doesn't go through what's on screen or a public place. It breaks if the place key format (`index`, `index:n`) changes.

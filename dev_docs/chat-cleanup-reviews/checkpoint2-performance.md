# Checkpoint 2: performance review

Commits: e1a2167 (overlay layer), ff7e269 (a request's answers while typing),
12660e6 (sub-agents, server), 29b5dde (sub-agents, cockpit).
Checked against: "cost proportional to what's on screen ... work happens only
for what's shown, once, and is remembered" and Part 2b's "nothing is
allocated per frame for it".

Ranked, worst first. Paths are relative to `meta-harness/crates/`.

## 1. The sub-agent pop-up rebuilds all its steps, whole texts included, every frame (high)

`cockpit-ui/src/app/draw/subagent.rs:26-42, 70-80`

Each frame the pop-up is open:
- `s.clone()` of the `Seen` (line 26), which copies the whole `prompt`
  (the full task, often KBs), `description`, `title`, `doing` and the ids.
- `view.under(&s.item_id)` walks **every** item under the sub-agent. For
  each tool call it clones the title. For each assistant message it builds a
  new `String` joined from all its text blocks: every message the sub-agent
  wrote, copied whole.
- The cap `STEPS_MOST` (200) is applied only when drawing (line 74), after
  the whole list has been built. Steps past the 200 are still copied.
- The 200 kept are all laid out in one `ScrollArea` that doesn't use
  `show_rows`, so every label is laid out, off-screen or not. egui's galley
  cache saves re-wrapping, but it hashes each full text every frame to find
  it. A message still streaming misses the cache on each chunk and is
  re-wrapped whole.

Cost: the pop-up is open while a sub-agent streams, so frames come at up to
60 fps. Take a Claude Code sub-agent with 300 steps and a few long answers
(10-20 KB each): each frame makes about 300 allocations, copies and hashes
around 50-100 KB of text, and lays out 200 widgets. That is several MB/s of
copying to show what at most ~15 lines of the scroll area can display. The
work grows with the sub-agent's whole history, not with what's on screen.
None of it is remembered between frames, even though `under` and the
items only change when an event arrives.

Fix shape: borrow from the view while drawing, with no owned step list.
Build only the visible slice (`ScrollArea::show_rows` over `under`'s
indices). If a summary is needed, key it on the items' `changed` counters,
or on `activity`.

## 2. Every `session/update` from any harness now gets an extra deep JSON clone and an extra lock (high: server, per chunk)

`harness-acp/src/adapter.rs:241-243`, `harness-acp/src/state/subagents.rs:33`

The new raw handler runs ahead of the typed one for **every** notification,
for every pack, sub-agents or not:
- `n.params.get("sessionId")...to_owned()` allocates a `String`.
- `n.params.get("update").cloned()` deep-clones the whole update `Value`.
  `take_raw` only takes `&Value`, so a borrow would do; the clone is pure
  waste.
- `lock(&on_child.state)` takes the session mutex, and `take_raw` checks
  `children` and the kind. For the agent's own updates (nearly all of them)
  it returns `false`. The typed handler then deserializes the message and
  takes the same mutex again.
- For a sub-agent's own update, `take_raw` clones the update a second time
  (`update.clone()` for `serde_json::from_value`, line 33) before
  deserializing.

Cost: once per chunk, for every session. A message chunk is small, but a
`tool_call_update` carries diffs, file contents and terminal output (often
tens of KB), and each one is copied once more for nothing. A sub-agent's
steps are copied twice. That makes two mutex acquisitions per update where
there was one.

Fix shape: borrow `n.params["update"]`. Peek at `sessionUpdate` and the
session id by reference, and lock only when the kind is a sub-agent kind or
the session is a child's. Deserialize a child's update from the owned params
(`serde_json::from_value(params.remove("update"))`), not a clone.

## 3. Each sub-agent step ships the whole `Subagent` (prompt included) and clones it twice to compare (medium: server)

`harness-acp/src/state/subagents.rs:106-108, 156-176, 129-131, 64-73`

- `apply_child` calls `change_subagent` on every `ToolCall` of a child (to
  set `doing`). `change_subagent` clones the whole `Subagent` twice
  (`before`, line 157; `now`, line 159) just to compare it, and then emits
  `SubagentUpdated { subagent: now }`. That event carries the full `prompt`
  and `description` again, though only `doing` changed.
- Cost per step: two full clones plus one event that is serialized to each
  subscriber and parsed in the cockpit. Take a sub-agent with 100 steps and
  a 3 KB prompt: about 300 KB over the wire and 600 KB of clones, all of it
  repeating the prompt.
- Starting one emits twice. `start_subagent` emits, and then
  `change_subagent` runs at once and emits again (Claude Code: title,
  description, state and cancel set on the announce, line 73; Antigravity:
  the description, line 131). Each of those also calls `update_item`, which
  emits the item.

Fix shape: compare only the field the change touched (the closure can
report whether it changed anything). If the prompt must be sent, send it once.

## 4. The controls under what you're on now do the item's work every frame while typing (medium: ff7e269)

`cockpit-ui/src/app/draw/host.rs:138-154`

Before ff7e269, typing mode used only `typing_rect()`. Now `item` is worked
out every frame in both modes:
- `view.spots()` (chat: `self.ui.spots.clone()`, `chat/nav.rs:432`) clones
  every on-screen spot. That is every drawn item, link, chip and code block,
  each with a `String` key: 100-300 allocations per frame, only to `find`
  one key (line 141).
- `view.here()` → `pending_item()` (`chat.rs:306`) scans **all** of the
  thread's requests (`requests()` keeps every one ever asked; none are
  removed) for one still waiting. That is O(all requests) per frame. Over a
  long session with many permission asks it is hundreds of entries.
- `bound` (line 145) now runs `applies()` (a `When` judged against the
  situation) on every `Layer::On` action before filtering by placement.
  Before, the placement was checked first and short-circuited.
- `at(...)` (line 146) clones each matching `Bound` (an `Action` with its
  strings) a second time. `bound_actions()` has already cloned the whole
  list. On top of that, a `groups` `Vec` is allocated per frame.

This runs every frame while you type and the agent streams (the common case
after sending). Fix shape: look up the one spot without cloning the list.
Keep the pending request's index, or have `pending_item` walk only the
pending ones. Filter by placement before `applies()`. Move the `Bound`s out
of `bound` rather than cloning them.

## 5. The strip clones each shown sub-agent whole every frame (medium-low)

`cockpit-ui/src/chat/subagents.rs:51, 65-67, 93-98`

`render_subagents` runs inside the chat's draw, every frame:
- It filters **all** sub-agents ever started in the thread (line 51) and
  clones each shown `Seen` whole, `prompt` included. The strip uses only
  title, doing, status, since and ended.
- When there are more than three and they fold into one count chip, every
  clone is still made.
- Each chip clones its `LayoutJob` to measure it (line 67) and builds a
  `words` `Vec` plus two `format!`s. Lines 93-98 are ordinary egui work.
  The clone at line 51 is not; it should borrow, with a short borrow scope
  like `drop(view)`.

## 6. Every `SubagentUpdated` bumps the thread's `activity`, and with it whole-stream work (medium-low)

`cockpit-ui/src/store.rs:409-413, 485-492`; `cockpit-ui/src/chat.rs:330-340`;
`cockpit-ui/src/app/capture.rs:186-190`

`apply_session` reaches the view through `self.view()`, which does
`activity += 1` for any event, a sub-agent's included. It also counts the
event as news (`arrived`) without checking that anything changed.
`subagent_now` doesn't report whether anything changed.

A sub-agent's state is not stream data. Even so, the bump invalidates:
- `shape()`: same thread, epoch and folds, but another `activity`, so
  `rows::shape_facts(view.stream())` hashes **every item in the stream**
  (chat.rs:332);
- `items_changed()` (epoch ^ activity): with a search active, the key bar's
  `search_status` rebuilds `items()` (every item's text) and re-ranks all of
  them (capture.rs:186-190).

When a step arrives in the same frame as its `ItemUpdated`, the two
coalesce. State-only events don't: `subagent_update` state changes
(`running` / `requires_action` / `idle`), `session_message` (the prompt),
the end-of-turn `Idle` for each one, and the double emit on start (finding
3). For a session with thousands of items, each of those costs a full
stream hash, plus a full search re-rank when a search is up.

Fix shape: have `SubagentUpdated` update `subagents` without touching
`activity`. Count it as news only when `subagent_now` saw a change.

## 7. Linear lookups over lists that grow with the session, on every drawn row and every frame (low)

- `cockpit-ui/src/chat/render.rs:778`: `view.subagent(subagent_id)` is a
  linear `find` over every sub-agent. It runs for every drawn sub-agent row,
  every frame.
- `chat/render.rs:785`: `view.under(&item.id).count()` walks the Flatten/Map
  chain item by item (O(steps)). Each drawn row pays it every frame, where
  `under.get(id).map_or(0, Vec::len)` would be O(1).
- `cockpit-ui/src/store.rs:99-108`: `subagent_now` does a linear `find` per
  event.
- `subagents_working()` (`chat/subagents.rs:39`) and `delegating`
  (`chat/live.rs:38`) each scan all sub-agents. They run 3 times per frame
  per chat (live line `live.rs:81`, panel top line `render.rs:882`,
  liveness `live.rs:38`).
- `ThreadView::subagents` is never pruned, and neither is the server's
  `subagents` / `children`. Over a day-long orchestrating session (hundreds
  of sub-agents), every one of the scans above walks them all, every
  frame. Each is microseconds, but they grow with the session's history,
  not with what's shown. Fix shape: a `SubagentId → index` map, and a
  working count kept in `subagent_now`.

## 8. The overlay layer allocates per frame and its stack isn't "a few entries" (low: e1a2167)

- `cockpit-ui/src/app/overlays.rs:62`: `before_input` collects the open
  kinds into a fresh `Vec<Overlay>` every frame while any overlay is up.
  `Vec::from_iter` on a filter allocates as soon as one element passes.
  This breaks Part 2b's "nothing is allocated per frame for it". It also
  works out `overlay_open` for all nine kinds, then `top()` checks them
  again via `contains`. Passing `|o| self.overlay_open(o)` straight to
  `outside_tap` needs no list.
- `cockpit-ui/src/app/capture.rs:219-221`: the hint labels push one stack
  entry **per chip**, so while hints are up the stack is hundreds of entries
  (one per spot on screen), not a few. `inside()` (`overlay.rs:105-107`) is
  a linear scan of it, but only on a press, so the cost is small. The "a
  few entries" premise doesn't hold for this kind; one bounding entry per
  kind, or a test against the hint rects kept in `Capture::Hint`, would
  restore it.

## 9. Server odds and ends (low)

`harness-acp/src/state/subagents.rs`
- Line 116: `subagents_started` runs on **every** `tool_call` and
  `tool_call_update` (`state.rs`, both call sites). For a pack with
  `started_by` (Antigravity), each run clones the whole `StartedBy` (four
  `String`s) before even looking at the input. That is four allocations per
  tool-call update, for every tool. Borrowing it (`as_ref()` with a
  split borrow, or cloning only on a hit) is enough.
- Line 118: `starting.entry(native.to_owned())` allocates the key even when
  it's already there. Worse, an entry whose call never leaves `Pending` (the
  turn cancelled while you're being asked) is never removed. It stays in
  `starting` for the session's life, along with the copied input list.
- Line 118 again: once a call has started its sub-agents and been removed
  from `starting`, a later update that carries `rawInput` again with
  `in_progress` or `completed` re-inserts it and starts the sub-agents
  **again**. That means duplicate sub-agents and duplicate items. The 1.4.3
  fixture drops `rawInput` on `completed`, so the fixture doesn't show it,
  but nothing in the code prevents it.
- Line 140: `subagents_end_with_the_turn` is O(sub-agents × children) at
  every turn end (`children.values().any(...)` for each sub-agent). Trivial
  at dozens; quadratic in the session's history.
- Line 49: `cancellable` is a linear scan of `children`, once per click.
  This one is fine.
- The per-chunk cost already in place (`update_item` clones the whole item
  for each `ItemUpdated`, `state.rs:409-413`, so a message costs O(n²)
  bytes) now also applies to every sub-agent's own stream
  (`apply_child` → `apply_as_sent`). When several sub-agents write at once,
  that cost multiplies by how many are writing. On the cockpit side, each
  of those chunks triggers finding 1's whole rebuild while the pop-up is
  open.

## Seen in passing (not performance)

- `cockpit-ui/src/store.rs:99-108`: `subagent_now` stamps
  `since = now`, and `ended = now` for any that aren't working, even when
  the event is a replay (`live == false`). After a resync or reconnect, each
  ended sub-agent of the session counts as "just ended": it comes back into
  the strip for 6 s and the strip repaints every second. Elapsed times also
  restart from the replay.

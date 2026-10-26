End review of the chat plan (d54286c..50a125f): performance.

Paths are relative to meta-harness/. Ranked most serious first.

## 1. A code block being streamed sends one highlight request per update, with its whole text

- crates/cockpit-ui/src/highlighter.rs:49-66 (`colours`), :86-99 (`tick`), :119-123 (`key`); crates/cockpit-ui/src/conversation.rs:146 (`code_block`); src/arch.rs:145-150 (`code_highlight`).
- The cache key is a hash of the fence and the full block text. While the agent writes a block, the text changes with every update. Each frame that draws the changed block makes a new key, queues it, and `tick` sends a POST with the whole block so far.
- How often: once per update while the block streams. That is dozens a second.
- Cost: for a 10 KB block streamed in 500 chunks, about 2.5 MB is uploaded through home's relay. The server parses the growing text 500 times on the blocking pool. Each answer is kept under a key that is never asked for again.
- These dead keys fill `known` toward `KEPT` (1024). Past that, `tick` clears everything (:86-88). Every finished block on screen is then asked for again. One long streamed block can flush the cache for every other chat and the code view.

## 2. Every row is laid out in full on open, on a thread switch, and on every frame of a resize

- crates/cockpit-ui/src/chat/render.rs:362-372 (`Measure`, `known`); :170-178 (`tend_folds` clears `heights` and `diffs`).
- A row is skipped only if its height is known for the same `Measure`, and `Measure` includes `width` and the text size. On first open, and after a thread switch, nothing is known, so every row is drawn in one frame. While a panel is dragged wider or narrower, the width changes every frame, so every row is drawn every frame of the drag.
- Cost at a chat of 3,000 items: 3,000 rows of markdown parsed and laid out in one frame. Every file change in the history is diffed at once with `similar` over full old and new file texts (`hunks_of`, render.rs:474-485, called even for folded calls at :568-569 and :597). That is 200 diffs of 2,000-line files in one frame. Every code block in the history is queued for highlighting at once (see 3).
- Under the "everything" verbosity, every output and diff is open by default. The first frame then draws every line of every diff and output in the history (see 5).

## 3. Code-block colours are asked for one block per HTTP request, never batched

- crates/cockpit-ui/src/highlighter.rs:89-99; crates/cockpit-ui/src/net.rs:826-831.
- `tick` sends one POST per queued block. When finding 2 draws every row, every code block in the history is queued in the same frame.
- Cost: a long session with 300 code blocks fires 300 POSTs in one frame through `/s/{server}/…`. Each carries the full block text. The browser caps connections per host, so these queue behind each other and delay the chat's other calls.
- The same burst repeats after each cache clear at 1024 (finding 1).

## 4. Request cards in the history are never skipped, and each is found by a linear search

- crates/cockpit-ui/src/chat/render.rs:363, :366 (`!asks`), :377, :630-631; crates/cockpit-ui/src/chat.rs:376-378; crates/cockpit-ui/src/store.rs:51-53.
- Every `Request` row is drawn in full every frame, wherever it is. That includes answered ones far above the view. Each card is a frame and three to five labels, including `request.detail` (a full command line or path).
- Each `Request` row also calls `view.request(id)` up to three times per frame. That is a linear scan over all of the thread's requests.
- Cost: a long session with 500 approvals lays out 500 cards every frame and does about 1,500 scans of 500 entries each (about 375,000 id comparisons) per frame. This runs on every frame the app paints. While a turn runs, the heartbeat alone paints about 5-7 frames a second (finding 6).

## 5. Opened diffs and outputs in the chat draw every line every frame

- crates/cockpit-ui/src/chat/render.rs:82-87 (`draw_change`), :579-588, :601-603, :612; crates/cockpit-ui/src/diff.rs:12-31, :42-49.
- An opened change draws each line as a frame, a horizontal row and three labels, plus a `format!` for the line numbers. Nothing in the chat skips the lines out of sight inside an opened row.
- Cost: an agent writing a new 2,000-line file, opened, is 2,000 frames, 6,000 labels and 2,000 string allocations per frame. This applies while the row is anywhere in the drawn band (finding 6). Under "everything", every diff is open by default.
- An opened call's text output is cloned in full each frame (`block_detail` → `block_summary`, render.rs:30-35; agents.rs:502). An opened command's output is copied into a new `RichText` each frame (render.rs:612). A 100 KB build log is copied and hashed every frame.
- `crate::diff::counted` walks every line of a change twice per frame for the "+12 −3" line (render.rs:64-66, :599), opened or not. A 2,000-line change costs 4,000 steps per frame to print two numbers that change only when the item does.

## 6. Three screens of rows are fully laid out every frame, and the live line repaints the whole app about 5-7 times a second

- crates/cockpit-ui/src/chat/render.rs:340 (`near` = the view plus one view height above and below), :366; crates/cockpit-ui/src/indicator.rs:148-153, :159; crates/cockpit-ui/src/chat/live.rs:115-117.
- A row that has a known height is still drawn in full if it is within one view height of the visible area. Each frame therefore lays out about three screens of content, not one.
- While a turn runs and its line is in sight, the heartbeat asks for a frame every `pixel` (10 s / about 48 px, about 0.21 s), and the breathing dot every 0.15 s. That lasts until 60 s of quiet. Each request repaints the whole cockpit, and every frame pays for findings 4, 5, 7 and 8 too.
- Per drawn assistant message per frame: `item.body.said()` joins all of its text into a new String (render.rs:534). `nav::links_of` does it again and scans the whole text for paths (render.rs:540; chat/nav.rs:11; conversation.rs:237-265). `parts` re-splits the text, and egui_commonmark re-parses each part's markdown (conversation.rs:123-139).
- Per drawn code block per frame: the full text is hashed with SipHash for the cache key (highlighter.rs:54, :119-123). It is copied into a new `LayoutJob` with one section per highlight run (conversation.rs:146-149; highlighter.rs:128-155), and egui hashes that job to find its galley. A 2,000-line block on screen is copied and hashed twice per frame, with thousands of sections each holding a cloned `FontId`.

## 7. The whole row list is walked, and a String allocated per row, every frame

- crates/cockpit-ui/src/chat/render.rs:345-385, :379, :127; crates/cockpit-ui/src/viewport.rs:317-325.
- Every frame, every row gets `view.in_stream`, a `Measure` that maps over each item it covers, a `heights` lookup, an `allocate_exact_size`, a pushed rect, and a `Spot` whose key is `row.first.to_string()`. Then the spots out of sight are thrown away (render.rs:127), and `scrolling.drawn` builds a new `stops` Vec of every row.
- Cost: at 3,000 rows, about 3,000 small String allocations and four Vecs of 3,000 per frame, at the repaint rates in finding 6. The spot keys are needed only for rows in sight.
- On top of this, the shape is rebuilt on every thread event (chat.rs:297-309, keyed by `view.activity`, which store.rs:248-251 bumps for every event, even ones that change nothing). Each rebuild collects the stream and builds three Vecs of its length (rows.rs:345-403). While streaming, that is once per frame.

## 8. A remembered chat search rebuilds and searches the whole chat's text every frame

- crates/cockpit-ui/src/app/draw/bars.rs:62 → crates/cockpit-ui/src/app/capture.rs:179-183 → crates/cockpit-ui/src/chat/nav.rs:343-345, :17-34.
- Once a `/` search in the chat finds something, the key bar shows "/query 3/12" every frame. To do so it calls the chat's `items()`. That builds an `Item` for every stream item, cloning the full text of every message, reasoning and plan. Then `rank_matches` runs a ranked and a full-text search over all of it.
- How often: every frame while the chat is focused, until another search replaces it.
- Cost: a chat of 3,000 items with long reasoning holds several MB of text. All of it is copied and searched on every frame, including the heartbeat's frames in finding 6. The search machinery is the views' shared one, but step 7 makes the chat's version of it the hot one.

## 9. Gathering the chat's actions scans the stream and copies the cursor's message every frame

- crates/cockpit-ui/src/chat/actions.rs:266-267; crates/cockpit-ui/src/chat.rs:268-292, :324-354, :382-384; crates/cockpit-ui/src/chat/render.rs:138, :712; crates/cockpit-ui/src/chat/nav.rs:45-46, :258-271.
- Actions are gathered at least once per frame, and the chat's `actions()` runs even when another view is focused (app/commands.rs:186, :194-199).
- While a request waits on you, each frame calls `pending()` about four times: render.rs:138, `Hosted::here` (render.rs:712), and actions.rs:266-267 (`pending()`, then `on_item()` → `pending_item()` → `pending()`). Each call walks the whole stream into a BTreeMap and clones every waiting `RuntimeRequest` with its options and questions. A request can wait for hours.
- `target()` calls `links_of` on the cursor's item. For an assistant message that is `said()` plus a path scan of its full text. `copy_here()` calls `said()` again. With a code block picked, `codes_here()` re-splits the message and clones every code block. That is two or three full copies of the cursor's message per frame.

## 10. The server looks up what was typed with a blocking SQLite call inside the async event task

- crates/harness-core/src/sessions.rs:41-50, :76-83; src/modules.rs:332-333; crates/nah-state/src/slots.rs:158-161; crates/nah-state/src/lib.rs:80-86.
- `with_typed` runs in the tokio task that forwards a session's events. For a user message whose text holds `HANDED_ON` and has no `typed`, it takes the state database's `std::sync::Mutex`, hashes the full sent text with SHA-256, and runs a query. All of this is synchronous on a runtime worker.
- How often: once per linked message live. On a resumed session's replay, `user_chunk` emits the item again for every chunk (crates/harness-acp/src/state.rs:449-457). Each chunk after the one carrying `HANDED_ON` costs a lookup that cannot match (the text is still partial).
- Cost: hundreds of linked messages replayed means hundreds of blocking queries in a row. While another caller holds the DB mutex, this session's whole event stream stalls, and so does the worker thread.
- `keep_typed` writes in the same way from the async `send` path (crates/harness-core/src/slots.rs:592-600).

## 11. Interleaved chunks from two senders become one item per chunk

- crates/harness-acp/src/state.rs:369-376, :344-354, :465-484.
- A session keeps one open message and one open reasoning. Any update from a different parent (the agent, or a sub-agent's call) closes them. The next chunk then opens a new item.
- When two senders' chunks interleave (two sub-agents streaming text, or the agent streaming while a sub-agent's calls update), every switch costs an extra close event, and every chunk becomes a new item.
- Cost: a few hundred chunks produce a few hundred items instead of two. Each is a full `ItemUpdated` over the socket, a new row in the cockpit, a store insert, and a shape rebuild. This grows the chat toward the thousands of items that findings 2, 4 and 7 scale with.

## 12. A resync keeps a full copy of every thread until the next event more than 10 s later

- crates/cockpit-ui/src/store.rs:221-225, :256-262.
- On `Resync`, the old threads are kept in `before` to tell replayed events from new ones. They are dropped only in `replayed()`, which runs only when a thread event arrives more than `REPLAY_S` after the resync.
- Cost: if the agents are idle after a reconnect, every item of every thread (outputs and diffs included) stays in memory twice, for as long as nothing arrives. That can be many MB in the browser.

## 13. `typed_messages` grows without bound

- crates/nah-state/src/migrations.rs:65; crates/nah-state/src/slots.rs:148-155.
- One row per message you send with links, keyed by the hash of the expanded text. Nothing ever deletes rows, and `at_ms` is written but never read.
- Cost: small per row (your text and its links as JSON), but it grows for the life of the project.

## 14. The tag places are rebuilt per thread event at O(T² log T) for a turn of T items

- crates/cockpit-ui/src/chat/things.rs:25-42; crates/harness-contracts/src/tags.rs:218-222.
- `tag_places` is keyed by `view.activity`, so it is rebuilt after every event. For each item it calls `message_number`, which filters the item's whole turn, collects the messages and sorts them.
- How often: once per event-frame while a tag in the chat is drawn (app/notes.rs:267 asks every frame).
- Cost: a long autonomous turn of 2,000 items with 200 messages means about 4 million checks and 2,000 sorts per rebuild, per frame while the agent streams. This code predates the plan, but the plan kept it on the chat's per-event path.

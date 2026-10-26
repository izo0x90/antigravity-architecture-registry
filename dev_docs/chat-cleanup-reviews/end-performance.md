# End review: performance

Scope: `git diff 3e1a805^..HEAD` (18 commits). Read-only; nothing was built or run.
Paths are relative to `meta-harness/`. Ranked most important first.

## 1. A kept search in the chat copies and re-ranks the whole session on every event

- `crates/cockpit-ui/src/chat/nav.rs:440-446` and `crates/cockpit-ui/src/app/capture.rs:186-191`.
- The chat's `items_changed` is `epoch ^ activity`. `activity` goes up on every event in the thread, including each streamed chunk.
- `search_status` runs every frame for the focused view. When the value changed, it calls `items()` and `rank_matches`.
- The chat's `items()` builds an `Item` for every item in the stream. Each one copies the item's text (`item_text` joins every block). Then `rank_matches` runs fuzzy name and text ranking over all of it.
- Once you search in a chat (`/foo`), the query stays in `desk.searches` for n/N. From then on, every streamed chunk re-copies and re-ranks the whole session.
- Cost: a session of 10,000 items at about 500 characters each is about 5 MB of copies plus a full fuzzy pass, per chunk, many times a second while the agent writes. This is the worst per-event cost in the cockpit.
- Commit 100abe6 made this the pattern for every view (plan Part 2 item 5). But for the chat, the counter it reads changes far more often than the items search sees. Fix: count only changes that alter what search reads (an item added or removed, or a message's text finished). Or re-rank only the items whose `changed` stamp is newer than the last ranking.

## 2. Row positions are recomputed for the whole stream whenever any drawn row's measure changes

- `crates/cockpit-ui/src/chat/render.rs:565-569` (bump) and `600-623` (`lay_out_rows`).
- `heights_changed` goes up when a row's `Measure` changes, even if its height is the same. `Measure.changed` is the newest `view.changed(i)` across the row. So every event that touches a drawn row bumps it.
- Each bump makes the next frame run `lay_out_rows` over every row. It allocates three vectors as long as the row list (`tops`, `heads`, viewport `stops`). Per row it does an `in_stream` lookup, a `HashMap<ItemId>` lookup that hashes a string, and three pushes. Then it hands the whole `stops` vector to the viewport.
- While the agent streams, the last row's text grows on every chunk. Both its measure and its height change, so every frame does this whole-list pass. The `moved` repaint adds another frame each time.
- Cost: with 10,000 rows, that is about 10,000 string hashes and about 130 KB allocated and freed per frame, for the whole of a long turn. This is the whole-list walk per frame that Part 2 item 6 was meant to remove. It now happens per change instead of per frame. During streaming that is the same thing.
- Fix: bump only when a height changes, since `tops` depends only on heights. Then shift `tops` from the changed row onward. For the last row, the common case while streaming, that is O(1). Or keep the heights in a `Vec` with prefix sums, such as a Fenwick tree.
- Related, and not changed here: `ChatStreamState::shape` (`crates/cockpit-ui/src/chat.rs:355,363`) still hashes every item's ids (`rows::shape_facts`) over the whole stream on every event. With a sub-agent shown alone, it still hashes the whole thread's stream, not that sub-agent's span (see 9).

## 3. The live line scans every row from the top on every frame while a turn runs

- `crates/cockpit-ui/src/chat/render.rs:519-530`.
- `shape.rows.iter().position(|r| view.in_stream(r.first).is_some_and(|i| i.turn_id == *id))` runs on every frame whenever a turn is running. The `start < first` test comes after the scan.
- The running turn is almost always the last one, so the scan walks the whole row list each frame, with a lookup and a `TurnId` string compare per row.
- Cost: O(rows) per frame for as long as the agent works, and the live line animates, so frames are continuous. With 10,000 rows, that is 10,000 compares per frame, for nothing in most frames because the turn is on screen.
- Fix: `RowsAt.heads` already marks where each turn starts. Search back from the end (`rposition` over `heads`), or record the running turn's first row when `RowsAt` is built. Check `start < first` before any scan.

## 4. The plan tree rebuilds every discussion's "whose move" words on every frame

- `crates/cockpit-ui/src/tree/plan.rs:155-166` (`PlanTree::share`).
- `share` runs every frame for every buffer, shown or hidden (`app/lifecycle.rs:68` calls `share_with_buffers` and then `desk.share()`).
- Each call now builds a `BTreeMap<DiscussionId, String>` over all of the plan's discussions. For each one it clones the id, builds a `SlotId`, runs `Agents::state` (which can clone an error or reason string), and `format!`s the words. Then it compares the whole map with the last one.
- Cost: with a few hundred discussions, that is several hundred string allocations and a map build per frame, per plan tree that exists, even when nothing changed.
- Fix: rebuild only when the review changed or the agents' state changed. That needs a version counter on `Agents`, bumped by `apply`. Or key the map on each asked slot's `AgentState`, which only a few slots have, instead of on each discussion.

## 5. Your message's chips are rebuilt from its pieces on every frame it's drawn

- `crates/cockpit-ui/src/chat/render.rs:754-756`, `crates/harness-contracts/src/links.rs` (`links_of`), `crates/cockpit-ui/src/conversation.rs:85`.
- For every user message drawn, `harness_contracts::links_of(pieces)` clones every `Link` and sorts them. A `Linked::Thing` holds a `Subject` with strings. Then `Messages::draw` clones each link again into a `Chip`, and `chipped` builds a new layout job.
- The plan says: "The chat matches chips to pieces once, when the message is first drawn, and remembers the result for that message." This is not remembered. It is redone every frame, for every user message near the screen.
- `chips_here` (`chat.rs:405-418`) does the same again for the cursor's message whenever it is asked.
- Cost: small per message, but it is repeated work on every frame and it grows with the links per message. It breaks the plan's own rule.
- Fix: cache the sorted links (or the chips) per item id. Invalidate with the item's `changed` stamp. Or have the store sort them once when the item arrives.

## 6. Two views still search again on every frame; the terminal re-copies all its matches on every update

- `crates/cockpit-ui/src/spec.rs:1211` and `crates/cockpit-ui/src/changes/mod.rs:1242` still return `None` from `items_changed`. `app/tests/views.rs:316-322` makes that the expected state.
- With a search kept in the spec card or the Changes view, `search_status` (`app/capture.rs:186-191`) calls `items()` and fuzzy-ranks every row on every frame. The Changes view's `items()` also `format!`s and joins each window's working list per row.
- Plan Part 2 item 5 asked every view to say when its items change. These two were left.
- Terminal: `crates/cockpit-ui/src/terminal/mod.rs:266-269` bumps `lines_changed` on every poll that brought anything. `items()` (`:704-712`) then rebuilds a `BTreeMap` of the screen's rows plus every line the server found in the scrollback, cloning each string, and re-ranks all of them.
- With a kept search in a busy terminal (build output), that happens at the update rate. The found set can hold thousands of the 10,000 scrollback lines, and it did not change. Fix: count screen changes and found changes apart, and cache the ranking of the found lines.

## 7. A sub-agent shown alone clones its whole record on every frame

- `crates/cockpit-ui/src/chat/actions.rs:150`.
- `self.thread()?.subagent(id)?.subagent.clone()` runs every time actions are gathered, which is at least once a frame. It clones the whole `Subagent`, including `prompt` (its full task) and `answer` (its full reply), which can be many KB each.
- Only `status`, `can_cancel` and `id` are read.
- Fix: read those three fields through the borrow. The `Ref` can be dropped after copying them.

## 8. The sub-agent strip scans every sub-agent ever started, twice a frame

- `crates/cockpit-ui/src/chat/subagents.rs:47` and `:70`.
- Every chat frame filters `view.subagents()` (all of the thread's sub-agents, never trimmed) to count the ones shown. When any are shown, it filters the whole list again.
- In a long session with many sub-agents, that is hundreds of entries checked per frame, per chat, mostly to find zero.
- Fix: the store already keeps `working`. Also keep a short list of those whose end was seen recently, and drop them as their `subagent_ended_s` passes. Then the strip touches only what it shows.

## 9. A sub-agent shown alone rebuilds its rows whenever anything in the thread changes

- `crates/cockpit-ui/src/chat.rs:347-371`, `crates/cockpit-ui/src/store.rs:106-111`.
- The shape key's facts come from `rows::shape_facts(view.stream())` over the whole thread. Any new item anywhere, such as the main agent's or another sub-agent's, changes the facts.
- Then `subagent_steps` walks from the sub-agent's item and runs `descends` on each item (a walk up the parent chain through `item_at` lookups). The shape is rebuilt, a new `Rc` is made, and that forces a full `lay_out_rows` (see 2).
- With the pop-up open on a sub-agent while the main agent works, every event in the thread rebuilds the sub-agent's rows and positions, even when none of its own items changed.
- Fix: compute facts only over the sub-agent's span. Or keep, per sub-agent, a count of its items that changes when one of its steps arrives (the store already finds the owner in `step_of_subagent`).

## 10. Every sub-agent state change sends its whole prompt and answer, and copies them on the way

- Server: `crates/harness-acp/src/state/subagents.rs:222-239` (`change_subagent`).
- Each change, such as `running` to `requires_action` to `running` to `idle`, a title, or a cancel flag, clones the whole `Subagent`. It emits it as `SubagentUpdated`, and the hub caches another clone (`crates/harness-core/src/hub.rs:113`). Every connected client then gets the full `prompt` and `answer` text again.
- `session_message` sets `prompt` or `answer` to a new whole text each time, which is right, but every later state change resends both.
- Cost: a few KB to tens of KB per state change, per sub-agent, per client. That is multiplied by many sub-agents in a busy turn, and by the home relay, which parses and republishes each one.
- Fix: send the long texts only when they change. One way is to split `SubagentUpdated` into state and text. Another is to send the texts once and have the cockpit keep them, since the item already holds the title.
- Smaller: `crates/cockpit-ui/src/store.rs:153` clones a tool call's title into `Seen.doing` on every update of a sub-agent's step, including output chunks, even when the title is unchanged.
- `cancellable` (`state/subagents.rs:59`) is a linear scan of `children`. That is fine at today's sizes; it is noted only because it runs under the state lock.

## 11. Small per-frame repeats

- `crates/cockpit-ui/src/status.rs` `pill`: each chip is laid out twice per frame, once with `layout_no_wrap(text.to_owned())` to size it and once more by its `Label`. Every status chip now goes through `pill`: the panels' top lines, the sub-agent strip and the request cards. egui caches the galleys, but each frame still pays an allocation and a hash of the text twice.
- `crates/cockpit-ui/src/app/draw/bars.rs:107-121` does the same in the key bar: `format!` the words, `layout_no_wrap` to measure, then the button lays them out again.
- `crates/cockpit-ui/src/agents.rs` `Agents::state` returns `CantRun(String)` by cloning the error or reason. It is called several times a frame: the top bar, the chat banner, `live.rs:38` and `:65`, the slot picker's choice per slot (`chat/actions.rs:138`), `attention::detail` per notification row, and a discussion's status. Each call clones when an agent can't run.
- `crates/cockpit-ui/src/chat.rs:346` `shape()` clones the thread id and now also the `alone` id to build its cache key, on each of its 16 call sites per frame.

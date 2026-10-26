Performance review of c7f8a98..d537e31. Ranked, most severe first.

**1. A discussion or tag on a whole chat session saves a full, uncapped copy of the session, and each new note adds another copy.** (src/chats.rs:226-238, crates/nah-things/src/lib.rs:463)
- `ChatSession` has `lasts() == false` and `detailed() == true`. So `Things::note` → `log()` → `owner.now()` writes the whole session into `.nah/state` logs, including every tool call's input and output as detail.
- `follow()` returns false for a session, so the `grown_most` cap (64 MB) is never applied. Nothing is shared between notes: N discussions or tags on `chat lead` make N full copies.
- Building the text costs O(turns × items): every turn filters every item. It starts with `hub.thread_state`, which walks and clones the entire hub cache while holding the lock every `publish` needs.
- Trigger: a lead session with 300 turns and 15k items (tens of MB of tool output). Every `discuss on="chat lead"` adds tens of MB to the small shared disk and stalls event publishing while it's built.

**2. The review now re-downloads the whole plan on every `ArchEvent::Changed`, including code-only changes.** (cockpit-ui/src/app/lifecycle.rs:133, review.rs `tick`)
- Before, the plan tree refetched only when `plans.contains(p)`. Now `invalidate()` → `arch_plan` + `arch_review_diff` on any change, e.g. each file an agent saves.
- Each reply bumps `generation`, which sets off:
  - `PlanTree::share` clones the whole `Plan` and every discussion on it.
  - `rows_of` (tree/plan.rs:64) rebuilds every row. It is O(parts × discussions): a 200-component plan has about 2,300 parts, × 300 discussions ≈ 700k `Subject` comparisons.
  - The spec view and graph take the review again.
- review.rs:61 also bumps `generation` on any change to the desk's discussions, even one on a terminal line or code. So every agent reply anywhere rebuilds the plan tree.

**3. The discussion buffer redraws everything every frame.** (discussion.rs:184-198, 85)
- Every message is drawn with `CommonMarkViewer` each frame inside a plain `ScrollArea::show`, with no virtualization. egui_commonmark re-parses on every `show`. 50 agent answers of 3 KB means about 150 KB of markdown parsed per frame.
- `shown()` clones the whole discussion, all text included. It's called from `render`, `tick`, `actions`, `title`, `status`, `things`, `takes_typed` and `target`: about 8 full clones per frame.
- The spec view's `render_thread` now draws markdown per frame too, and `draw_item` clones the item's discussions every frame.

**4. The panel's top line clones discussions per frame, and the new whole-file thing makes it worse.** (app/notes.rs:109-118, through `notes_named` from draw/host.rs:31)
- For each panel, each frame, `all.on()` scans and sorts every discussion for each thing at the cursor. Each match is `d.clone()`d whole.
- The code view now adds a `CodeFile` thing, which `touches` every line note in the file. Every discussion anywhere in the open file is cloned every frame.
- `draw_notes` (notes.rs:289-295) rebuilds every speech-bubble mark's hover string each frame.

**5. Graph `drawn_at` scans all cards for each note.** (graph/select.rs:609)
- `on_screen()` filters all `tree.cards` into a new Vec on every call.
- `draw_notes` calls it for every `Component` tag thing and discussion, each frame. 300 noted components × 2,000 cards is about 600k rect transforms per frame, and the graph repaints continuously while it animates.

**6. `discussion_read` reads the whole thing twice to show 40 lines.** (src/mcp/sets/discussions.rs:188, 198)
- `things.said(.., None)` rebuilds the thing in full: the O(T×I) session text from #1, or a terminal command's whole output.
- `things.said(.., Some(kept))` reads the whole kept log (up to 64 MB) into memory.
- `some_of` then keeps 40 lines. `Things::glimpse` / `lines` already read only the start.

**7. `tell_the_asked` works through events one at a time, waiting on each.** (src/discussing.rs:83)
- `slots.tell(..).await` (`expanded` + `start_turn`) runs inside the loop that drains the broadcast receiver. When no session runs, `hold_report` is a synchronous SQLite write on the async runtime.
- If the receiver lags, the news in between is never told to the slot (the code says so).
- Unconfirmed whether this happens in practice. Measuring `start_turn` latency under a streaming session would confirm.

**8. Lag recovery in Chats copies the whole hub replay under the hub lock.** (src/chats.rs:90)
- `hub.subscribe().0` clones every cached event, every item of every live thread, just to read the `TurnUpdated`s.
- Lag happens exactly when a stream is busy, and the copy blocks every `publish` while it runs.

**9. The `discussions` tool's access check clones every slot per discussion.** (src/mcp/sets/reach.rs:39, discussions.rs:152)
- `reaches()` clones every slot with its sessions (`slots.list()`) for each chat-subject discussion inside `retain`.
- `list()` clones every discussion with all its text, only to print one line each, capped at 100.
- Low severity.

**10. The discussion buffer re-asks the server what its thing says on every `ArchEvent::Changed`.** (discussion.rs:247-250)
- This happens for each open discussion buffer, on any change in the project.
- For a thing that is never copied to the logs (a file, a plan part), the server's `on_now` re-reads it whole each time, e.g. a 10,000-line file per agent save.
- Low severity.

**Network:** every reply re-sends the whole discussion (`DiscussionUpdated`, kept whole in the hub's replay). That's your decision on finding #2. The cost: a 100-message discussion goes out in full to every client on each message.

**No problems found:**
- nah-notify `discussion_news`: a map lookup per event.
- Held reports per slot in harness-core.
- The `numbers` map in chats.rs: bounded by open threads, pruned on `ThreadClosed`.
- `Things::pass` caching.

**Phone and touch (performance only):** the per-frame markdown parsing and discussion clones (#3, #4) cost most in wasm on a phone. Nothing else in this area depends on width.

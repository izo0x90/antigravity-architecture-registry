Performance review of 772aa54..0e24374, ranked with the most severe first. I read the code only and did not build or run anything.

1. **A log search reads every log, not just the ones asked for.** `crates/nah-things/src/logs.rs:335` (`Logs::search`)
   - It greps the whole `tagged/` folder with no limit on matches per file or overall (`per_file: usize::MAX>>1`, `max_file_bytes: u64::MAX`, page `usize::MAX>>1`). Every matching line is turned into a `String`, and only then is the list filtered down to `files`.
   - Trigger: `tag_search error` scoped to one tag, while another tag holds a 64 MB running-build log. Every "error" line in every log is read into memory.
   - The 2 s budget can run out on logs nobody asked about. The result then comes back marked partial and can miss lines from the files that were asked for.
   - The search holds `self.search`'s mutex for the whole time. `touch` (line 270) takes that mutex on every log `write` and `remove`. So a discussion reply, or noting a terminal command, can stall for up to 2 s behind a search.
   - Severity: high on a project with busy terminals.

2. **The panel top line compares every note against every thing at the cursor, every frame.** `cockpit-ui/src/app/notes.rs:110-143` (`notes_here`, called from `notes_named` in `app/draw/host.rs:31`)
   - It runs once per panel per frame. For each thing at the cursor it calls `Discussions::on` over all discussions, plus `Tags::on`.
   - Each `Subject::touches` (`harness-contracts/src/tags.rs:261`) allocates two `Vec`s and clones the `PlanId` and `ComponentId` for plan subjects.
   - The fallback `contains` → `same_thing` → `bare()` (`tags.rs:306/332`) clones both subjects, even when their kinds differ.
   - Cost: 500 discussions × 3 things × 3 panels is about 10k subject clones (plus strings) per frame. It repaints continuously while a terminal or chat streams.
   - Severity: medium-high; it grows with the total number of notes, not with what's on screen.

3. **A note on a whole chat session makes the chat gutter allocate heavily every frame.** `cockpit-ui/src/chat/things.rs:89-91` (`tag_drawn_at`)
   - For a `ChatSession` subject (new in this change, finding #11), `tagged_indexes` returns every item in the stream.
   - Then, for each visible spot, it runs `indexes.iter().any(|i| s.key == i.to_string())`. Link spots (`"12:3"`) never match, so they always walk the full list.
   - Cost: 40 spots × 3,000 items is about 120k `String` allocations per frame per note on the session. `draw_notes` calls this for every tag thing and every discussion.
   - Severity: high on a long chat while the agent streams.

4. **The discussion buffer copies the whole discussion several times per frame and draws every message.** `cockpit-ui/src/discussion.rs`
   - `shown()` deep-clones the whole discussion (all message text) in `render` (175), `things` (370; called each frame via `notes_here`) and `actions` (412). That's 3 or more full copies per frame.
   - `render` draws every message through `CommonMarkViewer`, which re-parses the markdown each frame. Nothing is virtualised; the content is in a plain `ScrollArea::show`.
   - `spec.rs:280-290` (`draw_item`) does the same: it clones the item's discussions every frame and draws them in full.
   - Cost: a 100-message discussion with long agent answers is hundreds of KB copied and parsed per frame.
   - Severity: medium.

5. **`draw_notes` asks every view where every discussion is drawn, every frame.** `app/notes.rs:295`
   - It loops over all discussions, not only those in this view, and calls `drawn_at` for each.
   - In the tree view (`tree/mod.rs:545`), each call runs `Target::of_subject` plus a `format!` in `key_of`, then scans the spots.
   - In the spec view (`spec.rs:1343` `drawn_rect`), each discussion on the shown card rebuilds `all_entries()`. That's `comments_in` per section over all discussions, plus a Debug-formatted key string per entry. It then runs `keys.contains` for each spot.
   - Cost: 30 discussions on one card, with replies, comes to O(D²) work and about 10k strings per frame.
   - Severity: medium.

6. **Each discussion message copies too much and holds a lock across the database write.** `nah-discussions/src/lib.rs:381-418`, `329-334`
   - Each reply clones `Discussion` and `Stored` whole, then clones the discussion twice more in `keep`.
   - `store.put` (a SQLite transaction plus an activity insert) runs while the `held` mutex is held. Every reader waits on it: the routes, `Reach::reaches`, approvals, `refresh`.
   - MCP `discussions` (`src/mcp/sets/discussions.rs:137`) clones every discussion with its full text on each call, just to list at most 100 lines.
   - `Reach::reaches` on a discussion subject clones it again (`reach.rs:41`).
   - `Discussions::find` (lib.rs:307) clones everything only to get the order.
   - Severity: medium once there are hundreds of discussions with long answers.

7. **Taking a tag off, or deleting a tag or discussion, re-reads every note from the database.** `nah-things/src/lib.rs:470-486` (`let_go`)
   - It calls `kept_files()` on each kind of note. That is a `SELECT json` of every tag row and every discussion row plus a full JSON parse (`nah-state/src/discussions.rs:37`, `tags.rs`).
   - Then `tell_noted` collects every subject and hands it to every owner.
   - Cost: O(all notes) database and parse work per untag.
   - Severity: low-medium.

8. **`Things::find` opens files once per found line per thing.** `nah-things/src/lib.rs:496-503`
   - For every found line it loops over all `among` items. On a file match it calls `logs.span()`, which opens the file and reads its header, then `count()`, which opens it again.
   - Cost: O(found lines × things), with 2 file opens per match. 15k matches means about 30k opens.
   - Severity: low-medium.

9. **Any architecture change makes every open discussion buffer re-read what it's on.** `discussion.rs:265-273`
   - Any `ArchEvent::Changed` (every ingest, any file, any plan) resets `asked` when the discussion is on code or a plan, so `on_now` is fetched again.
   - On the server, `PlanParts::plan` (`src/plan_things.rs:23`) uses `arch.plan()`, which deep-clones the whole plan; `plan_shared` would avoid that. `on_now` → `glimpse` clones it twice (`there` + `now`).
   - The "findings" section runs a full `validate`, and `changed` runs a full `review::diff`.
   - For chat subjects, `there` and `now` each call `hub.thread_state`, which scans the whole hub cache and clones the session's events twice.
   - Severity: low-medium; it repeats on every save while an agent edits.

10. **Growth.** Discussions are never dropped from the hub cache or the desk. Every new client gets all of them, with full text, on connect. `Noted::refresh` (`notes.rs:191`) clones all of them on every `DiscussionUpdated`, including the ones `refresh()` sends for renames. This is tied to the open "deleting discussions" decision and to decision #2. I'm noting it only because the per-update clone of everything happens inside the cockpit, not on the wire.

**Areas with nothing real:**
- Notification rules: nothing.
- `tell_the_asked`: nothing beyond a full `list()` on lag.
- Route threading: the discussion, terminal and MCP routes all use `spawn_blocking`.
- Logs bookkeeping maps (`counted`, `growing`, `touched`): bounded by the number of files.
- Rows in the plan tree are rebuilt only when the plan's discussions change. `rows_of` is O(parts × discussions) on each change; acceptable.

**What would confirm the uncertain ones:**
- #2, #3, #5: a frame-time trace (`performance_start_trace`) on a chat with 3k items, one discussion on the whole session, and 300 discussions on the desk.
- #1: a `tag_search` on one tag while another tag holds a 64 MB log, timing a concurrent discussion reply.

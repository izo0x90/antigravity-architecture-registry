# Stopping point 4: performance reviewer

I found 9 performance problems in b29fbfc..HEAD, most severe first. I only read the code and ran nothing.

**1. The chat view does O(n²) deep copies every frame. It freezes the cockpit on long sessions.**
- **Where:** `crates/cockpit-ui/src/chat/things.rs:26,37,49-54,75`.
- **Why:** `stream()` (`chat/render.rs:179`) deep-clones every `TurnItem` in the thread, tool outputs included. `tag_place(i)` calls it once per index.
- **Gutter path:** `tagged_indexes` runs `tag_place` for every index, so one subject costs n full-thread clones. `draw_tag_gutter` (`app/draw/host.rs:92`) calls `drawn_at` → `tag_drawn_at` → `tagged_indexes` for every tagged chat subject, every frame. Subjects from other sessions pay the full cost too, because the session is only compared after `tag_place` has run.
- **Cursor path:** `tag_things`' `opening` runs `tag_place_of` on each item up to the turn's first message, at two full clones per item. It is reached through `at_cursor`, which runs at least twice per frame per chat panel (`tags_here_named` at `host.rs:39`, and `tag_actions` via `gather`/`shown_at`). This happens even when there are no tags.
- **Cost:** 1,000 items with the cursor in a late turn is about 2M `TurnItem` clones per call. Each tagged chat message adds about 1M more per frame. Expect seconds per frame.

**2. A growing tag log has no size limit.**
- **Where:** `nah-tags/src/logs.rs:111` (`grow`) and `terminals.rs:225` (`tell_followed`).
- **Trigger:** tagging a running `yes`, `tail -f`, a dev server or `cargo watch`. Everything it prints is appended to its own file until the command ends or the tag comes off. `FILE_MOST` only applies to the shared file.
- **Cost:** on the small shared disk, this fills it.

**3. Every read of a log extent reads all of it.**
- `Logs::head` (`logs.rs:139`) only needs the first 16 KB but scans the whole extent to count lines, for each thing, on every `Tags::read` (each Tags-view rest, each `tag_read`).
- It also runs on every prompt that names a `#tag` (`expand`, `lib.rs:548`), which never uses the line count.
- `tag_read` with `thing`/`lines` (`mcp/sets/tags.rs`) loads the whole log into a String, folds it (one String per line) and collects every line, only to show a 2,000-line window.
- **Cost:** paging through a 50,000-line command means 25 full reads. Combined with #2, a single prompt or page can read gigabytes or run out of memory.

**4. Working out where a chat thing is held copies the whole session, under the event hub's lock.**
- **Where:** `Tags::held` (`lib.rs:250`) calls `there()` on every owner. `Chats` doesn't override it, so `now()` runs: `slots.list()` plus `hub.thread_state()`.
- **Why:** `thread_state` scans every entry in the hub cache and clones the session's events while holding the cache lock, which `publish` also takes. Then `turn_text` is built, only to produce a boolean.
- **Cost:** a `tag_read` or prompt on a tag with 50 chat things in a 2,000-item session makes 50 full scans and clones. While each one runs, every agent's event streaming waits.

**5. Code tags call git under the tags mutex on every `ArchEvent::Changed`.**
- `startup.rs:254` calls `refresh()` on every `ArchEvent::Changed`, including plan edits and approvals.
- For each `CodeLines`, `moved` and `name` each call `now_at`: `git2::Repository::open` plus a stat-keyed diff (`code_tags.rs`).
- For each range that moved, `snapshot()` runs `index.add_all("*")` and `write_tree` over the whole worktree (`nah-changes/timeline.rs:39`). With 20 tagged ranges and an edit at the top of the file, that is 20 full worktree scans while holding the tags lock.
- `tag()` and `untag()` call `current()` (the same `Code::moved`) on every existing thing in the tag. Adding to a 500-range tag means 500 repo opens.
- **Growth:** each new snapshot is a ref under `refs/nah/snapshots` that nothing prunes; the timeline's `prune` only drops its own. Refs and git objects pile up with every save that moves tagged lines, and `snapshots()` lists all of them on each call.

**6. Every change resends the whole tag (A.3 was not built).**
- `keep` (`lib.rs:568`) publishes `TagUpdated { tag }` with the full tag on every tag, untag, edit and refresh. `refresh` also writes the whole tag to the database.
- **Cost:** with 500 things, each change sends roughly 100 KB to every cockpit through home. The cockpit bumps its version, and `TagList::sync` rebuilds every row and its `serde_json` key.

**7. The cockpit clones whole tags every frame.**
- `at_cursor` (`app/tagging.rs:43,53`) clones each tag on the cursor's things, including its full `on` list. It runs at least 2× per panel per frame.
- `draw_tag_gutter` walks every tag × every thing, calls `drawn_at`, formats the hover text and does an O(bars²) `find` per bar, every frame.
- This is moderate for terminal and code; the chat case is #1.

**8. The Tags view lays out every row every frame and does an O(n²) lookup.**
- `tag_list.rs:200` runs `tag.on.iter().find` for each row, so an unfolded 500-thing tag costs 250k `Subject` comparisons per frame.
- Every row (4 labels each) is laid out every frame with no virtualization, about 2,000 labels.

**9. Smaller server-side costs:**
- **Terminal follow:** `tell_followed` takes one global `followed` mutex on every PTY chunk of every terminal once any follow exists. It clones `open.info()` and holds the lock across an unbuffered `write_all`. Busy terminals queue behind each other.
- **Tagging a finished command:** `tag()` holds the tags mutex while `owner.now` builds the command's whole output (`command_output(u64::MAX)`) and writes it. Now that scrollback is a setting, a large `scrollback_lines` makes this string as large as the setting allows.

I found nothing real in the palette changes (detail added to the ranking haystack is negligible), the code view's or tree's `drawn_at`/`things`, or the migrations.

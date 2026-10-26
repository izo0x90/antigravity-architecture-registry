## Performance review, 53cd6b5..c7f8a98 (most severe first)

1. **A chat turn is looked up again, in full, for every note on a chat turn, on every move (High).** The new `Chats::moved` (src/chats.rs:266) and `there` (:248) go through `turn()` (:106), then `running()` (:91). That does `slots.list()` (clones every slot), then `hub.thread_state()`, which scans every entry in the hub cache and clones the whole session's turns and items, tool output included. `Things::moved()` calls `refresh()` on tags and on discussions, so each chat-turn note pays this once per move. Moves come from every code ingest (each file save), every terminal width change, every slot close and every turn renumbering. Example: 20 notes on a session of 5,000 items holding 20 MB of tool output means about 400 MB copied per file save. It also runs on the `when_moved` listener thread (:52), the thread that must keep draining the broadcast. While it is busy it lags, and `Lagged` falls into `_ => {}`, so renumberings are dropped silently and notes stay on the wrong turn.

2. **Every move redoes git diffs and rewrites discussions (Medium).** `Discussions::refresh` (nah-discussions/src/lib.rs:190) calls `things.renamed` for every discussion. For a note on code lines that runs `Code::moved`, then `now_at`: two canonicalizes, an ignore check and a git diff of the file against its snapshot. Tags run the same pass separately, and nothing is cached between the two, so a thing with both a tag and a discussion is diffed twice. With 200 notes on code that is about 400 diffs per save. Each discussion that moved also gets its own SQLite transaction (`store.put`) and the whole discussion is broadcast through home to every cockpit. A terminal reflows all its lines on a width change, so every discussion on that terminal is rewritten and resent at each width step.

3. **The discussions lock is held across SQLite writes, and async handlers take it (Medium-low).** `keep` (:309), `refresh` (:200), `delete` (:536) and `bring` (:508, one transaction per comment brought in) all hold the `held` std mutex during `store.put`/`remove`, which commit with fsync. The `list`, `on` and `read` routes (src/routes/discussions.rs) lock the same mutex directly on runtime threads, not through `spawn_blocking`. So a write stalls a tokio worker. `list` also clones every discussion with all its text.

4. **`Chats::follow` now copies the full hub replay before knowing it needs it (Medium).** The diff moved `self.hub.subscribe()` (src/chats.rs:286) ahead of the check that the turn is still running. `subscribe()` builds `cache.replay()`, a clone of every cached event (all open threads' items, every discussion whole, every tag), and the result is thrown away. Every tag or discussion started on a finished chat turn (the common case) pays this for nothing.

5. **Tagging runs up to 500 diffs while holding the tags lock (Medium-low).** `same_now` (nah-tags/src/lib.rs:305, used at :316, :325, :357) calls `things.current()`, i.e. `owner.moved()`, for every thing on the tag at the same path or terminal. That is a git diff (or a chat-turn lookup as in #1) per thing, up to `MOST_ON` = 500. It runs twice per `tag()` call, and the second pass holds the `tags` mutex. Trigger: a "todo" tag on 100 places in one large file, tagged once more, does about 200 diffs of that file.

6. **An open item card re-reads the whole plan on every architecture change (Low-medium).** `invalidate` (cockpit-ui/src/spec.rs:1701) sets `card.asked = false` on any `ArchEvent::Changed`, including code ingests that have nothing to do with the plan. While an item card is open, each file save makes a full GET of the plan JSON (hundreds of components). `PlanTree::on_event` checks the plan id first; the item card should do the same.

7. **Growth without a bound (Low).**
   - Every message is its own file (`said-N.log`). `Discussions::open` reads every message file at start-up, and the hub cache holds a second full copy of every discussion, settled ones included, for the server's whole life.
   - `Logs.touched` gains a path for every message written and is only emptied by `search`. Nothing calls `Discussions::find` yet, so it grows forever.
   - The `numbers: HashMap<TurnId, u32>` in `Chats::when_moved` keeps every turn ever seen and is never pruned, not even on `SlotClosed`.

8. **Plan files are parsed twice, and failed comment moves are retried on every plan change (Low).**
   - `FsPlans::read` (arch-service/src/store.rs:187) parses each plan file twice: once as a `Value`, to find `comments`, and once as `Plan`. `save` re-parses and pretty-prints again while comments are waiting.
   - `move_waiting` (:201) runs on every `changed()` call (:313) for every waiting plan, not just the ones that changed. If a plan keeps failing inside `bring`, every plan-file change anywhere burns discussion numbers and writes and deletes message files again.

9. **Every reply re-sends the whole discussion, as you decided (accepted).** Each reply to a discussion with M messages copies O(M) text about four times on the server (`add` clone, `held` insert, publish, hub cache) and sends all of it to each cockpit.

**Cockpit per frame:** nothing new. `ReviewState::tick` rebuilds only when the discussions' version changes, `Discussions::on` isn't called per frame, and the new tree and item-card code allocates only trivially per frame.

**Not performance:** `tree/plan.rs` `rows_of` doesn't show each row's discussions' state, which C.3 asks for.

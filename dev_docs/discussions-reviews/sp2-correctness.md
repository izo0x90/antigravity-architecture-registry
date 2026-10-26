## Correctness review: 53cd6b5..c7f8a98 (discussions, steps 3–5)

Ranked, most severe first. I made no edits and ran no builds.

### 1. Plan comments get imported again, making duplicate discussions (High)
**Where:** `crates/arch-service/src/store.rs` (`FsPlans::changed` and `move_waiting`). `Discussions::bring` has no duplicate check.

**The cause:** `changed()` re-reads `comments` from any plan file it reads, and every such read imports them again.

**Sequences that go wrong:**
- **Git puts the old file back.**
  1. The server rewrites `plans/x.json` without its comments. The working tree is now dirty.
  2. The user runs `git checkout .`, `git stash`, `git restore`, switches branch or pulls.
  3. The file has `comments` again. The watcher calls `refresh`, then `changed`, then `move_waiting`, and each comment becomes a new discussion (d7, d8…).
  4. The new copies are open or answered again, so they block approvals. It doesn't help that the user had settled the originals.
- **A plan file that is JSON but not a valid plan** (a copy with a mismatched `id`, or `schema_version: 2`).
  1. `move_waiting` moves the comments, then removes `waiting`.
  2. It only rewrites the file `if let Some(Ok(plan))`, so the file keeps its comments.
  3. Every restart imports them again, and so does every save while the user hand-fixes the file.
- **Crash windows.**
  - `bring` puts each discussion in its own transaction, and the file is rewritten only after all of them. A crash in between means everything is imported again at the next start.
  - If `write_atomic` fails after `held.waiting.remove`, the comments stay in the file and come back at restart. That error also goes up through `changed()?`, so `ArchitectureService::refresh` stops before `files_changed`, and that batch's code changes are never processed.

### 2. At this commit nobody can answer a change request (High while this build runs)
**Where:** `answer_comment` and `reply_comment` were removed (`src/mcp/sets/review.rs`); the replacement agent tools are step 6. The cockpit only calls reply, reopen and settle (`spec.rs` 672–686): no answer, no delete.

**What goes wrong:** `settle` refuses an open change request (`lib.rs:432`). So any open change request blocks its component's approval permanently:
- one moved over from a plan file, or
- a new one the user starts with `C` in the spec view.

The only way out is calling the HTTP API by hand.

### 3. The check that a change request's answer names a real change is weaker than before (Medium; breaks rule 5)
**Where:** `lib.rs:419` together with `PlanParts::changed` (`src/plan_things.rs`).

**Plan parts:** for `Subject::Plan`, `changed` returns `!diff.is_empty()`, and a plan `touches` everything in it.
1. There is a change request on component `a`.
2. The agent edits only component `b`.
3. It answers naming `plan p`. The answer is accepted.

The old code required a change in the diff touching the anchor's component, so a plan-level change never satisfied a component's change request.

**Everything else** (code lines, terminal output, chat turns, plan items): `changed()` is `None`, so naming the thing itself is accepted with nothing changed.

### 4. Comments that fail to move are invisible and block nothing (Medium)
**Where:** `move_waiting` logs a warning and `continue`s.

**Triggers:** a state database error (locked, disk full) during `bring`, or one comment that won't parse (`move_into` is all-or-nothing).

**What goes wrong:**
- Those comments stay only in `held.waiting` and in the file.
- The blocker asks only the engine, so the component can be approved even though its plan file holds open change requests.
- The cockpit doesn't show them.
- They're retried only on the next change to that file or the next restart.

Before this change they blocked approval (or the plan loaded as corrupt, which was at least loud).

### 5. A stale update can overwrite a newer one, in clients and in the hub's cache (Low–Medium)
**Where:** `Discussions::refresh` (`lib.rs` 197–206) builds the new discussion under the lock but publishes after `drop(held)`.

**Sequence:**
1. Terminal lines move, and `refresh` clones d1 as it stands.
2. It drops the lock.
3. A reply lands through `add` and publishes d1 with the reply.
4. `refresh` then publishes its older d1, without the reply.

The cockpit, and late clients replaying the cache, show d1 without the reply until its next change. `keep` publishes under the lock; `refresh` should too.

### 6. Race between the watcher and moving comments (Low)
**Where:** `FsPlans::changed` reads the file with no lock held, then sets `waiting`. `move_waiting` isn't serialized.

**Sequence:**
1. The watcher reads the old file, which still has comments.
2. The start-up `move_comments` call moves them and rewrites the file.
3. The watcher sets `waiting` from its old read and calls `move_waiting`, importing everything twice.

This needs an edit to the plan file while the server is starting.

### 7. C.3 isn't built, though step 3 is ticked (Low; faithfulness)
**Where:** `crates/cockpit-ui/src/tree/plan.rs` (`rows_of`).

Rows show an item's state only:
- no discussion state per row (waiting on you, on the agent, settled);
- no discussions as rows under the item.

G.6 might be meant to cover this, but the step-3 check should say so.

### 8. Whose move it is can't be "the user's" on an agent's question (Low; plan inconsistency)
**Where:** `whose_move` (`harness-contracts/src/discussions.rs`). An open discussion is always `Agent(asked)`, and `asked` defaults to `lead`.

**Effect:** when an agent starts a question meant for the user, it shows as waiting on the lead. G.5's inbox item "a question asked of you" can't be worked out. This matches D.1 literally but conflicts with G.5; decide before step 8.

### Checked; nothing real found
- **Engine:** `add`, `start`, `delete` and `ask_for_change`. Checks are re-run under the lock, written files are cleaned up on failure, and a start that fails is given no number. Deleted numbers aren't reused, because AUTOINCREMENT keeps them.
- **Routes:** only the cookie's user or Home reach them, and `by` is hard-coded as the user. Agents can't settle. The slot name "user" is reserved.
- **Cleanup at start-up:** forgetting unused things' logs runs after discussions register. Message files are forgotten only when every stored discussion can be read.
- **Hub cache:** it drops removed discussions; the desk clears on `Resync`.
- **Approval blocker:** no deadlock with the plan lock. It still blocks every component for a discussion on the whole plan, as before.
- **Items:** `move_item` rejects cycles, ids aren't reused, and a review plan rejects component operations.

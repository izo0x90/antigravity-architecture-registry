Faithfulness review of 53cd6b5..c7f8a98 (steps 3–5: Parts C, D, E). Read-only; no builds run.

**Ranked problems (most severe first)**

1. **Old comments can be moved into discussions again and again, creating duplicates.** `crates/arch-service/src/store.rs:194, 201–216`. `read()` pulls `comments` out of the raw JSON even when the file isn't a valid plan. Example: a user copies `p1.json` to `p2.json` to branch a plan, and the file then "holds plan 'p1', not 'p2'".
   - `move_waiting` moves those comments into discussions and removes them from `waiting` (l.211).
   - It only rewrites the file when `held.plans` holds `Ok` (l.212), so here the file keeps its comments.
   - Every restart, and every watcher event on that file, moves them in again: d1, d2, d3… all copies of the same comment.
   - The same happens if `write_atomic` fails after the mover succeeded.
   - Two `move_waiting` runs at once (startup `move_comments` and a watcher `refresh`) both take a snapshot of `waiting` before either removes it, so both move the same comments.

   This breaks E.1 ("move into the engine once"). Severity: high. The duplicates are open discussions that hold approvals.

2. **Part of C.3 was deferred without asking.** C.3 says "Each row shows its state and its discussions' state (waiting on you, on the agent, settled)… its discussions are rows under it". `crates/cockpit-ui/src/tree/plan.rs:33–41` (`rows_of`) shows item and component rows only; no row shows a discussion state and no discussion appears as a row. The findings file moves it to "step 7 (G.6)" under "Checked against the plan", yet step 3 is ticked. The plan's Process says anything not built as written "comes to the user as a question… marked Decide". Severity: medium. A review plan can't yet be gone through item by item with its discussions, which is the plan's stated purpose.

3. **D.4 says today's rules "carry over", but three changes were recorded as "Built this way" instead of being raised as Decide:**
   - **A question can be settled with no answer** (`crates/nah-discussions/src/lib.rs:428–435`). The old `accept_comment` refused anything not answered.
   - **"Make it a change request" reopens without saying why** (`lib.rs:448–462`): settled → open, with no message. D.1 says "reopened is open again with why". Sequence: settle d1, then `POST /api/discussions/d1/change`; d1 is open with no reason given.
   - **"Names what changed" is checked only where the owner can say** (`lib.rs:408`). `Things::changed` returns `None` for code lines, terminal output, chat turns and plan items (`src/plan_things.rs:183`; items aren't in the diff). A change request on lines or on a review item can be answered by naming the same thing, unchanged, and then settled. Rule 5 holds only for a plan's components, sections and relations.

   Severity: medium.

4. **Rule 2 ("tags and discussions know no owner") is broken.** `nah-discussions` depends on `arch-graph` (`crates/nah-discussions/Cargo.toml:12`). `Discussions::blocker(plan, component)` (`lib.rs:550–562`) builds `Subject::PlanComponent` and writes plan-specific wording, and `answer` says "since it was last reviewed" (`lib.rs:409`). So the engine carries plan logic. The plan has plans ask the engine; the engine shouldn't know what a component is. Severity: low–medium (structure, not behaviour).

5. **The "Rules (each checked by a test or a lint)" aren't checked.** Nothing in `tests/clean_cut.rs` or `lint_test.rs` asserts that `CommentAnchor`/`Comment` are gone (Rule 1), that no other crate keeps discussions (Rule 3), or that the old `review_comments`, `answer_comment` and `reply_comment` are gone (the existing old-MCP-surface test wasn't extended). Severity: low now; nothing stops them coming back.

6. **Things built that the plan didn't ask for:**
   - **Deleting a discussion** (`src/routes/discussions.rs:128`, `lib.rs:532`). It appears nowhere in D. Deleting an open discussion is a second way to lift an approval hold, besides settling.
   - **A user answer route** (`routes/discussions.rs:112`, `by = USER`). The plan has agents answer. The user can answer their own change request and then settle it.
   - Neither was raised as Decide.

   Severity: low.

7. **E.1 says "nothing reads the old shape after", but a permanent reader stays.** `src/plan_comments.rs` and the `waiting` handling in `store.rs` keep reading the old `comments` shape on every plan read, for good. This was recorded as "Built this way", not Decide. Defensible under "as each plan is read", but it isn't what E.1 says. Severity: low.

8. **Approvals are unblocked if the check is missing.** `crates/arch-service/src/service.rs:891`: if `approvals_ask` was never set, or the `Weak` can't be upgraded (`src/startup.rs:334`), `approve` goes through. The server sets it before serving, so this is latent. Severity: low.

9. **Who answers isn't enforced, and "asked of you" can't be represented.**
   - The plan decided "the lead, unless you pick another agent slot". `answer` accepts any `by` (`lib.rs:403`), and `asked` isn't checked against real slots (`lib.rs:321`): `"asked":"leed"` makes a discussion no one answers.
   - `whose_move` makes every open discussion the agent's, so an agent's question to the user has no way to say it's waiting on the user. G.5's inbox ("a question asked of you") will need a change to the engine.

   Severity: low now; it blocks step 8.

10. **Between steps 5 and 6, agents have no discussion tools.** The old three are gone and the new five are not there (`src/mcp/sets/review.rs`). Any open change request now holds its component's approval with no way for an agent to answer. The findings file says so. Severity: a temporary state, but real if this commit is used.

**Areas with nothing found**
- C.1 and C.2 are built as planned: items, the four states, the review kind refusing anything but item ops, nesting with a cycle check, and the user setting state through a shell action (`crates/cockpit-ui/src/app/plan_items.rs`).
- D.1–D.3, D.5 and D.6 are built as written. Storage follows "Where things are kept": database rows, text under `state/discussions/`, events, activity with who made each change. Things that don't last are logged on start.
- E.2: `last_review` and the review diff stay the plan's (`service/reviewed.rs`).
- E.3 is tested.
- The `CommentAnchor` field and type are gone from the code.

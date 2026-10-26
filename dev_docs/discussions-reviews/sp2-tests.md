Review of the changed tests in 53cd6b5..c7f8a98, most severe first. I read the code only; I did not run cargo.

1. **The "change made somewhere else" rule passes for the wrong reason.** `crates/nah-discussions/src/lib.rs:737`
   - The test answers with `"plan p component b"` and only checks `is_err()`.
   - At that point `parts.changed` is empty, so `answer()` fails at its "hasn't changed" check. It never reaches the check that the change touches what the discussion is on.
   - If the touches check were deleted, this test would still pass. No other test covers that check; the old `review_loop` test that did ("a change elsewhere doesn't answer it") was deleted.
   - Risk: a change request could be closed by an answer naming an unrelated part, breaking rule 5.
   - Fix: push `"b"` into `changed` first, then assert the error says "name what you changed".

2. **Duplicate discussions when comments are moved over: no test.** `crates/arch-service/src/store.rs:204-215`
   - `move_waiting` calls the mover (the discussions are created), removes the plan from `waiting`, and only then rewrites the file. If the plan is held as `Err` (its file didn't parse) or `write_atomic` fails, the comments stay in the file.
   - The next `changed()` on that file, for example when the user fixes it or on a restart, puts them back in `waiting` and moves them again: d5–d8 copies of d1–d4.
   - Two `changed()` calls at once (the watcher plus `apply`) can also both copy the same `waiting` and move it twice.
   - For a user this means duplicated discussions, each one holding up approvals.
   - Needed test: a plan file with comments that fails `Plan::from_json` (or a write that fails), read twice, should produce one set of discussions.

3. **"Its relations" in the approval test name isn't tested.** `lib.rs:752-770`
   - The relation part only asserts `Subject::touches(&relation, …)`, a helper in the contracts crate, and never calls `blocker`.
   - The e2e test's relation comment (c3) is settled, so it only shows that a settled one doesn't block.
   - Nothing shows that an open discussion on a relation blocks both of its ends, which D.4 requires.
   - Fix: start one on `plan w relation …` in `review_e2e_tests.rs` and check that approval is refused.

4. **The cockpit never checks a discussion request it sends.** `crates/cockpit-ui/src/spec.rs:2049`
   - `assert!(s.composer.is_none(), "sent")` checks state that `send_composer` clears regardless of what it sent.
   - No cockpit test checks the body of start, reply, reopen, settle (accept) or change. The server's handlers use `deny_unknown_fields`, so a wrong field name would make every user action fail with a 400, and no test would catch it.
   - Fix: use the app-level `bodies(&app, "POST /api/discussions…")` helper for `C` + typing, reply, accept and reopen.

5. **The plan-tree test doesn't check nesting.** `crates/cockpit-ui/src/app/tests/plans.rs:24`
   - `["item:i1","item:i2","item:i3"]` comes out the same if `parent` is dropped, so "nested" isn't protected. Assert the parent of `item:i2`, or that it is hidden when `item:i1` is closed.
   - C.3 also says each row shows its discussions' state and the discussions appear as rows under it. `rows_of` (`tree/plan.rs`) does neither and no test covers it. This may be meant for step 8; worth confirming.

6. **Discussion search can miss matches.** `crates/nah-things/src/logs.rs:325-336`
   - The tool caps each file at `per_file+1` lines containing the longest word, and only then filters for every word.
   - Example: a message with 6 lines containing "second" and a 7th line "line second", searched with `find("line second", 5)`, finds nothing.
   - No test has more lines with the longest word than the per-file limit.

7. **Vacuous assertion.** `crates/arch-service/tests/review_loop.rs:145-146`: the comment says "A fresh read has nothing to move", but the code only asserts that the plan exists. Cut it, or assert the mover's list is unchanged after opening a new `FsPlans`.

8. **A test that only checks itself, and duplicates another.** `review_loop.rs:150` (`approval_asks_the_discussions…`): the closure returns a string the test wrote, and the test then finds that string. `review_e2e_tests.rs::a_discussion_on_a_plans_part_holds_its_approval…` covers the real engine. Cut the unit test.

9. **Duplicated item tests.**
   - `review_e2e_tests.rs:178` repeats the items/titles check already in `flow.rs::a_review_plan_holds_nested_items…`. Drop the e2e's last assertion and keep its `plan_list` text, the only check of nesting as agents see it.
   - Note that `flow.rs` only moves an item that has no children.

10. **Timing without headroom.** `tests/slots_tests.rs:391`: three polling loops of 50×50 ms (2.5 s each), spread over a server start, fake sessions and a slot closing. Under parallel `cargo test` load this is likely to flake. Use a shared wait helper with a deadline of about 10 s.

11. **Tests of wording or internals:**
    - `audits.rs:29` checks a hint's wording ("pick a plan in the graph").
    - `plans.rs:41` checks `describe() == "i2 FFF skipped (pinned)"`.
    - `bindings.rs:352` hard-codes a cursor key built from `Debug` formatting: `"logic#Comment(DiscussionId(\"d1\"))"`. Build it with `Entry{…}.key()` as `audits.rs` does.

12. **Behaviour the plan relies on that no test protects:**
    - `ask_for_change` (`lib.rs`) doesn't check status. A user can turn a settled discussion into a change request, and it reopens with no "why" message, against D.1 ("reopened is open again with why").
    - The server sending `DiscussionRemoved` on delete (only the cockpit's side of it is tested).
    - The routes `reply`, `reopen`, `change`, `DELETE` and `GET /api/discussions/{id}` (only start, answer, settle, list and on are covered).
    - That a discussion in another plan doesn't block approval.
    - `list()` order when two discussions start in the same millisecond (ties fall back to id order, where "d10" sorts before "d2").

Checked and fine:
- **D.6 lifecycle:** who may do what, lines that move, output gone from the terminal, a restart.
- **Activity record:** `nah-state` order, numbers never reused, discussions a later build wrote are left alone.
- **Moving old comments (E.3):** every anchor and status, read once (normal path only).
- **Date parsing (`millis`):** the old files only ever wrote `Z`-suffixed seconds, so this is fine.

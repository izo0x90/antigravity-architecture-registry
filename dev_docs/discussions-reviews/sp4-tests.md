I reviewed the tests in 772aa54..0e24374. I read code only and ran nothing. Problems are listed from most to least severe.

1. **Discussions surviving a real restart is only half tested.**
   - The engine's restart test (`nah-discussions/src/lib.rs` ~858) runs on `MemoryStore`, which keeps `Stored` as cloned structs. Nothing is ever serialized.
   - The `StateDb` test (`nah-state/src/tests.rs`, `discussions_are_kept_…`) builds every row with `said: Vec::new()` and `from: None`.
   - So no test sends a discussion's messages (`StoredSaid`: `text: Kept`, `changed`, `kind`) or the `from` duplicate key through the real database and back.
   - `StateDb::discussions()` silently drops rows it can't read. A serde mismatch would make every discussion vanish after a restart, and it would also break the "never brought in twice" check for moved plan comments.
   - Fix: extend the real-binary restart test (`terminals_tests.rs`, `Started`) with one answered discussion that has a reply. That one test would cover all of this.

2. **The plan-comments end-to-end test has a check that can't fail, and a unit test locks in a different rule.**
   - `review_e2e_tests.rs` ~368, "Read again: nothing comes in twice": `arch.refresh` re-reads a file that no longer has comments. Separately, `review_loop.rs` (~181) asserts that only a server start moves comments. The duplicate guard is never exercised at server level.
   - Plan E.1 says comments move "as each plan is read". The test makes "only at start" the rule: a plan file checked out later with comments blocks approvals until a restart. If that wasn't agreed with the user, this test protects the wrong behaviour.

3. **Assertions deleted without a matching code change** (A.3 says tag tests move "none rewritten to pass").
   - `cockpit-ui/src/app/tests/terminal.rs` ~433: "A place without a line … its live screen" was removed in 3ecc985. `TerminalBuffer::restore`'s `None` branch (`go_live`) is unchanged and now has no test.
   - `tests/tags.rs`, next/previous test: the "lines' bar: beside all of them" span check for a multi-line `TerminalLines` was dropped. Only a command's span is checked now (new #23 test).

4. **Tests that assert their own setup or reach into internals.**
   - `layout.rs` `typing_in_the_quick_look_stays_there_over_a_chat_or_a_discussion`: it sets `app.input_mode = Insert{Panel}` directly, then asserts that same value ("its box has the keyboard"). The discussion box taking the keyboard is not tested there. `show_target`'s result is discarded with `let _`.
   - `discussing.rs:78` resets `DiscussionView.writing` by hand. After the failed reopen, "Not yet." stays in the box with `purpose == Reopen` even when the discussion is told it's Open. The box then says "Why reopen d1?" for an action that is unavailable, and pressing `r` wipes the kept text. The reset hides this; test what happens instead.
   - `discussing.rs:72` reads `view.writing` / `view.purpose` instead of what's drawn.
   - `plans.rs:441` asserts `describe() == "i2 FFF skipped (pinned)"`, which is the fixture's pin plus wording.
   - `conversation.rs` `who_wrote_something_is_said_one_way` checks a three-entry lookup table.

5. **Tests of wording or layout** (finding #58).
   - `discussions_tests.rs` ~44: `assert_eq!` on the full `discussions` listing text. Many `contains` checks also use exact news sentences.
   - `review_e2e_tests.rs` ~455: `plan_list` must match an exact indented block. The `tag_read` checks at ~414 and ~429 use exact `N. what (tagged by lead; …):\n…` strings.
   - `discussing.rs:150`: exact long-press note.
   - `src/discussing.rs` unit test: mostly sentence prefixes. Its useful part is the `is_none` cases: who is told what.
   - Fix: assert ids, order and the state of each.

6. **Duplicates (cut or merge).**
   - "A change request's answer must name a change made since the review" is tested three times: the engine unit test, `discussions_tests` (d3), and `review_e2e` (d1). Keep the unit test; the end-to-end tests need one refusal each at most.
   - "A relation holds both ends; another plan's holds none" appears in both the engine unit test and `review_e2e` (~243–258). Keep the end-to-end check only for one blocked approval.
   - Review items: `arch-service/tests/flow.rs` and `review_e2e` `a_review_is_made_…` run the same add/nest/move/set and "is a review" refusal. In the end-to-end test keep only the tool's argument parsing.
   - `#` opening the picker is tested in four places: `conversation.rs`, `tags.rs`, `discussing.rs` test 1 (lines 89–97) and test 6. Cut test 1's part.
   - Opening a discussion from the gutter mark is tested in both `discussing.rs` test 2 (click) and the phone test (tap). Keep both input paths, but drop the duplicate open/kind asserts.

7. **Timing.**
   - `slots_tests.rs` `a_turns_tool_output_is_folded_…`: retry windows are 50×50 ms = 2.5 s. This same change had to raise a similar loop from 50 to 500, and the sibling test uses 10 s "for a busy machine". The `given` loop also never asserts that it found the echo before asserting the fold.
   - `terminals_tests.rs` `a_project_says_how_much_…`: `sleep 2` must cover connecting MCP and tagging. On a slow machine the tag lands after the command ends. The test still passes, but then it only covers "past the cap when noted", not "followed while running".

8. **Behaviour the plan relies on with no test.**
   - G.7: starting a discussion from a **plan item** in the cockpit. `C` on an item row or item card is never driven; `plans.rs` only checks the discussion row's things.
   - A failed start loses the first message. `DiscussWrite` takes `desk.discussing` before sending, and `open_started` only sets a note. Untested, and it differs from reply, which keeps the text to send again.
   - The server's `GET /api/discussions/{id}/on` is only in the route snapshot. The cockpit test fakes its answer, and G.2's "says when it changed since" is not asserted anywhere.
   - D.6: a discussion on **code lines** following a saved file through the server. Only tags are tested this way (`changes_tests`); discussions only use the terminal fixture.
   - D.3: a discussion on a chat turn is kept in the logs and still reads after its slot closes. Untested at server level.
   - D.5: a tag on a discussion, through the server.
   - F.2: a principal with `view.read` but not `discuss` can list and read but not start.
   - "The spec view's review works through it": only start, reply, settle and reopen are tested. Nothing checks that `]c` (next open point) opens or steps through the discussion buffer.

No problems found in these areas:
- the `nah-things` logs and search tests
- the notification rules test
- the slot `tell`/held-report tests
- the role/tool visibility test
- the `clean_cut` lints
- the turn-renumbering tests in `harness-acp`

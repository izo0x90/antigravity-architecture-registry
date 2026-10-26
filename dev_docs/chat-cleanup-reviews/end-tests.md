# End review: tests (3e1a805^..HEAD)

Reviewer brief: tests. Paths are relative to `meta-harness/`. Ranked most important first. This was a read-only review. Nothing was built or run.

## A. Tests that pass without the change they claim to protect

1. **`crates/harness-acp/tests/acp_adapter.rs:191` `a_request_the_agent_withdraws_is_no_longer_asked`** passes even if withdrawal handling is deleted. The mock (`src/testing.rs:251-258`) asks inside the running turn, drops the request, then ends the turn. `SessionState::end_turn` (`src/state.rs:305-310`) already marks every pending request of the turn `Cancelled`. The test reads the last status only after `turn_ended(1)`, so `adapter.rs:209-211` (`withdrawn.cancelled()` → `withdraw_request`) is never what it observes. Fix: have the mock hold after withdrawing (`[hold]`-style), then assert `Cancelled` before the turn ends.

2. **`crates/cockpit-ui/src/app/tests/chat_view.rs:807` `the_key_bar_cuts_only_a_chip_too_wide_by_width`** passes on the code before 790657a. The old 40-character cut makes the long chip about 280 px wide, which is under `SCREEN.x / 3` (533 px). It still starts with `"y Allow:"`. The "rest whole" chip, `"i Type a prompt"`, is under 40 characters, so the old code never cut it either. The bug that was fixed (a chip over 40 characters that fits being cut) is not exercised. Fix: add a bound action whose label is over 40 characters but under a third of the bar, and assert it is drawn whole.

3. **`chat_view.rs:783` `a_long_chat_draws_only_the_rows_near_the_screen`** does not protect item 6 ("a frame only touches the rows near the screen"). It asserts `app.desk.chat().ui.heights.len() < 300`. That is an internal cache, and the old renderer also only measured drawn rows. Before 419169c the old code walked every row's position each frame while measuring only the drawn ones, so this test passes on it. The test also duplicates `chat_view.rs:722` `a_long_chat_draws_only_what_is_near_sight`. Fix: merge the two into one test. Assert what a frame visits, either through a count of rows walked per frame or by checking that frame cost doesn't grow from 2k to 20k rows. Drop the `heights` assertions.

## B. Tests that only test themselves

4. **`crates/harness-acp/src/state.rs:970-998` `what_you_send_comes_back_the_same_from_the_replay`**, AsPieces branch. It turns each `Resource` block into text with `Replay::ours_written` and reads it back with `Replay::ours()`. That replay format belongs to no harness that takes pieces, so the branch only proves our writer matches our reader. The InText branch duplicates `pieces.rs:353` `an_agent_that_takes_pieces_gets_them_as_pieces_else_written_after_the_text`. Fix: cut the test and the `#[cfg(test)] pub fn ours_written` helper it alone needs (`pieces.rs:200-204`).

5. **`tests/harness_contract.rs:187-219`, the `started_by` branch.** The mock builds Antigravity's start-call input from the pack's own pointers (`testing.rs:411-419`: `by.title`, `by.prompt`, `by.kind`, `by.list`). The adapter then reads it back through the same pointers. Any pointer values pass. The real guard is `state/subagents.rs:365` (the recorded `subagent-antigravity-1.4.3.json` with the shipped pack). Fix: cut this branch. Also cut the `modes … "its-own"` assertion at `:215`, which duplicates `state/subagents.rs` `a_sub_agents_session_settings_are_not_the_agents`. Keep the `declare` branch. It is the only test that goes through the adapter's raw `session/update` handler.

6. **`crates/cockpit-ui/src/app/tests/menus.rs:261-270`, the `tall` closure.** It builds a fresh `egui::Context` and calls `actions::entry` directly with `Fingers::Yes` and `Fingers::No`. That only checks `ENTRY_FINGER >= 40` and egui's `interact_size < 30`, i.e. constants. It never checks that the which-key menu uses `Fingers::Yes` under a finger (`which_key.rs:126`, `self.fingers`). Fix: send a `Touch` event so `app.fingers` is set, then measure the drawn menu entry's rect.

## C. Behaviour the plan relies on that no test protects

7. **Pieces sent and replayed through the real adapter (Part 1, "round trip … for both the pieces path and the fallback").** The mock never declares `promptCapabilities.embeddedContext` (`testing.rs:206-209`). It also drops every non-text block (`testing.rs:223`). So none of these is tested: `adapter.rs:143` (AsPieces vs InText from what the agent declared), `profile.rs:208`, and `profile.rs:182-187` (`replayed_pieces`: the pack's replay vs ours). The server tests use `FakeSession` (`tests/common/mod.rs:118-133`), which echoes `input.pieces` straight into the item and skips ACP. Fix: one `acp_adapter` test per declaration. Send a prompt with two pieces and assert the blocks the mock received. Then `session/load` the mock replaying those blocks and assert the `UserMessage` text and pieces.

8. **The `subagents: {}` declaration (Part 2g, "the cockpit declares what it supports").** `profile.rs:171-175` adds it to `initialize` as raw JSON. No test reads it. The mock sends `subagent_update` whether it was declared or not (`testing.rs:404`). The mock's typed `seen().client` likely loses the extra field anyway. Fix: have the mock announce sub-agents only if `initialize` carried `clientCapabilities.subagents`, as Claude Code does. The contract test's `declare` branch then guards it.

9. **Stopping one sub-agent on the server.** The cockpit test asserts the `POST …/subagents/a1/cancel` (`subagents.rs` test, around line 133). The route is only in the snapshot. Nothing tests `harness-acp/src/session.rs:258` `cancel_subagent`, i.e. that it sends `session/cancel` with the sub-agent's own session id. Nothing tests that a sub-agent that can't be stopped alone gets an error (`state/subagents.rs:59` `cancellable` returns None). Fix: a mock `[sub-agent]` that records `session/cancel` session ids, and an adapter test of both cases.

10. **The time stamp kept across home (Part 2, item 1).** `src/home/links.rs` now uses `publish_at(event, at_ms)`. `tests/home_tests.rs:67-78` only reads `event.event`. No test shows that a worker's time survives the relay. `chat/tests.rs:536` `an_update_is_as_of_when_it_happened` says "one that came while the tab was hidden isn't news now", but it never asserts that (no `arrivals()` or `news` check). Fix: assert `at_ms` equals the worker's stamp in `home_tests`, and assert arrivals in the chat test.

11. **Other untested plan behaviour:**
    - Absolute paths in the agent's messages and calls as links, read from the thread's `cwd` (item 3): `chat/nav.rs:8-20` `folder_of` and `links_of`. There is no chat-level test with a `policy.cwd`.
    - A discussion's Resume or start for a lead with no session (item 12): `discussion.rs:458-478` `discussion.wake`. No test.
    - The chat banner's Resume of the last session: `chat/actions.rs:90`. No test.
    - Part 2d: the "at work" indicator is one setting for every chat and is saved with the layout (`lifecycle.rs`, `self.at_work`). Only the action id is tested (`chat_view.rs:297`). Nothing switches slots or reloads the layout.
    - Item 11, "built once … every list behaves the same": only the menu is tested. Nothing tests the half-typed-key list that also uses `actions::entry`, or a tap there.
    - Migration 16 (`nah-state/src/migrations.rs`) on an existing layout-15 database with `held_reports` rows and `typed_messages`. Only fresh databases are tested.
    - Part 2b, "tapping outside closes each kind": the hint labels (`Overlay::Hints`), the pickers inside the action list, and the feedback window have no app-level tap-outside test (`app/tests/overlays.rs:366` covers 4 kinds).

## D. Duplicated tests (merge or cut)

12. **Reading a replay back is tested three times, with the harness formats hand-built twice:**
    - `pieces.rs:293-319` (`replayed()` helper plus `a_replayed_message_reads_back…`);
    - `state.rs:941-968` (`a_replayed_message_of_yours_comes_back…`, its own inline copy of both formats);
    - `state.rs:1027` (`a_recorded_replay_reads_back_to_what_was_sent`, the recorded fixtures).

    The hand-built formats can drift from what the harnesses really send. Keep the fixture test as the source of each format. Keep `pieces.rs:322` `what_isnt_wholly_pieces_of_ours…`, the adversarial cases, but feed it the fixture's text. From `state.rs:941`, keep only the chunk-joining cases (same message id joined; Antigravity's whole chunks, two in a row are two).

13. **`crates/cockpit-ui/src/app/tests/subagents.rs:170` `how_many_are_at_work_is_said_on_the_live_line_and_the_panels_top_line`.** It never looks at the live line (that check is in `chat/tests.rs` `an_agent_silent_since…`). It asserts `subagents_working() == 2`, the same function the status line calls. Its top-line check is `s.ends_with(" 2")`, which any status ending in " 2" passes. Fix: cut it, and assert the exact top-line status inside `those_at_work_are_a_strip…`.

14. **`crates/cockpit-ui/src/request_card/tests.rs:104-106`** (`Under::Place("7")` → `At::Waiting`) checks placement plumbing. `chat_view.rs:747` `a_request_waiting_on_you_shows_its_answers_while_you_type` checks the same thing as the user sees it. Cut the unit assertion.

## E. Tests reaching into internals

15. **`subagents.rs:117-118`** sets `app.input_mode` and `app.asked = Some(("chat.open_subagent", None))` directly instead of typing `i` and double-tapping (or Enter on) the chip. That skips the path the plan describes (a chip opens it; typing under it stops). Fix: drive it with keys and a double tap.

16. **`crates/cockpit-ui/src/app/tests/views.rs:318` `each_view_says_when_what_search_lists_changes`** asserts which views return `Some` from `items_changed()`. It checks that the method is declared, not that the value changes when items change or that the key bar stops searching every frame. It also locks the plan's open gap (`Spec`, `Changes`) in as expected. Fix: one test per a view or two. Change the items, assert `items_changed()` moved, and assert it is unchanged across idle frames. Drop the list.

17. **`app/tests/overlays.rs:373` and `:418`** open the issues window by setting `app.feedback.is_issues_window_open = true` instead of its key (`F2`, `issues.toggle`). This is minor. The key path is what a user takes.

## F. Timing, wording, leftovers

18. **`state.rs:1000` `a_long_replay_reads_each_message_back_once`** asserts 3000 messages of 16 KB in under 5 s of wall-clock time, in a parallel test run. By estimate (not measured) the headroom is large. But this time bound is the only guard against quadratic work, and "read back once" is not observable: `told == 1` counts emitted items, not reads. Fix: count reads with a test counter, or compare 1k against 4k messages, instead of using an absolute time.

19. **`tests/slots_tests.rs:148`** asserts the whole joined header text (`"From the user (discussion d1):\n\nWhy?\n\n---\n\n…"`). The behaviour under test is that the link moves with its words, which `:149-151` already checks. Fix: drop the full-text `starts_with`, or reduce it to "both messages present, in order".

20. **`tests/harness_contract.rs:66-68`** has a stale doc comment. The comment about `_meta` and `toolu_task` belonged to the removed `SUB_AGENT_META`. It now documents `fn say`.

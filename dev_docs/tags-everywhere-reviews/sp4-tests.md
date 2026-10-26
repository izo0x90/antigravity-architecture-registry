# Stopping point 4: tests reviewer

## Review of the tests in b29fbfc..HEAD, most severe first

1. **A.4's main test is missing: nothing checks that tags survive a server restart.** The plan's A.4 asks for "comes back whole after a server restart". `tests/terminals_tests.rs:963` restarts only the shell (`POST …/restart`). The only reopen test, `crates/nah-tags/src/lib.rs:630`, checks tag names and never reads anything kept in a log. A restart runs `Tags::open` → `forget_unused`, which deletes every log file the tags loaded from the store don't point into, and then `Logs::open` renumbers the files. If the store loads a tag without its `kept`, or startup passes a different folder, every tagged command's output is deleted at boot, silently. Fix: in `nah-tags`, tag a growing thing and a whole thing, reopen with the same store and folder, and assert `thing()` returns both texts with `Held::Kept`.

2. **`tag_find` and `tag_read lines` count lines differently, and no test covers it.** `src/mcp/sets/tags.rs:154` runs `folded()` on every thing's text unless `full` is set: any line starting with `DETAIL` ("  │ ") is dropped. `find_text` counts lines in the raw text.
   - Trigger: tag a chat turn with tool output (or a command whose output has "  │ " lines, as miette-style diagnostics do). `tag_find` says "line 7"; `tag_read thing 1 lines 7-7` shows a different line.
   - The same filter strips real terminal output from `tag_read` unless `full` is passed.
   - No test passes `full` at all (D.3). The only test, `src/chats.rs`, checks the string `turn_text` builds.

3. **`a_running_command_tagged_again_after_its_tag_came_off_is_kept_once` (`tests/terminals_tests.rs:1015`) can pass without testing anything.** Nothing asserts the command was still running at the third `tag`. Under load (MCP connect plus three calls against 0.5 s ticks), the tags can land after it ended. `follow` then returns false, the text is written whole once, and every assertion passes. The fixed 200 ms sleep at line 1037 waits for the log flush with no polling and no margin. Fix: assert the tag's `kept.len` is `None` (still growing) after the third call, and poll `tag_read` for "exit code" instead of sleeping. The double-follow case is already covered without timing by `nah-tags` `a_thing_still_growing…` (lib.rs:657). If this one can't be made to prove the running path, cut it.

4. **The 50,000-line test has about 2 s of margin** (`terminals_tests.rs:969`, `seq 1 5000; sleep 2; …`). The reader has to see the output, `Mcp::lead` has to connect, and the tag has to land before the sleep ends. Otherwise more than 10,000 lines have scrolled away, `follow` writes "(N lines went…)", and `"lines":50002` fails. Under parallel CI this is a real flake. Waiting on a marker the shell prints, or `sleep 10`, keeps the test's meaning.

5. **The layout-10 migration test locks in data loss** (`crates/nah-state/src/migrations.rs:20`). It asserts that a two-piece `kept` becomes its first piece only, so the second piece's bytes are dropped. The `pieces` layout appeared in 124473b, on this branch only. A database from that build that tagged a running command (which added pieces) loses everything after the first piece. A piece in another file also loses that file at the next `forget_unused`. Either migrate pieces properly or drop the transient layout and its migration and test together (the no-shims rule).

6. **"An untagged command leaves no file" (A.4) isn't actually tested.** `terminals_tests.rs:959` counts files only after every tag is removed, and `forget_all_but` would delete a stray file anyway. Assert the folder is empty or absent after running commands and before the first tag.

7. **Code tags follow edits only because the test calls `refresh()` itself** (`tests/changes_tests.rs:810,816`). Production relies on `ArchEvent::Changed` → `refresh` (`src/startup.rs:265`), and nothing tests that a file write reaches it. Also:
   - The component half asserts only `read.contains("(src/util.rs:")`. It doesn't check the component's place moved after the insert (E.1).
   - `now_at` treating a pruned snapshot as current numbering (`code_tags.rs:90`) is untested.

8. **Chat (D.1): no test re-resolves a tagged message after the session is replayed or reloaded.** That is the renumbering risk the plan said to check first. `slots_tests.rs:856` only closes the slot. The cockpit and server number messages through the shared `message_number`, which is fine, but no test exercises the replay.

9. **Tags view (F.2): the buttons are only found by text, never clicked** (`app/tests/tags.rs:320-322, 349`). `drawn.contains("Note")` / `"Delete"` would pass with any label containing those words. No test checks that clicking "Take off" or "Go to it" sends anything. Separately, `tags.rs:371`, `rows(&app) > 2` "search still finds the folded things", counts `items()` and never searches. Replace it with a `/` search for a folded thing's text and assert the cursor lands on it, unfolded.

10. **Wording tests duplicated across layers.** The level words ("line 50 (of command 3)", "command 3 and its output: make") are asserted in the `terminal/mod.rs` unit test, and the picker top line is asserted word for word in `app/tests/tags.rs:84,87`, 577 and 612. Keep the words in the unit test; in the app tests assert only the POSTed subject before and after `Tab`. " d ago" (tags.rs:326) depends on `at_ms: 0` rendering as days.

11. **Duplicate in `terminal.rs`.** At :434, "a place without a line … its live screen" (restore `kept_place` → no cursor) repeats lines 398–403 of `the_cursor_is_there_only_while_not_typing…`. Cut it.

12. **Gutter column test.** The comment at `tags.rs:504` ("a line two share: both") doesn't match the assertion, which is `note == "a"` from tag a's own column. A long press on the shared muted column (should name d and e) is never tested.

13. **Rule 5 in the code view.** `tags.rs:629` asserts only that a bar exists, not that it sits left of the text, as the terminal (:58) and chat (:592) tests do.

## Not protected at all
- **A.3:** events still send the whole tag (`TagUpdated { tag }`, `harness-contracts/src/events.rs:64`). Not built, and no test.

## No problems found
- `logs.rs` unit test, `layout_9` migration test, palette size test, `#` picker test, `] t` / `ctrl+o` jump test, `every_action_says_what_it_does` lint plus the `debug_assert` (rule 6).

## Cut or merge
- Cut #11.
- Merge #10's word checks into the unit test.
- Fix or cut #3.
- Replace #9's `rows > 2` with a real search.
- Add the restart test (#1) and the `tag_find`/`tag_read` line-number test with `full` (#2) first.

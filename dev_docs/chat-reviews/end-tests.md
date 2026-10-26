End review of the chat plan (d54286c..50a125f): tests.

Paths are relative to meta-harness/. Most serious first.

1. crates/cockpit-ui/src/app/tests/chat_view.rs:67 and :134. The layout test cannot catch text drawn past the panel's right edge. `drawn_texts` keeps only shapes whose clip rect lies inside the panel. `shows` is the text cut to that clip. So `shows.right() > panel.right() + 0.5` can fire for at most half a pixel. Any text drawn with a wider clip (an Area, a layer, a widened child clip) is dropped before it is checked. The test also never flags text cut off at the right edge (`rect.right()` past the panel while the clip hides the tail). That was the reported symptom: an approval cut off, so the agent looked stuck. Plan rule 1 says the test "fails if anything is outside it." Only the left-edge check (:131), the "after it" check and the wrap check have any force. Fix: check every text shape, whatever its clip. Allow overflow only when the text is inside a scroll box narrower than the panel.

2. crates/cockpit-ui/src/app/tests/chat_view.rs:95, :98, :99, :100. Four "every kind of item" cases never draw their long line. Reasoning, a tool call's output and a command's output are folded by default under "normal" (render.rs:547-618), so the long path is never laid out. The file change has `diff: None`, so no hunk is drawn. These cases pass with nothing checked. No test draws opened output, opened reasoning, or a change's hunks in a narrow panel. Fix: run the cases under "everything", or open each fold.

3. crates/cockpit-ui/src/app/tests/chat_view.rs:152. Plan step 1 asks for "a mock test with two at once." This test feeds two hand-made `RequestUpdated` events into the cockpit. No test has the mock agent (crates/harness-acp/src/testing.rs) raise two permission requests at the same time. So nothing checks that the adapter keeps both pending and routes each answer to the right request. That is the server half of "several at once are each answerable."

4. tests/harness_contract.rs:183 with crates/harness-acp/src/testing.rs:371. The sub-agent contract test checks itself. The mock writes `_meta` at whatever pointer the pack declares, and the adapter reads at that same pointer. Any pointer passes, including a typo in packs/harness/claude-code/pack.toml. The real Claude Code shape appears only in crates/harness-acp/src/state.rs:707 (`by`), with its own copy of the pointer string, not the pack's. Nothing ties the shipped pointer to what Claude Code actually sends. Fix: check the pack's `parent_call` against a recorded Claude Code update, and drop the duplicate pointer string in the unit test.

5. crates/cockpit-ui/src/chat/nav.rs:169-175: unprotected. A change to any desk setting re-applies the chat's config defaults. That resets the verbosity the user picked, the indicator, and every fold set by hand (`set_verbosity` starts afresh). No test changes an unrelated setting after picking a verbosity or folding by hand. Rule 4 ("a verbosity setting per chat") depends on this.

6. tests/terminals_tests.rs:488. The "as typed" check reads the server's store (`state.harness.slots.typed`), not what the cockpit receives. It would pass if the wiring at src/modules.rs:333 (`read_typed_with`) were removed. It would also pass if the published user item never carried `typed` (crates/harness-core/src/sessions.rs:82). The test already listens to the event stream; it should check the user's own `UserMessage` item there. No test checks that a replayed session history shows the message as typed. The doc comment on `SlotStore::keep_typed` promises that.

7. crates/cockpit-ui/src/changes/tests.rs:271. This asserts the private `restore` field, not the cursor. In the first case, the list is never delivered afterwards, so "the cursor goes on it once the list is in" goes untested. Only the case where the list is already there (:277) checks the cursor. No test starts from a change in the chat and lands on that file in the Changes view (rule 7, "or go to the Changes view"). Only the receiving side is tested.

8. crates/cockpit-ui/src/highlighter.rs:170-173. `a_block_is_asked_for_once_and_kept` sets `Known::Asked` itself, then asserts it is not asked again. It pushes the reply into the private mailbox by hand. The first half repeats chat_view.rs:447 ("each once"), which checks the same thing through real frames. Cut it. chat_view.rs:448 injects colours with the test-only `knows`. So no test covers a reply going through `code_highlight` into the mailbox. The rule "refused or not reached: plain, not asked again" (highlighter.rs:95) and the `KEPT` reset are also untested.

9. crates/cockpit-ui/src/app/tests/chat_view.rs:238 and crates/cockpit-ui/src/indicator.rs:183. The bounds `again <= 0.3` and `a < 0.3` come from the heartbeat's spot geometry. Its pixel step is 10 s / 50 px = 0.2 s, so only 0.1 s of headroom. Shrinking `SPOT` (live.rs:12) to 40 px makes it 0.38 s, and both tests fail while behaviour is still right. The plan asks for a rate capped at about 30 a second. No test asserts that a repaint delay never goes below `FASTEST_S` (live.rs:116).

10. crates/cockpit-ui/src/app/tests/chat_view.rs:246 and :248. `again <= 1.0` while waiting cannot fail on the words' own schedule. The heartbeat's 0.15 s breathing dot always wins. `again < 60.0` when quiet is close to always true. Nothing checks that the "N s since anything" count moves each second. The "none" case at :258 is the only one that sees the words' schedule.

11. crates/cockpit-ui/src/app/tests/chat_view.rs:258. `ui.indicator.name() == "none"` reads back the value the test just set, from the chat's private state. Nothing checks that "none" stops the heartbeat being drawn: no trace, no dot, no shapes in its spot.

12. crates/cockpit-ui/src/chat/actions.rs:269: unprotected. "The keys are the one's you're on" is never tested. chat_view.rs:180 covers only "the first while you follow the latest" and "moves on once answered." No test puts the cursor on the second request's card and checks that `y` answers that one.

13. Duplicated tests. Cut or merge:
    - chat_view.rs:282-295 repeats rows.rs:577-597 (quiet folds a lone call, everything opens all, the same verbosity keeps what was set by hand, another starts afresh). Keep the rows.rs one for the defaults. Keep in chat_view only the settings row and the cursor landing on the run (:289).
    - chat/tests.rs:394-413 overlaps rows.rs:467-546 (open a run, open its output, fold all, open all). The cursor-on-the-run check is in both chat/tests.rs:410 and chat_view.rs:289.
    - chat_view.rs:392 (a sub-agent's items before what came between) repeats store.rs:425. Its fold and indent checks also overlap rows.rs:558.
    - chat_view.rs:519 "+2 −0" for a new file repeats diff.rs:143.

14. Tests of wording, not behaviour: chat/tests.rs:119 ("Unfold the 2 calls"), :122 ("Open src/parse.rs"); chat_view.rs:385 ("Unfold its sub-agent's work"), :494 ("Open lib.rs"), :554 ("Go to #flaky"), :467/:471/:475 (the exact "Copied … (N lines)." note), :272 ("Shows:", "At work:"), :423 ("Harness:"), :239 (no "RUNNING"). Each one is followed by pressing Enter or checking the result, except :554 (see 15). Drop the label checks.

15. crates/cockpit-ui/src/app/tests/chat_view.rs:557. Enter on a chip is checked only by "the Tags view is focused." It does not check that tag `x1` is the one shown. The label check at :554 is the only link to the right tag.

16. crates/cockpit-ui/src/app/tests/chat_view.rs:591-597. The search assertions are loose. The first hit may be any of 40..=43, and `n` may jump to any other hit. Search order (the next one below the cursor, then the one after) is not tested.

17. crates/cockpit-ui/src/indicator.rs:179: unprotected. The heartbeat test gives one update. Nothing checks "blips once for each update" (two updates far apart make two blips), or that updates closer than a blip's width merge into one.

18. crates/cockpit-ui/src/chat/live.rs:45-58: unprotected. "Quiet too long" must say whether the server can still reach the agent. chat_view.rs:248 checks only `starts_with("No word for 1 min")`. No test covers any `reach` wording: a slot that can't run, a harness starting, a harness failed.

19. crates/cockpit-ui/src/config.rs:145: unprotected. Nothing checks that `[chat] shows = "quiet"` in the config starts the chat quiet, or that an unknown `indicator` name is refused.

20. crates/cockpit-ui/src/app/tests/chat_view.rs:396 and :583-597. These reach into internals. `move_to("42")` depends on the store's internal stream order (the comment explains why it's 42). `app.desk.chat().ui.cursor` reads the chat's private UI state, though `cursor_key()` exists and is used at :289.

21. crates/cockpit-ui/src/app/tests/chat_view.rs:407. `the_settings_row_keeps_to_its_panel` builds the full app 90 times (45 widths × 2 statuses, 4 frames each). The fit rule changes at a few widths. A handful of widths around the breakpoints, plus the narrowest and widest, would protect the same thing.

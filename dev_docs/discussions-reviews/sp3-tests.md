Review of the tests in c7f8a98..d537e31, worst first. I read only; nothing was built or run.

**1. Plan behaviour that is broken and that no test reaches (high)**
- **An agent's question to the user can't be finished.** Use `discuss` with `ask: "user"` and you get a discussion asked of `user` (tests/discussions_tests.rs:317 creates d2 this way).
  - From then on the user can reply, but that leaves it Open, and `whose_move` is still User (harness-contracts/src/discussions.rs:139).
  - The user can't answer: the answer route is gone, and the test asserts the 405 (review_e2e_tests.rs:431).
  - The user can't settle: an open question gets "isn't answered yet" (nah-discussions/src/lib.rs:453).
  - Only delete ends it. Its inbox notification stays open for good.
  - The agent that asked is never told the user's reply: `tell_the_asked` skips anything with `d.asked == USER` (src/discussing.rs, the `continue`).
  - The e2e test stops at `GET d2 → asked == "user"`. A test where the user replies to d2 and the lead is told, and the discussion is settled or done, would fail today.
- **Any agent can answer any discussion.** `Discussions::answer` never checks that `by` is the slot asked. The lead can answer one asked of a worker, or one asked of the user. Answering is meant for the slot asked only (F.3, and the tool text "asked of you"). No test calls `discussion_answer` from a slot that wasn't asked.
- **The cockpit can't pick which slot to ask.** The plan's decision is "the lead, unless you pick another agent slot". `run_discuss` (app/starting.rs) never sends `asked`, and nothing offers a slot. No cockpit test covers it.

**2. Tests that test only themselves (medium)**
- **app/tests/layout.rs:221-273** (`typing_in_the_quick_look_stays_there_over_a_chat_or_a_spec`): for Spec, `in_panel` is hard-coded `String::new()`, so that half asserts nothing. The case that matters now is the Discussion buffer in a panel, which has its own typing box. Swap Spec for Discussion.
- **app/tests/discussing.rs, inbox test:** the test builds the notification itself with title "d3 answered", then asserts the inbox draws "d3 answered". Cut that line. The Enter-opens-d3 check is real.
- **discussing.rs, first test:** the "point 1" in `drawn.contains("point 1")` is fixture text the test injected. It doesn't check that the discussion shown is the one just sent ("Split step 2.").
- **conversation.rs:194:** `assert!(names_tag("see #", 4))` under "Typed, deleted, typed again" repeats line 187 word for word. A pure function can't see a Backspace. Cut it: tags.rs:501-510 is the real test of #56.
- **conversation.rs `who_wrote_something_is_said_one_way`:** it checks the mapping, not that every place uses it, which is what #45 asks for. Its `when` assertion repeats arch-graph/src/when.rs.

**3. Plan behaviour with no test (medium)**
- **G.2, the thing "as it is now, saying when it changed since":** `/api/discussions/{id}/on` appears only in the route snapshot. No test answers `discussion_on` in the cockpit, and none reads the "As it was when the discussion started" branch of `discussion_read` (mcp/sets/discussions.rs:199).
- **The cockpit's `discussion.change` action:** neither path is tested (answered → asks why; open → sends at once).
- **The reopen box after settling:** the box shows when settled only for a reopen. No test ever tells the cockpit the settled state, so that path is untested.
- **Starting by touch or picking the level:** no test presses Tab (`discuss.level`) while starting a discussion, and none covers a start the server refuses or an empty text.
- **G.6:** opening a discussion from a plan-tree row or a spec card is untested. moves.rs:222 calls `show_target` directly.
- **F.2 for discussions:** no test refuses `discuss`, `discussion_read`, `discussion_reply` or `discussions` on another slot's chat, or on a discussion about one. Only the tag tools' use of `Reach` is tested (slots_tests.rs:338-352).
- **F.3 to a sub-slot through the engine:** slots_tests.rs:493 calls `slots.tell` directly. The self-skip compares `said.by` with `slot.name`, and since the lead's id and name are both "lead", the e2e can't tell a name/id mix-up apart. Add a sub-slot whose name and id differ, asked by name, replying itself.
- **Rule 5's guard:** only the user settles because `by == "user"`, and that rests on slot names "user"/"you" being refused (harness-core/src/slots.rs:326). That refusal is untested.
- **Rule 4:** no lint. Nothing stops a second `text_box(`, and chat/render.rs uses `conversation::message` for user messages only. Rule 3's lint (clean_cut.rs) looks only for the text `trait DiscussionStore`, so a second store that doesn't declare it passes.
- **`#` typed mid-draft:** `names_tag` needs the text to end with `#`, so a `#` typed with the caret inside existing text never opens the picker. Untested, and probably wrong under #10.

**4. Duplicates to cut or merge (low)**
- **review_e2e_tests.rs:402-483** repeats engine tests: ends of a relation, another plan's, change needing why, delete (nah-discussions/src/lib.rs:749-810). Keep the route wiring (405, approval held and then freed) and drop the rest.
- **discussing.rs:** `a_thing_with_discussions…` and `at_a_phones_width…` both open by the terminal gutter mark. Merge them into the phone test, plus one mouse click.
- **moves.rs:468** (`C` → `discuss.start`) duplicates discussing.rs's first steps.
- **Counts and keys covered by snapshots:** `lead.len() == 41` (mcp_e2e_tests.rs:370) and `keys_of("discussion.reopen") == ["x"]` (bindings.rs:100) repeat the mcp_tools.txt and keys.txt snapshots.

**5. Wording tests (low)**
- The exact long-press note string (discussing.rs, `"2 discussions…\nd2 question: point 2\nd1 question: point 1"`).
- The full `discussions` listing compared with `assert_eq!` (discussions_tests.rs:320).
- The notification titles (rules/tests.rs:139,147).
- "Next discussion not settled" (bindings.rs:125).

They break on any rewording. Assert ids, order and whose move instead.

**6. Timing (low)**
- **slots_tests.rs:117:** in the test this change extends, the lead's report still waits only 50×20 ms = 1 s; the new part waits 10 s. Make it 10 s.
- **discussions_tests.rs:335:** `told().len() == before + 1` is a negative check with no wait. It holds only because events come in order, and it would pass if a self-tell came late.
- The cockpit tests use simulated time (a 0.8 s hold against the 550 ms threshold), so they won't flake.

**Touch and phone width (tests' view)**
- The phone test covers only tapping the gutter mark and text overlapping in the quick look.
- Untested at 390pt: the discussion buffer's Settle/Reopen header buttons, the `discuss.start`/`discuss.level` picker, and the plan tree's discussion rows.
- Selecting several lines (#13) is tested with mouse events only. On touch, a drag on the line numbers probably scrolls instead; replaying the test with `Touch` events would confirm it.
- `C` is the only way to start a discussion. Whether a finger can start one depends on `discuss.start` reaching the bar, and nothing checks that.

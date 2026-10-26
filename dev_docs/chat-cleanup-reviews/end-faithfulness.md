# End review: faithfulness to chat-cleanup-plan.md

Scope: `git diff 3e1a805^..HEAD` (18 commits). Paths are relative to `meta-harness/`.
Most important first. Each gap gives the plan's words, then the code.

## 1. Part 1: replayed messages are read during the replay, not when drawn

- Plan (Part 1, a harness that takes no pieces): "The reader only runs for a message on screen, once."
- Plan (Part 1, tests): "A replay of a session with thousands of linked messages, which does no per-message work until a message is drawn."
- Code: the adapter reads every replayed message of yours as soon as it has all arrived, for both harness rules and for our own fallback format. See `crates/harness-acp/src/state.rs:202` (`close_user`) and `crates/harness-acp/src/profile.rs:182` (`replayed_pieces` returns `Replay::ours()` for a harness that takes no pieces).
- The planned test was swapped for its opposite. `crates/harness-acp/src/state.rs:1001` (`a_long_replay_reads_each_message_back_once`) checks that each message is read once during the replay, and only times the whole run (under 5 s).
- Plan: the fallback format is "written and read by one module (in `nah-things`, next to the code that pastes it)."
- Code: the format and its reader are in `crates/harness-acp/src/pieces.rs:17` (`OURS`). They are not next to the code that pastes, which is `crates/nah-things/src/attach.rs`.

## 2. Part 2 item 11: the entry piece is used by two menus only, not by the lists or the panels' menu buttons

- Plan: "It's built once, as one entry piece shared with the other lists, so the panels' menu buttons and every list behave the same, not patched in one place."
- Code: `actions::entry` (`crates/cockpit-ui/src/actions.rs:757`) is called only by the which-key menu (`app/draw/which_key.rs:154`) and the key bar's half-typed-key popup (`app/draw/bars.rs:172`).
- The action list and every picker still draw their own rows. See `crates/cockpit-ui/src/pick_list.rs:177`. Those rows do not get the finger height (`ENTRY_FINGER`, `actions.rs:801`).

## 3. Part 2b: two listed kinds are outside the stack, and a new kind still needs new closing code

- Plan lists "the floating buttons under what you're on" and "the toasts" as overlays. Toasts are to let taps through.
- Code: the toast (`crates/cockpit-ui/src/feedback.rs:593`) is not an `Overlay` kind.
- The floating item buttons (`app/draw/host.rs:210`) join the stack only when they are drawn for the quick look (`host.rs:227`, `part_of_the_look`). Under a panel they are not in the stack.
- Plan: "A new kind … is a new entry in the stack, not new closing code." Also: "each kind of overlay says only what it draws and how it behaves."
- Code: whether a kind is open, and how it closes, are separate per-kind `match` arms in the shell. See `crates/cockpit-ui/src/app/overlays.rs:11` (`overlay_open`) and `:26` (`close_overlay`). A new kind needs new arms in both, plus `rules`.
- Plan test: "Tapping outside closes each kind."
- Code: `app/tests/overlays.rs:25` covers 4 of the 8 kinds: palette, menu, issues, and quick look. Feedback and Hints are not tested at app level. Keys is tested only in the `overlay.rs` unit test.

## 4. Part 2 item 12: "what to do about it" is not part of the one answer, and one picker still words agents its own way

- Plan: "One answer, in the shared agents service: an agent slot's state … with its words and what to do about it (resume, new session)."
- Code: `AgentState` (`crates/cockpit-ui/src/agents.rs`) carries only its words, `tone()` and `busy()`. Each place builds its own offer:
  - the chat: `session.resume_last` / `ResumeLast` in `chat/actions.rs`;
  - a discussion: `discussion.wake` / `Command::Wake` in `discussion.rs`.
- The two offers use different labels ("Resume the last" vs "Resume lead's session" / "Start a session for lead").
- Plan: "Every place that names an agent reads it: … the pickers."
- Code: the "Who answers …?" picker (`discuss.ask`) lists slots by name and role only. See `crates/cockpit-ui/src/app/starting.rs:49` and `:58-63` (`slots_to_ask`). It never reads `agents.state`.

## 5. Part 2 item 5: two views still don't say when their items change

- Plan: "Each view says when its items change."
- Code: the Spec card and the Changes view still return `None`. The test locks that in: `crates/cockpit-ui/src/app/tests/views.rs:318-322` asserts `silent == [Spec, Changes]`.
- The commit (100abe6) says "don't yet". This is half done, and the test makes the gap the expected result.

## 6. Part 2 item 4: the store still works out whose an event is, and gets agent-caused turns wrong

- Plan: "Look at whether the server should mark events by who caused them (you or the agent) instead of the store inferring it."
- Code: the server marks nothing. The store still infers it from the kind of event:
  - a turn's first appearance is "your prompt" (`crates/cockpit-ui/src/store.rs:517`);
  - every `UserMessage` is yours (the `news` check just below it).
- Result: a turn that another agent starts is treated as your prompt. This covers a lead's message, a discussion's news, and a delivered held report (`harness-core/src/slots.rs`, `tell` and the held-report delivery). Its "From …" message does not count as the agent's news.
- I can't see from the code whether this decision and its cost were written up for the user.

## 7. Part 2g: no "message one sub-agent" control, not even shown as unavailable

- Plan (Controls): "cancel one, or message one: offered where a harness supports it, shown as not available otherwise, never faked."
- Code: only "Stop the sub-agent" exists, available or not (`chat/actions.rs`, `chat.stop_subagent`). No action for messaging one sub-agent exists in any state. Neither harness supports it, but the plan asks for it to be shown as not available.

## 8. Part 2 item 6: a frame still walks the whole row list in common cases

- Plan: "keeping row positions so a frame only touches the rows near the screen."
- Code, while a turn is running: every frame scans the rows from the top to find the running turn's first row. See `crates/cockpit-ui/src/chat/render.rs:521` (`shape.rows.iter().position(...)`). In a long chat that turn is near the end, so this walks almost every row.
- Code, while streaming: the row's height changes, which bumps `heights_changed` (`render.rs:568`). `lay_out_rows` (`render.rs:600`) then rebuilds every row's position and the viewport's stops. That happens on most frames during streaming.

## 9. Part 1 "Showing it": chips are not matched once and remembered

- Plan: "The chat matches chips to pieces once, when the message is first drawn, and remembers the result for that message."
- Code: `harness_contracts::links_of(pieces)` collects and sorts the links again every frame for each drawn message (`chat/render.rs:755`) and in `chips_here` (`chat.rs:409`). Nothing is kept per message.
- The approach also changed. The plan matches "each `#name` / `@path` in them … to the piece that names it". The code puts each link's character span (`at=start-end-include`) inside the piece's label (`harness-contracts/src/links.rs`, `Piece::label`). The spans are read back, so no matching by name happens. Faithful to "the data itself says so", but it is a different design from the plan's text.

## 10. Part 2 item 3: the folder comes from each thread, not from the server once

- Plan: "The server tells the cockpit its project folder once, and the chat uses it as the terminal does."
- Code: the chat uses each thread's `policy.cwd` (`crates/cockpit-ui/src/chat/nav.rs`, `folder_of`). The server sends nothing new. A thread with no `cwd` gets no absolute-path links.

## 11. Part 1 tests: the round trip doesn't cover the pieces path as a harness replays it, or "show"

- Plan: "A round trip: send, replay, show, for both the pieces path and the fallback text path."
- Code: `crates/harness-acp/src/state.rs:971` replays both paths through our own fallback format. The `AsPieces` resources are rewritten with `ours_written`, so no harness's replay rule is used. The test also stops at the adapter's item; it never draws.
- The recorded-replay test (`a_recorded_replay_reads_back_to_what_was_sent`) and a cockpit chip test (`app/tests/chat_view.rs:~622`) cover parts of this separately. No single send → replay → show test exists.

## 12. Part 2d: the pick is kept in the saved layout, not in the UI config

- Plan: "It's set in the UI config (`[chat] indicator`), with the theme's other settings."
- Code: picking it from the action list stores `at_work` in the saved layout (`crates/cockpit-ui/src/saved.rs`, `SavedUi.at_work`; `app/keeping.rs`). That value then overrides the config on every frame (`app/lifecycle.rs`). So the setting is kept in two places, and the layout's copy wins.

## 13. Part 2 item 9: copies of shared helpers remain

- The commit says "no copy of the hex loop is left in the crate". But `tests/serve_tests.rs:177` (the root package) still has its own SHA-256 hex loop, not `nah_packs::sha256_hex`.
- `harness-core` now exports `now_ms` (`crates/harness-core/src/lib.rs:24`), and nah-timeline's copy was removed for it. But `crates/harness-acp/src/state/subagents.rs:41` adds a new private copy, even though harness-acp depends on harness-core.

## Built but not asked for by the plan

None of these appear in the plan. Each should be in the findings as a deviation, if it isn't already.

- **Requests an agent withdraws.** They are cancelled and taken off the screen. See `crates/harness-acp/src/adapter.rs` (`ask` now takes a `RequestCancellation`) and `state.rs` (`withdraw_request`).
- **Held reports keep their links.** Migration 16 also adds `held_reports.links` (`crates/nah-state/src/migrations.rs:69`). A discussion's news now moves the user's links with its text (`src/discussing.rs`, `news`). The commit calls this a fix for links that pointed into the wrong text. The plan didn't list that bug.
- **One sub-agent can be stopped over HTTP.** The new route is `POST /api/threads/{thread}/subagents/{subagent}/cancel` (`src/routes/harness.rs`). This is part of 2g's controls, but it is a new surface and is not mentioned in the plan.

## Process

- Plan (Order): "Each step is built, tested, checked live … and committed on its own." The "Part 2, the rest, item by item" step put items 1, 2, 3 and 4 in one commit (113e2a6).

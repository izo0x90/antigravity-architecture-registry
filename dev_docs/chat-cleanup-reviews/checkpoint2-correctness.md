# Checkpoint 2: correctness review (e1a2167, ff7e269, 12660e6, 29b5dde)

Ranked most important first. Paths are under `meta-harness/crates/`. Each item has a concrete sequence that goes wrong.

## 1. A sub-agent's settings replace the main session's controls and model list (12660e6)

`harness-acp/src/state/subagents.rs:32` (`_ if child =>` goes to `apply_child`, which calls `apply_as_sent`), then `state.rs:480` (`ConfigOptionUpdate` calls `set_config`, which replaces `thread.controls.config` and `catalog.models`).

The connector publishes each child's configuration on the child's own session (`native-subagents.js` `publishConfiguration`, and `reportModel` re-routed through `reportConfiguration`). The repo's own fixture shows it: `subagent-cancelled-claude-code-0.95.0.json` sends three `config_option_update`s under `a645dc829a223811b`, the last with `mode` (1 option, "Manual"), `model` (1 option, `claude-opus-5-5`), `effort` (1 option) and `agent`.

Sequence: the main session is in auto mode with 3 models. The agent starts one sub-agent. The main thread's controls become mode = Manual (the only choice), a model list of one, and an extra "Agent" select. The cockpit's mode and model pickers for the lead now show the sub-agent's settings. `CatalogUpdated` also shrinks the models to one. Picking a value sends `set_config` for the main session with the child's option ids.

The same path also lets a child's `usage_update`, `current_mode_update`, `session_info_update` and `available_commands_update` overwrite the main thread's usage, mode, title and slash commands.

The fixture test (`a_sub_agent_claude_code_announces_...`) runs this exact sequence but never checks `thread.controls`.

## 2. A sub-agent's answer is lost in hand-back mode, and a follow-up prompt is never shown (29b5dde, 12660e6)

`state/subagents.rs:92` (after 29b5dde, `session_message` keeps only `recipientSessionId`, as the prompt) and `:29-30` (a `session_message` on a child session is dropped).

- **Hand-back.** When Claude Code runs a child in hand-back mode (`SubagentHandback`), the connector hides that tool call (`routeRfdHandback` returns `null`). The report goes only as a `session_message` from the child to the parent (`linkedSessionMessages`). The answer is not "its own last message". Sequence: a hand-back child finishes. Its report appears nowhere: not in the pop-up, not under its item, not in the chat. Only the main agent's paraphrase remains.
- **Follow-up prompt.** `resumeChild` → `publishPrompt` without `replacesLaunch` publishes the new prompt only on the child's session. Sequence: the agent sends a second instruction to an idle child (SendMessage). The child goes back to `running`. "What it was told" in the pop-up still shows the first prompt, and the new task is never shown.

## 3. Under the modal sub-agent pop-up, the typing box keeps the keyboard; Enter sends to the agent (29b5dde with e1a2167)

`cockpit-ui/src/app/draw/host.rs:67`: `typing` excludes only the feedback, issues and inspector windows, not `Overlay::Subagent`. `chat/conversation.rs` `draw_in` calls `request_focus` every frame while `typing`. `overlay_keys` returning early from `handle_keys` does not stop egui's TextEdit.

Sequence:
1. Send a prompt. The chat stays in typing mode (as ff7e269 says).
2. A sub-agent chip appears. Double-tap it (`pointer_on_content` → `nav.enter` → `open_subagent_here`). The pop-up opens and `input_mode` stays `Insert`.
3. Type "stop" and press Enter. The text goes into the hidden box and is sent to the main agent.

This breaks the plan's rule: "What's under an overlay doesn't take keys or taps while it's up". Escape closes the pop-up but does not leave typing, so "keyboard back where it was" is right only by accident.

## 4. While you type, a request's answers still don't show unless the cursor happens to be on it (ff7e269)

`chat/actions.rs:284`: `here = index == on_item()`, and `on_item()` is the cursor, else the first pending request. ff7e269 only changed where the answers go (`At::Waiting`), not when they show. Part 2 item 13 says "What waits on you always shows its answers, typing or not."

- **Sequence A.** Tap a sub-agent chip (the cursor goes to its item, `subagents.rs` `move_to`), or tap any message, then type and send. The sub-agent asks for permission. Its card shows in what waits on you with no Allow or Deny. In typing mode `j`/`k`/`y`/`d` go to the box, so the only way to answer is Escape first: the bug item 13 describes.
- **Sequence B.** Two sub-agents ask at once (Claude Code runs them in parallel). Only the first card gets buttons. The second has none until the first is answered.

Also, in floating mode while typing, the `waiting_controls` area (`host.rs:150`) sits at the card's bottom + `space_s`. That is over the sub-agent strip and the top of the typing box (Order::Foreground). A tap meant for the box can land on Allow.

## 5. After a page load, resync or session resume, every past sub-agent looks "just ended", and times come from the cockpit's clock (29b5dde)

`store.rs:99-106` (`subagent_now`: a new `Seen` gets `since: now`, and `ended: Some(now)` when not working), `store.rs:374` (Resync rebuilds the views), `chat/subagents.rs:51`.

Sequence: a thread has 5 finished sub-agents. Reload the cockpit, or reconnect, or resume the session (the connector's RFD replay announces each as `running` and then `idle`).
- For 6 seconds the strip shows them as just ended.
- With more than 3, the folded chip says "0 sub-agents working" (`subagents.rs:86` counts only working ones).
- Every chip and the pop-up say "0 s" (`elapsed` = ended − since).
- A sub-agent that has run 10 minutes shows "0 s" after a reload.

This contradicts Part 2 item 1 (the server stamps when things happened). `Subagent` carries no start or end time.

The resync also wipes `threads` (`*self = Self::default()`). If the replay lands over more than one frame, `render_subagent` finds no thread and closes an open pop-up (`draw/subagent.rs:28`).

## 6. Stopping a sub-agent that waits on you leaves its request pending (12660e6, 29b5dde)

`draw/subagent.rs:185-188`: Stop shows when `working()`, which includes `Waiting`. `adapter.rs:266-290`: the permission handler never looks at `responder.cancellation()`.

Sequence:
1. A child is `requires_action` (permission card up).
2. Tap "Stop it". `session/cancel` goes to the child, and Claude Code withdraws the child's request with `$/cancel_request` (`awaitingUser`: "Claude Code withdrew it as it stopped the child").
3. The request stays `Pending` in `SessionState` for the rest of the turn (the turn goes on, so `end_turn` doesn't cancel it). The card keeps Allow/Deny, and `live.rs` shows `Liveness::Asking`. Answering it replies to a withdrawn request.

## 7. A sub-agent can stay "working" forever, or a stopped one can read as done (12660e6)

`state/subagents.rs:139-145` (`subagents_end_with_the_turn` skips every told-of child), `state.rs:333` (`end_all`), `state/subagents.rs:147-149` (a new one defaults to `Running`).

- **Connection drops.** Claude Code is running a sub-agent and its process dies. `end_all` fails the turn, but the child stays `Running`. The strip chip, the panel's top-line count, and `delegating` in `live.rs` keep saying it works, with a growing timer, until the thread view is closed.
- **Announced with no state.** `restore` (native-subagents.js) announces a resumed child's ancestors with no `state`. `subagent_update` without `state` keeps the default `Running`, and nothing ever ends it.
- **Antigravity turn stopped.** Press Stop during an Antigravity sub-agent. The turn ends `Cancelled` (or `Failed` when the connection closes). The sub-agent becomes `Idle`, which is "ended", and its item `Completed` (the done tag). Part 2f calls this exact failure "faithful but misleading": the turn's end status is ignored.

## 8. The overlay stack follows draw order, not open order: the quick look outranks the sub-agent pop-up opened over it (e1a2167, 29b5dde)

`app.rs:635-637` draws `render_quick_look`, then `render_subagent`, then `render_item_controls`. While the quick look is open, the item controls belong to it (`part_of_the_look` → `drawn_at(QuickLook, …)`). `overlay.rs:101` `top()` takes the last drawn that is open.

Sequence:
1. A quick look shows a chat with the cursor on a sub-agent.
2. Tap its "Open" item control. The pop-up opens and the quick look stays.
3. From the next frame, `drawn` = [QuickLook, Subagent, QuickLook(controls)], so `top` = QuickLook. Then:
   - A tap on "Stop it" or "Where it started" is outside the quick look's rects. It closes the quick look and is swallowed; the button does nothing.
   - Escape goes to the quick look's own back handling (own_back), not to the pop-up.
   - The pop-up's modal rule never applies: keys reach the panels under it.

## 9. A sub-agent of a sub-agent goes to the top level, without its prompt (12660e6)

`state/subagents.rs:29` and `:147`. The connector announces a grandchild on its parent's session (`child.parentSessionId` = the child's session) and sends its prompt and reply only on the child's and grandchild's sessions.

Sequence: sub-agent A starts sub-agent B.
- B's `subagent_update` (on A's session) creates B with `self.parent = None`, so its item is a top-level item of the turn, not under A.
- B's prompt (`session_message` on A's and B's sessions) is dropped, so "What it was told" never shows.
- A's step count and pop-up don't include B.

## 10. Antigravity: a reopened session has no sub-agent at all (12660e6)

`state/subagents.rs:115-118`: `input.pointer(&by.list)`. In the replay, `rawInput` is a JSON string, not an object (`chat-cleanup-probes/combined-agy-143.json`, `replay[2]`: `"rawInput": "{\"Subagents\":[…]}"`).

Sequence: run a sub-agent, then resume the session. The pointer on a `Value::String` is `None`, so nothing starts. History has no sub-agent line, only "Running invoke_subagent". The plan's "history stays calm: one folded line where it started" fails after any reopen.

Parsing the string wouldn't be enough either: a replay has no turn end, so `subagents_end_with_the_turn` never runs and the replayed sub-agent would stay `Running`. The plan asks for the replay as a fixture; there is none.

## 11. A background sub-agent's steps after the turn ends create a phantom turn (12660e6)

`state.rs:371` (`item_turn`): with no running turn and no replay turn, `apply_child` → `push_item` → `start_turn_record(Completed)`.

Sequence: the agent starts a sub-agent with `run_in_background: true` and ends its turn. The child goes on (the connector says "its background work can still report"). Its next tool call opens a new numbered "turn" with no message of yours. Your next prompt's turn is numbered one higher than after a reload, which breaks "turns keep their numbers after a reload". `ThreadView::place` also can't find the parent in that turn (`*t == turn`), so the step is placed as a top-level item of the phantom turn.

## 12. A press whose release never arrives eats the next tap (e1a2167)

`overlay.rs:140-160`: `taking.button` is cleared only by a `PointerButton { pressed: false }`.

Sequence: with the action list open, press outside it (it closes; `taking.button = true`). Drag off the browser window and release there, or lose the pointer, so no release reaches egui. The next click anywhere (a panel item, the new overlay's button) has its press and release removed and does nothing.

A second finger's touch in the same frame isn't taken (only the first `Touch Start`'s id is), so it reaches the panels under a modal overlay.

## 13. A failed Stop says nothing (29b5dde)

`net.rs:563`: `cancel_subagent` uses `|_| {}`.

Sequence: a viewer without SessionsControl, or a sub-agent the server no longer has as cancellable (`Unsupported` from `session.rs`), or a network error. Tap "Stop it": no note, no toast, the button stays, and the sub-agent keeps "working". Double taps send two cancels, and nothing shows a stop is on its way.

## 14. The plan's cost rule is broken in two hot paths (12660e6, 29b5dde)

"Cost proportional to what's on screen":

- **Every update is copied.** `adapter.rs` (raw handler) clones the whole `update` JSON of every `session/update`, for every harness, before the typed parse. That includes big tool outputs and file contents, and harnesses with no sub-agents. Child updates are cloned a second time (`state/subagents.rs:33`, `from_value(update.clone())`).
- **The pop-up rebuilds everything each frame.** `draw/subagent.rs:134-144` collects every step under the sub-agent, cloning each message's full text and the whole `Seen` (prompt included), every frame it's open. Only the last 200 are shown. A sub-agent with thousands of steps does thousands of string copies per frame. The tests use one sub-agent with one step.

## 15. Smaller, each concrete

- **The strip never folds again.** `chat/subagents.rs` `subagents_each`: once the folded chip is tapped, `subagents_unfolded` is true for good. Later, 10 sub-agents each get a chip and the strip wraps over several rows above the box.
- **Duplicate Antigravity start.** `state/subagents.rs:118`: `starting` is filled again by any later update that carries the input. If a start call's `in_progress` and `completed` updates both carry `rawInput` (Antigravity does this for its other calls: in_progress carries `RAW` in the 1.4.3 fixture), the second one starts the same sub-agents again, with the same ids `"{native}/{n}"` and new items.
- **A resize closes the issues window.** The issues window's resize handle is grabbed a few points outside `response.rect`, so a drag to resize counts as a tap outside and closes it (`Outside::Closes`).

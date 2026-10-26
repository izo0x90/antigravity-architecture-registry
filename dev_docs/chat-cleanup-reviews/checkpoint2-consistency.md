# Checkpoint 2: consistency review

Commits: e1a2167 (one overlay layer), ff7e269 (a request's answers shown while typing), 12660e6 (sub-agents, server side), 29b5dde (sub-agents, cockpit side). Paths are relative to `meta-harness/crates/cockpit-ui/src` unless they say otherwise. Ranked most serious first.

## 1. The sub-agent pop-up is drawn by the shell, outside the buffer and host model, with hand-built buttons (29b5dde)
`app/draw/subagent.rs:17-114` reads `self.desk.shared.agents` straight from the shell and draws the content itself. Its three controls are plain egui buttons: `Button::new("Stop it")` at :85, and `ui.button("Where it started")` and `ui.button("Close")` at :92 and :95.
- **What already exists for this:**
  - The quick look is an overlay that hosts a view: `render_view(Here { at: Host::Look, kind }, …)` in `app/look.rs:130-152`.
  - Controls are actions with a `Control` placement (`actions.rs:461-480`), drawn by `crate::actions::controls` / `host_controls` (`app/draw/host.rs:120-125`).
  - Stopping is already an action: `session.stop` in `chat/actions.rs:120`.
  - "Not available" is said with `When::Unavailable` / `offer_if` (`nav.rs:489-492`).
- **What follows from building it by hand:**
  - "Stop it" isn't an action. It isn't in the key bar, the which-key menu, `:` or `?`, it has no key, and the modal overlay blocks every other key, so on a keyboard it can't be reached at all.
  - "Can't stop it alone" is a hand-written label at :89. It should be an unavailable action with that reason.
  - "Close" repeats what the overlay's tap-outside and Escape already do.
- **The steps are a second renderer of chat items:**
  - Messages become plain text, joined by hand at :37. `agents::block_summary` (`agents.rs:502`) already does this, and the chat itself renders messages with markdown and links.
  - Tool calls are drawn on their own monospace lines.

## 2. A status-word map is built by hand when the configured status vocabulary exists (29b5dde)
`chat/subagents.rs:20-29` (`state_words`) hard-codes a word and a colour for each `SubagentStatus`. All three places that use it (the chip at `chat/subagents.rs:97`, the stream line at `chat/render.rs:793`, the pop-up at `app/draw/subagent.rs:53`) show the same `icon::CIRCLE` for every state.
- **What already exists:** the status vocabulary in `status.rs:1-4` and `:89-121` (`look`, `chip`, `text`). It is configured as `[status.*]` in `config/default.toml`, and jobs set the precedent:
  - `[status.job_queued]` … `[status.job_done]` at `default.toml:607-634`, with icons `play`, `hand`, `x-circle` and `check-circle`;
  - the mapper `status::job` is used in `queue.rs:158` and `queue.rs:435`.
- **Fix:** `[status.subagent_*]` entries plus a `status::subagent(SubagentStatus)` mapper.

## 3. `elapsed` duplicates `span_words` (29b5dde)
- `chat/subagents.rs:32-35` is the same formula, letter for letter, as `span_words` in `chat/live.rs:18-20` ("12 s", "3 min").
- `chat/subagents.rs:57` asks for a repaint every second, without condition. `span_changes` (`chat/live.rs:23-25`) already says when such words next change, and the live line uses it through `Next::In`.

## 4. The sub-agent's line in the stream counts and words its steps a second way (29b5dde)
- **New code:** `chat/render.rs:785-789` counts `view.under(&item.id)`, which holds direct children only. The count comes from a new `under` index in the store (`store.rs:89`, `:233`). It is worded " · N steps", with no fold mark, although the commit message says "(folded under it)".
- **What already exists:** the row model has `row.nested` (`chat/rows.rs:30`, built at `:369-375`), and `render_item` already has `row` in scope. A call with nested steps is already drawn as `fold_mark(open)` + "N steps by its sub-agent" (`chat/render.rs:706-711`). The store also has `descends` for any depth (`store.rs:327-336`).
- **What follows:** the new index is a second record of the nesting (against derive, don't duplicate). The pop-up also uses it, so it misses steps nested deeper than one level.

## 5. The chip's frame and sizing are hand-built (29b5dde)
- **Where the cursor is:** at `chat/subagents.rs:72` it is marked with a `colors().focus` stroke. Every other place in the chat marks it through `item_frame` (`chat/render.rs:286-300`) with a `colors().selection` fill, including the waiting cards drawn just above the strip (`chat/render.rs:205`).
- **Sizing:** the chip is measured before it is placed, so a wrapping row moves it whole (`chat/subagents.rs:66-70`). `status::chip` (`status.rs:101-121`) already does exactly this.

## 6. Sizes and timings are hard-coded instead of coming from the theme or config (29b5dde)
- **Pop-up sizes:** the pop-up's width is `640.0` (`app/draw/subagent.rs:22`), and its heights are `screen.height() * 0.2` and `* 0.4` (`:64`, `:70`). The quick look's size comes from `theme.quick_look_width` and `theme.quick_look_height` (`theme.rs:253-254`, used at `app/look.rs:136`).
- **Strip timing:** `ENDED_STAYS = 6.0` (`chat/subagents.rs:13`) is a timing of the strip. The live line's timings are config, in `[chat] receiving_s` / `quiet_after_s` (`default.toml:783-784`, `config.rs:134`).

## 7. One count, two wordings (29b5dde)
- The folded chip says `"{working} sub-agents working"` (`chat/subagents.rs:86`). It folds on `shown.len() > 3`, and that count includes sub-agents that have ended, so with one still working it reads "1 sub-agents working".
- The live line says the same thing with singular and plural cases (`chat/live.rs:79-85`).

## 8. The next and previous sub-agent actions don't follow the other "next X" actions (29b5dde)
- `chat.next_subagent` and `chat.prev_subagent` (`chat/actions.rs:139-142`) have no `.group(Group::Navigate)`. Every other "next X" action has it: `code.next_def` (`code.rs:935`), `spec.next_point` (`spec.rs:936`), `tag.next` (`app/tagging.rs:129`). So they sit in the wrong group in the which-key menu and the action list.
- The same commit names its ids two ways: `chat.next_subagent` and `chat.subagent.open`.
- `chat.subagent.open` has no group either. `queue.open_session` uses `Group::GoTo` (`queue.rs:242`).

## 9. Two pack mechanisms say which steps are a sub-agent's (12660e6)
- **The two mechanisms:**
  - Claude Code's pack keeps the top-level `parent_call = "/claudeCode/parentToolUseId"` (`packs/harness/claude-code/pack.toml:37`). It is read by `parent_of` (`crates/harness-acp/src/state.rs:503-515`) and nests a step under the Task call's item.
  - The commit adds `[provides.settings.subagents] declare = true` (`pack.toml:56-57`). Its child-session steps go under the Subagent item through `apply_child`, which skips `parent_of`.
- **What follows:**
  - The sub-agent settings are split across two places in the pack and two paths in the adapter. The plan says "One rule in the adapter, and each pack names its way" (Part 2f).
  - The fake harness's contract path tests only `parent_call` (`crates/harness-acp/src/testing.rs:247`, `:386-395`).
- **Fix:** move `parent_call` under `subagents` (or remove it if `declare` replaces it).

## 10. The adapter's raw dispatch is written twice, once in a test (12660e6)
- The raw notification handler in `crates/harness-acp/src/adapter.rs:233-248` checks for `session/update`, pulls out `sessionId` and `update`, and falls back to the typed handler.
- The test helper `replayed` (`crates/harness-acp/src/state/subagents.rs:223-240`) rewrites that dispatch instead of calling one shared function.
- So the handler the adapter really runs isn't the code under test.

## 11. The server stores a derived copy of the prompt, through a second `first_line` (12660e6)
- **The copy:** `crates/harness-acp/src/state/subagents.rs:139-140` sets Antigravity's `description` to `first_line(prompt)`, and the test asserts the two are equal. That stores a cut copy of a field the model already carries.
- **The second helper:** `first_line` (`:198-204`) is a second version of `tags::first_line` (`tags.rs:45`), cut at 120 characters in both. It differs only by adding `…`.
- **Fix:** the cockpit can cut the prompt when it shows it.

## 12. Errors for one sub-agent's stop are worded two ways and dropped (12660e6, 29b5dde)
- **Wording:** `HarnessError::Unsupported` says "stopping one sub-agent" in the trait's default (`crates/harness-core/src/lib.rs:152`) and "stopping this sub-agent alone" in the ACP session (`crates/harness-acp/src/session.rs:261`).
- **Dropped result:** `net.cancel_subagent` drops the result with `|_| {}` (`net.rs:564`), and `Act::StopSubagent` is given no `done` (`agents.rs:312`). A refused stop says nothing. Calls that can be refused, such as `Act::Steer` (`agents.rs:314`, `net.rs:560`), pass `done` so the note shows the error.

## 13. The overlay commit leaves leftovers and a second way of saying where the palette was drawn (e1a2167)
- **Dead field:** `QuickLook::opened` (`quick_look.rs:30-32`, set at `:39`) is now never read. Its doc comment still describes the rule this commit removed. That breaks the clean-cut rule.
- **Two ways in one commit:** the feedback windows were changed to return their rect (`feedback.rs`: `render_modal` / `render_issues_window` return `Option<Rect>`). The palette's rect is instead read back from egui memory (`app/draw/panels.rs:64`).
- **Allocation:** `before_input` collects a `Vec<Overlay>` every frame (`app/overlays.rs:62`). Both the plan ("Nothing is allocated per frame for it") and `overlay.rs:7-8` say it doesn't. `outside_tap` could take `|o| self.overlay_open(o)`; the Vec is only needed because of the borrow.
- **A second closing rule:** a setting's list of choices is left out of the stack and closed by egui's own rule (`app/overlays.rs:57-61`).
- **Read twice:** the way back out is read twice per frame (`app/keys.rs:48` and `:55`).

## 14. Test helpers are copied instead of using the shared ones, and the new overlay kind isn't in the per-kind test (all four)
- **`app/tests/subagents.rs`:**
  - its `click` (:160) shadows the shared `click` in `app/tests.rs:89`;
  - `drawn_text_at` and `collect_text` (:140-157) repeat `drawn_containing` (`app/tests.rs:73`);
  - `shown` (:54) only wraps `texts`;
  - `session` (:12) repeats `chat/tests.rs:39`.
- **Other tests:**
  - `app/tests/overlays.rs:9` adds a `tap` that is `click` with times;
  - ff7e269's test builds its `PointerButton` events by hand (`app/tests/chat_view.rs:765-766`) instead of calling `click`.
- **Missing kind:** `Overlay::Subagent` (29b5dde) wasn't added to the per-kind table in `a_tap_outside_closes_each_and_reaches_nothing_under_it` (`app/tests/overlays.rs:25-38`). Nothing checks that a tap outside it doesn't reach the panel underneath.

## 15. The store sets a sub-agent's start and end times on replay too (29b5dde)
- `ThreadView::subagent_now` (`store.rs:99-107`) sets `since` and `ended` to `now` for every event.
- The same event's `arrived` is only counted when `live` (`store.rs:488`).
- **What follows:** after a reopen, every sub-agent that had already ended comes back into the strip for `ENDED_STAYS`, showing "0 s".

## 16. A sub-agent target is named by its raw id (29b5dde)
- `Target::Subagent.in_words` is `"sub-agent {id}"` (`nav.rs:154`). That id is a session UUID for Claude Code, or `"{call id}/0"` for Antigravity (`crates/harness-acp/src/state/subagents.rs:137`).
- The sub-agent has a title, and that title is what every other place shows.

# Checkpoint 2 review: responsibility and coupling

Commits: e1a2167 (overlay layer), ff7e269 (answers while typing), 12660e6 (sub-agents, server), 29b5dde (sub-agents, cockpit). Paths are relative to `meta-harness/`. Most important first.

## 1. A sub-agent's own session state is written into the parent thread
`crates/harness-acp/src/state/subagents.rs:101-104` (`apply_child`) sends every typed update from a child session through `apply_as_sent`, which is the parent session's handler. So the child's updates replace the parent `Thread`'s state:
- `ConfigOptionUpdate` → `set_config`, `state.rs:480`;
- `CurrentModeUpdate` → `thread.controls.current_mode`, `state.rs:476`;
- `SessionInfoUpdate` → the thread's title, `state.rs:481`;
- `UsageUpdate` → the thread's usage, `state.rs:486-489`;
- `AvailableCommandsUpdate` → the catalog, `state.rs:471`;
- `Plan` → the thread's plan, `state.rs:670`.

The recorded fixture shows this happens. In `fixtures/subagent-cancelled-claude-code-0.95.0.json` the child session (`a645dc82…`) sends three `config_option_update`s, and each one overwrites the main agent's controls (its model and mode pickers) with the sub-agent's.

It also skips a step. The mode→policy step lives in the adapter's typed handler (`adapter.rs:253`), so a child's mode change sets `current_mode` without updating `policy`, and the thread's mode and policy stop agreeing.

Only the item updates (calls, messages, thoughts) belong under the sub-agent. Session-level updates from a child need their own target, the `Subagent`, or should be dropped.

## 2. The shell draws a sub-agent's content itself, beside the quick look
`crates/cockpit-ui/src/app/draw/subagent.rs` is a second way to show content over the panels. The quick look already does this: it holds a buffer and renders that buffer's view (`quick_look.rs:19`, `look.rs`). The sub-agent pop-up instead:
- keeps chat-domain state in `CockpitApp` (`subagent_look: Option<(SlotId, SubagentId)>`, `app.rs:295`);
- reads the agents desk service directly to draw (`subagent.rs:25`);
- imports the chat buffer's internals: `crate::chat::subagents::{elapsed, state_words}` (`subagent.rs:11`), and `crate::chat::status_tag`, made `pub(crate)` for this (`subagent.rs:76`, `chat.rs`);
- renders items again by matching `ItemBody` (ToolCall and AssistantMessage, with text joining) in the shell (`subagent.rs:31-44`). The chat's row renderer already does this.

Result: the shell knows the chat's data model and its wording. `Ask::Subagent` (`nav.rs:121`, handled at `asks.rs:65`) is a chat-specific ask whose only job is to set that shell field. A target shown in the quick look (or a sub-agent buffer) would use the mechanism that already exists.

## 3. The pop-up's controls are hand-drawn buttons, outside the actions system
`subagent.rs:85,92,95` draw `Stop it`, `Where it started` and `Close` with `ui.button` / `egui::Button`. `subagent.rs:104` calls `agents.act(StopSubagent)` directly. None of these is an `Action`:
- they have no keymap entries;
- they aren't in the action list;
- they get no host placement.

The pop-up is `modal: true` (`overlay.rs` rules), so the keys can't reach them either: on the keyboard only Escape works. `Close` repeats the overlay's own closing. This breaks "hosts draw controls; keys come from the keymap only".

## 4. The stack doesn't own its kinds: closed enum plus match tables over other parts' fields
- The kinds are a closed `enum Overlay` with a separate `ALL` array (`overlay.rs:13`, `:56`).
- Whether each kind is open is a match in the shell, `overlay_open` (`app/overlays.rs:11`), which reads eight owners' internals: `quick_look`, `feedback.is_inspecting / is_modal_open / is_issues_window_open`, `capture`, `pending_keys` / `pending_count`, `input_mode`, `palette`, and now `subagent_look`.
- Closing is a second match, `close_overlay` (`:27`). It writes `feedback`'s fields directly (`:30-32`) instead of calling a close on `FeedbackInspectorState`. It also copies the end-the-hints sequence from `which_key.rs:77` (`on_capture(&[], true)`, `capture = None`).
- 29b5dde added one kind (`Subagent`) by editing the enum, `ALL`, `rules`, `overlay_open` and `close_overlay`: five places in two files, one of them new closing code. That is what Part 2b's "a new kind is a new entry in the stack, not new closing code" rules out.
- "Top" is the order drawn in the last frame (`overlay.rs:101`), which is the order of the `render_*` calls in `app.rs` `frame()`. It isn't the order the overlays opened. 29b5dde reordered `ALL` to put `Subagent` before `Menu`, which changes nothing: `ALL` is used only as a filter (`app/overlays.rs:62`).

## 5. Escape, modality and giving the keyboard back are still per kind
- `overlay_keys` returns before it reads `modal` whenever `own_back` is set (`app/overlays.rs:74-81`). `Menu` and `Palette` are `modal: true, own_back: true`, so their `modal` flag is never read. Their keys are still kept from the panels by their own `InputMode` handling in `keys.rs`. Five of the nine kinds (QuickLook, Hints, Keys, Menu, Palette) still handle Escape in their own code.
- "Closing gives the keyboard back" is three separate memories:
  - `palette_from` (`app.rs:313`, which predates these commits);
  - the new `menu_from` (`app.rs:293`, `app/overlays.rs:85-98`);
  - the quick look's `before`.

  The new kinds (Subagent, Issues, Feedback) have none. The plan asks for one rule, not a field per kind.

## 6. Things over the panels left outside the one stack, with their own rules
- **A setting's choice list** (`InputMode::Choosing`) uses egui's own popup closing. `before_input` skips the stack for it with a special case (`app/overlays.rs:59`).
- **Toasts** (`render_toast`, `app.rs:633`) aren't entered. The plan names them as letting taps through.
- **Floating item controls** count as an overlay only when `host == Host::Look` (`draw/host.rs` `part_of_the_look`). This causes a bug: `render_item_controls` runs after `render_subagent` (`app.rs:636-637`), both at `Order::Foreground`. The floating controls (for example "Open" under the sub-agent's own line, shown while the cursor is on it) can draw over the centred pop-up. A tap on them counts as outside the pop-up, so it closes the pop-up and the tap is lost.

## 7. Two ways for an overlay to say where it was drawn
Feedback and issues now return their rect (`feedback.rs` `render_modal` / `render_issues_window`). The palette's rect is instead read from egui memory by the palette's private area id (`draw/panels.rs:64`), with `Palette::area()` made `pub(crate)` so the shell can do it. The shell is now tied to the palette's egui id; the palette could return its rect as feedback does.

## 8. Two ways of placing a sub-agent's steps, and an existing capability left unused
- The adapter keeps the earlier `parent_call` rule:
  - `_meta` pointer, `state.rs:504` `parent_of`;
  - `packs/harness/claude-code/pack.toml:37`;
  - it nests steps under the Task call's item.

  12660e6 adds a second rule (child session id → under the new `Subagent` item, `subagents.rs:101`). Both are live for the same pack, and they pick different parent items. Part 2f says "One rule in the adapter, and each pack names its way".
- `Capabilities.subagents { supported, lifecycle_events }` (`harness-contracts/src/capabilities.rs:97`) is the existing place a harness says what it offers, and nothing fills it in. What a harness offers for sub-agents now lives only in pack settings and in each `Subagent.can_cancel`.

## 9. ACP wire parsing moved into the session state module
`state/subagents.rs:24-98` parses raw ACP JSON itself:
- the `sessionUpdate` discriminator;
- `sessionId`, `title`, `description`, `capabilities.cancel`, `state.state` / `stopReason`;
- `recipientSessionId`, and the joining of content text.

ACP→contract conversion otherwise lives in `convert.rs`. `take_raw` also deserializes child updates into `acp::SessionUpdate` inside the state (`:33`), next to the adapter's typed handler. That is how the policy step in finding 1 gets skipped. The format now has two owners, and the dispatch is split between `adapter.rs` and `state`.

## 10. Copies of data that already lives somewhere
- `change_subagent` (`subagents.rs:156-178`) copies `Subagent.status` and `.title` into its item's status and title. That is the same fact in two records, kept in step by hand.
- For Antigravity, `description` is `first_line(prompt)` cut at 120 characters plus "…" in the adapter (`subagents.rs:130`, `:198`). That is a display cut decided on the server and stored as a second, truncated copy of `prompt`.
- The cockpit store's new `under: HashMap<ItemId, Vec<usize>>` (`store.rs:64`, `:234`) is a second index of the parent→child relation. The stream already keeps that relation (`store.rs` `place` / `descends`, about lines 300-335) and `chat::rows::nesting` reads it. `under` holds direct children only, so the line's "N steps" and the pop-up's step list miss nested items, while the stream folds every depth.

## 11. The cockpit's clock stamps a sub-agent's start and end
`store.rs:99-106` (`subagent_now`) sets `Seen.since` and `Seen.ended` to the cockpit's `now` when the event is applied. Part 2 item 1 removes exactly this (old activity looks new). On reconnect, `subagents_now` (`state.rs`, snapshot) replays every sub-agent, so `since` becomes the reconnect time and "how long it's run" starts again from zero. When a sub-agent started is the server's fact.

## 12. "Waiting on a sub-agent isn't quiet" is worked out in the chat alone
`chat/live.rs:38-45` adds `delegating` to the chat's own liveness logic. Part 2 item 12 asks for one answer about an agent's state in the agents service, read by every place that names an agent. The inbox, plan tree and pickers won't know an agent is waiting on its sub-agent.

## 13. Smaller
- **ff7e269:** which `At` places sit under something is written twice: `app/commands.rs:211` (`matches!(At::Item | At::Typing | At::Waiting)` → `Layer::On`) and `draw/host.rs:150-151` (the groups). A new place needs both edits.
- **e1a2167:** `QuickLook.opened` (`quick_look.rs:30-32`) is now dead: its only reader, the old click-outside rule in `look.rs`, was removed, but the field and its doc were kept. That isn't a clean cut.

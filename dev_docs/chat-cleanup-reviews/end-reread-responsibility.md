# End re-read: responsibility

Scope: `git diff 182025f..516ef76` (e43de96, 8bc4bca, 516ef76). Paths are relative to `meta-harness/`. Ranked, most important first.

## 1. Home mixes two clocks in one stream; the cockpit assumes one.

- The cockpit now estimates one skew per stream: `crates/cockpit-ui/src/agents.rs:243-256` resets it on `Resync` and keeps the least gap between "when it happened" and "when it came".
- Home's stream for a linked server carries two clocks. The linked server's events keep its stamps (`src/home/links.rs:200-203`, `publish_at`). The stream's own markers are stamped on home's clock: `Resync` and `Replayed` in `src/home/socket.rs:89-90`, and `Resync` again in `crates/harness-core/src/hub.rs:170` (`reset`). An event the linked server sent with no stamp is also given home's clock (`links.rs:201`).
- How it goes wrong: say the linked server's clock is behind home's by D. The home-stamped `Resync` gives the smallest gap, so it becomes the skew. Every event from that server then reads as D seconds old. "No word for N min" and every age are off by D. If the server is ahead, it happens to work.
- The coupling: the cockpit relies on a rule ("one stream, one clock") that the server's relay doesn't keep, and no contract states it. The relay should stamp everything on one clock, or the skew should be kept per origin.

## 2. "The server's now" is owned by the agents service, and only some views use it.

- `crates/cockpit-ui/src/agents.rs:281` (`server_now_ms`) is the corrected clock. Only the sub-agent strip uses it (`chat/subagents.rs:42, 101`).
- Other views still compare server stamps with the browser clock (`crate::attention::now_ms()`): notification ages (`inbox.rs:90`, `app/attention.rs:95`), a sign-in's time left (`chat.rs:467`, `until_ms`), change windows (`changes/mod.rs:779`), and tag ages (`tag_list.rs:193`). So two clocks now read server times, and which one a view uses depends on the view.
- The skew is a fact about the desk's event stream. It is read where the stream is unpacked (`app/lifecycle.rs:165`), but it's kept inside `Agents`. A view that needs server time has to go through the agents service.
- The shell's search throttle (`app/capture.rs:195`, `FOUND_AGAIN_MS`) uses `attention::now_ms()`, a wall clock that lives in the notifications module, instead of the frame's clock. The test has to sleep for real (`app/tests/chat_view.rs`, `thread::sleep(600ms)`).

## 3. A failure remembered only by this client overrides the server's word on the slot.

- `crates/cockpit-ui/src/agents.rs:366`: `state()` checks `cant_open` before anything the server says about the slot.
- Only this client's own `open()` clears it (`agents.rs:531`). No server event clears it. If the session starts from another device, or the server later reports the slot running, this client still says "can't run" everywhere the agent is named: the top bar, the inbox, discussions and the slot picker.
- It also duplicates `errors`. Both maps are written for the same failure at `agents.rs:521-522` and `:595-596`, with different wording ("No harness instance is available yet." and "no harness instance is available yet"). `PutErrorAway` clears one and leaves the other (`agents.rs:505`).

## 4. The chat bends its typing-box rect to steer where the host puts controls.

- `crates/cockpit-ui/src/chat/render.rs:276` stretches `ui.typing` up over the sub-agent strip. `typing_rect` is meant to be "where its typing box is" (`hosted.rs:125`).
- The host anchors the box's controls to that rect. It places them below the rect, or above it when there's no room (`app/draw/host.rs:159, 211-213`). The chat moves the rect so the host's controls land above the strip.
- So the buffer encodes the host's placement rule, and the interface no longer means what its doc says. This is the same leak as the earlier `room_under_waiting` finding. A host that placed controls another way would cover the strip again, or leave a gap.

## 5. The chat decides a harness capability by itself.

- `crates/cockpit-ui/src/chat/actions.rs:174` always offers "Message the sub-agent" as unavailable, with the reason "Its harness doesn't let one sub-agent be messaged alone".
- What a harness can do with sub-agents is declared by the server: `SubagentCapabilities` (`crates/harness-contracts/src/capabilities.rs:97`). The chat already reads `lifecycle_events` from it, and stopping a sub-agent reads `can_cancel`. This one is hard-coded in the UI.
- `chat/actions.rs:412` is a `run` arm for an action that is never available.

## 6. Limits of the label format live apart from the code that writes it.

- `crates/harness-acp/src/pieces.rs:175` (`PIECES_MOST = 256`) says "no message is sent with more links than this". Nothing on the sending side enforces it. A message with more than 256 links is written, and on replay `Replay::read` returns `None` (`pieces.rs:218`), so the user's message comes back with the pasted content.
- `crates/harness-contracts/src/links.rs:127` (`LABEL_MOST = 16 KB`) caps what `read_label` reads. `Piece::label` (`links.rs:136`) in the same module can write a longer label: a thing's subject JSON plus one span per link. The reader then rejects it.
- The plan's rule is "one owner per format". Here the writer and the reader disagree on the format's limits, and one of the limits lives in another crate.

## 7. Row positions are kept twice and grown in step by hand.

- `crates/cockpit-ui/src/chat/render.rs:584-585`: each row whose height changed calls both `rows_at.grew` (`render.rs:64`) and `self.ui.scrolling.grew` (`viewport.rs:337`). These are two copies of the same tops: `RowsAt.tops/bottom` in the chat, and `Measured.stops` in the viewport. Both are built from one loop in `lay_out_rows` (`render.rs:637-640`).
- The two `grew` functions also behave differently. The viewport's clamps the height to zero and the chat's doesn't. Any future path that changes one copy and not the other leaves the scroll targets and the drawn rows out of step.

## 8. A search is kept under two different keys.

- `app/desk.rs:59-60`: the query (`searches`) and the misses (`missed`) are kept per `BufferKind`. `app/desk.rs:65`: what was found (`found`) is now kept per `BufferId` (`app/capture.rs:89, 93, 164, 191`).
- So a chat in a panel and the chat in the quick look share one query but keep separate matches. The key bar works out a count for a buffer that was never searched, using the other buffer's query. The fix moved one half of the state.

## 9. The agents service carries the UI's words and colors, and each caller still adds its own.

- `crates/cockpit-ui/src/agents.rs:81-87` (`way_out`) returns button captions ("Resume its last session", "Start a session"). `agents.rs:367, 376` put display prefixes ("couldn't start a session: ", "can't run: ") into `CantRun`'s string, so the state carries text instead of a reason. `agents.rs:75` returns an `egui::Color32`.
- Callers still write their own versions next to it. `chat/render.rs:388-389` writes its own no-session sentences. `chat/actions.rs:101` matches the `NoSession` variant and calls `way_out()` for the same fact. `attention.rs:61` picks which states to name.
- `Agents::changes()` (`agents.rs:275`) is a second name for what the other desk services call `version()` (`tags.rs:87`, `discussions.rs:33`). `nav::changes_of` (`nav.rs:503`) hashes parts into a change number, next to the `Changes` counter in the same file (`nav.rs:513`). Both are documented as "which change of what a view lists this is".

## 10. Smaller ones

- `app/going.rs:164`: the shell still names `Target::Discussion` to apply the "a panel shows discussions" exception. `chat/actions.rs:415` still names `BufferKind::Chat` in `Ask::Show` for "Where it started". Only one half of the earlier "shell names the buffer" finding was fixed.
- `app/look.rs:36-42`: Talk now drives the chat buffer's target on every call. The shell names `BufferKind::Chat` and the lead's target to undo the chat's sub-agent mode.
- A missing `at_ms` (0) means "now" in two consumers: `src/home/links.rs:200-203` and `crates/cockpit-ui/src/agents.rs:249`. Each applies its own clock. This is not decided once where `Stamped` is read.
- `crates/harness-acp/src/state.rs:193-194`: `replays_by` sets how replays are read, and it also turns on "a replay is under way" (`replaying`). That flag gates only sub-agent events (`state/subagents.rs:211, 237`).
- `tests/serve_tests.rs:176` now uses `nah_packs::sha256_hex`. The server's tests now depend on the packs crate for a general hash helper, which widens the earlier finding about that helper.

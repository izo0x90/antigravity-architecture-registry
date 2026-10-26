End review of the chat plan (d54286c..50a125f): responsibility.

Paths are relative to `meta-harness/`. Line numbers are as of 50a125f. Most serious first.

## 1. "As typed" is carried by a text marker, kept in the slot store by a hash, and stuck on by the event forwarder

- `crates/harness-contracts/src/links.rs:56` makes the prompt's wording (`HANDED_ON`, "What's linked above, as it is now:") a contract constant. Four parts now parse or test for that string:
  - `crates/nah-things/src/attach.rs:61` writes it;
  - `crates/harness-core/src/sessions.rs:44` sniffs for it;
  - `crates/cockpit-ui/src/conversation.rs:412` (`as_typed`) splits the sent text on it;
  - `crates/cockpit-ui/src/chat/rows.rs:129` (`folds`) tests for it.

  The words the agent is handed are now the cockpit's data format. Rewording the prompt breaks the chat's display and its folding.
- What the user typed is a property of the user-message item. It is kept somewhere else: in `SlotStore` (`crates/harness-core/src/slots.rs:44-46`, `crates/nah-state/src/slots.rs:108-160`), under a SHA-256 of the expanded text. Slot persistence now also stores message display data. Two messages that expand to the same text share one record.
- `SessionManager` sits below `SlotManager`, and it now calls back up into it. `modules.rs:333` hands it a late-bound closure (`read_typed_with`, `sessions.rs:57`). Then `sessions.rs:82` patches every forwarded `ItemUpdated` event. The adapter's own `SessionState` never holds `typed`; only the copy published to the hub does. That lookup is a blocking SQLite read inside the async event-forwarding task.
- The keep rule and the lookup rule are two separate tests:
  - it is kept when `text != typed.text` (`slots.rs:596`);
  - it is looked up only when the text contains the marker (`sessions.rs:44`).

  A message with links but nothing handed on (a pointer link, or nothing the slot may read) is never kept. Its chips are lost.

## 2. A second diff engine in the cockpit

- `crates/cockpit-ui/src/diff.rs:66-95` (`hunks`, on the new `similar` dependency, `crates/cockpit-ui/Cargo.toml:31`) builds `Hunk`s on the client. The chat calls it at `crates/cockpit-ui/src/chat/render.rs:474-485`.
- The Changes view's hunks come from the server, through git (`crates/nah-changes/src/compare.rs:191`). The plan says "the Changes view's diff, shown folded; no second diff".
- The two engines can split and number the same change differently. A call's "+12 −3" can then disagree with what `g x` shows in the Changes view for that same file.

## 3. The highlighter is changed by reading it, beside `Ask`

- `crates/cockpit-ui/src/highlighter.rs:38` and `:49-66`: `Highlighter::colours(&self)` pushes network requests onto a `RefCell` queue. `tick` (`:77`) sends them.
- Buffers reach it through `Reader<Highlighter>` (`conversation.rs` `Messages`). So any buffer that draws a code block changes a desk service just by reading it.
- `crates/cockpit-ui/src/shared.rs:6` and `:45` say a shared service is changed only by the shell, and buffers change it only by asking. `Ask` already has a variant for each service with work to do (`Agents`, `Terminals`, `Tags`, `nav.rs:73-107`). This is a second request path. It is the only `Held` service with interior mutability on its read side.

## 4. A second file-and-line finder

- `crates/cockpit-ui/src/conversation.rs:237-275` (`places`, `is_path`) finds `path:line` in a message's words. `crates/cockpit-ui/src/terminal/links.rs:28-90` (`links`, `link`, `has_extension`) already does this for terminal lines.
- The rules differ:
  - the terminal takes a bare `Cargo.toml`; the chat does not;
  - the terminal maps an absolute path inside the project to a project path; the chat drops every absolute path;
  - the punctuation that ends a word is a different list in each.
- The same text gives different links in the chat and in the terminal.

## 5. A sub-agent's parent is ambient session state, so items made outside `apply` inherit it

- `crates/harness-acp/src/state.rs:323`: every new item takes `self.parent`. That field is set only in `apply()` (`:373-376`), from the last ACP update.
- Some items are made outside `apply()`:
  - the user's prompt (`begin_turn`, `:162`);
  - a steered message (`inject`, `:174`);
  - a request's item (`:622`, from the permission call).

  These items get whatever parent the last update left.
- Example: a main-agent request, or your steering message, that comes right after a sub-agent's update is nested under the sub-agent's Task call. The parent should be decided per update and passed in, not held as a mode.

## 6. "Waits on you" is decided in two places

- `crates/cockpit-ui/src/agents.rs:164` (`Agents::waits`) counts an answer on its way. The chat uses it (`crates/cockpit-ui/src/chat.rs:279`).
- `crates/cockpit-ui/src/request_card/answers.rs:66-67` (`answers`) checks only `status == Pending`. The inbox uses only that, and always holds the keys (`crates/cockpit-ui/src/inbox.rs:363`, `keyed: true`).
- After you answer in the inbox, the chat has moved on, but the inbox still offers that request's answers and their keys until the server's echo arrives. A second `y` there posts a second answer. The answering set belongs in the one place both offer answers from (`request_card::offer`), or both should ask `Agents::waits`.

## 7. The chat adds its own copy action

- `crates/cockpit-ui/src/chat/actions.rs:278` and `:485-490` (`chat.copy`, its own "Copied … (n lines)" note) make a third view-owned copy, after `code.copy` (`code.rs:941`) and `terminal.copy` (`terminal/mod.rs:748`).
- The plan says code blocks use "the copy mechanism decided in the selection TODO (one)". `TODO.md:24-27` records that mechanism as not decided yet. Each view keeps its own copy instead.

## 8. The chat's two settings have two owners

- The `chat.verbosity` and `chat.indicator` actions set the view's own state (`chat/actions.rs`).
- `crates/cockpit-ui/src/chat/nav.rs:169-175` writes `[chat] shows` and `indicator` from the UI config back over that state. It does this whenever `Settings` changes, including a change to only the code, graph or changes settings (one `Kept<Settings>`).
- A config reload throws away what the user picked in the chat.

## 9. A sent message's chips have their own colour rule

- `crates/cockpit-ui/src/conversation.rs:76` (`Messages::draw`) colours every chip `accent`.
- The typing box's chips get the tag's own colour from `Tags::link_colour` (`crates/cockpit-ui/src/app/linking.rs:228`).
- The same link is one colour while typed and another once sent.

## 10. The agents service carries the socket's state

- `crates/cockpit-ui/src/agents.rs:403` reads the `NetClient`'s public `ws_state` mutex in `tick` and passes it on as `Agents::connected()`. The top bar reads the same field directly (`app/draw/top_bar.rs:190`).
- Connection state now lives in two places, and the agents service owns a network concern. Any other buffer that needs it has to go through the agents service.

## 11. The shared config names a buffer's type

- `crates/cockpit-ui/src/config.rs:131` and `:140`: `ChatUi.shows` is `crate::chat::Verbosity`, defined in `chat/rows.rs`. The shared config and `Settings` (`shared.rs`) now depend on the chat's internals.
- Every other view's settings types are defined in `config.rs` itself (`ChangesUi`, `CodeUi`).

## 12. The entity store guesses which events are replays

- `crates/cockpit-ui/src/store.rs:17`, `:205` and `:256` (`REPLAY_S`, `before`, `replayed`): after a `Resync`, the store keeps a copy of every thread for 10 s. It compares new events against that copy to decide what is "news".
- The store's job is "latest of every entity" (its header). It now also holds the live line's rules for what counts as activity: a queued turn and an answer's echo are not news (`:282`, `:301`).
- That rule belongs with the live line. Whether an event is a replay is something the server knows.

End review of the chat plan (d54286c..50a125f): permissions.

Checked and found sound:
- The one new route, `POST /api/code/highlight` (src/arch.rs:145), takes `Needs<can::ViewRead>` first, as `/api/code/analysis` does. It reads no file. It only parses the text it is sent with a loaded pack. It caps the text at 256 KB.
- No agent tool was added or changed (nothing under src/mcp).
- `typed_messages` (crates/nah-state/src/migrations.rs:65) is written only from `SlotManager::send` with `Origin::User`. That is reached only from the prompt route, which takes `Needs<can::SessionsControl>` (src/routes/slots.rs:199). No route or agent tool reads the table back. It lives in `.nah/state/`, which `private_paths` keeps from agents' file access.
- `typed` reaches only the event stream (ticketed socket, home relay). Agent-facing readers of the hub (chats.rs `turn_text`, `turn_reply`, nah-notify `reply`) read `text`/`said()`, never `typed`. A lookup needs the exact sent text, so an agent-written ticket brief cannot make the cockpit show different words as "yours".

Findings, most serious first:

1. A sub-agent's request can take the approval meant for another request.
   - Where: crates/cockpit-ui/src/store.rs:158-167, crates/cockpit-ui/src/chat.rs:92 and 290, crates/cockpit-ui/src/chat/actions.rs:268-269, crates/cockpit-ui/src/request_card/answers.rs:104, crates/harness-acp/src/state.rs:323 and 622.
   - Who: any agent that runs sub-agents (Claude Code's Task).
   - How: Sub-agent items are now put into the middle of the stream, right after their Task call's other items. Before this change, new items only went at the end. The chat cursor is a bare stream index (`ui.cursor: Option<usize>`). Nothing moves it when an item is put in before it. Say the user is on request A ("Allow: edit main.rs"), and A sits right after a Task's items. A sub-agent request B then arrives. B goes in at A's index, so the cursor is now on B, and `y` (or Enter on the item) answers B. With no cursor, the keys go to the first pending request in stream order (chat.rs:290). A request under an earlier Task outranks one that came first and was already shown, so `y` moves to it the moment it arrives.
   - Also: `open_request` (state.rs:622) gives the request item `self.parent`. That is whatever the last applied update's `_meta` named. So a request from the main agent that follows a sub-agent update is nested under, and ordered as, that sub-agent's Task.
   - Effect: the user approves a command they did not pick.

2. Agent output drives unbounded highlight calls to the server under the user's credentials.
   - Where: crates/cockpit-ui/src/highlighter.rs:49-66 and 85-96, crates/cockpit-ui/src/conversation.rs:144-149 and 174, src/arch.rs:141-148.
   - Who: any agent whose chat is open in any tab.
   - How: An unclosed fence counts as a code block (conversation.rs:174). Its text changes with every streamed chunk. The cache key is a hash of fence plus text, so each chunk is a new key. `tick` then POSTs the whole block so far to `/api/code/highlight`, once per changed frame, with no rate limit and no skipping of streaming messages. A 200 KB block streamed in 2,000 chunks sends about 2,000 requests averaging 100 KB, so roughly 200 MB is parsed on the server's blocking pool, per open tab. Past 1,024 blocks the cache is cleared (highlighter.rs:87), and everything on screen is asked for again. The route has no per-client bound on concurrent parses.
   - Effect: an agent can tie up the shared blocking pool (arch queries, SQLite) just by writing a long code block.

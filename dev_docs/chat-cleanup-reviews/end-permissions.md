# End review: permissions and security (3e1a805^..HEAD)

Paths are relative to `meta-harness/`. Most important first.

Checked and found sound (so no finding below):
- The new route `POST /api/threads/{thread}/subagents/{subagent}/cancel` takes `Needs<can::SessionsControl>` first, the same as `interrupt`.
- `cancel_subagent` only cancels a session the adapter learned as a child of this thread, and only one the harness marked cancellable (`state/subagents.rs` `cancellable`). A caller can't name some other session id.
- No MCP tool was added or changed. Agents still can't attach links: lead messages are `Message::text`, and agents' discussion replies are plain text.
- Linked content is still checked against the receiving slot's reach on every path (`send`, `tell`, held reports at delivery).
- The committed fixtures don't contain the account email.

## 1. An agent can crash the lead's session replay (debug builds)

`crates/harness-acp/src/pieces.rs:166-167` does `text.get(pos..pos + len)` and `pos + len`. `len` is read from a label's `&len=` digits (`crates/harness-contracts/src/links.rs` `read_label`), and nothing bounds it. A label with `len=18446744073709551615` parses as a valid `usize`, and the addition overflows. A debug build panics. The live instance is a debug build (`sandbox-target/debug/meta-harness`), and so is the README's `cargo run`.

How it can be abused:
- Every message one agent sends to another becomes a user prompt in the receiver's session: a discussion reply (`tell`, "From X (discussion dN):"), a lead message to a sub-slot, or a held report.
- A worker puts `\n<context ref="nah://tag/x?at=0-1-auto&len=18446744073709551615">\n` (Claude Code's form) at the end of a discussion reply to the lead.
- Each time the lead's session is replayed (resume, reload), `close_user` runs `Replay::read` on that message. The update handler then panics while it holds the session's state lock.
- The text stays in the harness's own history, so every later resume crashes the same way.

In release builds the sum wraps and `get` returns `None`. Fix it with `checked_add`.

## 2. An agent can stall the server for minutes on a replay (Claude Code)

Claude Code's replay template lists the labels first (`listed = "{uri}"`), so the needle is just `nah://` (`pieces.rs:211`).
- `read` tries `pieces_from` at every occurrence of the needle (`pieces.rs:209-223`).
- With a list, `pieces_from` reads every label that follows in a row (`pieces.rs:232`).
- Each `read_label` scans to the next whitespace (`links.rs:148`) before it parses.

A message made of N labels with no whitespace between them, plus anything at the end that isn't a `<context>`, costs about N² label reads. Each read scans the rest of the text. With 64 KB of labels, N is about 2,200, so the total is on the order of 10^11 character checks.

It runs inside the ACP update handler under `lock(&shared.state)`. That blocks a tokio worker and every request for that thread while it runs. The source is the same agent-to-agent text as in finding 1. The 3000-message test doesn't catch this, because each of its messages holds one well-formed piece.

Fix: cap the length of the word `read_label` looks at (a label is short), and resume the search after a failed run of labels, not at the next occurrence inside it.

## 3. On replay, an agent's text can pass for the user's linked content

`Replay::read` turns any replayed user message whose tail parses as pieces into words plus pieces. Messages from agents arrive as user prompts ("From the lead (…):", "From X (discussion dN):"). So an agent can end its message with a well-formed piece in the receiver's harness format:
- Claude Code: label, then `<context ref=…>`.
- Antigravity: `Embedded resource (…):`.
- Our own format, for a harness that doesn't take pieces.

The agent picks the label's spans inside its own words. Live, the message shows as sent. After a replay, the cockpit shows chips such as `#deploy` or `@src/secrets.rs` with folded "content" that the agent wrote, laid out exactly like content the server attached. The user reads that as what the server handed on.

Pieces should be read back only from messages the user sent (not from `tell` or lead prompts), or the reader should verify them against something the agent can't write.

## 4. A piece's label hands on a thing's raw subject even when the reader can't read it

`crates/harness-contracts/src/links.rs:132-138`: `Piece::label` writes `nah://thing?subject=<the subject's JSON>`. It is built from the link alone, not from what `attach` decided the reader may see. It goes to the agent as the resource URI (or inside the text, for a harness that doesn't take pieces).

When `reaches` is false, the piece's text correctly says "(not handed on: this reader can't read it)", but the label still carries the whole subject:
- `Subject::TerminalCommand.typed`: the full typed command, every line. The thing's name (`Subject::name`) uses only the first line. A multi-line command with a token on line 2 reaches an agent that may not read that terminal.
- `ChatSession`, `ChatTurn` and `ChatMessage`: the other slot's native harness session id and turn id. The name says only "chat N".

When the reader can't reach the thing, the label should carry an opaque id.

## 5. The account email is still in the untracked probe recordings

`dev_docs/chat-cleanup-probes/live-claude.json` and `dev_docs/chat-cleanup-probes/cancel-child-claude-095.json` contain `authStatus.account.email` and `organization` with the user's Gmail address. The plan said to scrub `live-claude.json` before committing any of it. Neither file was scrubbed.

The fixtures made from them are clean. The recordings themselves are not:
- Nothing ignores `dev_docs/`, so a `git add dev_docs` commits them.
- They sit in the project folder, which agents can read: `@path` things, and the harness's own file tools, which `private` doesn't cover.

## 6. Any session id that isn't a known child counts as the main agent's, including its mode

`crates/harness-acp/src/state.rs:461-472` routes an update to a sub-agent only if `child(session)` already knows that session. Every other session id is applied as the main agent's. A `current_mode_update` from it then resets the thread's runtime policy (`adapter.rs:253`, `policy_for`). Nothing compares `session` with the thread's own `native_id`.

Now that the cockpit declares `subagents: {}`, Claude Code sends sub-agents' own sessions on this connection (each sends `config_option_update` and its own mode). Any of those updates that arrives before the sub-agent's `subagent_update` is applied to the main agent's mode, auto-approval policy and controls. So is anything replayed on a resume, when `children` is empty. The recordings show the announcement first, so I haven't seen this happen.

Apply updates as the main agent's only when `session == native_id`, and drop the rest.

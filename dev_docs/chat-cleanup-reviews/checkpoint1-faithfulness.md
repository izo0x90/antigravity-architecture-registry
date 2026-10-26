# Checkpoint 1: faithfulness review of fca4ad8 (Part 1, plus Part 2 item 10)

Reviewer brief: faithfulness to `chat-cleanup-plan.md` (the rule, Why, Part 1,
Part 2 item 10). Read-only; no cargo run. Paths below are relative to
`meta-harness/` at commit fca4ad8. Ranked most important first.

## 1. Claude Code's pack rule assumes one piece per message. A message linking two or more things does not read back on replay.

- Plan, "Building it / Replay": "Each harness pack declares how its replay marks an attachment (Claude Code: a label piece then `<context ref="…">`) … The adapter reads a replayed message once by that rule and our labels." The rule: "Checked at scale … not one message with one link."
- Code: `packs/harness/claude-code/pack.toml` (the `replayed_piece` line added at ~38) is `"{uri}\n<context ref=\"{uri}\">\n{content}\n</context>"`. That template expects each label to be followed directly by its own context.
- What claude-agent-acp actually does (`~/.local/share/nah/packs/claude-code/1.1.0/node_modules/@agentclientprotocol/claude-agent-acp/dist/acp-agent.js`, `promptToClaude`, ~9925-9990): it pushes every resource's label into `content` and every `<context …>` into a separate `context` array, then `content.push(...context)`. So two pieces replay as `words L1 L2 \n<context ref="L1">…</context>\n<context ref="L2">…</context>`.
- `crates/harness-acp/src/pieces.rs` `Template::head_at` (around lines 205-230) reads `L1L2` as one label. It then finds that this doesn't equal the `ref`, returns None, and `read` (line 235) returns None. The message is shown as it came, with the pasted contexts in its text: the old behaviour the plan replaces.
- The tests don't catch it. `a_replayed_message_reads_back_to_its_text_and_pieces_whoever_wrote_it` (pieces.rs:310-315) builds the "Claude" replay with `template.write` per piece, which interleaves label and context. The state test (`state.rs:935`) uses one piece. The only real recordings have one link. The builder's live check (findings, Step 2) also used one link.

## 2. "Nothing scans a whole session when it's replayed": every replayed message is read back eagerly in the adapter, and the test asserts the opposite of the planned one

- Plan, "The rule": "Nothing scans a whole session when it's replayed; work happens only for what's shown, once, and is remembered." Plan, Tests: "A replay of a session with thousands of linked messages, which does no per-message work until a message is drawn." Plan, fallback: "The reader only runs for a message on screen, once."
- Code: `crates/harness-acp/src/state.rs:176` `close_user` runs `Template::read` over the whole raw text of every replayed user message. It is called from `push_item_in` (state.rs:356) and `replayed()` (state.rs:170), during replay, whether or not the message is ever drawn. This applies to the fallback path too: `profile.rs:167` `replayed_pieces` returns `Template::ours()` for a harness without pieces, and the adapter reads it the same eager way.
- Each chunk of a replayed user message also emits a full `ItemUpdated` with the growing raw text, pasted content included (state.rs:503-509, via `update_item`, which clones and emits). The read-back on close then emits it again. Per replayed linked message that's 3+1 full-item events for Claude Code. The raw text is also held twice while the message is open (`open_user` raw plus the item's text).
- Test: `state.rs:964` `a_long_replay_reads_each_message_back_once` asserts that all 3000 are read back during the replay (`read_back == 3000`), with a wall-clock bound (state.rs:980). The planned test was the reverse: no per-message work until drawn.
- The builder's findings admit this as a deviation ("read back by the adapter as it arrives … not deferred until it's drawn"). The plan's own Replay bullet ("the adapter reads a replayed message once") conflicts with its rule and Tests bullet. Not resolved in the plan or put under "Decide".

## 3. "Showing it": the chat neither matches chips to pieces nor remembers anything per message

- Plan: "The chat matches chips to pieces once, when the message is first drawn, and remembers the result for that message."
- Code: the matching moved to the adapter. Links are decoded from labels in `pieces.rs:48` `links_of` and flattened by `harness-contracts/src/links.rs:118` `Piece::split`. The chat draws `links` straight from the item (`crates/cockpit-ui/src/chat/render.rs:649-650`). `crates/cockpit-ui/src/conversation.rs:84-86` `Messages::draw` rebuilds the `Vec<Chip>` and the chipped galley every frame. Nothing is remembered per message.
- The builder's findings admit the "where" part only under the eager-reading deviation, not the absence of per-message memoising in the chat.

## 4. The fallback format's owner is not where the plan put it

- Plan: "A harness that doesn't accept pieces. It gets one text, in a fixed, labeled format written and read by one module (in `nah-things`, next to the code that pastes it)."
- Code: the format (`OURS`, pieces.rs:18), its writer (`blocks`, pieces.rs:82, with `takes_pieces == false`) and its reader (`Template::read`) are in `crates/harness-acp/src/pieces.rs`. `crates/nah-things/src/attach.rs:63` `attach` only produces each piece's content.
- Admitted in the findings as a deviation. Not put to the user under "Decide".

## 5. The item does not carry "its pieces"; the link-to-piece association is dropped

- Plan, Replay: "The `UserMessage` item then carries your words and its pieces separately." The approach: each `#name` / `@path` becomes a chip "by matching it to the piece that names it".
- Code: `harness-contracts/src/items.rs:39` `UserMessage { text, links, handed_on: Vec<String> }`. `Piece::split` (links.rs:118) merges all pieces' links into one sorted list and keeps only their texts. Which chip goes with which folded text is lost, both live (`state.rs` `yours`) and on replay (`close_user`).

## 6. Planned round-trip test (send, replay, show, for both paths) is not one test; "show" is never fed from a replay

- Plan, Tests: "A round trip: send, replay, show, for both the pieces path and the fallback text path."
- Code: the parts are tested separately.
  - Sending is covered by `pieces.rs:348`. For the pieces path it checks only the blocks, with no replay.
  - Replay through `SessionState` is covered by `state.rs:935`, for the Claude and Antigravity templates only. The fallback (`Template::ours`) is never replayed through `SessionState`, and `Profile::replayed_pieces` (profile.rs:167) has no test.
  - Showing is covered by `crates/cockpit-ui/src/app/tests/chat_view.rs:617`, on a hand-built item.
- No test takes `blocks()` output through a replay into an item and then into the chat.

## 7. The "recorded replays" in the test are not the recordings

- Plan, Status: probe recordings "become test fixtures". Plan, checks 3-4 established only that the plain label `nah://tag/probe-note` survives.
- Code: `pieces.rs:321` and `pieces.rs:324` are labelled "The recorded replays themselves", but they are hand-edited strings. The label was rewritten to `nah://tag/probe-note?at=8-19&include=auto`, and Claude Code's text was collapsed into one string; it was really three chunks, the first ending "…Answer with the word only." (see `dev_docs/chat-cleanup-probes/probe-claude-095.json`).
- No fixture from `chat-cleanup-probes/` is used.
- The new label form (query string; for things, URL-encoded JSON in `subject=`) was never probed against Antigravity. Claude Code's connector inserts the uri unescaped, which is fine for Claude.

## 8. README still describes the removed table

- Plan: "Removed, completely: the `typed_messages` table … `AsTyped`…". The clean-cut rule: what's replaced is deleted in the same change.
- Code: `README.md:377-380` still says "Your messages show as you typed them … the server keeps what was typed by the text the agent was handed, so a session's replayed history shows it too."

## 9. Built without being asked by the plan

These are recorded in the commit message and the findings, but they are not in Part 1:
- A new persisted column, `held_reports.links`, in migration 16 (`crates/nah-state/src/migrations.rs:69-70`, `crates/nah-state/src/slots.rs:122-134`; `HeldReport.links` at `crates/harness-core/src/slots.rs:53`). Held messages now store links and are expanded at delivery, not when held. For a discussion message, this keeps a second copy of the user's `Said` text and links, which the discussion already persists (`src/discussing.rs:43` `news`). That runs against "No side copies of data that already lives somewhere". The text was already copied before this change; the links are new.
- `Message::after` / `Message::joined` (`harness-contracts/src/links.rs:142`, `:148`) and the discussion link fix (`src/discussing.rs:43`).
- `linking.rs` dedupe now keys on `link.to` instead of the heading.

## Checked and found as planned

- "Removed, completely": the `typed_messages` table is dropped by migration 16. Also gone: `keep_typed`, `typed_kept`, `sent_key`, `TYPED_KEPT`, the in-memory `typed` map, `read_typed_with`, `Typed`, `typed_wanted`, `with_typed`, `AsTyped`, `TurnInput.typed`, `as_typed`, the old tests, and harness-core's `sha2` dependency. `git grep` at fca4ad8 finds none of these outside the historical migration 15 and the README (item 8).
- Part 2 item 10: `src/modules.rs` no longer wires `SessionManager` to the slots. `crates/harness-core/src/sessions.rs` has no call into the slots.
- Sending: `nah-things` `attach` returns pieces. The session sends them as embedded resources when `embeddedContext` is declared (`profile.rs`, `Declared.pieces`; `session.rs`, `crate::pieces::blocks`).
- The fallback format's writer and reader are tested together (`pieces.rs:348`, and the "ours" case at `pieces.rs:310`). The label's writer and reader are tested together (`a_label_says_what_its_piece_links_and_where`).

## Already admitted in `chat-cleanup-findings.md` (read after writing the above)

- Item 2: admitted ("Deviation: a replayed message is read back by the adapter as it arrives … not deferred until it's drawn").
- Item 4: admitted ("Deviation: the fallback format's owner is `harness-acp` … not `nah-things`").
- Items 1, 3 (the missing per-message memo), 5, 6, 7 and 8: not mentioned. The findings say Claude Code was "checked live" with a single-link message, and that Antigravity was "checked against its recorded replay in tests only", although that replay was hand-edited (item 7).
- Item 9: described as "found and fixed on the way", not as an unplanned addition.

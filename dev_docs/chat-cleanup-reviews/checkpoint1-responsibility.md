# Checkpoint 1 review: responsibility and coupling (commit fca4ad8)

Ranked, most important first.

## 1. The "whole if either link asks" rule is decided twice, and the label drops each link's own include, so a replayed message's links differ from the live ones

- `meta-harness/src/linking.rs:826-830` merges the links to the same tag or thing. It sets `Attached.include = Whole` if any link asks for whole, and keeps each link with its own `include`.
- `crates/nah-things/src/attach.rs:74,133` builds the `Piece` from `a.links` and drops `Attached.include`, the include that was actually used. So the effective include isn't in the piece.
- `crates/harness-acp/src/pieces.rs:26` works the same rule out again from `piece.links` to write the label. The label holds one `include` for all the spans.
- `pieces::links_of` (`pieces.rs:64-75`) gives every span that one include on read-back.

Result: the live item (`state.rs:184-187`, via `Piece::split`) has per-link includes, but the item rebuilt from the replay has the merged one. The cockpit draws include on each chip (`crates/cockpit-ui/src/conversation.rs:359-363`: background for whole, underline for pointer), so a chip changes look after a replay. The round-trip test (`pieces.rs` `sent()`) misses this because both links to `#flaky` are `Auto`.

What it couples: one policy is written in two crates (server `linking.rs`, adapter `pieces.rs`), and the adapter re-decides something the server had already decided.

## 2. The label is a persisted encoding of contracts types, owned by the ACP adapter and not by the types' owner

`crates/harness-acp/src/pieces.rs:22-77` writes and reads `Link`, `Linked`, `Include` and `Subject`. All four are `harness-contracts` types (`links.rs:10-41`). The encoding mixes three approaches:

- `Linked` variants are hand-coded (`tag/`, `thing`). A new `Linked` variant means editing the ACP adapter.
- `Include` words are hand-coded in `include_word` and its parse (`pieces.rs:39-45`, `64-69`). This is a second serialization beside `Include`'s own serde `snake_case` (`links.rs:31-33`).
- `Subject` goes through `serde_json` (`pieces.rs:33`, `61`).

These labels end up in each harness's own session history, outside NAH's control and kept for good. So `Subject`'s serde derive in `harness-contracts` has silently become part of a persisted wire format. Renaming a `Subject` field or variant breaks read-back of every older session, and nothing in `harness-contracts` tests it.

This breaks "one owner per format": the type's owner and the format's owner are different crates. The label for a `Piece` belongs next to `Link`/`Piece` in `harness-contracts/src/links.rs`, with its round-trip test there. `harness-acp` would keep only `Template`, which is how a harness wraps a label and content.

## 3. nah-things now carries message spans it never reads, and builds the transport type

- `crates/nah-things/src/attach.rs:26` adds `pub links: Vec<Link>` to `Attached`. `attach` never reads it; it only moves it into `Piece { links: a.links, .. }` (`attach.rs:74,133`). It returns `harness_contracts::Piece`, a prompt-transport type.
- So every producer of an `Attached` has to fill in a field that isn't its business. `crates/nah-tags/src/lib.rs:415-416` writes `links: Vec::new()` with the comment "its caller's to give", and `linking.rs:842,851` does the same before pushing the link at `:860`.

The layer that reads things through their owners is now coupled to message span bookkeeping. `linking.rs` owns both the links and the grouping. `attach` could return one text per `Attached` (in order), and `linking`/`modules.rs` would pair them with their links.

## 4. The message item splits the pieces into two unrelated lists, and turning a turn's input into an item is written twice

- `ItemBody::UserMessage` (`crates/harness-contracts/src/items.rs:40-44`) has `links: Vec<Link>` and `handed_on: Vec<String>`, built by `Piece::split` (`links.rs:118-128`). `split` sorts the links and keeps the texts in a separate list, so the item no longer says which text a link handed on.
- The contract already has `Piece`, which holds exactly that pairing. The item flattens it, and the cockpit can't tell which folded text belongs to which chip.
- Building the `UserMessage` body from a `TurnInput` is written in the adapter (`crates/harness-acp/src/state.rs:184-187`, `SessionState::yours`). It is written again in the fake harness (`meta-harness/tests/common/mod.rs:122`). So the integration tests check a copy of the adapter's mapping, not the mapping itself.

## 5. The packs' replay templates are copied into tests as literals

Where each harness writes a piece is a pack fact (`packs/harness/claude-code/pack.toml:38`, `packs/harness/antigravity-acp/pack.toml:107`). The same strings are hard-coded in three tests:

- `crates/harness-acp/src/pieces.rs` tests: `const CLAUDE`, `const ANTIGRAVITY`.
- `crates/harness-acp/src/state.rs`, in `a_replayed_message_of_yours_comes_back_to_its_text_and_pieces`.
- `crates/harness-acp/src/state.rs`, in `a_long_replay_reads_each_message_back_once`.

The "recorded replays" test checks a copy, not the pack. Editing a pack's `replayed_piece` would fail no test. The tests should read the template from the pack settings, as the profile does.

## 6. The same fact is held twice inside the adapter

- **Whether the harness takes pieces:** `Link.takes_pieces` (`crates/harness-acp/src/session.rs:74`, set at `adapter.rs:139`) is a copy of `AcpSession.declared.pieces` (`adapter.rs:140`). Two fields on the same session say one thing.
- **The replay template is parsed twice:** once to validate it (`settings.rs:85-87`) and again per connection in `Profile::replayed_pieces` (`profile.rs:169`). The second parse uses `.ok()`, so a parse failure there would quietly show messages raw instead of failing where settings are loaded. The parsed `Template` should be held in the settings.

## 7. A replayed message's text is kept twice while it arrives, and its unread form goes out to every client

`crates/harness-acp/src/state.rs:48,503-508` appends each chunk both to `open_user.1` and to the item's own `text`, so there are two copies of one message. On top of that, `update_item` publishes an `ItemUpdated` for every chunk. That update has the raw text, with each piece's full content (up to 16 KB) inlined as words. `close_user` then sends a rewritten one.

The cockpit and every other subscriber get a form the adapter already knows is wrong, followed by a correction. The adapter should hold the raw text alone and push the item once, when it's read back. That keeps one copy and one event per message.

## 8. `pieces::blocks` also builds the rest of the prompt

`crates/harness-acp/src/pieces.rs:82-108` now turns attachments (files, images) into ACP blocks too. That code moved here from `session.rs`'s `prompt_blocks`. The module's job, by its own doc, is the pieces format. Building the whole prompt is a second job, and image/file handling has nothing to do with pieces. The conversion of a whole `TurnInput` belongs in `session.rs`, which would call `pieces` for the pieces part.

## 9. One result has two shapes, and a held report re-spells `Message`

- `Expand` returns `(Vec<Piece>, Vec<String>)` (`crates/harness-core/src/slots.rs:202`, and `Things::attach` at `attach.rs:63`). `SlotManager::expanded` wraps the same result into `harness_contracts::Expanded` (`links.rs:134`).
- `Expanded` sits in the crate shared with the cockpit, but only the server uses it (`slots.rs:569-579`, `src/routes/slots.rs:193`).
- `HeldReport` gets `text` + `links` (`slots.rs:51-53`), which is `Message` field for field. It is reassembled at `slots.rs:663` (`Message { text: r.text.clone(), links: r.links.clone() }`). It should hold a `message: Message`.

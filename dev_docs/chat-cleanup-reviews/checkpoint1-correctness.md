# Checkpoint 1 (fca4ad8): correctness review

Read-only review. I checked the findings below by reading the code and the installed claude-agent-acp 0.95.0 source (`~/.local/share/nah/packs/claude-code/1.1.0/node_modules/@agentclientprotocol/claude-agent-acp/dist/acp-agent.js`). I also ran a line-for-line Python port of `Template::read` / `next_piece` / `head_at` / `links_of` against the sequences below. Most important first.

## 1. Claude Code: a message linking two or more different things never reads back. It shows raw, with no chips.

`crates/harness-acp/src/pieces.rs:180-205` (`head_at`), `:210-235` (`read`); `packs/harness/claude-code/pack.toml:40`.

claude-agent-acp does not write each piece as `{uri}` followed by its `<context>`. In `promptToClaude` (acp-agent.js ~9923-9988), each embedded resource adds its URI text to `content` and its `\n<context ref="…">…</context>` block to a separate `context` array. That array is added after everything else (`content.push(...context)`). The message is stored and replayed as: words, L1, L2, …, ctx1, ctx2, …. The one-piece probe can't show this; two pieces do.

Sequence:
1. The user sends `see #a and #b` (two different tags). That is two pieces with labels L1 and L2.
2. The server restarts, or the slot reopens, so `session/load` replays the message. The chunks share one messageId: `see #a and #b`, `L1`, `L2`, `\n<context ref="L1">\nA\n</context>`, `\n<context ref="L2">\nB\n</context>`.
3. `close_user` runs `read`:
   - `next_piece` finds `nah://` at L1. In `head_at`, the first `{uri}` runs to the first `\n<context ref="`, so `this = "L1L2"` (no whitespace, so it is accepted).
   - The second `{uri}` is `L1`. It differs from `L1L2`, so the result is None.
   - L2 fails the same way (`L2` vs `L1`). Each `nah://` inside a `ref="…"` fails on whitespace or a missing terminator.
   - `read` returns None. I confirmed this with the port.
4. The item keeps the raw text: words, both labels and both full contents inline in the user's bubble, no links, nothing folded.

This is the common case: any message with two distinct `#tag`/`@thing` links, on the main harness. The same applies when a message has a file attachment (the `[@a.rs](file://…)` text lands between the labels and the contexts) or an image. Attachments aren't produced by the server today, though.

The tests miss it. `a_replayed_message_reads_back_to_its_text_and_pieces_whoever_wrote_it` (pieces.rs ~282) builds the "Claude" replay by calling `template.write` per piece in sequence (L1 ctx1 L2 ctx2), which Claude never sends. The `Template` model (one `{uri}`… `{content}` unit per piece) cannot express Claude's real layout (all labels, then all contexts), so a pack template change alone can't fix it.

## 2. Antigravity (no message ids): two of your messages in a row merge, then the merge is read as pieces. Words get hidden or chips land on the wrong words.

`crates/harness-acp/src/state.rs:486-516` (`user_chunk`: `same` is true when either id is None), `:176-181` (`close_user`).

Antigravity 1.4.3 sends no `messageId` (combined-agy-143.json, replay-agy.json). Two consecutive user messages with no agent item between them are appended into one `open_user` raw. This happens when a prompt is stopped before any answer, or when a turn fails on quota or an error before output. The case is the one named by `messages_replayed_one_after_the_other_are_told_apart_by_their_ids`. The merge existed before this commit, but then the merged text was shown whole. Now it is parsed.

Sequence A:
1. You send `see #a` (piece L1, at=4-6). The agent fails before saying anything.
2. You send `try again`. The agent answers.
3. On replay, raw = `see #aEmbedded resource (L1):\nA-content` + `try again`.
4. `read` returns words `see #a` and a piece whose text is `A-contenttry again`. Your second message vanishes from the stream into the first message's folded "what went with it", and the turn count drops by one.

Sequence B: you send `hello` (no links; stopped), then `see #a` (L1 at=4-6). On replay, raw = `hellosee #aEmbedded resource…`. The words are `hellosee #a` and the chip is drawn on chars 4-6 (`ose`), not `#a`. Both sequences confirmed with the port.

## 3. Marker text inside a piece's content or in your words makes phantom pieces. Words are cut and dropped, and chips point at tags the message never linked.

`pieces.rs:162-178` (`next_piece` searches the whole remaining text, piece contents included), `:221-226` (body cut at the next match; only `end > len` is checked).

Claude sequence (confirmed with the port):
1. A message from #1 is shown raw. You copy part of it (`L_old\n<context ref="L_old">\nOLD\n</context>`, L_old = `nah://tag/old?at=0-3…`).
2. You send `see #new: <pasted> thanks`, linking #new at 4-8.
3. On replay, `next_piece(text, 0)` matches the pasted label inside your words. The words become `see #new: `, and the pasted text plus ` thanks` disappear: the tail is trimmed off by `rfind("\n</context>")` before the real L_new.
4. You get a phantom piece `OLD` with a chip on chars 0-3 (`see`) linking tag `old`, plus the real piece.

Antigravity sequence (tail is empty, so any match splits the content):
1. You link `@lines` of a terminal that shows `cat dev_docs/chat-cleanup-probes/…` output or a test fixture containing `Embedded resource (nah://tag/x?at=0-2&include=auto):\n…`.
2. On replay, the real piece's text is cut at that line.
3. A phantom piece with a chip on chars 0-2 of your words, linking tag `x`, is added.

Nothing ties a match to the labels actually sent with the message. Any well-formed label whose spans fit the words is accepted.

## 4. The label keeps one `include` per piece, so the round trip changes links (live and replayed chips differ).

`pieces.rs:22-27` (`label`: `include` is Whole if any link is Whole, else the first link's), `:66-72` (`links_of` gives that one value to every span).

Sequence:
1. You type `#flaky … #flaky`, the first chip set to Pointer and the second left Auto. `linking::attached` groups them into one piece.
2. Live, the item has two links (Pointer, Auto): one underlined chip and one plain.
3. After a replay the label says `include=pointer`, so both links come back as Pointer and both are underlined. Auto plus Whole comes back as Whole for both (background on both).

The "one test of the round trip" (`a_label_says_what_its_piece_links_and_where`) only uses links with the same include, so it passes.

## 5. Every replayed message is scanned, and the scan is quadratic in the message's length.

`pieces.rs:166-176` together with `:188-190`. For each `nah://` occurrence in the text (Claude's needle is just `nah://`), `head_at` runs `find("\n<context ref=\"")` from that point to the end of the text before checking for whitespace. For a message with no pieces the find fails every time, so the cost is (number of occurrences) × (text length).

Example: you paste a 1 MB ACP trace or log containing about 10k `nah://` URIs. Replaying that session does about 10^10 byte comparisons inside `close_user`, holding the session state mutex, so every update and request on that session waits.

`read` runs on every replayed user message (not only on-screen ones), once per message. That fits "once", but not "only for what's shown". The 3000-message test uses Antigravity's needle and no stray occurrences, so it doesn't exercise this.

## 6. Migration 16 makes every earlier linked message in existing sessions show its pasted contents inline.

`crates/nah-state/src/migrations.rs:69`.

Before this commit, a linked message was sent as its words + `\n\n---\nWhat's linked above, as it is now:` + contents. It was shown "as typed" only through `typed_messages`. Migration 16 drops that table, and no reader exists for the old pasted format (`Template::ours()` uses `--- Linked (…)`).

Sequence: upgrade, then open any existing session that has linked messages. Every such message now shows the full pasted tag and thing contents in your bubble, with no chips. The drop is irreversible, so the data needed to show them as typed is gone.

## 7. The "recorded replays" in the test are not the recordings, so the label format is unverified on Antigravity.

`pieces.rs` ~295-299. The test says "The recorded replays themselves", but both strings were edited. The real recordings (combined-agy-143.json, probe-claude-095.json) used the label `nah://tag/probe-note`, with no `?at=…&include=…`. No recording shows Antigravity passing a URI with a query, `%2C`, or a percent-encoded JSON `subject` through to `Embedded resource (…)` unchanged. No recording shows how it lays out two resources either: separator, order relative to the text. If it normalises or re-encodes the URI, `links_of` fails or the label stops matching, and the message shows raw. Claude's two-resource layout (#1) shows that "one piece recorded" doesn't settle "several pieces".

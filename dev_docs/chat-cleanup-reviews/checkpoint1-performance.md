# Checkpoint 1 (fca4ad8): performance review

Scope: the commit "your messages sent as their words and a piece per link, read back from the harness's replay". Ranked most important first. Paths are relative to `meta-harness/`.

## 1. A replayed message is cloned whole on every chunk, and Claude Code now sends 1 + 2k chunks per message

`crates/harness-acp/src/state.rs` `user_chunk` (about lines 486-516), `update_item` (375-381), `close_user` (176-181).

Every replayed user chunk after the first does three things:
- appends the chunk to `open_user.1` (`raw.push_str`, line 503);
- appends it again to the item's `text`;
- runs `update_item`, which clones the **whole item** (`self.items[idx].clone()`) and `emit`s it.

`close_user` then clones the item once more when it rebuilds the body.

Before this commit a linked message was one text block, so Claude Code replayed it as one chunk. With pieces, Claude Code replays each message as its words, then the label and then the `<context>` for each piece, so 1 + 2k chunks (the test at state.rs ~946-948 records this). Each chunk clones the text accumulated so far, so the cost per message grows with the square of the chunk count:

| Pieces per message | Cloned per message (16 KB of pieces) | Over 3,000 messages |
|---|---|---|
| k = 3 | about 50 KB | about 150 MB |
| k = 10 | about 21 clones averaging about 8 KB, so about 170 KB | about 500 MB |

On top of that, each message's text is held twice while it accumulates, and the minimum needed is 48 MB (one copy).

Every one of these emits is wasted. The replay runs inside `LoadSessionRequest` (adapter.rs ~92-100), before `AcpSession` exists. Nothing can have called `events()` yet, so the broadcast has no receivers and `let _ = self.events.send(event)` (state.rs:118) drops every clone. All of it also happens under the `shared.state` std `Mutex`, inside the ACP notification handler (adapter.rs:230-237), on a runtime thread.

**Fix:** while replaying, accumulate only into `open_user`, and push the item once from `close_user`. `close_user` already runs at the top of `push_item_in`, so the item keeps its order. That makes each message one copy and one push, with no per-chunk clone or emit.

## 2. ATTACHED_MOST does not bound what a message carries; link count is unbounded and multiplies finding 1

`crates/nah-things/src/attach.rs:70-75` and `:129-133`, `crates/harness-acp/src/pieces.rs` `label`, `crates/harness-contracts/src/links.rs` `Message::valid`.

The 16 KB budget counts only the pieces' text. The following are all outside it:
- **A piece for every link past the budget.** Each still produces its own piece: heading plus `"\n(left out: the message's 16 KB are spent; …)"` (about 90 B). Before this commit, everything left out collapsed into one line.
- **A label for every piece.** A thing's label carries its subject as URL-encoded JSON (about 150-250 B). Claude Code's replay writes it twice (`{uri}` and `<context ref="{uri}">`).
- **No cap on links per message.** `Message::valid` only filters spans.

So a message's size is about 16 KB plus (links × ~600 B). The commit's own attach test builds 2,000 links. At that size, one message gives:
- **Sending:** about 1.2 MB and 2,000 `EmbeddedResource` blocks.
- **Replay:** about 4,001 chunks. Through finding 1, that is about 4,000 clones of a text growing to about 1.2 MB, roughly 2.4 GB copied under the state lock, for one message.
- **Cockpit:** 2,000 `ui.label`s whenever its fold is open (finding 6).

**Fix:** cap the pieces per message, and send every left-out link as no piece at all, or as one shared piece.

## 3. Every `SlotManager::send` snapshots and clones the whole session under the state lock (an older problem that this commit makes larger)

`crates/harness-core/src/slots.rs:596`, `let mut events = session.events();`, then `crates/harness-acp/src/session.rs:162-169` and `state.rs` `replay()`.

`events()` runs `state.replay()`, which clones every turn, item and request of the session while holding the `shared.state` std Mutex on a runtime thread. `send` only reads the live part, filtered to its own turn. It does this even when `wait` is `None` and the stream is dropped unread.

This commit puts each user message's pieces into its item (`handed_on`, up to 16 KB or more, see finding 2). In a session with 3,000 such messages, every message sent to a slot (user, lead, discussion and held reports through `tell`) clones at least 48 MB of `handed_on`, plus all assistant and tool output, before the prompt goes out. Every ACP notification for that session waits behind it.

**Fix:** subscribe without the snapshot (`state.subscribe()` alone), and only when `wait` is `Some`.

## 4. Joining held reports is quadratic in their count

`crates/harness-contracts/src/links.rs:148-157` (`Message::joined`) and `:142-145` (`after`), used by `crates/harness-core/src/slots.rs:663-664`.

For each of R messages, `joined` does three full passes over the accumulated output (T is its total length):
- **Copy into `head`.** `format!("{}{between}", out.text)` copies all of the accumulated text into `head`.
- **Copy again in `after`.** `format!("{head}{}", self.text)` copies it again.
- **Count chars.** `after` runs `head.chars().count()` over all of it.

So joining costs O(R × T), where the code it replaced (`each.join`) was linear. Held reports build up for as long as a slot has no running session: sub-slot reports to a closed lead, and discussion news. At 200 reports of 4 KB (T = 800 KB) that is about 320 MB of copying plus 160 M chars decoded, inside an `async fn` on the runtime.

**Fix:** append into one `String`, carrying the running char offset forward.

## 5. Reading a piece back scans without a bound for every `nah://` it meets

`crates/harness-acp/src/pieces.rs:205-230` (`head_at`, line 218), called from `next_piece` (187-202).

For every occurrence of the needle, `head_at` runs `text[pos..].find(ends)` and only then rejects the candidate when the slice contains whitespace. Claude Code's template has an empty lead, so the needle is bare `nah://`. With Claude's template, `ends` is `"\n<context ref=\""`. With Antigravity's it is `"):\n"`.

Each false candidate (a `nah://` in a piece's content or in the user's words) therefore scans to the next piece's marker, or to the end of the text. The cost is O(candidates × length), and the user's words have no size limit. For example, a pasted 100 KB log with 1,000 labels in it costs about 100 MB scanned when that one message is replayed. Linking a file of this repo's own source (pieces.rs, the state.rs tests) puts dozens of candidates into every 16 KB piece.

**Fix:** a label holds no whitespace, so stop the search at the first whitespace, or cap it at the longest possible label. That makes each candidate cost O(label).

## 6. Cockpit: an open "what went with it" copies and hashes every piece on every frame, one label per piece

`crates/cockpit-ui/src/chat/render.rs:658-660`.

`RichText::new(piece)` with `piece: &String` clones the String on every frame, and egui hashes the full text on every frame to find its cached galley. That is about 16 KB copied and hashed per open message per frame, and more when finding 2 applies. The commit also turned one label into one label per piece. With many links (finding 2) that means thousands of widgets per frame for one item. The item is drawn whole once any part of it is near the view (render.rs ~486-490).

**Fix:** use one label, or a prepared galley cached on the item id and the `changed` count.

## 7. Small per-send copies

None of these matters at once per send. They are listed because each is a copy the code doesn't need.
- **`p.text` cloned.** `pieces.rs:88` clones every `p.text` even though `blocks` owns `input` (iterate `pieces` by value). That is one extra copy of up to 16 KB per prompt. The steer path (`session.rs`, `blocks(input.clone(), …)`) copies everything once more.
- **Template re-parsed.** `pieces.rs:92` `Template::ours()` re-parses the template on every send to a harness that takes no pieces.
- **Two copies of the pieces.** `state.rs:184-186` `yours()` clones `input.pieces` into the item, while `input` itself goes on to be sent, so each live message holds its pieces twice until the prompt is built.

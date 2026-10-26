Review of 0dfa7ff..HEAD (links), looking for anti-patterns and bolt-ons, most severe first. I read the code only; nothing was built or run.

1. **The `@` picker is a second code search beside the graph's own** (`crates/cockpit-ui/src/app/linking.rs:115-170`).
   - It keeps its own query, answer count, inbox and hit-to-choice mapping. The graph view already does this in `graph/actions.rs:227-275` and `graph/mod.rs:390-401`: one counter per query, results by kind, and a waiting flag.
   - It also checks `shell_searches` every frame (`find_links`). It should react to the existing `palette.take_search()` hook (`draw/panels.rs:71-77`).
   - Concrete bug: answers are matched by query string, not by a counter. Type `@ma`, backspace to `@m`, retype `ma` before the answers arrive. Both pairs of answers match "ma", so `answers` reaches 4 and every hit is listed twice.

2. **The palette's `gives_back` flag stays on after a pick** (`palette.rs:228-268`).
   - In the palette's writing, type `#fl` and pick a tag. `put_link` brings the writing back (`put_back`), but `gives_back` stays true and the filter still holds "fl" (`close` and `put_back` don't clear it).
   - Now press Esc. `back()` runs `leave_giving_back`, which closes the palette. A writing opened from the action list should have returned to the list.
   - It also sets `given_back = "fl"`, and `panels.rs:89-96` writes "fl" into the view's box whenever the palette was opened from Insert.
   - To confirm: open discuss.write from the list, then do the sequence above.

3. **Links reach discuss.write through a side channel** (`palette.rs:65,366`, `starting.rs:105`).
   - The invocation carries only the text; the links are parked in `palette.sent_links`, and only discuss.write takes them.
   - A tag's note (`tagging.rs:50`, also `Param::Writing`) offers `#`/`@`, shows chips, `ctrl+i` and the check, then silently drops the links.
   - Fix: the invocation's value carries the `Message`, or `link.thing`/`tag.insert` are only offered where the writing takes links.

4. **The message's shape is copied in several places.**
   - Server: `Prompt`, `Starting` and `Saying` each re-declare `text` + `links` (`routes/slots.rs:167`, `routes/discussions.rs:96,117`, plus the `From<Saying>` impl). They should `#[serde(flatten)] message: Message`.
   - Cockpit: three bodies are built by hand with "add links if any" (`net.rs:516-525`, `net.rs:897-905`, `discussion.rs:137-141`). `save_draft` and `check_links` already just send `json!(message)`.
   - The `notes` parsing is copied twice in `net.rs`.
   - Rule sketch: `rule: {pattern: '$B["links"] = serde_json::json!($M.links)'}`, plus a `field_declaration` rule matching `links: Vec<harness_contracts::Link>` outside `harness-contracts`. The second also flags `Said`/`StoredSaid`, which could hold a `Message` too.

5. **The budget warning is given twice.** The check under the box (`Checks`) was added, but the after-send notice the user rejected is still there: `SlotManager::send` returns notes, `Prompted.notes`, and `agents.rs:81` sets them as the notice. Discussions (`tell`) drop the notes, so chat and discussions now behave differently. `Prompted.notes` also carries a garbled doc comment left from the edit (`routes/slots.rs:182-183`).

6. **`Checks` copies the wait-a-moment-then-send logic of `drafts.rs`** (`changed`/`tick`: pending, since, `request_repaint_after`), in `linking.rs:240-293`.
   - A failed check leaves `asked = true` with no notes. It is never retried, so no warning shows until the text changes.
   - Entries for closed buffers are never removed.

7. **Work done every frame** (`tend_links`, `linking.rs:403-434`, called from `lifecycle.rs:62-63`).
   - Every frame it builds a colour map of all tags, clones and trims every box's text, and recolours every chip, even when no box has a link.
   - Chip colours are copies of the tags' colours. Resolve them when the box is laid out, and check only when a box changes.

8. **`TypingBox.text` is `pub`, so link bookkeeping can be bypassed** (`conversation.rs`).
   - `chat.clear` runs `input.text.clear()` (`chat/actions.rs:289`) and leaves the links behind. They then move with each edit past the end of the text (never shown) and are saved into drafts by `draft()`, which is untrimmed.
   - The repo's existing `rust-no-public-struct-fields` rule already flags `pub text` and the new `pub warning`.

9. **`thing_read` copies `tag_read`'s one-thing branch** (`mcp/sets/things.rs:41-70` vs `tags.rs:136-152`): the line range, reach check, "has N lines" and "lines a-b of N". It borrows `tags::line_range` through `pub(super)` and adds a second `READ_MOST`. They should share one helper.

10. **The "where you're writing" condition is copied** between `link_actions` and the `tag.insert` action (`linking.rs:119`, `tagging.rs:482`).

No real problems found in the server expansion path: one `Expand` closure, the `#word` scan deleted, queue briefs and lead messages plain text. Draft storage is also fine.

Review of 0dfa7ff..HEAD (links), on separation of responsibility and coupling. Six problems, most severe first.

1. **The rule on who may read what is applied to reads but not to links.** `src/linking.rs:8-19` and `crates/nah-things/src/attach.rs:5,58-93` hand a linked thing to any slot. The comment says "only the user links things". But `thing_read` (`src/mcp/sets/things.rs:59`) and `tag_read` (`src/mcp/sets/tags.rs:142,162`) still check `Reach`, and so does the subject of a discussion (`routes/discussions.rs:109`, `may_read`). How it goes wrong:
   - The user tags slot A's chat turn and links `#thattag` in a prompt to sub-slot B.
   - If the turn is small, B gets it whole, which `tag_read` would refuse B.
   - If it's big, B gets its start plus "`tag_read` (tag X, thing 1) reads the rest", and that call answers `NOT_YOURS`.
   - A discussion asked of B can't be *on* A's chat, but its text can link it.

   The plan says the server "checks the reader may read it", so this rule now lives in the MCP layer only and the link path skips it. Severity: medium-high. Agents are pointed to reads they're refused, and the rule that keeps one slot's chat from another isn't kept.

2. **The palette carries links beside the action's value.** `palette.rs:367` stores the links of the last writing sent, and `app/starting.rs:105` takes them as whatever `discuss.write` sends. Two ways it goes wrong:
   - **Links lost:** a tag's note written in the palette (`tagging.rs:50`/`237`) sets `sent_links` and nothing takes them. A `#` chip typed in a note is dropped without a word.
   - **Stale links sent:** `discuss.write` run from outside (`app/outside.rs` `run_named`, by the page or an agent) then picks up those leftover links. The discussion goes out linking the note's tag, with spans into a different text.

   Severity: medium-low.

3. **The shell reaches into every buffer's typing box every frame.** `Navigable::typing_box` (`nav.rs:857`) hands out `&mut TypingBox` where `typed_in(&str)` used to be. Then:
   - `app/linking.rs:206-221` writes the public `warning` field and recolours chips in every buffer's box.
   - `app/draw/panels.rs:93-98` puts text straight into a box.
   - `Desk::checks` is private to the shell.

   This sits beside the existing pattern: drafts go out as `Ask::Draft` and come back through `Shared`. Tag colours are already in `Shared.tags`, which a buffer could read itself; checks could be a held service the buffer asks. Severity: medium (structural). Every box now depends on the shell running `tend_links` before it draws.

4. **The `@` picker is a second client for the code index.** `app/linking.rs:120-170` repeats what `GraphState::find_query`/`send_find`/`find_choices` already do (`graph/actions.rs:227-273`, `graph/mod.rs:395`): query, one search per kind, an inbox, dropping stale answers, turning hits into choices. The copy has drifted:
   - it uses a fixed `FOUND_EACH = 12` instead of the configured `ui.find` limits;
   - it ignores `total` and `partial`, so a search cut at its time limit looks complete;
   - it sets a note for each failed kind.

   The plan gives this picker to the code index's owner. Severity: medium-low (two copies to keep in step).

5. **The cockpit hard-codes the server's rule for when there's something to say.** `app/linking.rs:60` only asks `/api/links/check` when a link is Whole, because `attach.rs` only writes notes for Whole today. Meanwhile `src/linking.rs:12` silently skips a link whose tag was deleted, so it's never said. Sequence: link `#flaky`, someone deletes the tag, send. The agent gets nothing for it and the user isn't told, before or after sending. Severity: low-medium.

6. **The link check goes through the slot manager and does a full expansion.** `/api/links/check` is registered in `src/routes/slots.rs:29,195` and reaches the expansion through `SlotManager::expanded` (`harness-core/src/slots.rs:560`). That makes harness-core's slot manager hold a links-only closure (`slots.rs:192`) just so a route that involves no slot can call it. Each check also reads up to 16 KB of contents to return only the notes. It reruns 0.4 s after each edit settles while a Whole link is in the box. Severity: low.

No real problems found with keys (`ctrl+i` comes from the keymap config), the drafts' storage, or the discussions crate storing links.

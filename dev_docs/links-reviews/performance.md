Ranked findings, most severe first:

1. **The per-message budget doesn't apply to pointers, and pointer lines have no length limit.** `crates/nah-things/src/attach.rs:83`. `ATTACHED_MOST` only limits things that go in whole. Once the budget is used up, every remaining thing falls back to a pointer, and each pointer still adds a heading plus its first 10 lines, read with `self.lines`. Nothing cuts those lines short; `some_of(…, 400)` only applies when `lines` fails. The old `expand` stopped at `EXPANDED_MOST` for everything.
   - Trigger: link a tag that is on 300 things, or a file whose first line is a 1 MB minified bundle or a long terminal output line.
   - Result: the agent receives roughly 3,000 lines or megabytes on every send. That costs tokens and context on every message, and the sender is never told.

2. **`/api/links/check` re-runs a full expansion whenever the text changes, not only when the links change.** `cockpit-ui/src/app/linking.rs:65,72` and `src/routes/slots.rs:195`.
   - The check is keyed on the whole `Message`, but the notes depend only on the links: `attach` measures `out.len() - start` and ignores the text. With one link set to Whole, every typing pause of 0.4 s sends a request.
   - Each request carries the full text (a 400 KB paste goes through Home each time).
   - The server reads every linked thing, including the Auto and Pointer ones that can never produce a note, builds the whole expansion text, then throws it away.
   - Fix: key the check on the links only, send only the links, and only expand the Whole ones.

3. **`attach` reads each thing 2–3 times.** `nah-things/src/attach.rs:57,74,83`.
   - `glimpse` (line 57) loads the entire content through `owner.now` (whole file, or a terminal command's full output). If a Whole link isn't small, `glimpse` runs again at line 74; a pointer runs `lines` at line 83, which loads and splits everything again for 10 lines.
   - `src/linking.rs:14` calls `things.current` outside `things.pass`, so the owner's move detection (file diff) runs once per link.
   - At 10,000 lines that is about 3 full reads plus a diff per link, per send, per check (see 2), and per slot that a discussion message is told to.

4. **Several costs every frame, whether or not anything is linked.**
   - `tend_links` (`app/linking.rs:196`) runs each frame from `lifecycle.rs:63`:
     - It sorts every tag (`tags.list()`) and builds a `BTreeMap` with cloned `TagId`s, even when no box has a link. With hundreds of tags, that is hundreds of String allocations plus a sort per frame.
     - It walks every buffer, including ones not on screen. For each typing box, `writing.message()` (line 207) clones the text and links, and `trimmed()` copies the text again and counts its characters.
     - It formats `"{id:?}"` keys twice per buffer.
   - `TypingBox::draw_in` clones the text unconditionally (`conversation.rs:314`).
   - `report_unsent` (`chat/nav.rs:71`) now clones the draft every frame before comparing; it used to compare `&str` with no copy.
   - Result: about 4 full copies of the text per box per frame. A busy agent stream repaints continuously, so a 400 KB draft costs roughly 100 MB/s of allocation and copying.

5. **The chip layout cost grows with text length times the number of chips.** `conversation.rs:265,267`. For each chip, the layouter runs `text.chars().count()` once and `char_indices().nth()` twice, each a full scan of the text, on every layout pass. With 20 chips in a 10,000-line box, that is about 60 scans of 400 KB per frame while typing. Converting the sorted chip offsets in a single pass would avoid it.

6. **The `@` picker sends 2 searches per keystroke with no delay.** `app/linking.rs:162-168`, files and symbols. Typing a 15-character path sends 30 requests to the code index, and every answer except the last is thrown away. This matches how the graph view's find already behaves, so it is low severity.

7. **`Checks.each` entries are never removed.** `app/linking.rs` `Checks`. An entry is removed only when its box has no Whole link at the next tend. A buffer closed while it still has a Whole link leaves its entry, including a full copy of the message, for the rest of the session. The leak is small. Also, a check request that fails leaves `asked = true` and is never retried.

No real problems in these areas:
- Server locks: `tags.attached` holds the lock only to clone; expansion runs inside `spawn_blocking`.
- Draft saving: it already waits for a pause and sends the whole draft, as it did before.
- `follow_edit`: it runs only on actual edits and only when the box has links.

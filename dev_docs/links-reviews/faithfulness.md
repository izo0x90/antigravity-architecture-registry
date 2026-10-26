I found seven problems, ranked most severe first. The first one breaks the per-message budget; the second goes against what the plan says. I didn't build anything, so the confirmations suggested below still need running.

1. **A pointer's "first lines" have no size limit, so the budget doesn't hold** (high). `crates/nah-things/src/attach.rs:81` takes `self.lines(subject, kept, 1, 10)` and joins those whole lines with no byte cap. The 400-character fallback (`some_of`) only applies when no lines come back. Linking a one-line minified file (for example a 2 MB `app.min.js`) with Auto makes it "not small", so it goes as a pointer, and its 10 "first lines" are the whole 2 MB, sent in the prompt. The old `#` expansion was always capped by `EXPANDED_MOST`. `/api/links/check` doesn't warn either, because it only reports Whole links.

2. **The server never checks that the reader may read what's linked** (medium; differs from the plan). The plan says the server "checks the reader may read it". The code says the opposite and doesn't record it as a decision: `attach.rs:4-5` has "Only the user links things, so everything linked is handed on", and `harness-core/src/slots.rs:172-175` "the user chose to show it". `src/linking.rs` has no reach check. Two concrete effects:
   - **Discussions:** `routes/discussions.rs:125` refuses a discussion whose subject the asked slot can't read, but not the links. A discussion asking `reviewer` with a linked `#t` (a tag on the lead's chat turn) hands that turn to `reviewer` whole if it is small.
   - **Dead pointer:** if that chat turn is big, it goes as a pointer telling the agent to use ``tag_read` (tag t, thing 1)``. `tag_read` refuses it for that slot (`src/mcp/sets/tags.rs:142,162`), so the agent gets the first lines and an instruction that can't work.

3. **The planned "Tests" don't cover the budget rule** (medium-low). The only check test asserts empty notes for a small thing (`tests/terminals_tests.rs:498`). Nothing tests:
   - Whole past 16 KB becoming a pointer plus a note
   - the Auto limits (4 KiB / 100 lines)
   - the after-send notes in `Prompted`
   - user links carried through discussion `tell` (`src/discussing.rs:81`)

4. **Links in palette writing other than "start a discussion" are dropped without a word** (low).
   - The tag note (`app/tagging.rs:50`, `Param::Writing`) draws chips and runs the check, but saves plain text.
   - `Palette::sent_links` (`palette.rs:367`) is only taken by `app/starting.rs:105`. Leftover links from a note can be attached by a later `discuss.write` that gets its value from outside the palette (`app/outside.rs:85`), with offsets into a different text.

5. **A link to a tag deleted (or a thing gone) before sending disappears without telling anyone** (low). `src/linking.rs:2-3` and `Tags::attached` return None. The chip stays in the box, the agent gets nothing, and the user gets no note.

6. **`gives_back` is never cleared after a pick** (low; I haven't run it). The sequence: `#` in the palette's writing, then `hold_for` and `give_back_typing` (`app/draw/panels.rs:234`), then a pick, then `put_back` reopens the writing with `gives_back` still true. Escape in that writing then goes through `leave_giving_back` (`palette.rs:246-256`) and closes the palette instead of the usual back step. A test that picks and then presses Escape in a `from_list` writing would confirm it.

7. **Link spans are never validated on the server** (low). In `harness-contracts/src/links.rs:113`, `l.end - lead` underflows when a client sends `end < start` (for example `{"start":5,"end":0}` with text `"  x"`). That panics in debug builds and wraps in release. The routes `/api/slots/{slot}/prompt` and the discussion routes accept any span.

Also built though the plan didn't ask for it: the user decided notes are said under the box before sending (b3178b3), but the after-send notice is still there (`crates/cockpit-ui/src/agents.rs:94`). Notes for the same message can show twice.

These areas look as planned: contracts, the scan removed, agents' messages plain, queue briefs plain, drafts keeping links, chip styles, `ctrl+i` cycling, the `#` picker with no tags, the `@` picker including `path:a-b`, the give-back on Escape or a space, `thing_read` names round-tripping, the debounced check, chips not shown in history.

Ranked findings for 0dfa7ff..HEAD, most severe first. I only read the code; nothing was built or run.

1. **Pointers have no size limit (high).** `crates/nah-things/src/attach.rs:781` builds a pointer's preview from `self.lines(.., 1, POINTER_LINES)`. That takes 10 whole lines with no byte cap. The budget check (`room`) only applies to the Whole path.
   - Example: `@dist/app.min.js` (one 800 KB line) on Auto has a sample that ends in `…`, so it is treated as big and sent as a pointer. The pointer then pastes the whole 800 KB line.
   - A tag on 200 big terminal commands gives 200 uncapped pointers. The old code capped the whole expansion at 16K.
   - The agent's context fills up, or the harness rejects the prompt.

2. **The plan's read check isn't done (medium).** The plan says the server checks the reader may read what's linked. `src/linking.rs` and `modules.rs:943` hand everything over.
   - Example: tag a turn in slot B's chat `#secret`, then start a discussion asking sub-slot A with "see #secret". `routes/discussions.rs:109` refuses B's chat as the discussion's subject for A, but the link hands its contents to A anyway.
   - It also depends on size: a small thing goes in whole, while a big one becomes a pointer to `tag_read`, which refuses A.

3. **The `@` picker eats typing (medium).** It only lets go on "a space with nothing found" (`palette.rs:477`). The code index search is fuzzy, so something nearly always matches.
   - Example: typing "@lead please check this⏎" puts everything after `@` into the picker's field, and Enter picks the top hit. The box ends up with `@some_symbol ` and the sentence is lost.
   - To confirm: check whether `/api/search` returns hits for common words.

4. **Clearing the prompt leaves its links behind (medium-low).** `chat/actions.rs:289` does `self.ui.input.text.clear()`, so the links stay. `put_link` and `put` add text without moving old links, so a stale link can come back into range.
   - Example: the box has "see #secret " with link [4,11). Clear the prompt, then use "Link a file or a symbol" twice to get "@a.rs @b.rs ".
   - The old [4,11) link is now inside the text. `laid_out` skips drawing it because it overlaps, but `message()` sends it, so #secret's contents go to the agent without the user seeing it.

5. **Symbols picked over a plan resolve to nothing (low-medium).** `app/linking.rs` searches with the current source, which can be `Source::Plan`, and builds `Subject::Component` from the hits. `code_tags.rs` `component()` only looks in the code graph. A planned component therefore reaches the agent as "(nothing of it is here)", with no warning to the user.

6. **A text ending in `…` is taken as cut (low).** `attach.rs:764` and `:773` read a trailing `…` as truncated. A 10-byte file or chat message ending "Wait…" linked Whole shows the warning "too big to go in whole with the rest (past 16 KB)" and goes as a pointer.

7. **Some links are dropped without a word (low).** `linking.rs:831` keeps the first of two links with the same heading. `@a.rs` as Pointer followed by `@a.rs` as Whole sends a pointer, with no note. A tag deleted after it was picked is also dropped silently, while its chip is still drawn.

8. **Keeping links through edits misjudges some edits (low).** `follow_edit` finds the edit by matching the start and end of the old and new text, which is ambiguous when characters repeat.
   - Example: "x @@y", where the link is the second `@y`. Deleting the first `@` drops the link even though the edit was outside it.
   - Typing a `y` inside `#flaky`, just before its last letter, is taken as typed after it: the link is kept and the chip no longer covers what was edited.
   - Ctrl+Z never brings a dropped link back.

9. **`gives_back` isn't cleared after a pick (low).** In `palette.rs`, after a pick from the palette's own writing box, the flag stays set. A later Escape goes through `leave_giving_back`: `put_back()` returns None, so the palette closes outright instead of going back to the list. The old search text (for example "fla") is then put into the chat box if the palette was opened from typing.

10. **Spans aren't validated (low, malformed clients only).** `Message::trimmed` computes `l.end - lead`, which underflows when `end < start`; that panics in a dev build (overflow checks on). In the cockpit, `laid_out` does `text[start..end]`, which panics on a stored draft with start > end. Neither the server routes nor `set_draft` check spans.

11. **The check reruns on every pause in typing (performance, low).** Its notes depend only on the links, but `Checks::tend` keys on the whole message. Each 0.4 s pause re-reads every linked file in full, and lines kept at a snapshot are diffed again.

12. **No touch control for changing a link (low).** Whole, pointer or as-it-fits can only be changed with `ctrl+i`, so a phone has no way to do it. That breaks the touch-first rule. In Firefox, ctrl+i may also be taken by the browser.

Nothing real found in: drafts saving and loading with links, the migration, a late check reply (it's matched against the message it was for), discussions' links surviving a restart, and agents' own messages (plain text throughout).

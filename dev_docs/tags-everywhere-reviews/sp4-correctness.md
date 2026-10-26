# Stopping point 4: correctness reviewer

I reviewed `b29fbfc..HEAD` in meta-harness by reading the code only; nothing was built or run. Problems below, most severe first.

1. **Any file the server can read can be read through a tag** (`src/code_tags.rs:146-170`, `:96`). `named` only checks `root().join(path).is_file()`. `join` with an absolute path drops the project root, and `../` is not stopped.
   - Sequence: MCP `tag {tag:"x", subject:"/home/u/.ssh/id_rsa lines 1-50"}`, then `tag_read x`. `Code::now` falls back to `lines_text` (the git diff of that path fails, so the lines count as unmoved) and returns the file's text.
   - The HTTP route takes a `Subject::CodeLines` JSON directly, so the same works there.
   - Severity: high (security). It only needs TagsEdit + ViewRead, and only in a git repo.

2. **A tagged running command's log grows without limit** (`logs.rs:111` `grow`, `terminals.rs:225` `tell_followed`). The 8 MB split applies only to the shared file; a growing thing's own file has no cap.
   - Sequence: press `M` on `npm run dev`, `tail -f` or `yes`. The file grows until the command ends, filling `.nah/state` on a small, shared disk.
   - `Logs::read` (`tag_read` with `thing`, even with `lines`) loads the whole file into a String.
   - `head` reads the whole file to count lines on every `tag_read` and on every prompt that names `#tag` (`expand`).
   - Severity: high.

3. **A chat turn or message tagged while it is still running is kept cut off, and labelled "as it is now"** (`chats.rs:158`, `nah-tags/src/lib.rs:262` `said`). Chats has no `follow`, so the log gets the text as it is at that moment. `said` always prefers the log, while `held` reports `Source` because the session is still there.
   - Sequence: an agent tags `chat lead turn 7` (its own running turn), or the user picks the turn with `Tab`. Later `tag_read` shows the partial turn, without the answer, marked "as it is now".
   - This breaks rule 3.

4. **The cockpit chat's tag code is quadratic to cubic per frame** (`chat/things.rs:26-63`). `stream()` clones every item and runs `items_of` per turn.
   - `tagged_indexes` calls `tag_place`, and so `stream()`, once per item; the gutter calls it for every tagged subject every frame.
   - `tag_things` runs `tag_place_of` (another `stream()`) inside a `position` over the stream. It is reached every frame through `tags_here_named` and through `shell_actions` (`tag_actions`).
   - With a chat of about 1,000 items and one tagged message, I expect the cockpit to freeze. Timing a frame would confirm it.

5. **Tagged code lines show the wrong text** (`code_tags.rs:130-140`).
   - `now()` reads `first..last` from the subject, not the position `now_at` worked out. They are only moved on `ArchEvent::Changed`, which fires only when the graph changes.
   - Sequence: tag `README.md lines 10-12`, add 3 lines at the top, run `tag_read`. You get the new lines 10-12, labelled "as it is now", and the gutter bar is on the wrong rows.
   - When lines changed among them: `held` is Gone, yet the text shown is today's file at the old numbers.
   - When the file is deleted: nothing comes back, although the snapshot has it. Rule 3 says code "lasts" in the snapshots.

6. **Migration 9 throws existing tagged text away** (`nah-state/src/migrations.rs:40`). Plan A.2 says the kept tag's text moves into a log; the migration just deletes `said`.
   - Sequence: an existing tag on terminal lines, upgrade, restart. `tag_read` says "gone, nothing kept of it".

7. **Turn numbers aren't stable when a session is replayed** (D.1 says to check this first; `harness-acp/src/state.rs:345-368`, `:236-245`).
   - On replay, consecutive user messages merge into one turn.
   - Live, items arriving with no prompt running start an extra turn.
   - Sequence: send a prompt, cancel it before any reply, send another, then resume the session after a restart. Every later turn number shifts by one, so tags point at the wrong turn. Replaying such a session would confirm it.

8. **Logs of tags this build can't read are deleted** (`nah-tags/src/lib.rs:223` with `nah-state/src/tags.rs:24`). `tags()` skips JSON it can't read ("stays as kept"), but `forget_unused` then removes their log files.
   - Sequence: tag with a newer build, switch to an older branch and start it, switch back. The tag points to a deleted file, or to a file number since reused.

9. **The same code lines can be tagged twice with one tag** (`lib.rs:410`). The duplicate check compares subjects including the snapshot. After a restart (`last` snapshot is None), or after an edit elsewhere in the repo, tagging the same lines again gets a new snapshot id, so they go on the tag twice.

10. **Agents can't take a tag off by the name `tag_read` gives** (`chats.rs:135`).
    - A closed slot's thing becomes `chat session abcd1234 turn 1` after the next `refresh`; `named` reads that as a slot called "session abcd1234", so `untag` fails.
    - After the slot starts a new session, the stored `chat lead turn 3` resolves to the new session's turn 3.

11. **The command a tagged line was in isn't recorded (C.2)** (`terminals.rs` `name`). "(of command N)" is worked out live. The `refresh` on `register` after a server restart, or after the shell restarts, rewrites the stored name without it.

12. **Untagged text stays on disk.** Untagging removes a log file only when nothing else points into it. A secret printed by an untagged command stays in a shared `tagged-N.log` as long as any other tag uses that file. The plan says "Nothing untagged is logged".

13. **`tag_find` and `tag_read` number lines differently.** `tag_find` counts raw lines; `tag_read` with `lines` counts after folding tool output (unless `full`). A match `tag_find` reports at line 33 of a chat turn gives "has 4 lines" from `tag_read`.

14. **Small cockpit and follow gaps:**
    - "Name a tag" picked from the palette, rather than after typing `#`, inserts the bare name with no `#` (`tagging.rs` `TagInsert`), so the prompt names no tag.
    - If a command ends between `follow`'s `written` and its `push` (`terminals.rs:510-520`), the log misses the tail and the end line until the next output. If the shell ends in that gap, they are never written.
    - A finished command tagged after part of its output scrolled away is logged with no "(N lines went…)" note.

15. **Plan items not done:**
    - A.3: events still carry the whole tag (`TagUpdated { tag }`).
    - A.1: search doesn't use fff-search.
    - Each code-lines move takes a new git snapshot (a whole-project `add_all`), under the tags lock, and these snapshots are never released.

**Checked and no problem found:**
- Lock order between the tags, the followed commands, a terminal's shell and the slots.
- Log permissions and links.
- UTF-8 cuts in `head`.
- Empty and zero-size handling in `written`.
- Rule 1: the database holds no text; names and activity hold none either.
- Rule 6: there is a test that every action has a description.

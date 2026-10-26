# Stopping point 2: responsibility reviewer

Review of b29fbfc..HEAD: separation of responsibility and coupling. Read only; nothing was built or run. Findings are ranked by severity.

1. **High: every output chunk of a followed command writes the whole tag to the database and broadcasts it.**
   - Where: `src/terminals.rs` `happened` → `tell_followed` (~L203, L222–250) → `nah-tags/src/lib.rs` `grew` (~L389–404) → `keep` (L503).
   - `nah-terminal/src/lib.rs:461` sends `Happening::Changed` once per PTY read. Each one does a log write (plus a `== … more …` header line), a SQLite transaction rewriting the whole tag JSON, and a `TagUpdated` carrying the whole tag. The tag now includes `Tagged.kept`, which lists every log piece.
   - Trigger: `M` on a running `tail -f`, a dev server, or a slow build. The pieces list never stops growing, so every event and every row rewrite gets bigger. Total bytes grow quadratically, through Home to every cockpit.
   - Coupling: the storage layout of the server's log files (`Kept`/`Piece` in `harness-contracts/src/tags.rs:58–78`) travels on the shared event protocol, which the cockpit never reads. A.3 ("events carry what changed, not the whole tag twice") is not built.

2. **High: tagging I/O runs on the terminal's reading thread, under two locks.**
   - Where: `tell_followed` holds `Terminals.followed` while calling `more`. That calls `grew`, which holds the `Tags` mutex during a file write, a SQLite write and `self.current()`, which calls back into `Terminals::moved`.
   - Effect: a slow disk or a busy database stalls that shell's output. Every tag request (cockpit, MCP `tag`/`tag_read`/`expand`) and every other terminal's followed output queues behind it.
   - Cause: the owner pushes into the tags synchronously with no queue between them.

3. **High: `Resolve::follow` has no way to stop following, so output gets logged twice.**
   - Where: `lib.rs:207`, `terminals.rs:1242–1252`.
   - Trigger: `M` with tag `build` on a running command, take `build` off, then put `build` back. Both `Followed` entries stay until the command ends, and both call `grew(build, cmd)`. `grew` finds the re-added `Tagged` by subject (L395), so every later chunk is appended twice and the log no longer matches what the command printed.
   - A deleted tag also leaves the terminal following and rendering `lines_text` until the command ends.

4. **Medium: a log file can be removed while a new tag's first piece is in it.**
   - Where: `Tags::tag` writes the first piece with the tags lock dropped (`lib.rs:~372–374`). `untag`/`delete` call `forget_unused` under the lock (L419, L437), and that removes files based only on the tags map.
   - Sequence: tag A is the only thing pointing into `tagged-1.log`. Tag command 2 with B; its piece goes into `tagged-1.log`. Before B takes the lock, untag A: B isn't in the map yet, so `tagged-1.log` is deleted. B then records a `Kept` pointing into a deleted file, and once the scrollback goes, `tag_read` says "gone".
   - Cause: log cleanup lives in `Logs`, but who is writing to the logs is known only in `Tags`, outside its lock.

5. **Medium: generic palette and panel code holds tagging state.**
   - `cockpit-ui/src/app/draw/panels.rs:~165`: the generic palette host resets `self.desk.tag_level = None` every frame the palette is closed.
   - `Desk.tag_level` is an index into `Things.at`, recomputed from the cursor every frame (`tagging.rs:241, 273`). It refers to the list position, not to a subject, so if the buffer's list changes while the picker is open, the same index picks a different thing.
   - The picker's state should be owned by the tag action (or carried by the palette stage), not cleared by the panel drawer.
   - `palette.rs:~541` finds the cycle action by bare id without its origin, while the stage refresh just above it matches origin and id.

6. **Medium: two places name a terminal thing, with different rules.**
   - Server: `Terminals::name` (`terminals.rs:1206–1210`) adds `(of command N)` using `prompt <= first && end.is_none_or(last < end)`.
   - Cockpit: the terminal buffer's `things` (`cockpit-ui/src/terminal/mod.rs:918–943`) builds its own words using `command_at(line)` with `output.is_some()`.
   - The picker's top line and the stored `what` can therefore disagree. Rule 2 says one owner names a thing.
   - The server's display suffix is also part of the name that identifies the thing, so `named()` has to strip it again (`terminals.rs:1228`): display text and identity are coupled.

7. **Medium: line moves after a re-layout are tracked in a second, partial place.**
   - Server: `Resolve::moved` / `lines_now` already says where a line is now.
   - Cockpit: `terminal/lines.rs:52, 239–250` records `relaid` per buffer, and `TermPlace.layouts` (`terminal/mod.rs:~825, 890`) maps a jump's line only when `here` (same buffer, same terminal, same shell start).
   - Trigger: record a jump on t1 line 500 in panel A, show another terminal in A, resize t1 from panel B, then `ctrl+o` in A. The old line number is used unmapped and you land on the wrong line.
   - To confirm: check whether `show()` resets `relaid` on switching terminals. The code suggests it does.

8. **Low–medium: reads fetch everything to show 4,000 characters.**
   - `Resolve::now` has no extent or limit. `Terminals::now` renders the whole command (`command_output(.., u64::MAX)`, `terminals.rs:1219`), and `Tags::now` reads every log piece in full (`lib.rs:301–305`), then `some_of` cuts to 4,000 characters.
   - `Tags::tag` also calls `owner.now()` and throws the result away when it follows.
   - A Tags view resting on a tag over large outputs reads every one whole on each read.

9. **Low: the words for "where it came from" are written three times.**
   - `nah-tags` `expand` (`lib.rs:~468–471`), MCP `held_words` (`src/mcp/sets/tags.rs:~897`) and `cockpit-ui/src/tag_list.rs:~676` each spell out "gone from where it was: as it was when tagged".

10. **Not built: log search (A.1).** Nothing wires `fff-search` over `.nah/state/tagged/`.

No problems found in these areas:
- Permissions and capabilities: the MCP tools keep `Capability::ViewRead` / `TagsEdit`.
- Keys: the Tab cycle key comes from keymap config (`ListKeys.cycle`).
- Asks and desk services: buffers reach the shell only through `Ask` and `Reader`.
- Theme: no new colour paths.
- Shell vs buffers: tagging code uses the `Things` interface only and names no buffer type.
- `nah-tags` still depends on no owner crate.

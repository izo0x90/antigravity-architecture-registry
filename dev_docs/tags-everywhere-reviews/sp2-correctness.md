# Stopping point 2: correctness reviewer

Ranked findings. I only read the code; nothing was built or run.

1. **High: a command partly scrolled out comes back cut short, labelled whole and current (rule 3).** In `nah-tags/src/lib.rs:221`, `said()` uses the source whenever `now()` returns `Some`. `Terminals::now` (`src/terminals.rs:454`) calls `command_output(u64::MAX)`. Two things make that text partial: `screen::text` clamps `from` to the oldest kept line, and `gone_before` keeps a command whose end is still kept, moving its output start to `oldest`. So it returns `Some(partial)` and `Held::Source`, and the complete log is never read.
   - Sequence: the C.4 run (`seq` with 50,000 lines, 10,000 scrollback). After it ends, while the shell lives, `GET /api/tags/big` gives `held:"source"` and about 10,000 lines.
   - `tag_read thing 1 lines 1-2` gives the command line followed by `~40000`.
   - `#big` in a prompt hands the agent the cut-down text.
   - The test only reads after a restart, so it misses this.

2. **High: a followed command silently loses everything after its next line falls out of the kept lines.** In `tell_followed` (`terminals.rs:238`), `lines_text(f.next, end)` returns `None` once `f.next < oldest`. `f.next` then never advances, and only the "(exit code …)" line is logged at the end. Triggers:
   - `clear` or `reset` inside a tagged running command. `clear` sends `ESC[3J`, and `feed_to` adds the whole history to `dropped`. Example: `while :; do clear; date; sleep 1; done`.
   - `scrollback_lines = 100` (the allowed minimum) with fast short lines: one 4 KB read of `yes ""` is about 2,000 rows, past 100 + 512 slack.

3. **Medium-high: every chunk of output rewrites and re-sends the whole tag; A.3 isn't built.** Each Changed event goes `grew` (`lib.rs:372`) → a new log piece with its own header, `store.put` of the whole tag JSON (all pieces), and `TagUpdated` with the whole tag. This runs synchronously on the terminal's reading thread while holding the global `followed` lock.
   - Sequence: tag a running `tail -f`, or a 20 MB build. Pieces grow without limit, so SQLite writes and websocket traffic grow O(n²).
   - The shell's output stalls behind the database writes, every other terminal's followers wait on the lock, and the 4096-slot hub can lag slow clients into a Resync.

4. **Medium: output is logged twice after untag then re-tag.** Untag never stops the follower. Re-tag registers a second one. Both closures call `grew(id, subject)`, which finds the re-added entry.
   - Sequence: `M` "dev" on a running `npm run dev` → untag → `M` "dev" again. Every later line is in the log twice.

5. **Medium-low: `tag()` checks under one lock and writes under another (lines 312–366).** The lock is dropped for `follow` and `logs.write`, which can be slow because it writes all the output so far.
   - Delete during that gap: `found(..).unwrap_or(kept)` (line 360) brings the deleted tag back.
   - Two tags of a new name at once (user and agent) → two tags with the same name, different ids.
   - The same subject twice at once → two entries and two followers. Both followers then write to the first entry, so it gets every chunk twice.
   - The "already on" and `MOST_ON` checks aren't repeated after re-locking.
   - An untag or delete in the gap runs `forget_unused` without the new piece in the in-use set. It can delete the file just written (e.g. a fresh `tagged-N.log` after older ones were removed), so the stored `Kept` points at a missing file.
   - After a restart with no files left, numbering starts again at `tagged-1.log`. A stale `Kept` then reads another tag's bytes, labelled "as it was when tagged".

6. **Low-medium: a failed tag leaks memory.** If `logs.write` (line 357) or `keep` fails after `follow` has registered, `held` stays `Some` and the follower stays registered. Every later chunk is buffered in memory until the command ends, which is never for a dev server.
   - Sequence: full disk, or `.nah/state/tagged` not writable. The user sees "Couldn't tag it" while memory keeps growing.

7. **Medium-low: `ctrl+o` lands on the wrong line after a width change.** `Lines::line_now` (`cockpit-ui/src/terminal/lines.rs:248`) adds one offset per new layout (`by` = how far the end moved). The server moves each line by the change in rows above it (`Layout::place`).
   - Sequence: cursor on line 100, then 50 lines of 150 characters below it at 200 columns. Jump away, narrow to 100 columns, `ctrl+o` lands on about line 150.
   - The layout count also comes from whichever `Lines` the buffer holds then; it restarts when the buffer switches terminals.

8. **Low-medium: the log doesn't match what was printed.**
   - `screen::text` trims trailing blank lines (`screen.rs:115`) on every chunk, so `printf 'a\n\n\n'; sleep 1; echo b` is logged as `a\nb`.
   - A long line still being written that has wrapped onto the cursor's row is split by an extra newline.
   - A redraw that moves the cursor up (multi-line progress bars) is never logged; only the first drawing is kept.

9. **Low: migration 9 deletes the old tag's text instead of moving it into a log.** A.2 says the text moves to a log. With `kept: None`, the user's existing terminal tag now reads "gone, nothing kept".

10. **Low: log files nothing points to are only removed on untag or delete.** There is no clean-up at start-up. A crash between `logs.write` and `keep` leaves a file, against "a file no tag points into is removed".

11. **Low: `tag_read` line-range edge cases (`src/mcp/sets/tags.rs:85`).**
    - `lines:"100-200"` on a 50-line thing returns the header with nothing under it and no message, because `get(99..50)` is `None`.
    - A huge `from` overflows `from + READ_MOST - 1`: a panic in debug builds, wrong output in release.

12. **Low (performance): reads load whole texts.** `read()` and `expand()` load each thing's whole text (whole log files, or a command's full output under the terminal lock) only to show 4,000 characters. A tag on many large commands makes each cockpit read, and each prompt naming it, load all of them.

Areas with nothing found:
- **Locks:** lock order checked across `followed`, tags, `held` and terminal state; no deadlock. `happened` runs after the state lock is released.
- **Order of early pieces:** text told while the tag is still being stored stays in order, because the `held` guard lives through the replay loop.
- **Cockpit Tab and focus:** the picker consumes Tab and the test types afterwards, so Tab doesn't steal the text box's focus.

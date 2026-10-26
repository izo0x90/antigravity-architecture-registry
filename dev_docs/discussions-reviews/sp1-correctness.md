## Review of 772aa54..53cd6b5 (Parts A and B): ranked findings

I read the code only and ran nothing. I found 10 problems, most severe first.

**1. A message the agent takes mid-turn renumbers later turns, so existing chat notes silently point at a different turn.** `crates/harness-acp/src/state.rs` `steered()` (~L174–205)
- `steered` inserts a turn at `place+1` and gives every later turn a new number. `Chats` has no `moved`, and nothing calls `Things::moved` for chats.
- Sequence: turn 1 is running and "later" is queued as turn 2. You tag `chat lead@… turn 2`. While turn 1 runs you send "also this", the agent takes it, and "later" becomes turn 3. The tag now names the "also this" turn.
  - `held`, the gutter and go-to all point at the wrong turn.
  - The log still holds the old text, so the tag shows "Source" with text that doesn't match the turn it names.
- A `Chats::follow` thread on the old turn number (`t.ordinal == turn`, src/chats.rs ~L200) never sees its turn end. It keeps waiting until the thread closes.
- A second likely problem: everything already in the turn after the injected item moves to the new turn. That includes text the agent streamed between `inject` and the "injected" reply. On replay that text belongs to turn 1, so message numbers in turn 2 differ after a reload, which is what finding #16 was meant to fix. To confirm: check whether the agent replies to the steering request before it actually reads the message.

**2. A database at layout 10 now refuses to start.** `crates/nah-state/src/migrations.rs`
- Migration 10 was deleted, so `LATEST` is 9. Any `state.sqlite` already migrated by a branch build d4c5b67..772aa54 hits `found > LATEST`, returns `StateError::Newer`, and the server exits.
- The local `.nah/state/state.sqlite` is at 9, so it isn't affected. Any other checkout that ran those builds is.

**3. Tagging the same code lines twice gives duplicates (a regression).** `crates/nah-tags/src/lib.rs` `tag()` L293/L302
- The old code compared `current(stored)` with the new subject. The new code compares the stored subject as it was last refreshed.
- Sequence: tag `src/a.rs lines 10-12`, then insert 3 lines at the top. Before the next ingest refresh, tag `src/a.rs lines 13-15`. The stored entry is still (10,12), so it isn't seen as the same thing and a second entry is added. After the refresh, `tag.on` holds the same lines twice, and `untag` removes only one.
- Terminal lines are safe here: their refresh runs synchronously.

**4. The running-log cap is skipped when output is already past the cap.** `crates/nah-things/src/lib.rs` `log()`, with `src/terminals.rs` `follow()`
- If the first chunk exceeds `grown_most`, `more` returns false and `follow` returns false. `log` then deletes the capped file and writes `owner.now()` in full, with no cap.
- After that the command is no longer followed, and nothing says the rest is missing.
- Example: `running_log_mb = 1`, then tag a running build that has printed 2 MB of scrollback. You get a 2 MB file that stops at that moment, with no "rest isn't kept" line.

**5. A chat turn's end can be missed.** `src/chats.rs` `follow()` ~L190
- `running()` sees the turn as not finished, then `hub.subscribe()` is called. If the turn ends between the two, the end is never seen.
- The log keeps only what was there "so far", and the thread stays blocked until `ThreadClosed`. Terminals guard against this; chats don't.

**6. Code snapshots of tags this build can't read get deleted.** `src/code_tags.rs` `noted()`, `nah-tags` `subjects()`
- `subjects()` leaves out tags this build can't read, so `Code::noted` drops their snapshots at start-up. A newer build's tag on code lines loses its snapshot. Logs are protected against this (#19); snapshots aren't.
- Latent: `register()` calls `tell_noted` straight away. Any kind of note registered after `Code` (discussions, later) will have its snapshots dropped before it registers.

**7. `POST /api/tags` accepts any `Subject` without checking it exists, for owners that keep their things.** `src/routes/tags.rs` L73, `nah-things` `note()`
- `note()` checks existence only through `settle` or the log write. These all get kept and show as gone straight away:
  - `PlanItem` (owned, but `now` is None and `there` is false; Part C isn't built)
  - `Plan { plan: "nope" }`
  - a `PlanSection` with an unknown section
  - `CodeLines` past the end of the file

**8. Search: the header line uses up a file's match limit, and some searches redo work every time.** `nah-things` `find_kept`
- `per_file: most` also counts the `== <tag> · <what> · tagged by …` header. A search for the tag's name loses one real match per file once `most` is reached.
- `growing` entries are never removed when a command ends. Every search marks every file ever followed as changed, so FFF re-reads all of them.
- If FFF never lists some log files (for example a global `*.log` ignore), `listed.len() != ours.len()` stays true, and every file is passed to `changed()` on every search.
- `has()` doesn't trim the query but `containing()` does, so the two search paths match differently.

**9. Some names don't round-trip (#25).**
- `chat session <id> turn N` can never be named back: `Chats::named` searches only slots' sessions, and that name is given exactly when the session is no slot's.
- `CodeLines` paths that contain ` line ` or ` lines ` are split at the wrong place by `split_once`.

**10. Text that isn't detail can be folded as detail.** `src/chats.rs` `turn_text`
- Only user and assistant messages are escaped. A line in the text of a `Notice`, an `Error`, a `CommandExecution` command, or a tool name that starts with `DETAIL` gets folded as detail (#29/#50).

**Also:** A.5 isn't done. `.nah/ui/state.json` is still on disk: nothing moves or removes it (the layout is in the database), and `.nah/.gitignore` still lists `ui/`.

**Areas where I found nothing:**
- Lock order between `followed` and `shell`, and the terminal command ending while it's being noted (#22).
- A cleanup racing a new note, in `tag()`'s re-check, `let_go`, and the `handed` set.
- Log headers being imitated (#60), and counting lines without reading the whole file (#32).
- Plan subject naming against the cockpit's section keys.

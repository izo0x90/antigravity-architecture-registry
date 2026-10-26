# Stopping point 4: faithfulness reviewer

I reviewed the change read-only (`git diff b29fbfc..HEAD`, plus the code around it). The plan's own "As built" section (dev_docs/tags-everywhere-plan.md, about lines 360–420) records several deviations. Below, "recorded" marks those. Everything else is unrecorded. Ranked, most severe first.

1. **A running command's tag log has no size cap (A.1: "a new one past a size (8 MB)").** `crates/nah-tags/src/logs.rs:111` (`grow`) gives a growing thing its own file and never splits or limits it. Tagging `tail -f app.log`, `yes`, or a dev server's output writes to `.nah/state/tagged/` until the command ends, with no limit. Reading it back loads the whole file into memory: `Logs::read` (logs.rs:130) via `Tags::said` (lib.rs:262). `tag_read` with `thing` does this even when only `lines: "1-2"` is asked for (`src/mcp/sets/tags.rs:158`). The result is a full disk (the disk is small and shared) and a server out of memory. **High.** Unrecorded.

2. **Chat turn numbers change after a reload, and nothing handles it (D.1).** The D.1 check found the problem and recorded it: a message added to a running turn becomes two turns when replayed, so later turns renumber. The build went ahead anyway. After that reload, `ChatTurn` / `ChatMessage` point at a different turn:
   - `there()` (default `now().is_some()`) is true, so the read says "as it is now" while the text comes from the log.
   - The chat gutter (`chat/things.rs` `tag_drawn_at`) marks the wrong turn.
   - Go-to (`chat/nav.rs` `show_target`) lands on the wrong turn.
   **Medium-high.** Recorded as a known problem, not fixed.

3. **Chat things don't come back whole (rule 3, D.2/D.3).**
   - A message or turn tagged while the agent is still answering is logged as it was then. Chat has no `follow`, and `said()` always prefers the log, so the tag stays partial forever. Recorded.
   - Unrecorded: `turn_text` (`src/chats.rs:87-106`, `_ => {}`) leaves out reasoning, proposed plans, todo lists, tool call inputs, file diffs (only the path is kept) and `Unknown` items. So once the session is gone, `full` cannot give "everything" (D.3), and D.2's "every item of it" doesn't hold.
   **Medium.**

4. **Code doesn't actually "last" for tags (E.3, rule 3).** Unrecorded.
   - Lines changed since tagging: `now()` (`src/code_tags.rs:130-137`) returns today's file text at the old line numbers, not the snapshot's text, though the snapshot is kept. `said()` then returns `Held::Gone` with that text, so `tag_read` prints "(…; gone):" followed by unrelated current lines. The test only checks the name ("changed since tagged").
   - A deleted component, or lines in a deleted file, give nothing. `settle` (line 176) snapshots only `CodeLines`, never components, and gone files are never read from the snapshot.
   **Medium.**

5. **Planned steps replaced by cheaper ones (recorded, but the plan doesn't say the user agreed):**
   - A.2 "moves the one kept tag over (its text into a log)": migration 9 (`crates/nah-state/src/migrations.rs:40`) deletes `said`, so that tag's text is lost.
   - A.3 "Events carry what changed": not built. `TagUpdated { tag }` still sends the whole tag.
   - A.4 "comes back whole after a server restart": the test restarts the shell, not the server (`tests/terminals_tests.rs`, `a_command_tagged_while_it_runs…`).
   - A.1 "Searchable with … fff-search": not used. A new MCP tool, `tag_find`, does its own substring search instead.
   - **Medium** as a process matter: the memory rule is "ask before changing the plan".

6. **Levels from the Agreed section are missing (unrecorded).**
   - "Chat: session, then turn, then message": there is no session subject.
   - "Code: file, then symbol, then lines": there is no file subject.
   - "A selection means exactly the lines selected": the code view has no selection. `code.rs:1071` `things` offers only the cursor's single line plus the definitions around it, so multi-line `CodeLines` can only be tagged through MCP.
   **Medium-low.**

7. **A finished terminal command tagged late is logged cut short, with no note saying so.** `src/terminals.rs:468`: `command_output` keeps only the lines still in scrollback, and gives empty output while a full-screen program is open (`unwrap_or_default`). Either way the log keeps the shortened text with no "(N lines went…)" note, unlike the follow path. Trigger: run `seq 1 20000`, wait, then tag the command. **Low-medium.**

8. **Code tags: cost and leftover snapshots.** Every `ArchEvent::Changed` calls `refresh()` (`crates/nah-tags/src/lib.rs:304`). With the tags lock held, that runs a git diff per tagged line range in each of `moved`, `name` and `there`. It can also take a new snapshot (`code_tags.rs:64/185`). These snapshot refs are never dropped, even on untag. **Low-medium**; to confirm, time `refresh` with about 50 tagged line ranges in a large repo.

9. **`tag_read`'s `full` flag is ignored without `thing`.** The list form always folds detail (`src/mcp/sets/tags.rs:180`), though the parameter's description says "Everything". **Low.**

10. **Agents aren't told how to name chat or code things.** The `tag` tool's `subject` description (`src/mcp/sets/tags.rs:26`) gives only the terminal forms, not `chat <slot> turn N message M`, `component <id>` or `<path> lines a-b`. Agents can't discover them except by reading an existing tag. **Low.**

11. **Built without the plan asking:**
    - Migration 10 (migrations.rs, `(10, …)`) converts a `kept.pieces` layout that only ever existed on this branch (124473b, removed in d4c5b67). It keeps the first piece and drops the rest. It is a compatibility layer, which the user's no-shims rule disallows.
    - New keys `Space k r/c/n/x/d/o`, recorded as ★ awaiting the user.
    - The Tags view's empty-state text (`crates/cockpit-ui/src/tag_list.rs:171`) still says only terminal things can be tagged.
    **Low.**

**No problems found in:**
- B (levels at the cursor, `Tab` cycling, the picker's top line, tests).
- C.3 (scrollback is a setting).
- C.4 (the 50,000-line test).
- F.1 gutter: the graph has no gutter, recorded.
- F.2 / F.2a (Tags view folds, row buttons, solid tag icon, colour swatches).
- F.4 (`#` in the chat box).
- Rule 1 (the database holds no subject text; tested).

**Not reviewed:** G (palette) and H (jump list) were built before b29fbfc and are outside this diff.

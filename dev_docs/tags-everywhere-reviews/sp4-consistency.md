# Stopping point 4: consistency reviewer

Review of b29fbfc..HEAD: consistency with existing patterns. Ranked, most severe first.

1. **`tag_find` brings in a second search grammar, and the search the plan names wasn't used.** In `crates/nah-tags/src/lib.rs:239-262` (`find_text`) and `crates/nah-tags/src/logs.rs` (`Logs::find`), matching is written by hand: `line.contains(query)`, case ignored unless the query has capitals, the whole query treated as one phrase.
   - Plan A.1 says the logs are searched "with the file search already used (`fff-search`)". In this codebase that is `arch_search::RepoFiles::lines` (`crates/arch-search/src/files.rs:139`), and `arch_search::rank_text` (`lib.rs:85`) has the same matching for in-memory text.
   - The MCP `search` tool (kind `text`) says "Query words all must match; 'word … ^word … !word excludes". `tag_find` with `make error` finds only lines containing that exact phrase, so the same query gives different results in the two tools, and the operators silently fail.
   - Severity: medium. Agents get wrong or empty answers from the tool meant for this.
   - The `nah-tags` dependency lint (`lib.rs` test at the end: only harness-contracts and harness-core allowed) explains why it was hand-rolled. The matcher could be passed in from `src/mcp/sets/tags.rs`.

2. **Two namers for the same thing.** Rule 2 says the owner names a thing. The server's `Resolve::name` and the cockpit's `Navigable::things` words disagree for every subject, so the tag picker's top line and the Tags view or `tag_read` name one thing two ways:
   - Terminal: `terminal/mod.rs` `things` says "lines 100–1023" (en dash) and "command 3 and its output: make". `src/terminals.rs` `name` says "lines 100-1023" and "terminal t1 command 3".
   - Code: `code.rs:1069-1083` says "line 12 of src/a.rs" and the short name. `src/code_tags.rs` `name` says "src/a.rs line 12" and "fn foo (src/a.rs:40)".
   - Chat: `chat/things.rs` says "message 2 of turn 3: …". `src/chats.rs` says "chat lead turn 3 message 2".
   - Also, `named()` parses only the hyphen form. A user copying "lines 100–1023" from the picker into an agent prompt for `tag`/`untag` gets "isn't a line number". Severity: low to medium.

3. **The code view gets a second bar strip next to its own coloured bars.** `app/draw/host.rs:44-51` puts a 17.5px host gutter left of every `tag_gutter()` view. The code view returns `true` (`code.rs:1096`), but it already draws 3px change bars in its own gutter (`code.rs:531`, `change_marks`: ok/info/error).
   - Plan F.1 says "the code view's gutter takes it".
   - The default theme's tag colours sit next to the change colours: `tag_4` #a3e635 beside `ok` #4ade80, `tag_5` #38bdf8 beside `info` #7cc4ff, `tag_6` #fb7185 beside `error` #f87171.
   - Result: a tagged, modified line shows two near-identical bars a few pixels apart. Severity: low to medium (misreading which lines changed).

4. **The second-stage picker is built a third way.** The existing pattern keeps "which one" in desk state and opens a bound action by id: `goto.pick` through `desk.choosing` and `bound_actions()` (`app/going.rs:118`), `tag.goto.pick` through `desk.tag_picking` and `run_by_id`.
   - `TagEditWhich` (`app/tagging.rs:229-242`) instead hand-builds a `Bound { leader: None, keys: [], layer: On }` and opens it directly.
   - `ShellCommand` lost `Copy` so it could carry `TagEdit(edit, TagId)` and `Untag(Option<(TagId, Subject)>)`, payloads that were passed as the picked `value` before.
   - Effect: the opened picker isn't among bound actions, so it shows no key hint. Low severity, but a parallel mechanism.

5. **Chat text is assembled twice on the server.** `src/chats.rs` `running()` and `message_text()` repeat `SlotManager::read` and `text_of` (`crates/harness-core/src/slots.rs:644-660`, `782`), with a different join. `text_of` concatenates text blocks with `""`, `message_text` with `"\n"`. So `slot_read` and `tag_read` give different text for the same assistant message. Low severity.

6. **A third date format.** The log header uses the new `httpdate` dependency ("Tue, 20 Oct 2026 21:13:57 GMT", `nah-tags/src/lib.rs:129-132`). Server timestamps elsewhere are RFC 3339 (`arch_service::service::now_rfc3339`), and `jiff`/`chrono`/`time` are already in Cargo.lock. Someone grepping the logs alongside other server records sees mixed formats. Low severity.

7. **"Who tagged it" has three helpers.**
   - `tags::tagged_by` (cockpit, new): "tagged by you".
   - `tag_list::who` plus inline formatting: "tagged by you 3m ago".
   - `mcp/sets/tags.rs::who`: "the user", and "?" when empty.
   - The cockpit's two should be one. Low severity.

8. **Duplicated literal.** The text "({n} lines went before they could be kept)" is formatted twice: `src/terminals.rs` `tell_followed` and `follow`. If one changes, a running command's log and its first chunk disagree. Very low.

9. **The cycle-key hint is in a different place from the other picker hints.** `palette.rs:605-607` draws "Tab: Tag … instead" as a second title line. The pick and back hints are in the footer line ("Enter submits · Esc goes back", `palette.rs:~326`). Low severity: two places to look for a picker's keys.

10. **Settings value silently clamped.** `src/startup.rs:244` does `scrollback_lines.max(100)`. `scrollback_lines = 0` (or 50) becomes 100 with no error. Every other `.nah/*.toml` value is either used as given or rejected through the `deny_unknown_fields` / `format!(".nah/x.toml: {e}")` error path. Low severity.

Checked and consistent:
- Keys: the `[keymap.list] cycle`, the `k = "Tags"` leader group and the `tag.*` leader bindings all come from `default.toml`. The `k` leader isn't used elsewhere.
- Controls: the Tags view's row buttons use the existing `Hosted::placed`, and the `#` picker reuses `pending_palette` / `take_palette_request`.
- Folds use the shared `fold_here` / `fold_all`, and times use `attention::age`.
- The "every action has a description" rule is covered by the existing `every_action_says_what_it_does` test in `app/tests/bindings.rs`, not only by the new `debug_assert`.
- The `.nah/terminals.toml` loading follows the other settings files.
- The old `tags.*` action ids have no leftover bindings.

Not confirmed: whether `arch_search::RepoFiles` honours a gitignored `.nah/state/` when pointed at it. That decides if finding 1's fix is a drop-in. Scanning a temp folder under a `.gitignore` would settle it.

# Stopping point 4: responsibility reviewer

## Review: separation of responsibility and coupling (b29fbfc..HEAD)

Ranked, most severe first.

**1. Two different rules for "same subject" make code-line tags duplicate, and agents can't untag them.**
- The shared contract treats a snapshot that hasn't been taken yet as matching any snapshot (`crates/harness-contracts/src/tags.rs:109`). `nah-tags` instead compares with plain `==` (`crates/nah-tags/src/lib.rs` ~410 for the duplicate check, ~466 in `untag`). `Code::settle` stamps a fresh snapshot id (`src/code_tags.rs` settle).
- Duplicate: tag `src/a.rs line 12`, edit any file, press `M` on line 12 again. The new snapshot id differs, so the duplicate check misses and the same lines are added a second time. The cockpit's `tags.on` (which uses `contains`) already showed the line as tagged.
- Untag: an MCP `untag` with the name `tag_read` showed (`src/a.rs lines 4-5`) parses through `Code::named` to `snapshot: ""`. That never equals the stored subject or `current()` of it, so the agent gets NotOn every time. The cockpit only works because it sends back the stored subject.
- Severity: high for agents, medium for the user.

**2. Chat names depend on the slot's current session, and nothing renames them when it changes.**
- `Chats::words` (`src/chats.rs`) says `chat lead turn 3` only while that session is the slot's current one. `Chats::named` without `@` resolves to the current session.
- The chats owner never calls `Tags::refresh` (`src/startup.rs`, `chats()` registers only). The stored `what` is renamed only when a terminal resizes or code changes.
- Sequence: tag turn 3 in session A, start session B in `lead`. `tag_read` still shows `chat lead turn 3`. An agent that untags by that name gets NotOn, and one that tags with it tags B's turn 3. Severity: medium-high.

**3. A thing tagged while still growing is frozen, and labelled as current.**
- `Tags::said` and `glimpse` (`crates/nah-tags/src/lib.rs` ~359–380) always read the log when `kept` is set. `held()` separately reports `Source` whenever the owner says it's `there`.
- `Chats` implements no `follow`. So a turn or message tagged mid-answer keeps its partial text forever, and `tag_read` / the Tags view call it "as it is now". This breaks rule 3, and `M` on the turn in progress is the usual case. Severity: medium-high.

**4. Names don't round-trip for components.**
- `Code::name` gives `Function foo (src/x.rs:10)`, but `Code::named` accepts only `component <id>` (`src/code_tags.rs`).
- The MCP schema tells agents to use the names `tag_read` shows, so a component can't be untagged (or tagged again) by its shown name. Severity: medium.

**5. The "detail line" format is a text prefix shared between the chat owner and the MCP layer.**
- `DETAIL` (`"  │ "`, in `harness-contracts/src/tags.rs`) is written by `chats.rs::detail`. `mcp/sets/tags.rs::folded` then folds it in every owner's text.
- Terminal output whose lines start with that prefix gets folded into "(N lines: `full` gives them)".
- `tag_find` numbers raw lines (`Logs::find`, `Tags::find_text`), but `tag_read` `lines` indexes the folded text unless `full` is set. So "line 40" from `tag_find` is a different line in `tag_read`.
- `Tags::expand` (agent prompts) doesn't fold at all, so there are two renderings of the same thing with different rules. Severity: medium.

**6. The code owner's refresh runs at the wrong trigger, on the wrong thread, inside query methods.**
- `startup::code_tags` refreshes on every `ArchEvent::Changed`. That includes PlanEdited, Approved and StageChanged, not only `Ingested`. It runs synchronously inside `ArchitectureService::emit`, on whichever thread edited the plan.
- `refresh` holds the global tags mutex while running a git diff for each code-lines tag. `Code::moved`, which is a query, writes new snapshots (`self.snapshot()`), and every owner's subjects get renamed.
- `Tags::owner` finds the owner by calling `name()`, so `Code::name` runs `now_at` (a git diff) just to answer "is this yours". `tag`, `read` and `held` each call it again.
- Result: every plan edit or approval blocks tag reads and writes, and runs git diffs on the emitting thread. Files the index doesn't ingest never trigger a refresh. Severity: medium.

**7. A second text search beside the existing one.**
- `Tags::find_text` and `Logs::find` are a hand-rolled substring search with their own smart-case rule. Plan A.1 says to search with `fff-search`, which `arch-search` already uses for grep.
- Matching differs from the project's other search: no fuzzy, regex or grep modes. Severity: low-medium.

**8. Each thing is named twice, once by the cockpit buffer and once by the server owner (rule 2).**
- The terminal buffer rebuilds `line N (of command M)` (`terminal/mod.rs` things). The chat gives `message m of turn t: …`, and code gives `line 12 of src/a.rs` or the bare definition name.
- The server owners say `terminal t1 line N (of command M)`, `chat lead turn t message m`, `src/a.rs line 12`, `Function foo (src/a.rs:10)`.
- The tag picker's top line and the Tags view / agents use different words for the same thing, and the formats can drift. Severity: low.

**9. `#` is detected in the chat's draw code.**
- `chat/render.rs` ~330 hard-codes the character and the "start of a word" rule, then requests the shell's `tag.insert` palette.
- This bypasses the keymap and is a different path from the existing `/` mechanism (`Action::on_typing`) for "typed prefix offers something". Severity: low.

**10. Small layer leaks.**
- The generic `actions::Choice` carries a tag-specific `Mark::Tag`.
- `theme::caret_to_end` is text-edit caret logic placed in the theme module.
- Severity: low.

**Clean areas**
- **Buffers naming shell action ids:** buffers reach the shell only through existing interfaces. `placed()`, `take_palette_request`, `Navigable::things` / `drawn_at` / `tag_at` / `typed_in` follow established patterns (the graph already places `nav.back`). The Tags view's own actions are gone.
- **Tag events:** no parallel event system.
- **Chats reading the event hub:** `thread_state` is an established accessor (slots and notify use it).

**Outside this brief, but serious**
- **Path traversal at HEAD:** `Code::named`, `now_at` and `lines_text` join the agent's path onto the project root unchecked. `tag {subject: "/etc/passwd line 1"}` followed by `tag_read` returns the file. The uncommitted change to `src/code_tags.rs` in the working tree fixes it (a check that the path stays in the project, plus a test), but it isn't committed.
- **Chat performance:** `tag_place` clones the whole stream on every call (`chat/things.rs`). `tag_things` (run every frame for the panel's top line) and `tag_drawn_at` (every frame for each chat-tagged subject in the gutter) call it once per item. That is O(n²) item clones per frame on a long chat, whether or not anything is tagged.

Key files: `/Users/izo/code/antigravity_architecture_registry/meta-harness/crates/nah-tags/src/lib.rs`, `/Users/izo/code/antigravity_architecture_registry/meta-harness/src/code_tags.rs`, `/Users/izo/code/antigravity_architecture_registry/meta-harness/src/chats.rs`, `/Users/izo/code/antigravity_architecture_registry/meta-harness/src/startup.rs`, `/Users/izo/code/antigravity_architecture_registry/meta-harness/src/mcp/sets/tags.rs`, `/Users/izo/code/antigravity_architecture_registry/meta-harness/crates/harness-contracts/src/tags.rs`, `/Users/izo/code/antigravity_architecture_registry/meta-harness/crates/cockpit-ui/src/chat/things.rs`, `/Users/izo/code/antigravity_architecture_registry/meta-harness/crates/cockpit-ui/src/chat/render.rs`.

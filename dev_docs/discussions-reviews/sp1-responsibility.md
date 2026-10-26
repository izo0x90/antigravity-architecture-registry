Review of 772aa54..53cd6b5 (meta-harness), covering responsibility and coupling. Ranked, most severe first.

**1. Code snapshots are dropped for tags this build can't read, and whether that happens depends on start-up order** (medium: lost data)
- `crates/nah-things/src/lib.rs:95-97`: `Notes::kept_files` can fail, but `subjects()` cannot.
- `nah-state/src/tags.rs:41-46`: `tags()` silently skips any tag row this build can't parse, so `Tags::subjects` (`nah-tags/src/lib.rs:157`) leaves out every subject of that tag.
- `Things::tell_noted` (`lib.rs:204`) runs on every `register`, `moved`, `let_go` and `forget_unused`. It calls `Code::noted` (`src/code_tags.rs:225-244`), which runs `drop_snapshot` on every "tagged lines" snapshot not in that list.
- Trigger: a tag holding `CodeLines` or `Component{was}` plus one subject this build can't read (a newer build's DB, or a branch switch). At the first `things.register(...)` in `main.rs`, its snapshots are deleted. After that its lines fall back to `Now::At(first,last)` (`code_tags.rs:104`), which gives the wrong lines once the file has changed.
- Finding #19 protects logs in this case through the raw-JSON `kept_files`. Snapshots get no such protection.
- The same destructive callback also fires as each owner registers. That is safe only because `startup::tags` (which calls `noting`) runs before the owners. Any kind of note registered later (discussions) would have its snapshots dropped.

**2. The chats owner doesn't report when turns are renumbered** (medium)
- `crates/harness-acp/src/state.rs:174-205`: `steered` inserts a turn and renumbers every later turn, emitting `TurnUpdated`.
- `src/chats.rs` identifies turns by ordinal only (`turn()`, `there()`, and `follow()` at line 203). It has no `Resolve::moved` and never calls `Things::moved`.
- Trigger: turn 1 is running; you queue a prompt (turn 2, `Queued`, which counts as non-terminal so it is followed when tagged); you tag "chat lead turn 2"; you steer a message into turn 1 and the agent takes it.
  - The steered message becomes turn 2 and the queued prompt becomes turn 3.
  - The tag, the gutter mark and go-to now point at the steered turn.
  - The follower thread waits for ordinal 2 to end, so the tag's log is filled with the wrong turn's text.
- Tags on `ChatMessage{turn:1, message:≥2}` that moved go to `Gone`/`Kept` the same way.

**3. The shared layer is worded for tags and terminals** (low–medium; breaks plan rule 2)
- `nah-things/src/lib.rs:340`: `fold` writes "tag_read's `full` gives them" into any note's read. It used to be in the MCP tag set. When discussions read through `fold`, agents will be pointed at the wrong tool.
- `nah-things/src/lib.rs:378-379`: the cap message says "a running command's log". It is also written for chat turns, since `Chats::follow` grows too.
- That cap is set by `running_log_mb` in `.nah/terminals.toml`, parsed in `startup::terminal_settings`, so a terminals setting silently caps chat logs.
- Log names are hard-coded `tagged-N.log` in one folder (`logs.rs:43-49`; `Things::open` takes one dir). The decided layout keeps discussions' text in `state/discussions/`, which this layer can't yet do.

**4. The server copies the cockpit spec view's card sections** (low–medium)
- The section list now exists three times: `src/plan_things.rs:106` (`SECTIONS`), `cockpit-ui/src/spec.rs:59-75` (`Section::key`), and the MCP `tag` description (`src/mcp/sets/tags.rs:58`). Nothing ties them together.
- The cockpit sends `PlanSection{section: entry.section.key()}` as JSON, bypassing `named`'s check. Adding or renaming a cockpit section gives tags that `PlanParts::there` (line 175) reports as gone at once.
- `PlanParts::section` re-renders what the view draws, including running `arch.validate` for "findings" and the diff for "changes". Its text isn't what the user saw. Even within the file, "logic" has two formats: `card()` at line 96 drops step descriptions that `section()` at line 45 keeps.

**5. A second way to read code from disk** (low; unconfirmed)
- `src/plan_things.rs:28-34` (`code`) does its own `read_to_string(root.join(anchor.file))`.
- `Code::lines_text` / `Code::file` (`src/code_tags.rs:88-92, 109`) already do this, with checks: canonicalize, stay inside the root, respect `.nah/ignore`.
- Plan "code" sections skip those checks. A symlink out of the repo, or an ignored file, would be read through the plan owner. To confirm: check whether ingest anchors components in symlinked or ignored files.

**6. Two names for plan parts in the cockpit** (low; finding #25)
- `cockpit-ui/src/nav.rs:136-140`: `Target::describe` says "open's logic in plan quick-look".
- `Subject::name` says "plan quick-look component rust:app::open section logic".
- The go-to note and the picker therefore name the same thing differently.

**7. The name grammar is written in one place and parsed in another** (low)
- `harness-contracts/src/tags.rs:208-227`: `Subject::name` formats every kind.
- Each owner's `named` parses its own kinds back by hand.
- They already disagree: `PlanParts` owns `PlanItem` and `Subject::name` emits "plan X item Y", but `PlanParts::named` (`plan_things.rs:143-170`) rejects it. Its error message doesn't list items, and `now`/`there` return None/false for it. It can't be reached until Part C.

**8. Each `let_go` re-reads every note store** (low; performance)
- `Things::let_go` (`lib.rs:398-413`) calls `kept_files()` (a full JSON scan of the tags table) and `tell_noted` once per call.
- `Tags::delete` (`nah-tags/src/lib.rs:383`) calls it once per thing, up to `MOST_ON` = 500 times.

**Areas where I found nothing:**
- Cockpit shell and buffers: the shell names no buffer type. `notes.rs` goes only through `Navigable::things`, `tag_at` and `draws_things`, and the spec view reaches out only via `Ask::Source`.
- No new keys, permissions or events. MCP still reaches the things layer, but only through `Tags::things()` to fold reads.
- `notes.rs` is tag-only for now (`TagHere`, `row`). It calls itself "one way for every kind of note", but the second mark for discussions isn't built.

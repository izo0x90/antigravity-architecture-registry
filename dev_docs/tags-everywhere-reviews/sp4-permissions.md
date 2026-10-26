# Stopping point 4: permissions reviewer

Permissions review of b29fbfc..HEAD (meta-harness). Ranked, most severe first.

**1. Critical: any agent with `tags.edit` can read any file the server can read, including the cockpit token, which makes it the user.**
`src/code_tags.rs:143-155` (`Code::named`) builds `Subject::CodeLines` from whatever path the agent gives. The only check is `self.arch.root().join(path).is_file()`. Joining an absolute path replaces the root, and `../` is never stopped. `lines_text` (`:87-91`) then runs `read_to_string(root.join(path))`. The old read paths both canonicalize and check the result stays inside the repo (`starts_with(root)`): `routes/files.rs:38-48` and `arch-service/src/service.rs:395-398`. This one does neither, and symlinks inside the repo aren't resolved either.
Sequence: a `sub` or `worker` agent (the shipped roles have `tags.edit` + `view.read`, `nah-access/config/access.toml`) runs:
1. MCP `tag {tag:"x", subject:"/home/u/.config/nah/cockpit-token line 1"}`, or `"/proc/self/environ line 1"`, which also exposes `NAH_COCKPIT_TOKEN` and any API keys in the server's environment.
2. `settle` takes a snapshot, which works in any git project. `lasts` is true, so nothing goes to the logs.
3. MCP `tag_read {tag:"x"}` (`view.read`) returns the text through `Tags::glimpse` → `Code::now`. `now_at` never stops it: if `changes.file` errors on an outside path, the result is `Now::At`, and `lines_text` also runs for `Changed`.

The agent now has the cockpit token, so it can act as `Principal::User` with every capability, including `shell.drive` and approvals. `cockpit.rs:9-10` leaves "agents reading the token file" to the harness sandbox, but here the unsandboxed server reads it for the agent. Inside the repo the same hole reads files the indexer ignores (`.env`, `.nah/` config). `tag_find` (`view.read`) also searches these files' contents.
Fix: in `named`, canonicalize and require the result to be inside the canonical root (as `code_analysis` does). Do the same check again in `now_at` / `lines_text` for subjects already stored. Also consider honouring the ignore rules.

**2. High: tags let roles without `slots.observe` read any agent's chats, tool outputs included.**
`src/chats.rs:135-155` (`Chats::named`) resolves `chat <any slot> turn N [message M]` for every slot. That includes the lead, queue workers, and any past session by id prefix: `chat lead@ turn 1` matches with an empty prefix. `tag_read {full:true}` returns the whole turn. That includes every tool call's output and `CommandExecution` output (`turn_text`, `:87-110`). `tag_find` searches them.
The only existing tool for this is `slot_read`. It needs `slots.observe`, which only the lead has, and `self.sub()` limits it to sub-slots, the last N messages, and no tool outputs (`src/mcp/sets/slots.rs:133-183`).
Sequence: a `sub` agent runs `tag {tag:"peek", subject:"chat lead turn 3"}`, then `tag_read {tag:"peek", thing:1, full:true}`. It gets the lead's prompts and whatever its tools printed (file contents, command output, secrets the user approved it reading). Tagging also copies the turn into `.nah/state/tagged/`, where it stays for later reads.
Fix: when the caller isn't the user, check `slots.observe` (and the `sub()` scope) at tag/read time for `ChatTurn`/`ChatMessage` subjects. Otherwise the tag tools are a second way into slot reading.

**3. Medium: an agent can make the server fill the disk or run out of memory through a tagged running command.**
Tagging a running `TerminalCommand` gives it a file of its own through `Logs::grow` (`nah-tags/src/logs.rs:110-120`). `Terminals::follow` (`src/terminals.rs:510-522`) writes to that file until the command ends. There is no size cap: the 8 MB `FILE_MOST` applies only to shared files.
`tag_read {thing:n}` calls `Tags::thing` → `said` → `logs.read(kept)`, which reads the whole file into memory before `READ_MOST` trims it (`src/mcp/sets/tags.rs:154-166`).
Sequence: the user is running `tail -f`, `yes`, or a long build. A `worker` agent runs `tag {subject:"terminal t1 command 7"}`. The log grows without limit on a small shared disk. Each `tag_read` of that thing loads all of it.
Fix: cap a growing file (stop following and note it was cut), and read it in bounded windows.

**4. Low: agents can name and read terminal outputs from earlier shells.**
`terminal t1@<shell>` subjects let `tag`/`tag_read` reach output from earlier shell starts. `terminal_read` doesn't serve these, but it's the same capability (`view.read`) and the same data, so there's no new permission boundary. Mentioned only for completeness.

**Areas with nothing found:**
- **Routes:** `src/routes/` is unchanged in this range. Every tags route takes `Needs<can::ViewRead>` or `Needs<can::TagsEdit>` first, and `principal_of` only ever gives User, Home or Anonymous there, so agents can't post raw `Subject`s through it.
- **Agent tool declarations:** `tags`, `tag_read` and `tag_find` declare `ViewRead`; `tag`, `untag` and `tag_edit` declare `TagsEdit`. The snapshot in `tests/snapshots/mcp_tools.txt` matches.
- **Home relay and sockets:** nothing new in this range.
- **Tag logs:** the folder is 0700 and files 0600, links aren't followed, and `Kept.file` is written only by the server (agents can't set it), so there's no traversal through log names.

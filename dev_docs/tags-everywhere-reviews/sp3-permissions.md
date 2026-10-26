# Stopping point 3: permissions reviewer

Permissions review of d4c5b67..HEAD (meta-harness)

The diff adds no route, socket or agent tool. On the server side it only changes `crates/nah-tags/src/logs.rs` (the chmod). Every new cockpit tag action (rename, colour, note, clear note, delete, take off, `#` insert) goes through `TagAct` → `net.tag/untag/edit_tag/delete`. Those reach routes that already check capabilities in `src/routes/tags.rs`: `list` and `read` need `can::ViewRead`; `tag`, `untag`, `edit` and `delete` need `can::TagsEdit`. The agent tools in `src/mcp/sets/tags.rs` declare the same capabilities (`tags`, `tag_read`, `tag_find` → ViewRead; `tag`, `untag`, `tag_edit` → TagsEdit). Agent changes are recorded under the slot's label. The diff's tests send only to those existing endpoints, and no new `net.` call appears in it.

Ranked findings:

1. **A tag edit's confirmation can act on a different tag than it names. Medium: delete is permanent.** `crates/cockpit-ui/src/app/tagging.rs:287-299`, with `crates/cockpit-ui/src/tag_list.rs:86`.
   - **The cause:** the Delete confirmation is built for tag X ("Delete X (from N things)"), but the value it sends back is only `"yes"`. When it runs, `TagEdit(Delete)` looks up the tag again with `tag_to_edit(here)`, from wherever the cursor is at that moment. `desk.tag_editing` pins a tag by id only on the `TagEditWhich` path, i.e. only when several tags were at the cursor.
   - **How it goes wrong:**
     1. Tags view, cursor on tag X's row, the user presses Delete. The confirmation is open.
     2. Meanwhile X is deleted elsewhere: an agent with `tags.edit` (`sub`, `worker` and `lead` all have it) uses `tag_edit delete`, or the user does it from a second cockpit.
     3. `TagList::sync` no longer finds `Row::Tag(X)`. It keeps the same index (`unwrap_or(self.cursor)`), so the cursor is now on the next tag, Y.
     4. The user picks "yes" and Y is deleted.
   - Rename and Note do the same while their text box is open: the typed name or note goes onto Y.
   - In a terminal or chat view, if another tag lands on the same line during the confirmation, `tag_to_edit` returns `None` and the confirmed action silently does nothing.
   - **Fix direction:** carry the tag id in the action's value (or in `tag_editing` for every opening), not in the cursor.
   - **To confirm:** a test in `app/tests/tags.rs` that opens `tag.delete`, delivers a `TagRemoved` for X, then picks "yes" and checks which `DELETE /api/tags/…` is sent.

2. **`cockpit_run`, `cockpit_keys` and `cockpit_type` let an agent act as the user beyond its role. Medium, but older than this change; noted because the brief asks about other ways to the same effect.** `src/mcp/sets/cockpit.rs:37-58` sends calls to the cockpit through `/ws/cockpit` (`src/routes/cockpit.rs:21`, a ticketed socket on purpose). The cockpit then runs them with its own credentials, so the server sees `Principal::User` with every capability (`src/cockpit.rs:130`).
   - An agent with `view.drive` (the `lead`, while the user has turned on "Let agents drive the cockpit") can therefore:
     - type into terminals: `shell.drive`, which no role has;
     - answer requests and approve plans: `requests.answer`, `plans.approve`;
     - stage changes: `changes.stage`;
     - prompt other slots with `#tag` expansion: `sessions.control`.
   - None of these are in the lead's role. Each change is recorded as by "user" (`src/routes/tags.rs:63`), so tag edits made this way look like the user's.
   - Nothing in `crates/cockpit-ui/src/remote.rs` limits which action ids an agent may run.
   - This diff adds no new reach here: the new tag actions need `tags.edit`, which the lead has, and the chat box could already be typed into. If an agent driving the cockpit is meant to stay within its role, that is a design decision.

3. **The chmod in `Logs::open` follows symlinks. Low.** `crates/nah-tags/src/logs.rs:48-54`.
   - `dir.exists()` and `std::fs::set_permissions` both follow symlinks, and the folder is `<repo>/.nah/state/tagged` (`src/startup.rs:265`).
   - If a repository commits `.nah/state/tagged` as a symlink (the `.gitignore` doesn't stop committed files):
     - start-up chmods the link's target to 0700;
     - the server writes terminal and chat output for tagged things into the target;
     - `forget_all_but` deletes `tagged-*.log` files there.
   - The writing and deleting were already there before this change; the chmod is new.
   - **Fix direction:** check `symlink_metadata` and refuse a link.
   - **To confirm:** run start-up with a symlinked `tagged`.

4. **0700 on the tag logs doesn't keep them from agents. Low, as designed.** Agents run as the server's OS user, so a slot's own file tools can read `.nah/state/tagged/*.log` (terminal output that was tagged) without `view.read`. That includes an agent in the `external` role (`can = []`) running on the same machine. `access.toml` says harness tools are outside NAH's capability checks, so this is known, but the commit message "made the server user's only" means only other OS users are kept out.

Areas with nothing real found:
- **Routes:** all tag routes take `Needs<…>` first; the diff adds no routes.
- **Agent tools:** every tag tool declares its capability. No agent tool wraps a cockpit-only tag action; delete and edit are `tags.edit` on both paths.
- **Home relay:** no new relayed path. Home is `Principal::Home` with every capability, as before.
- **`#` in the chat box:** `TagInsert` only appends a tag's name to the chat's own input buffer. Only `chat/nav.rs` implements `typed_in`, so nothing reaches a terminal. Expansion still happens on the server in `prompt`, which needs `SessionsControl`.
- **What agents can read:** the diff adds no agent-readable data. The gutter, the hover names and "tagged … when" are drawn in the cockpit only.

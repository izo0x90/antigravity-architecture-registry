# Review of fcb0865..HEAD (logins, round 2), permissions

Five problems, most severe first. The routes are user-only (`Needs<…>`, cookie or home bearer). `/auth/cancel` takes `can::SessionsControl`, the same as begin, complete and logout. The hand-back reads at most 64 KB, drops the reply and sends only to the port it checked. The other ways I checked for reading a kept login (static files, the tags' and links' `file <path>`, which leave out hidden paths, and `/api/fs/read` with `..` or a lexical path) are closed.

## 1. An agent's file access refuses only its own current kept home, not `.nah/harness/` (or `.nah/state/`)

`crates/harness-acp/src/profile.rs:183` builds `WorkspaceFs::new(vec![cwd], self.kept_home.iter().cloned().collect())`. The private list is this one harness's `kept_home`, taken from its current `home` setting. `crates/harness-acp/src/workspace.rs:26-31` refuses only that list. `/api/fs/read` (`src/routes/files.rs:50`) uses `arch_service::private_dirs` (`.nah/state` and `.nah/harness`), but the agent's file access does not. So the two disagree about what is private.

Sequence A (one harness reads another's login):
1. `.nah/harnesses.toml` has `[claude-code] home = "kept"` and `[antigravity] home = "kept"`. Claude Code is signed in, so `.nah/harness/claude-code/.credentials.json` exists (Linux).
2. An Antigravity session (`client_fs = true`) asks `fs/read_text_file` for `.nah/harness/claude-code/.credentials.json`.
3. `allowed()` compares the path with `[<project>/.nah/harness/antigravity]` only. The read succeeds, and the agent has Claude Code's OAuth tokens.

   In `auto_accept_edits` this needs no approval, because `Profile::auto_approve` allows `ToolKind::Read` and `Edit`. A `write_text_file` to the same path plants a login just as easily.

Sequence B (a former kept home stops being private):
1. Antigravity runs with `home = "kept"` and signs in. The token is now in `.nah/harness/antigravity/antigravity-acp/acp_token.json`.
2. The setting changes to `home = "native"` (by the user, or by an agent; see Decide 1), and the instance is refreshed (`refresh_instance` → `reconfigure`).
3. `entry.home` is `None`, so `kept_home` is `None` and the private list is empty. The old token is still on disk, and any Antigravity session can now read it through `fs/read_text_file`. `/api/fs/read` still refuses it.

The same gap leaves `.nah/state/` (the discussions' and tags' logs, `state.sqlite`) readable and writable through the agent's file access, which `private_dirs` says is "never served".

## 2. One write to `.nah/.gitignore` puts kept logins into git snapshots and the changes tool

The only thing keeping `.nah/harness/` out of the changes view, the timeline's snapshots and the `changes` MCP tool is the `harness/` line in `.nah/.gitignore`. That file is not private to the agent's file access. `ensure_nah_gitignore` (`crates/arch-service/src/store.rs:146`) checks only that a line equal to `harness/` exists, so it never repairs a negation added after it.

Sequence:
1. Antigravity has a kept home and is signed in. A prompt-injected Antigravity session, in `auto_accept_edits`, calls `write_text_file(".nah/.gitignore", "...\nharness/\n!harness/\n")`. That is an Edit, so it is auto-approved, and the path is outside the private list.
2. The turn ends. `Timeline::turn_ended` → `Changes::snapshot` → `index.add_all(["*"])` (`crates/nah-changes/src/timeline.rs:44`) adds `.nah/harness/antigravity/antigravity-acp/acp_token.json` to a commit under `refs/nah/snapshots/`. The token stays in the repository's objects until that snapshot is pruned.
3. The agent calls the `changes` MCP tool (`Capability::ViewRead`, which agents hold) with `path: ".nah/harness/antigravity/antigravity-acp/acp_token.json"`. With the files as one side, `show_untracked_content(true)` (`crates/nah-changes/src/compare.rs:53`) returns the whole file as added lines. The agent reads the refresh token without any shell command or approval.
4. A restart does not repair the gitignore, because the `harness/` line is still there. The cockpit's changes view shows the file to every device, and a user's `git add -A` commits it.

This bypasses round 1's fix 4 ("out of git, so out of the changes view, the timeline's snapshots and the changes tool") through the agent's own file access. Decide 2 (agents with a shell) does not cover it.

## 3. A dangling symlink passes the private check, so a login can be planted in a kept home

`resolve_client_file_path` → `canonicalize_ancestor` (`crates/workspace-fs/src/lib.rs:76-79`) treats a dangling symlink as a missing file. It canonicalizes the parent and appends the link's own name, so `resolved` is `<root>/<link>`. That passes both the root check and `WorkspaceFs::allowed`. `write_client_text_file` checks symlink targets only when `fs::canonicalize` succeeds (`:151`), which fails for a dangling link. `fs::write` (`:168`) then follows the link.

Sequence:
1. The project contains a committed symlink `notes/cache.json -> ../.nah/harness/antigravity/antigravity-acp/acp_token.json`, which dangles while signed out.
2. Antigravity uses `home = "kept"` and is signed out. A session writes `notes/cache.json` with an attacker's token JSON.
3. The write lands in the kept home. On its next start Antigravity is signed in to the attacker's account, and the project's work goes to it.

The same hole lets the write reach any path outside the session's root. The new private check inherits it.

## 4. A kept home reached through a symlink loses every protection but the agent's file access

`make_home` (`crates/harness-acp/src/driver.rs:33`) uses whatever already exists at `.nah/harness/<instance>`, including a symlink. `/api/fs/read` compares the file's canonical path with the lexical `<root>/.nah/harness` (`src/routes/files.rs:50`; `private_dirs` does not canonicalize). Git's ignore rule is also lexical.

Sequence:
1. A shared repository commits `.nah/harnesses.toml` with `[antigravity] home = "kept"`, and a tracked symlink `.nah/harness/antigravity -> ../../docs/ag`. A tracked file inside an ignored folder stays tracked.
2. The user signs in. The cockpit shows the home as kept. The token is written to `docs/ag/antigravity-acp/acp_token.json`.
3. `GET /api/fs/read?path=docs/ag/antigravity-acp/acp_token.json` serves it, because the canonical path is not under `.nah/harness`. It also shows as untracked in the changes view and the `changes` tool, and every turn's snapshot keeps it. `git add -A` commits it.

Only `WorkspaceFs` (which canonicalizes the private dir) refuses it.

## 5. The hand-back always connects to 127.0.0.1, whatever loopback host the link named

`callback_port` accepts a `redirect_uri` on `localhost` or `[::1]` (`crates/harness-acp/src/auth.rs:340-355`, and its own test uses `[::1]:4001`). `hand_back_callback` checks only `loopback(&url)` and the port, then connects to `("127.0.0.1", waits_on)` (`:377`).

Sequence:
1. An agent's login prints `redirect_uri=http://[::1]:4001/` and listens on `[::1]:4001` only.
2. Another local process (any agent's shell command) listens on `127.0.0.1:4001`. A port number is free separately in each address family.
3. The user pastes `http://[::1]:4001/?code=…&state=…`. The server sends `GET /?code=…&state=…` to the other process, which now has the authorization code. The login is never finished and runs out.

With nothing on `127.0.0.1:4001`, such a login can never be finished: "the sign-in isn't waiting any more". The hand-back reaches a socket other than the login's own.

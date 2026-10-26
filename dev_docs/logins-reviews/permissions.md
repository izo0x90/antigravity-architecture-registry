# Review of fcb0865..HEAD (logins), permissions

The sign-in routes themselves are unchanged and still take `Needs<can::SessionsControl>` first (`src/routes/harness.rs:95-111`). No agent tool was added. The paste replay cannot inject headers, because whitespace (CR/LF included) is refused. The problems are in where the kept home sits and in who can reach it.

## 1. A kept home is inside the agent's file sandbox: Antigravity can read and overwrite its own login token

- `src/startup.rs:40` puts a kept home at `<project>/.nah/harness/<harness>/`.
- `packs/harness/antigravity-acp/pack.toml:144` moves `GEMINI_HOME` there, so the token lands at `<project>/.nah/harness/antigravity/antigravity-acp/acp_token.json`.
- Antigravity runs with `client_fs = true` (`pack.toml:110`). Its `fs/read_text_file` and `fs/write_text_file` are served by `WorkspaceFs` with the session's cwd (the project) as the only root (`crates/harness-acp/src/profile.rs:164`, `crates/harness-acp/src/adapter.rs:189-215`). Nothing excludes `.nah/` or the kept folder.

Under `native`, `~/.gemini` is outside that root, and `resolve_client_file_path` refuses it. Under `kept`, the server serves it. Concrete sequence:

1. The project sets `antigravity = "kept"`.
2. The user signs in.
3. A prompt injection (a README or a fetched page) tells the agent to read `.nah/harness/antigravity/antigravity-acp/acp_token.json`.
4. The server returns the refresh token to the agent, which can repeat it into chat or into any web/tool call. The server asks no one.

Writing works the same way. The agent overwrites `acp_token.json` with another account's token, and from then on every session in the project runs on the attacker's Google account, with all prompts and code sent to it.

For Claude Code the same applies through its own Read/Edit tools. `CLAUDE_CONFIG_DIR` points into the project (`packs/harness/claude-code/pack.toml:80`), so `.credentials.json` and the user-scope `settings.json` (hooks, permission allow-lists) now sit inside the folder the agent edits freely. Before, they were outside it.

The plan's "the folder made (the server's user only)" (0700 in `crates/harness-acp/src/driver.rs:49`) protects nothing against agents, because they run as that same user.

## 2. Agents can rewrite `.nah/harnesses.toml` and so choose where every harness's home is

`harness_homes` (`src/startup.rs:25-46`) trusts `.nah/harnesses.toml` completely. Any agent that can write project files can edit it: Antigravity through `fs/write_text_file` (same root as above), Claude Code through Edit. On the next server start, the agent's choice applies:

- **`antigravity = "kept"` plus a planted token.** The agent writes its own `acp_token.json` into `.nah/harness/antigravity/antigravity-acp/` first. After the restart, the probe in `crates/harness-acp/src/control.rs:357-361` reports "signed in" just because the file exists, and the harness runs on the planted account (credential swap without the user signing in).
- **`claude-code = "/home/<user>"` or `"."`.** A value is joined to the project with no check (`src/startup.rs:42`). An absolute path or `..` escapes the project. `make_home` then runs `set_permissions(dir, 0o700)` on whatever folder that is (`driver.rs:49`), and writes a `.gitignore` containing `*` into it when it has none (`driver.rs:53-55`). With `"."` in a project without a root `.gitignore`, the whole project becomes ignored.

No capability guards this setting, and nothing in the change gives agents a reason to need write access to `.nah/harnesses.toml`. The same file's launch overrides (`src/packs.rs:46-60`, re-read on every `POST /api/instances/{id}/refresh`) let an agent that writes it set the harness program to anything. That is older than this change, but `[home]` is now added to a file agents can write.

## 3. The only thing keeping tokens out of git, the changes view and agent tools is a `.gitignore` that may never be written or can be deleted

- `make_home` writes the kept folder's `.gitignore` only `if !ignore.exists()` (`crates/harness-acp/src/driver.rs:53`). A configured path that already has a `.gitignore` not covering everything gets none. Example: `claude-code = "config/claude"`, where `config/claude/.gitignore` lists only `*.log`.
- An agent can also delete `.nah/harness/<h>/.gitignore` (finding 1). It is put back only when the instance is created again.
- `.nah/.gitignore`, which `crates/arch-service/src/store.rs:121-150` maintains for `cache/` and `state/`, does not list `harness/`.

Once the token file is not ignored:

- It is untracked, so it appears in the Changes view, and a stage-all commits it.
- The timeline snapshot (`crates/nah-changes/src/timeline.rs:44`, `add_all` with default options) writes it into `refs/nah/snapshots/…` commits that persist.
- The `changes` MCP tool returns any file's hunks by path to agents (`src/mcp/sets/changes.rs:174-176`), including an untracked `acp_token.json` or `.credentials.json` in full.

## 4. Kept tokens are served to any cockpit session through `/api/fs/read`

`GET /api/fs/read` (`src/routes/files.rs:31-58`, `Needs<can::ViewRead>`) serves any file whose canonical path is inside the repository. That now includes `.nah/harness/antigravity/antigravity-acp/acp_token.json` and `.nah/harness/claude-code/.credentials.json`. Through home it is `/s/{server}/api/fs/read?path=.nah/harness/…`.

So a long-lived refresh token goes to any browser holding the cockpit cookie or home's session: the phone or iPad the plan is built for, a shared device, or a stolen cookie. Under `native` these files were outside the repository and refused with 403. The plan's goal is browser-to-project-server only for the login steps, not for the stored credential.

## 5. The paste is replayed to a port that may no longer belong to the login

`finish_agent` (`crates/harness-acp/src/auth.rs:164-180`) takes the port from `Running::Agent`. Nothing clears that entry when the agent's login task ends: the 300 s timeout, a failure, or the user pressing "Stop signing in", which only clears cockpit state (`crates/cockpit-ui/src/agents.rs`, `Act::StopSigningIn`).

After the login's listener closes, the OS can give that ephemeral port to any other local listener. That includes one an agent starts in its shell (it can learn the port by listing `127.0.0.1` listeners while the login waits). Then `hand_back_callback` (`auth.rs:227-248`):

- connects to that listener;
- sends `GET <user-pasted path>`;
- with a non-2xx reply, returns its status line in the error (`auth.rs:247`); with a 2xx reply, reports the paste as taken.

Sequence:

1. The user begins a sign-in and is slow, past 300 s.
2. An agent binds the freed port.
3. The user pastes the callback address.
4. The authorization `code` and `state` go to the agent's listener.

If the provider does not use PKCE, that code can be exchanged by the agent.

The read is also unbounded with no timeout: `read_to_string` at `auth.rs:243` waits until the peer closes. A listener on that port that streams or never closes holds the request open indefinitely and grows memory without limit.

`Running::Agent` should be dropped when `ended` resolves, and the replay should only happen while the login task is still alive.

## 6. Turning the setting on breaks every harness; existing launch overrides stop the server

`.nah/harnesses.toml` is now read by two parsers that each reject the other's content:

- `own_launches` (`src/packs.rs:55`) reads the whole file as `BTreeMap<String, OwnLaunch>` with `deny_unknown_fields` (`src/packs.rs:34`). A `[home]` table is taken as an instance named `home`: it is missing `program` and has unknown field `antigravity`, so it fails. `harness_launches` then marks every harness `Unavailable` ("fix the file it names"), both at start and on every refresh.
- `harness_homes` (`src/startup.rs:26-28`) uses `deny_unknown_fields` with only `home`. A project that already has an own launch (`[antigravity] program = …`) in that file makes `harnesses()` return an error (`src/startup.rs:184`), and the server does not start.

The test `a_harness_whose_home_the_project_keeps_is_started_with_it_there` (`tests/packs_tests.rs`) never passes that file through `harness_launches`, so it doesn't catch this.

This is not a permission issue, but it means the protections above can only be exercised on a project with no harnesses available. It also means one agent write to the file (finding 2) disables all harnesses or the server.

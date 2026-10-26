# Review of 40c2464 (logins, round 3), permissions

Three problems, most severe first.

What I checked and found closed: the four auth routes take `can::SessionsControl` and act only on the sign-in their `id` names. `/api/fs/read` takes `can::ViewRead` and refuses the private paths as asked, as found, and through a linked `.nah/harness`. Every ACP instance gets `private_paths` from `harness_launches`, and no route builds a `HarnessEntry` from a request. An agent's file access now refuses `.nah/state`, `.nah/harness` and `.nah/.gitignore`. A write through a dangling link is refused, at the last part of the path and in the middle (`create_dir_all` fails on the link). `..` is taken out before the path is resolved, and the file is opened at the path that was checked. The hand-back to an IP literal goes only to that address.

## 1. Only the home's own last folder is checked for a link, so a kept login can still be written outside `.nah/harness/`

`make_home` (`crates/harness-acp/src/driver.rs:34`) checks only whether `.nah/harness/<instance>` itself is a symlink. It does not check a link above it (`.nah/harness`) or a link inside it. Two of the four protections are lexical, or depend on where the file really is: git's ignore rule and `project_file`'s ignore check (`src/code_tags.rs:55`, used by the links' and tags' `file <path>`). So once the login's real location is outside `.nah/harness/`, those two no longer cover it. Round 2's permissions point 4 is still open in both of these forms.

Sequence A (a link above the home, no `git add -f` needed):
1. A shared repository commits a symlink `.nah/harness -> ../docs/ag` and `.nah/harnesses.toml` with `[antigravity] home = "kept"`. `.nah/.gitignore`'s `harness/` matches directories only, and git stores a symlink as a link, not a directory, so the link is committed like any other file.
2. On start, `make_home(".nah/harness/antigravity")`: `symlink_metadata` looks at the last part only, which doesn't exist yet, so it passes. `create_dir_all` follows `.nah/harness` and makes `docs/ag/antigravity`. The cockpit shows the home as kept at `.nah/harness/antigravity`.
3. The user signs in. The token is written to `docs/ag/antigravity/antigravity-acp/acp_token.json`.
4. Nothing ignores `docs/ag/`. Every turn's snapshot (`crates/nah-changes/src/timeline.rs:44`, `add_all`) stores the token. The `changes` MCP tool, which agents hold through `ViewRead`, shows it as added lines. An agent can quote it through a link's `file docs/ag/antigravity/antigravity-acp/acp_token.json`, because `project_file` checks the real path against the ignore rules and that path is neither hidden nor ignored. The user's `git add -A` commits it. Only `WorkspaceFs` and `/api/fs/read` refuse it, because they canonicalize `.nah/harness`.

Sequence B (a link inside the home: the agent's file access and `/api/fs/read` are open as well):
1. A shared repository force-adds a symlink `.nah/harness/antigravity/antigravity-acp -> ../../../docs/ag`. A tracked file in an ignored folder stays tracked, and a clone checks it out. A clone also makes `.nah/harness/antigravity/` as an ordinary directory, mode 0755.
2. `make_home` sees a real directory and uses it as it is, without making it 0700.
3. Antigravity writes its token through the link to `docs/ag/acp_token.json`.
4. An Antigravity session asks `fs/read_text_file` for `.nah/harness/antigravity/antigravity-acp/acp_token.json`. `WorkspaceFs::allowed` (`crates/harness-acp/src/workspace.rs:27-28`) compares the canonicalized path (`<root>/docs/ag/acp_token.json`) with `.nah/harness`, both as written and canonicalized. Neither matches, so the read succeeds, even though the path was asked for under `.nah/harness/`. Unlike `/api/fs/read`, `allowed()` never checks the path as asked. `GET /api/fs/read?path=docs/ag/acp_token.json` serves it too. Snapshots, the `changes` tool and links expose it as in A.

A fix has to check that the home's real path is under the canonicalized `.nah/harness` (and that nothing below it links out), not just its last part. `WorkspaceFs::allowed` should also check the path as asked, as `files.rs` does.

## 2. `.nah/.gitignore` is never repaired when a negation is committed, so kept logins can still go into snapshots and the `changes` tool

The commit makes `.nah/.gitignore` private to the agent's file access. That closes round 2 point 2 only for writes made through that access. `ensure_nah_gitignore` (`crates/arch-service/src/store.rs:152`) still checks only that a line equal to `harness/` exists. The other half of that report, that a negation added after it is never repaired, is still true.

Sequence:
1. A shared repository (the same threat as point 1, which this commit set out to cover) commits `.nah/.gitignore` with `harness/` followed by `!harness/`. A user's `git pull` brings it in the same way.
2. On start, `ensure_nah_gitignore` finds the `harness/` line and leaves the file as it is. The later `!harness/` wins.
3. `.nah/harness/antigravity/antigravity-acp/acp_token.json` is now an untracked file that isn't ignored. Each turn's snapshot stores it, the `changes` MCP tool returns its contents to any agent, the cockpit's changes view shows it to every device, and `git add -A` commits it.

The server has to own this rule, not just check that a line exists. One way is to keep the rule where the project's files can't override it, for example by putting the private paths in the snapshot and changes code's own exclusions. Another is to refuse to start a kept home while a later line re-includes `harness/`.

## 3. A `localhost` hand-back goes to 127.0.0.1 first, whichever loopback the agent listens on

`hand_back_callback` (`crates/harness-acp/src/auth.rs:461-463`) turns a `localhost` callback into `[127.0.0.1:port, [::1]:port]` and connects to the first that accepts. A port is free separately in each address family, so the hand-back can reach a socket other than the login's own. This is the same hole round 2 point 5 found for `[::1]`, still open for `localhost`.

Sequence:
1. An agent's login prints `redirect_uri=http://localhost:4001/` and listens on `[::1]:4001` only. Node 17 and later does this for `listen(port, "localhost")` where `localhost` resolves to `::1` first, as on macOS.
2. Another local process (any agent's shell command, or any other program of the user's) listens on `127.0.0.1:4001`.
3. The user pastes `http://localhost:4001/?code=…&state=…`. The server connects to `127.0.0.1:4001` first, which accepts, and sends it `GET /?code=…&state=…`. That process now has the authorization code. The hand-back counts as delivered (`Ok(Ok(()))`), and the real login runs until its time is up.

Neither shipped pack names `localhost`: Antigravity's link names `127.0.0.1`, and Claude Code's login takes a pasted code instead. So this can only happen with a pack that uses `localhost`.

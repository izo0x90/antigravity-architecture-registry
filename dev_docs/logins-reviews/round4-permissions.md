# Review of 0dea926 (logins, round 4), permissions

Four problems, most severe first.

What I checked and found closed. For a kept home, `make_home` now refuses a link at `.nah`, `.nah/harness` or `.nah/harness/<instance>`. The project it compares against (`AcpDriver.project`, from `repo_root()`) is canonical, and it is the same path `kept_harness_home` is built from, so the `take_while` walks every folder between the project and the home. `home_of` refuses a folder of your own that reaches the project through a link, including on a case-insensitive disk. A dangling link on the way fails at `create_dir_all`, so the harness doesn't start. A literal `!harness/`, `!/harness`, `!harness/claude-code/` (and the same for `cache`/`state`) is dropped at start. Nothing new reaches a login. The new `gone()` and `state.last` carry the same reasons as before. A stopped `Finishing` task drops its `LoginRun`, which is `kill_on_drop`.

## 1. Nothing under a kept home is checked, so a shared repository can still send a login outside `.nah/harness/` or into a tracked file (round 3 permissions 1, sequence B, not fixed)

`make_home` (`crates/harness-acp/src/driver.rs:34-41`) checks the home and the folders above it. When the home already exists, it uses it as it is (`if dir.exists() { return Ok(()) }`). It doesn't look at what's inside, and it doesn't set the mode to 0700. `WorkspaceFs::allowed` (`crates/harness-acp/src/workspace.rs:27-28`) still checks only the resolved path, never the path as asked. The findings' Fixed 5 says "no folder on the way to a kept home may be a link". But the folders the login is actually written through are below the home, and nothing checks those.

Sequence (a link inside the home):
1. A shared repository force-adds the symlink `.nah/harness/antigravity/antigravity-acp -> ../../../docs/ag`. Git keeps a tracked path tracked even inside an ignored folder, and a clone checks it out. That makes `.nah/harness/antigravity/` an ordinary 0755 folder.
2. On start, `make_home` finds no link at `.nah/harness/antigravity`, `.nah/harness` or `.nah`. The folder exists, so it's used as it is.
3. The user signs in. Antigravity writes `{gemini}/antigravity-acp/acp_token.json` through the link, to `docs/ag/acp_token.json`.
4. `docs/ag/` isn't ignored. Every turn's snapshot (`crates/nah-changes/src/timeline.rs:44`, `add_all`) stores the token, and the `changes` tool shows it to any agent. An agent's `fs/read_text_file` of `.nah/harness/antigravity/antigravity-acp/acp_token.json` is allowed, because the resolved `<root>/docs/ag/acp_token.json` isn't under `.nah/harness`. `GET /api/fs/read?path=docs/ag/acp_token.json` serves it. `git add -A` commits it.

Sequence (a tracked file, no link at all):
1. A shared repository force-adds `.nah/harness/antigravity/antigravity-acp/acp_token.json` with dummy content.
2. `make_home` uses the existing folder. Antigravity overwrites the tracked file with the real token.
3. A tracked file isn't covered by any ignore rule. `index.update_all` in the snapshot stores the change, the `changes` tool and the cockpit's changes view show the token, and `git commit -a` commits it.

A kept home has to be checked for what it holds, not only for the folders above it. No link anywhere below it, and nothing under `.nah/harness/` tracked by git (`git ls-files .nah/harness` empty), or the harness doesn't start. `allowed()` should also refuse the path as asked, as `/api/fs/read` does.

## 2. The `.gitignore` repair misses negations written as globs, and accepts a `harness/` line git doesn't read as one

`undoes` (`crates/arch-service/src/store.rs:153`) drops a `!` line only when its first path segment is the literal `cache`, `state` or `harness`. Git matches negations as globs. The presence check (`store.rs:156`) compares `l.trim()` with `harness/`. Git trims only trailing spaces, so leading whitespace is part of the pattern.

Sequence A (a glob):
1. A shared repository commits `.nah/.gitignore` with the lines `cache/`, `state/`, `harness/`, `!*`. `!**/harness/`, `!harness*` or `![h]arness/` work the same way.
2. On start, `undoes("!*")` takes the first segment `*`, which isn't in `NAH_IGNORED`, so the line stays. `harness/` is there, so nothing is written.
3. For `.nah/harness`, the last matching line is `!*`, so git doesn't exclude it and descends into it. Every file below matches `!*` too. `.nah/harness/antigravity/antigravity-acp/acp_token.json` is untracked and not ignored. Snapshots store it, the `changes` tool returns it, and `git add -A` commits it. `cache/` and `state/` are let back in the same way.

Sequence B (leading whitespace):
1. A shared repository commits `.nah/.gitignore` with `cache/`, `state/`, and ` harness/` (a leading space or tab).
2. `missing` trims the line, finds `harness/` "there", and adds nothing.
3. Git reads the pattern as ` harness`, which matches nothing. The kept logins aren't ignored, as in A.

The server can't recognise every pattern that undoes its own. Either the snapshot, `changes` and git-facing code exclude the private paths themselves, or the server owns `.nah/.gitignore` outright: written whole at start (with its entries last), and nothing else kept in it.

## 3. A `.nah/.gitignore` that is a symlink passes the check, but git doesn't read it

`ensure_nah_gitignore` reads `.nah/.gitignore` with `std::fs::read_to_string` (`store.rs:147`), which follows a symlink. Since 2.32, git refuses to read an in-tree `.gitignore` that is a symlink. It opens the file without following links, warns, and skips it.

Sequence:
1. A shared repository commits `.nah/.gitignore` as a symlink to `../.nah-ignore`, a tracked file holding `cache/`, `state/`, `harness/`.
2. On start, the server reads the three lines through the link. Nothing is missing or undone, so nothing is written, and the link stays.
3. `git status` warns that it can't access `.nah/.gitignore` and lists `.nah/harness/antigravity/antigravity-acp/acp_token.json` as untracked. `git add -A` commits the token.

When `.nah/.gitignore` is a link, it should be replaced with a regular file (`write_atomic` already renames over it), not read through.

## 4. The repair runs only at start, so a negation pulled in while the server runs lets logins in until the next restart

Round 3 permissions 2 named `git pull` as one way a negation comes in. `ensure_nah_gitignore` is called only from `state_dir`, `cache_dir`, `open_facts` and `FsPlans::new`, and every caller runs at start (`src/startup.rs:127, 200, 269, 323, 332`, `src/main.rs:142`). The server doesn't run it again while it serves.

Sequence:
1. The server is running, and the user is signed in with a kept home.
2. The user runs `git pull`, which brings in `!harness/` after `harness/` in `.nah/.gitignore`.
3. The next turn's snapshot stores `.nah/harness/antigravity/antigravity-acp/acp_token.json`. The `changes` tool shows it to agents, and the cockpit's changes view shows it to every device. The token stays in the repository's object store under the snapshot ref after any later repair. `git add -A` before a restart commits it.

The fix for point 2 (the server excluding its private paths itself, rather than trusting the file) closes this too.

# Review of fcb0865..HEAD (logins), correctness

Paths are relative to `meta-harness/`. The problems are ranked most severe first.

## 1. `.nah/harnesses.toml` already belongs to the launch overrides: `[home]` breaks every harness, and an existing override stops the server from starting

- `src/startup.rs:21-33` (`harness_homes`) reads `.nah/harnesses.toml` as `Settings { home }` with `deny_unknown_fields`.
- The same file is already the project's harness-override file (`packs/index.toml:34`, `overrides = [..., "{project}/.nah/harnesses.toml"]`). `src/packs.rs:47-58` (`own_launches`) parses it as `BTreeMap<instance, OwnLaunch>`, and `OwnLaunch` is also `deny_unknown_fields` with a required `program`.

Sequence A, using the feature as the README documents it:
1. Write `.nah/harnesses.toml` = `[home]\nantigravity = "kept"`.
2. `own_launches` reads `home` as an instance's launch and fails with `unknown field antigravity` / `missing field program`.
3. `harness_launches` (`src/packs.rs:87`) then marks every harness pack `Unavailable { reason: ".../.nah/harnesses.toml: ...", fix: "fix the file it names" }`. No harness can run, Claude Code included.

Sequence B, a project that already overrides a launch:
1. `.nah/harnesses.toml` = `[antigravity]\nprogram = "/x"` (the form tested in `tests/packs_tests.rs:111`).
2. `harness_homes` rejects the unknown field `antigravity`.
3. `startup::harnesses` returns `Err`, and `src/main.rs:207` exits. Upgrading makes the server fail to start.

The two settings can't share the file in any form. The new test (`tests/packs_tests.rs:156`) calls `harness_homes` alone and never runs `harness_launches` on the same file. There is also an inconsistency: a bad overrides file makes each harness unavailable with a reason, but a bad `[home]` aborts the whole server.

## 2. Antigravity's default (native) home on macOS always reads as "Not signed in", even right after a successful sign-in

- `packs/harness/antigravity-acp/pack.toml:141` sets `kept = "{gemini}/antigravity-acp/acp_token.json"` unconditionally.
- `AGY_ACP_FORCE_FILE_STORAGE=1` is only set in `[provides.settings.home]` (line 146), which applies only when the project keeps the home.
- The plan's own finding is that on macOS the token goes to the Keychain unless that variable is set. The pack lists `macos-arm64` as an install platform.
- `crates/harness-acp/src/control.rs:174-178` turns a missing file into `SignedOut`.

Sequence (macOS project server, no `.nah/harnesses.toml`):
1. The user signs in. The callback is handed back and the login ends `Ok`.
2. `on_change(Some(true))` sets `SignedIn` and spawns `refresh()` (`instance.rs`, the `on_auth` closure).
3. `probe` finds no `acp_token.json` (the token is in the Keychain) and sets `SignedOut`.
4. The cockpit shows "Not signed in" with the Sign in header (`render.rs:271-272`, `actions.rs:203`).
5. `nah-notify` (`rules.rs:110`) opens a new "Antigravity needs signing in" notification after every successful sign-in, and again on every later refresh.

A session that reports a catalog sets `SignedIn` (`apply_catalog`), but the next refresh flips it back.

## 3. A stale login file makes the status flip-flop: the refused session's "signed out" is overwritten at once

- `crates/harness-acp/src/adapter.rs:324-326` calls `signed_out(Some(false))`.
- The `on_auth` closure (`instance.rs`) always spawns `refresh()` after setting the status.
- `probe` (`control.rs:174-178`) trusts the file's existence.

Sequence: `acp_token.json` exists but its refresh token is revoked or expired (a password change, a 7-day testing-app token, a revoke in the Google account).
1. A slot opens a session.
2. Antigravity prints its login link, and the session is refused "not signed in".
3. The status is set to `SignedOut`, and notify opens "needs signing in".
4. The refresh finds the file and sets `SignedIn` again, so notify ends the notification as Done (`rules.rs:115`).
5. The harness line says "Signed in" while every session is refused as not signed in.

This happens on every session attempt. The plan's rule says "present when its login file is there", but a file that is present is not proof of a usable login, and here it overrides the agent's own evidence.

## 4. Two sign-ins started at once: the cockpit can show one login's link while the server waits on the other's port, so the correct paste is refused

- `crates/cockpit-ui/src/agents.rs:284-286` (`Act::SignIn`) records nothing while the request is in flight. `login` stays `None`, so `actions.rs:202-203` keeps "Sign in" in the header while the agent starts (seconds, more under QEMU).
- `crates/harness-acp/src/auth.rs:97-99` takes the waiting login only at the start, and `:133` `replace`s at the end.

Sequence: the user clicks Sign in → Google, sees nothing for a few seconds, and clicks again.
1. Begin #1 and begin #2 both find `running` empty and start two agent processes.
2. Link 1 arrives: `replace(Agent{port1})`, and reply 1 reaches the cockpit.
3. Link 2 arrives: `replace(Agent{port2})` aborts #1 (kills its agent).
4. If reply 2 reaches the cockpit before reply 1 (two HTTP requests through the relay, in no fixed order), the cockpit is left showing link 1. Even in order, a user who has already opened link 1 completes Google's page against port 1.
5. Pasting `http://127.0.0.1:<port1>/?code=…`, exactly the page that failed, gives `Refused: "that isn't the address of the page that failed to load: it starts with http://localhost:<port2>/"` (`auth.rs:229-236`).

The same happens with two tabs or devices. The cockpit also has a gap of its own: after `ran_out`, `Act::SignIn` doesn't clear the old login either, so the header keeps offering Sign in during the new request.

## 5. A sign-in started while a paste is being checked: `finish_agent` takes whichever login is there, possibly the new one

`crates/harness-acp/src/auth.rs:165-173`: the port is read under the lock, the lock is dropped for `hand_back_callback` (network I/O, and the agent may exchange the token before replying), then `take()` matches any `Running::Agent`.

Sequence:
1. Tab A pastes the right address, and `hand_back_callback(port1)` is under way.
2. Tab B (or a retry) begins a login. Its `take()` at `:97` aborts login 1 mid-exchange.
3. Begin B finds its link and stores `Agent{port2, ended2}`.
4. A's `hand_back` returns (or errors: the agent was killed).
5. If it returned, A's `take()` gets **B's** entry and waits up to 60 s on B's `ended`. B's `task` handle is dropped (`..`), so it is detached and no longer abortable.
6. B's user pastes and gets "no sign-in is waiting: sign in again", while B's agent process keeps listening until its 300 s run out.

The "started again meanwhile" check (`:171-172`) only catches the case where nothing is stored yet.

## 6. The callback's reply decides the outcome, not how the login ended; there is no timeout on reading it

`crates/harness-acp/src/auth.rs:241-248` and `:170`:
- Any reply other than `HTTP/1.x 2..` is an error.
- An I/O error on `read_to_string` is propagated with `?`, and nothing bounds the read.
- In every one of these cases `finish_agent` returns before reaching `ended`, and the `Running::Agent` entry stays.

Sequences:
- **Redirect on success.** The agent's listener answers the callback with a 302/303 to a "signed in" page (the usual loopback-OAuth reply; gemini-cli does exactly this). The login succeeds, the task sends `on_change(Some(true))`, and the cockpit says "Not taken: harness failed: callback rejected: HTTP/1.1 302 Found". `login.refused` is set and Finish signing in stays offered. A second paste gets "isn't waiting any more (its time ran out?)".
- **Connection reset.** The listener writes its reply and closes with a reset (a one-shot handler shutting down). `read_to_string` fails, and the user sees an I/O error (502) for a login that worked.
- **Listener never closes.** The listener holds the connection open (keep-alive despite `Connection: close`, or a reply sent only after something else). `complete` never returns, and the cockpit shows "Checking what you pasted…" indefinitely (`finishing` is never cleared).

The plan's live check stopped at Google's page, so Antigravity's real callback reply has not been seen.

## 7. Logins left running: a dropped begin request, and Cancel

- **Dropped begin request.** `crates/harness-acp/src/auth.rs:104-140`: if the `begin` HTTP handler future is dropped while it waits for the link, the `task` JoinHandle is dropped without `abort()`. This happens when the browser tab is closed or reloaded, the phone loses its connection, or the relay drops the request (any wait up to `wait_s`). The agent runs its login detached. Antigravity listens on its port for 300 s, nothing can complete or cancel it (it was never stored in `running`), and its end later calls `on_change(Some(false))` regardless of what happened since. Each retry under QEMU leaves another emulated agent process.
- **Cancel.** `Act::StopSigningIn` (`agents.rs:298`) only clears the cockpit's state and is labelled "Nothing changes". The server's `Running::Agent` (an agent process with an open listener) keeps running until its 300 s run out or the next begin.
- **A failed begin also kills the previous login.** `begin_agent` aborts the previous waiting login (`:97-99`) before knowing whether the new one gets a link. If the new one times out (`:136-139`), the earlier login, whose link the user may already have open, is gone too.

## 8. `wait_s` defaults to 30 s and counts the agent's start-up, which is slow when emulated

`auth.rs:94,136`: the 30 s timer starts before `with_connection` launches the agent, so it covers process start, `initialize`, `authenticate` and the link. The Antigravity pack sets no `wait_s`, yet its own check allows 300 s because "Emulated it takes a while to answer" (`pack.toml`, `[tool.check]`). On the developer sandbox (the QEMU launch), if start-up plus `initialize` takes more than 30 s, every sign-in fails with "the login showed no link within 30 seconds", and the agent is aborted just before printing it. Not measured here, but the pack's other timeout says it is slow.

## 9. The cockpit drops or hides paste errors

- `crates/cockpit-ui/src/agents.rs:409-414`: `AuthFinished(Err)` is applied only if `self.login` is `Some`. Before this change, the error went to the slot's errors (`done`).
  - Sequence: the cockpit tab is reloaded mid-login (the login state is lost, but the server still waits), or the user cancelled. "Finish signing in" is still listed (`actions.rs:205-207`, without a header). A wrong paste, or "no sign-in is waiting", is then silently discarded and the user sees nothing.
- `crates/cockpit-ui/src/chat/render.rs:105-108`: when `ran_out`, the panel returns before drawing `refused`. A paste sent from the palette after the timer ran out shows no reason either.
- Replies are not tied to a login. If a new sign-in starts while an older paste is being checked, the old reply lands on the new login. An old `Err` (for example "the sign-in was started again meanwhile") is shown as "Not taken" on the new link, and an old `Ok` clears the new login.

## 10. "Ended before it showed a link" can win the race over the real ending

`auth.rs:123-131`: `biased` only orders the branches when both are ready at the same poll. `link_to` is dropped when stderr closes (the forwarder's `says` returns `None`). `done_to` is sent only after `with_connection` has unwound.
- **Agent crash.** When the agent crashes during `initialize` or `authenticate`, stdout and stderr close together. The link channel often resolves first, so the user gets "the login ended before it showed a link" instead of the agent's error (for example a refused method).
- **Instant success.** For a login that succeeds at once (the doc comment's "a key in its environment"), `drop(transport.guard)` kills the process before `done_to.send`. The same race can, rarely, report that success as a failure while the status says signed in.

## 11. A link with no parsable callback port is accepted, and the login can then never be finished

- `auth.rs:132`: `callback_port` may return `None`, and the login is still stored and shown.
- `:169`: every paste then fails with "the login's link names no address to hand back to". The entry stays, so it doesn't run out either, until a new begin.

Sequence: a `redirect_uri` the parser doesn't handle. Examples: `[::1]`, a form other than `%3A`/`%2F`, or `redirect_uri` that isn't the first match (another parameter containing `redirect_uri=`, such as a `state` that embeds it). The user signs in on Google, and every paste is refused with no way forward. This should fail at begin, since the link cannot be completed.

## 12. A home given as a folder gets `chmod 700` and a `*` `.gitignore`, whatever folder it is

`crates/harness-acp/src/driver.rs:411-424` applies `make_home` to any path from `[home]`. Examples:
- `claude-code = "."` (or `".nah"`): the project root is made `0700`. If the root has no `.gitignore`, one containing `*` is written, and the whole repository becomes ignored by git.
- `antigravity = "/Users/me/.gemini"` (pointing at the native folder explicitly): the user's own folder is `chmod`ed and gets a `.gitignore`.

## 13. A `[home]` key that isn't an instance id is silently ignored

- `src/startup.rs:32-40` accepts any key.
- `driver.rs:433` looks homes up by instance id (`antigravity`), not by pack name (`antigravity-acp`, the name `pack install` and the README's harness tables use).
- `antigravity-acp = "kept"`, or a typo, leaves the harness on its native home with no error. The snapshot's `home` is `None`, so the only sign is the missing house icon.

## 14. A variable set by the launch and by `home.env` resolves differently for the process and for the profile

- `driver.rs:407` appends `home.env_for(dir)` to `launch.env`.
- `Places::new` (`settings.rs:248`) takes the **first** match.
- The child process gets the **last** (`Command::env` in order).

Sequence: an own launch in `harnesses.toml` with `env = { GEMINI_HOME = "/x" }`, plus `[home] antigravity = "kept"`. The agent runs with `GEMINI_HOME=<project>/.nah/harness/antigravity`. The server reads skills and the `kept` login file (sign-in status) under `/x`, so the status and skills come from a different home than the agent's.

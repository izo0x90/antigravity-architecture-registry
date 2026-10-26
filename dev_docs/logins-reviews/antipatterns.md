# Review of fcb0865..HEAD (logins), antipatterns

Paths are relative to `meta-harness/`. Ranked most severe first.

## 1. Sign-in status is guessed from whether a file exists, and the guess overrides what the agent itself just reported

`crates/harness-acp/src/control.rs:174-177` (the `kept` setting, `packs/harness/antigravity-acp/pack.toml` `[provides.settings.login] kept`)

`probe` reports SignedIn when `{gemini}/antigravity-acp/acp_token.json` exists and SignedOut when it doesn't. A file existing doesn't tell you whether the agent is signed in.

- **macOS, native home (the default).** The pack lists `macos-arm64`. `AGY_ACP_FORCE_FILE_STORAGE=1` is only in `[provides.settings.home] env`, so it is only set when the home is kept. With a native home, Antigravity stores its login in the Keychain and the file is never written. A user who is signed in is shown "Not signed in" (`chat/render.rs:378`) and gets the "Sign in" header button for good.
- **Expired or revoked token, file still there.** Opening a session prints the link. `adapter.rs:325` calls `signed_out(Some(false))`, which sets SignedOut. `instance.rs` `on_auth` then spawns `refresh()`. `probe` finds the file and sets SignedIn again. The status flips back within seconds, contradicting the session refusal that had just happened. From then on, the cockpit offers sign-in only because of the error-text match in finding 2.

The agent's own refusal (its link printed) is the real signal. The file check is a stand-in for it, and it wins over that signal.

## 2. The cockpit decides "needs login" by searching error text for "authentication", and the new path depends on that

`crates/cockpit-ui/src/chat/render.rs:136` (existing code), relied on by `crates/harness-acp/src/adapter.rs:326` and fixed in place by `crates/harness-acp/tests/acp_login.rs:98` (`err.to_string().contains("authentication")`, "the cockpit offers to sign in on it").

The refusal is `HarnessError::AuthRequired`. `src/server.rs` already maps it to 401, and `NetError.status` carries that 401 to the cockpit. The cockpit ignores the status and instead lowercases the error message to look for "authentication", which comes from `#[error("authentication required: …")]`. Rewording that message, or any other error whose text happens to contain the word (for example a tool's "authentication failed" in a turn failure), changes whether the sign-in button appears. Because of finding 1, this text match is the only thing that keeps sign-in offered after a refused session. Keep the typed status (401, or an `auth_required` flag on the slot error) and drop the substring check.

ast-grep rule to find these across the workspace:

```yaml
id: error-text-match
language: rust
rule:
  any:
    - pattern: $E.to_string().contains($S)
    - pattern: $E.to_lowercase().contains($S)
    - pattern: $E.message.contains($S)
  inside:
    kind: function_item
    stopBy: end
message: deciding on an error's text; match its variant/status instead
```

## 3. The cockpit treats every failed paste as "wrong paste, still waiting", even though the server says otherwise

`crates/cockpit-ui/src/agents.rs:409-413`, `crates/cockpit-ui/src/chat/actions.rs:247`

The server distinguishes the failures by type and status:

| Error | Status | Meaning |
|---|---|---|
| `Refused` | 400 | Wrong paste; the login is still waiting |
| `Conflict` | 409 | No sign-in is waiting, or it isn't waiting any more |
| `Provider` | 502 | The login failed, or didn't finish within 60 s |

`Reply::AuthFinished(Err(e))` discards `e.status`. It keeps the `Login`, clears `finishing`, and sets `refused`. The "Finish signing in" header button and the countdown stay.

Sequence: the agent's own 300 s runs out a few seconds before the cockpit's clock does (the cockpit starts counting when the reply arrives; see finding 10). The user pastes and gets 409, "the sign-in isn't waiting any more … sign in again". The cockpit still shows "Finish signing in" as the main action, with time left on the clock. The same happens after any 502: the login on the server has been taken and ended, but the cockpit invites more pastes. Each of those then gets 409 "no sign-in is waiting".

## 4. A callback the agent rejects leaves a dead login registered, and the next error is a guess

`crates/harness-acp/src/auth.rs:170` (the `?` on `hand_back_callback`), `auth.rs:240`, `auth.rs:247`

When the agent's listener answers non-2xx, `hand_back_callback` returns `Provider("callback rejected: HTTP/1.1 400 …")` before `finish_agent` takes the `Running::Agent`. Antigravity, like the mock (`testing.rs`, `respond_with_error`), ends its login on a bad callback. The task finishes and `on_change(Some(false))` fires, but `Running::Agent { port, ended }` stays in `running`, pointing at a closed port.

Sequence: the user presses Cancel on Google's page, so the address is `http://127.0.0.1:<port>/?error=access_denied`. The result is 502 "callback rejected", and the cockpit keeps waiting (finding 3). The next paste fails to connect and gets `Conflict("… isn't waiting any more (its time ran out?) …")`. That message is a guess, while the real reason sits unread in the stored `ended` receiver. On a rejected callback, `finish_agent` should take the entry and report `ended`. The connect-failure branch should read `ended` instead of guessing.

## 5. Two separate mechanisms for "run a login, find its link, give up after a while"

`crates/harness-acp/src/auth.rs:92-135` against `crates/harness-acp/src/login.rs:21,27,49,62-76`

The command login (`LoginRun::start`) has its own `ANY_LINK` default (`https://\S+`), its own `Regex::new` with the same error text, its own hard-coded `LINK_WAIT` of 30 s, and its own reader tasks that scan stdout and stderr. The new agent login adds `LoginSettings::link_pattern()` with the same default regex duplicated (`settings.rs:160`), a separate `wait_s.unwrap_or(30)` (`auth.rs:94`), and a separate scanner (`launch::says` over the new `Said` broadcast).

The README says `wait_s` covers only "a login it runs itself", so a pack can't change the command login's 30 s, and `LINK_WAIT` and `wait_s` can drift apart. This goes against "one mechanism per need". `LoginRun` should take `link_pattern()` and `wait_s`, or both should share one "watch the lines for the link, with a deadline" helper.

## 6. An invalid link pattern is silently ignored when a session opens, and it isn't checked when the settings load

`crates/harness-acp/src/adapter.rs:257` (`link_pattern().ok()`)

With `on_session = true` and a bad `link` regex, `.ok()` turns the error into `None`, and `asks_login` becomes `pending()`. `session/new` then hangs while signed out, which is exactly the bug the plan set out to remove, and nothing reports why. `begin_agent` (`auth.rs:95`) reports the same pattern as `InvalidConfig`, but only once someone presses Sign in.

The README says "wrong or unknown ones stop the harness, saying why". `AcpSettings::parse` (`settings.rs:54`) doesn't compile `link`, so a wrong pattern doesn't stop the harness. Compile it once at parse time, store the `Regex` in the settings, and remove both runtime `Regex::new` calls.

ast-grep rule for errors silently converted to `Option`:

```yaml
id: result-ok-swallowed
language: rust
rule:
  pattern: $X.ok()
  not:
    inside:
      any:
        - pattern: let _ = $$$
        - kind: if_expression
message: Result dropped to Option; say why or propagate
severity: hint
```

## 7. "The login ended before it showed a link" discards how it ended, despite the comment saying it reports it

`crates/harness-acp/src/auth.rs:127-131`

The comment says "It ended before printing one: as its end says", but the code returns a fixed string and never awaits `done`. `link` fails as soon as the inner watcher sees `said` close, which happens on stderr EOF. If the agent process crashes or exits during `authenticate`, stderr usually closes before the ACP connection notices the stdout EOF and `done_to.send` runs. `biased` only helps when both are already ready at the same poll. The agent's actual error is lost.

On this path the `task` JoinHandle is also dropped without `abort()`, so it is detached. After a `link` error, await `done` with a short bound, then report its error.

## 8. `finish_agent` can take a newer login than the one the paste was meant for

`crates/harness-acp/src/auth.rs:165-171`

The port is read under the lock, the lock is released, the address is handed back (network I/O), and then `running` is `take()`n. If `begin` runs in between (a second tab, or Sign in clicked again), it aborts the old login and stores a new `Running::Agent`. `finish_agent` then takes the new entry. That login's JoinHandle is dropped (detached), so later pastes for it get "no sign-in is waiting". `finish_agent` also waits up to 60 s on the new login's `ended` and reports that as the result of the old paste. The "started again meanwhile" check only catches the `None` case. Tag each login with an id (or compare the port) and take only the entry that matches.

## 9. Paste replies aren't tagged, so a late reply changes a different sign-in

`crates/cockpit-ui/src/agents.rs:76`, `:295`, `:408-413`

`Reply::AuthFinished(NetResult<()>)` carries no slot, instance or login identity, unlike `AuthStarted(slot, …)`. Sequence: paste (slow round trip through home) → Cancel sign-in (`login = None`) → Sign in again → the new challenge arrives → the old paste's reply arrives. `Ok` clears the new login. `Err` marks the new login "Not taken: …" with the old error. One `login: Option<Login>` is also shared by every slot and instance, so a sign-in for one harness is overwritten by a reply about another.

## 10. Fixed timeouts and an unbounded wait on the paste request

`crates/harness-acp/src/auth.rs:184`, `:243`; `crates/cockpit-ui/src/agents.rs` `Login::new`

- `END_WAIT` is a hard-coded 60 s, while the other login timings (`within_s`, `wait_s`) belong to the pack.
- `hand_back_callback` calls `read_to_string` with no timeout before `END_WAIT` applies. If the agent's listener holds the connection open while it trades the code with Google, or never closes it, `complete` hangs with no limit, and so do the HTTP call from the cockpit and the relay through home.
- `within_s` is sent as a duration and the cockpit starts counting when the reply arrives. The agent started its 300 s before it printed the link, so the cockpit always shows more time than is left (relay latency plus the time to find the link). The server knows when the login started and should send that deadline.

## 11. `.nah/harnesses.toml`: read errors are ignored and unknown harness names are dropped

`src/startup.rs:32-35`, `:31-44`

- `Err(_) => Settings::default()` treats every read error (permission denied, the path being a directory, invalid UTF-8) as "file absent". The result is that every harness silently uses its native home, the user's own `~/.gemini`, which the project had asked not to use. Only `NotFound` should mean the default. This copies the existing `terminals.toml` pattern at `startup.rs:276`.
- Keys are never checked against the harness instances. `antigravity-acp = "kept"` (the pack name, which `pack list` shows) is accepted and does nothing, and the harness keeps its native home with no warning. `deny_unknown_fields` covers only the top-level table.

ast-grep rule for the read-or-default pattern:

```yaml
id: read-error-as-absent
language: rust
rule:
  pattern: |
    match std::fs::read_to_string($P) { Ok($T) => $A, Err(_) => $B }
message: every read error treated as a missing file; match ErrorKind::NotFound
```

## 12. The loopback port is found by hand-parsing the URL as a string

`crates/harness-acp/src/auth.rs:217-224`

`callback_port` uses `split_once("redirect_uri=")`, which also matches inside any parameter whose name ends that way. It decodes only `%3A`, `%3a`, `%2F` and `%2f`, and recognises only `127.0.0.1` and `localhost`, so a `[::1]` redirect isn't understood. When it returns `None`, nothing is said at `begin`: the user signs in on Google first, and only the paste fails with "the login's link names no address to hand back to" (`auth.rs:169`). Parse the link with a URL parser at `begin`, and refuse a link with no loopback port there.

## 13. With `on_session` set and no `link`, any https URL on stderr refuses the session

`crates/harness-acp/src/settings.rs:160`, `crates/harness-acp/src/adapter.rs:257`

`on_session` uses `link_pattern()`, whose default is `https://\S+`. Under that default, an agent that logs any URL on stderr while a session opens (a docs link, an update notice) is refused as "not signed in", and its status is set to SignedOut. The broad default suits only the "show a link" use. For the refusal it should be required: setting `on_session` without `link` should be rejected at parse time.

## 14. Cancel sign-in leaves the agent's login running on the server

`crates/cockpit-ui/src/agents.rs:298`, `crates/cockpit-ui/src/chat/actions.rs:270` ("Nothing changes")

`StopSigningIn` only clears the cockpit's state, and `AuthController` has no cancel. On the server, the `Running::Agent` task keeps the agent process and its loopback listener alive for up to 300 s. Before this change the agent login was fire-and-forget too, but now it is long-lived and there is a cancel button for it. A cancel call would abort the task (`replace` already knows how).

## 15. `BROWSER = "true"` is written into two launches by hand

`packs/harness/antigravity-acp/pack.toml:96` and `:103`, with both asserted in `tests/packs_tests.rs`

The pack's launch environment is copied into `[install.launch]` (the emulated developer platform), with a comment, "BROWSER as the pack's own launch (below)", to keep the two in step. A later change to one launch (or a third platform launch) will drift. If launches can't share an env, the pack format needs a common `env` that every launch inherits, rather than a copy.

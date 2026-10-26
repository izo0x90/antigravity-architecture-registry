# Review of fcb0865..HEAD (logins, round 2), responsibility

Six problems, most severe first.

## 1. The server never checks which sign-in a paste or a Cancel is for. Only the cockpit does.

- `crates/harness-acp/src/auth.rs:300` (`complete`) and `:326` (`cancel`)
- `src/routes/harness.rs:88-110` (`CompleteAuthRequest { response }`, and `cancel_auth` takes no body)
- `crates/cockpit-ui/src/agents.rs:313-327`

The contract gives every sign-in an id (`SignInWaiting.id`). Findings item 6/9 says pastes are tied to a sign-in by that id. But only the cockpit uses the id: it reads it from its own copy of the snapshot and uses it to sort out the reply. The server keeps the sign-in, yet the route carries no id. `complete` and `cancel` act on whatever sign-in is under way when the request arrives.

How it goes wrong (Claude Code, two devices):
1. Device A shows sign-in 1. The user opens its link and gets a code.
2. Device B taps Sign in. Sign-in 2 replaces 1 with a new `claude auth login` process and a new OAuth state.
3. Device A pastes code 1. Its snapshot may still say id 1, or already say 2. Either way the request carries no id.
4. The server takes sign-in 2 out of `under` ("a login command takes one answer") and writes code 1 into login 2's stdin. Login 2 fails, and the status flips to `SignedOut`.
5. Device B's sign-in is gone, killed by a stale paste it never saw.

Cancel has the same gap. A Cancel tapped on device A against a stale view of sign-in 1 stops sign-in 2, which B has just started.

The check that a request still matches the current sign-in has to be made where the sign-in is held, on the server. `complete` and `cancel` should take the id and refuse when it doesn't match. The cockpit should only show what the server said.

## 2. The server knows why a sign-in ended but doesn't say. The cockpit guesses from clocks, in two separate ways.

- `crates/harness-acp/src/auth.rs` (`end`, `stop_under_way`, `finish_agent` all send `AuthEvent::Waiting(None)`)
- `crates/cockpit-ui/src/agents.rs:85, 141-153` (`RAN_OUT_SLACK_MS`, the `ran_out` set)
- `crates/cockpit-ui/src/chat.rs:237` (`ran_out: left_s == Some(0)`)

The server decides how a sign-in ends: its time runs out (`expire`, or the agent login's timeout), Cancel, a new sign-in replacing it, success, or failure. It publishes every one of these as `waiting: None`.

The cockpit rebuilds the reason itself, in two separate places:
- `Agents::apply` sets "ran out" when `waiting` disappears while the device clock is within 3 s of `until_ms`.
- `sign_in_now` separately treats `left_s == 0` as ran out while `waiting` is still there.

How it goes wrong:
- On device B the user cancels with 2 s left. Every device, A included, then shows "The sign-in to Antigravity stopped waiting: its time ran out." It was cancelled.
- A device whose clock is more than 3 s behind the server never shows "ran out" when the server really does end the sign-in at its time. It falls back to a plain "Sign in to X", or to nothing if the status isn't `SignedOut`.

This is not the countdown-clock point listed as Kept: the reason for the ending is a fact only the server has. The fix belongs in the event: `Waiting(None)` should carry why it ended, for example a field in the snapshot. That would remove `RAN_OUT_SLACK_MS` and both client-side guesses.

## 3. The server's time limit works two different ways, with different results.

- `crates/harness-acp/src/auth.rs:122-127, 155` (`expire`, used only by command logins)
- `crates/harness-acp/src/auth.rs:209, 219` (`tokio::time::timeout` inside the agent login's task, then `AuthEvent::Status(Some(result.is_ok()))`)

A login command's time limit is a separate timer task. When it fires, `end` runs, which sends `Waiting(None)` and leaves the status alone. An agent login's time limit is a `timeout` wrapped around the login itself. When it fires, the login counts as failed, so it sends `Status(Some(false))` and then `end`. The same setting (`within_s`) therefore has two mechanisms with two outcomes. A third outcome comes from Cancel and from a sign-in being replaced: both abort the agent task, so no status is sent at all.

How it goes wrong:
1. Antigravity is signed in (its kept token is there, and sessions work).
2. The user picks Sign in from the action list, for example to switch accounts, then leaves it.
3. After 300 s the timeout fires. The instance is set to `SignedOut`, and the cockpit shows "Not signed in" plus the Sign in header.
4. Nothing about the login has changed. Doing the same with Cancel leaves it signed in.

A sign-in that runs out should end the same way whatever its kind: one deadline mechanism, which ends it the way Cancel does.

## 4. The kept-home folder is defined in two crates, and each half decides on its own what is private.

- `src/packs.rs:93` (`"kept" => project.join(".nah/harness")`)
- `crates/arch-service/src/store.rs:126-134` (`HARNESS_DIR = "harness"`, `private_dirs`)
- `crates/harness-acp/src/workspace.rs:28` (the agent's file access refuses `kept_home`)
- `src/routes/files.rs:50` (`/api/fs/read` refuses `private_dirs`)

Where a kept home goes is written as a literal `.nah/harness` in the server's settings reader. What `.nah/.gitignore` keeps out of git, and what `/api/fs/read` refuses, comes from a separate constant in arch-service. Nothing links the two. If either one changes, kept logins become both served and committed.

The "private" decision is also made twice, with different answers:
- The agent's file tool refuses whatever folder the home actually resolved to (`kept_home`).
- The HTTP route refuses only the fixed `.nah/harness`.

With `home = "secrets/agy"`, an agent's `read_text_file` of `secrets/agy/antigravity-acp/acp_token.json` is refused. `GET /api/fs/read?path=secrets/agy/antigravity-acp/acp_token.json` serves the same file. The Kept note says such a home "isn't protected", but half the code does protect it.

One owner should name the folder, and one rule, applied by both routes, should say what is private.

## 5. The login link pattern is compiled twice, and a failure that can't happen has an error path of its own.

- `crates/harness-acp/src/settings.rs:69` (`check` compiles it and throws it away)
- `crates/harness-acp/src/profile.rs:27, 42` (`login_link: Option<Regex>`, compiled again with `.ok()`)
- `crates/harness-acp/src/auth.rs:108-109` (`link_pattern()` turns `None` into a fresh `InvalidConfig`)

Findings item 18 says "compiled once". In fact `AcpSettings::parse` compiles the pattern to check it, then `Profile::new` compiles it again and drops the error. Because `parse` refuses a bad pattern, and the driver falls back to `AcpSettings::default()` (whose pattern is any `https://` link), `login_link` is never `None`. Three things guard one fact: the `Option`, the swallowed error, and a third message in auth.rs ("the login link pattern is wrong", which has lost the regex's reason). If `Profile` is ever built from settings that weren't parsed, a sign-in fails with that reasonless message instead of the real one.

`parse` should produce the compiled pattern, and `Profile` should keep it as a plain field rather than an `Option`.

## 6. The begin reply duplicates the snapshot, and two crate internals are exported for no one.

- `src/routes/harness.rs` (`begin_auth` returns `Option<SignInWaiting>`)
- `crates/cockpit-ui/src/agents.rs:421-431` (the reply's payload is ignored: "What's to be done comes in the instance's snapshot too")
- `crates/harness-acp/src/lib.rs:33` (`pub use launch::says`)
- `src/packs.rs:114, 133` (`harness_instances` beside the same list built inline in `harness_launches`)

- **The begin reply.** The sign-in under way reaches clients two ways: the begin reply and the snapshot. The only client throws the reply away. Any other client that used it would show a sign-in the snapshot may already have replaced, because the snapshot update can arrive before the reply.
- **`says`.** It is exported publicly, but nothing outside `harness-acp` uses it, so an internal of the stderr-reading mechanism becomes crate API.
- **The instance list.** It is built twice in `packs.rs`. `harness_launches` skips a duplicated instance and logs a warning; `harness_instances` (used by `pack which`) keeps duplicates. The two lists can disagree on which pack owns an instance name.

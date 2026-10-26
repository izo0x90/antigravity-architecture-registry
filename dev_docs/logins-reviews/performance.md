# Review of fcb0865..HEAD (logins), performance

Ranked most severe first. Paths are under `meta-harness/`.

## 1. Every signed-out session open launches the agent a second time (a full probe), with no dedup

`crates/harness-acp/src/adapter.rs:325` calls `(self.signed_out)(Some(false))` when the link shows up while a session opens. That callback (`crates/harness-acp/src/instance.rs:83-93`) always `tokio::spawn`s `instance.refresh()`, even when the snapshot already says SignedOut. `refresh` runs `read_configuration` (a blocking-pool scan of every skill root and tool file) and then `probe`, which starts the agent again through `with_connection`. For Antigravity that is a second QEMU-emulated `agy_acp_server.par` start plus `initialize`.

Sequence: Antigravity is signed out and the queue runs a plan. Each worker attempt (`src/queue.rs:59`, which retries failed attempts) opens a session. That launches one emulated agent, which starts its own login and opens a 127.0.0.1 listener before it is killed. The `AuthRequired` then spawns a probe that launches a second emulated agent. So every failed open costs two agent start-ups instead of one. Opens that fail at the same time (several workers, a chat slot and an MCP `slots` open at once) each spawn their own concurrent `refresh`. Nothing coalesces them. The probe can't learn anything new here: the status was just set to SignedOut, and without an agent signal the probe only checks whether the `kept` file exists. The same unconditional refresh also follows every `begin_agent` end (`auth.rs`, the `on_change(Some(result.is_ok()))` in the spawned task).

## 2. A login the agent runs itself has no bound of the server's own, and "Stop signing in" never reaches the server

`crates/harness-acp/src/auth.rs:133` stores `Running::Agent { task, .. }`. Its task holds the agent process (the transport guard inside `with_connection`) until the agent's own `authenticate` returns. The server knows `login.within_s` (300 s for Antigravity) but never aborts the task when that time is up. It relies on the agent timing itself out. The `AuthController` trait (`crates/harness-core/src/lib.rs:97-105`) has no cancel either. In the cockpit, `Act::StopSigningIn` (`crates/cockpit-ui/src/agents.rs:298`) only clears local state.

Sequence: the user taps Sign in, gets the link, then taps "Cancel sign-in". The emulated Antigravity process, its listener and the spawned `says` task stay alive for the full 300 s. For a harness whose login doesn't time itself out (any pack that sets no `within_s`), the process stays until the next `begin` replaces it, which may be never. The finished task's `JoinHandle` and `Ended` receiver also stay in `running` after the agent ends.

## 3. Repeated taps on Sign in each start another agent; nothing in flight is tracked

`Act::SignIn` (`crates/cockpit-ui/src/agents.rs:284-287`) sends `begin_auth` on every activation and keeps no pending flag. The "Sign in" header control stays until the first reply arrives, and that reply can take up to `wait_s` (30 s) plus QEMU start-up. On the server, `begin_agent` (`crates/harness-acp/src/auth.rs:97`) only aborts a login that is already stored in `running`. A login still waiting for its link isn't stored yet, so it isn't aborted.

Sequence: on a phone, nothing happens visibly for 10-30 s, so the user taps Sign in three times. Three emulated agents start in parallel and each opens a listener. Each later one aborts the earlier one only once it has printed its own link. So the waste is two extra full agent start-ups per stray tap, and with several devices the same applies per device.

## 4. Handing back the callback can hang the request forever

`hand_back_callback` (`crates/harness-acp/src/auth.rs:243`) does `stream.read_to_string(&mut reply).await`, with no timeout and no size bound. `END_WAIT` (line 174) only covers waiting on `ended` afterwards.

Sequence: the agent's listener accepts the connection and answers, but doesn't close it. It keeps the connection alive or is stuck after the token exchange. `complete` then never returns, so the HTTP handler, the relayed request through Home and the TCP connection are held indefinitely. The cockpit shows "Checking what you pasted…" (`finishing`) until the page reloads. The `Running::Agent` entry also isn't taken, so a second paste connects to the same port again and stacks another hung request.

## 5. The cockpit repaints everything once a second for up to five minutes, shown or not

`crates/cockpit-ui/src/agents.rs:370` calls `ctx.request_repaint_after(1s)` from `Agents::tick`, which runs every frame whether or not any chat buffer shows the login (`app/lifecycle.rs:109`). While a login with `within_s` is pending, the whole desk (every panel, its layout and its actions) is laid out and painted once a second for up to 300 s. That happens even when the chat holding the login is closed or in another panel set, and on battery-bound devices (a phone, an iPad) that are the target of this plan. The countdown only needs a repaint while a buffer that shows it is drawn.

## 6. Per-frame status work grows: extra lookups and allocations on every chat header

`ChatStreamState::status` (`crates/cockpit-ui/src/chat/render.rs:377-385`) now calls `self.instance()` up to three more times per frame. Each call is a `RefCell` borrow plus two map lookups, and `instance_id_in` clones an `InstanceId`. On top of that it `format!`s "Not signed in"/"Signed in", clones `home`, and `format!`s a ~70-character detail string, every frame, for every chat panel on screen. Each value is computed again from scratch although it only changes when the snapshot does. This is small per frame, but it repeats at frame rate (and at 1 Hz for the full desk during a login, see 5). One `instance()` borrow at the top of the function would do.

## 7. A regex compiled on every session open and every begin

`crates/harness-acp/src/adapter.rs:257` and `crates/harness-acp/src/auth.rs:95` call `LoginSettings::link_pattern()`, which runs `regex::Regex::new` each time. For Antigravity (`on_session = true`) every session open compiles the pattern again. The pattern is fixed once the pack is parsed, so it belongs in the `Profile`, compiled once. Compiling it there would also surface a bad pattern once, instead of `.ok()` silently dropping it on every open.

## 8. Blocking file-system calls on async runtime threads

- `crates/harness-acp/src/control.rs:176`: `profile_for.places.path(kept).is_file()` is a synchronous `stat` inside the async probe operation. It runs on every `refresh`, and those refreshes are triggered as described in 1. On a project on a slow or network mount (the sandbox's shared folders), this blocks a runtime worker.
- `crates/harness-acp/src/driver.rs:44-57`: `make_home` (`create_dir_all`, `set_permissions`, `exists`, `write`) runs synchronously inside `async fn create` (`driver.rs:70` onward) on the runtime. It runs once per instance creation. `read_configuration` already uses `spawn_blocking` for the same kind of work.

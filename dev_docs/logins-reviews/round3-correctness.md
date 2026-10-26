# Review of 40c2464 (logins, round 3), correctness

Seven problems, most severe first. Paths are relative to `meta-harness/`.

## 1. An ended sign-in's event wipes the newer sign-in from the snapshot

`crates/harness-acp/src/instance.rs:82-87`: every `AuthEvent::Ended` sets `auth.waiting = None`, whatever its id. `Waiting` (line 76) is also applied without an id check. The controller sends both outside the slot's lock, so an event about an old sign-in can arrive after the newer one is set.

- **Command login (Claude Code), the likely case.** `complete` takes the `LoginRun` out of the slot (`auth.rs:370`/`376`) and runs `run.finish` for up to `FINISH_WAIT` (120 s).
  1. Device A pastes the code. The slot is now `None`, and the snapshot still shows sign-in 1.
  2. Device B, or A from the palette, taps Sign in. `begin` (`auth.rs:345`) finds nothing to replace, so it sends no `Ended` and sets sign-in 2 to `Starting`. `begin_command` starts a second `claude` login, and sign-in 2's link is shown.
  3. Sign-in 1's `finish` returns, by success, by failure, or by "didn't finish". `auth.rs:394` calls `self.slot.ended(id, …)`, and that call doesn't check the slot.
  4. The instance clears `waiting`. Sign-in 2's link, its Finish signing in and its Cancel disappear on every device, while its login command is still held in the slot.
  5. Claude Code has no `within_s`, so nothing ever ends sign-in 2. Its process stays alive until the next Sign in. The snapshot's `ended` names sign-in 1.
- **Two begins at once (two devices).** Begin A replaces the slot and drops the lock (`auth.rs:345`). Begin B replaces A and sends `Ended(A)`, then `Waiting(B)`. Only then does A run line 351 and send `Waiting(A, "Starting…")`. The snapshot now shows the stopped sign-in A: a paste or Cancel names A and is refused or ignored, until B's link arrives.
- **The agent's link reader (`auth.rs:234-246`).** It checks `current` under the lock but sends `Waiting` after dropping it. A begin landing between the two leaves the stopped sign-in's link on screen over the new one's "Starting…".

## 2. While a pasted code is being checked, the sign-in is nobody's: Cancel does nothing, and a new Sign in runs alongside it

The same take as above (`auth.rs:376`) empties the slot for the whole of `run.finish`.

- **Cancel.** `cancel(id)` → `slot.end` (`auth.rs:105-114`) finds `None` and returns `Ok`, so the route answers 204. The login command goes on, and it can still sign in after the user cancelled it. The snapshot keeps showing the sign-in with its Cancel button until `finish` returns. This contradicts the README ("until it's taken, cancelled …") and the "Cancel ends it on the server" check.
- **New Sign in.** Two `claude` login processes then run at once against the same home (case 1 above). The older one can finish last and write its credentials over the newer one's.

## 3. A kept home through a linked `.nah` or `.nah/harness` lands the login in a tracked folder, and git doesn't ignore the link

`crates/harness-acp/src/driver.rs:34` refuses a home only when its last component is a link. `packs.rs` builds the kept home as `<project>/.nah/harness/<instance>`. `.nah/.gitignore` lists `harness/` (`arch-service/src/store.rs:124`), and a pattern ending in `/` only matches directories, so git doesn't ignore a symlink named `.nah/harness`.

1. A repository commits `.nah/harness` as a symlink to `docs/logins`.
2. A user clones it and sets `home = "kept"`.
3. `make_home(<project>/.nah/harness/claude-code)` sees that the leaf isn't a link and creates `docs/logins/claude-code` through the link.
4. The token is written there. `git status` shows it untracked and not ignored, so the next `git add -A` commits the login.

The file route and agent access are refused (both canonicalize `.nah/harness`), but the "never committed" rule is broken. A linked `.nah` does the same. The check needs to cover every ancestor below the project, not just the leaf.

The in-project refusal for a folder of your own has the same gap. `src/packs.rs:117` compares paths by name only (`normal.starts_with(project)`), with `..` removed but links not followed. Say `~/p` is a symlink to the project. `home = "~/p/agent-home"` normalizes to `$HOME/p/agent-home` and is accepted, but it is really `<project>/agent-home`:

- `/api/fs/read` serves it (`private_paths` only covers `.nah/*`).
- `.nah/.gitignore` doesn't cover it.

A project opened through a linked path, with an absolute real-path home, does the same in reverse.

## 4. A sign-in that fails or runs out says nothing unless the harness is "Not signed in"

`crates/cockpit-ui/src/chat/render.rs:99` draws the sign-in banner only when `needs_login() || now.is_some() || starting`. Before this commit, running out set the status to signed out, so the "its time ran out" line had a banner to appear in. Now "only success sets a status".

- **Signed in already.** A harness that is `SignedIn`, or `Unknown` (before or after a failed check), starts a sign-in, for example to switch accounts. When that sign-in runs out or fails, `waiting` is cleared and the banner disappears without a word on every device. `auth.ended` (`RanOut` / `Failed { reason }`) is in the snapshot but never drawn.
- **Stale reason.** `ended` is only cleared by a new `Waiting`. Say a sign-in fails, the harness then reads signed in (login done another way), and later Sign out sets `SignedOut`. The banner then shows "The sign-in to X failed: <old reason>" for a sign-in long gone.

## 5. A begin request that goes away leaves the sign-in stuck (and a dropped paste request kills the login with no end said)

The server's sign-in still lives partly in the request's future. A page reload, a closed tab or a mobile network drop closes the connection, and hyper/axum then drop the handler future.

- **`complete`, command kind (`auth.rs:392-399`).** The `LoginRun` is owned by the request's future. Dropping it kills the `claude` login (`kill_on_drop`) partway through the code exchange. The slot is already `None` and `slot.ended` never runs, so the snapshot shows the sign-in waiting indefinitely, since Claude Code has no deadline:
  - a paste is refused ("isn't under way any more");
  - Cancel answers 204 and changes nothing;
  - Sign in isn't offered as a header button while a sign-in is shown (`actions.rs:245`, `again` needs `now` to be `None` or ran out).
- **`begin`, command kind.** The begin request goes away during `LoginRun::start`, which waits up to `wait_s` = 30 s for the link. The process is killed, and the slot stays `Running::Starting`, shown as "Starting the sign-in…" on every device indefinitely (no `within_s`). Cancel does clear it.
- **`begin`, agent kind.** The `wait_s` "showed no link" limit (`auth.rs:289-293`) lives only in the request's `select!`. Once the request is gone, an agent that never prints its link is held as "Starting…" until `within_s`, and indefinitely for a pack without one.

## 6. The begin reply misnames how its sign-in ended

The cockpit shows begin errors in the slot's error banner (`agents.rs:402-410`).

- **Deadline or quick failure during the start (`auth.rs:180-181` and `auth.rs:286`).** A sign-in whose deadline passes during `LoginRun::start` gets `current() != Some(id)` and returns `stopped()`: "that sign-in was stopped (cancelled, or a new one started)". In the agent case, the slot was ended as `RanOut`, the task is aborted, `ended_to` is dropped, and the reply is again `stopped()`. The snapshot says "its time ran out" while the device that started it shows "cancelled, or a new one started".
- **The agent task ends before `put` (`auth.rs:266-268`).** `tokio::spawn` can run the task on another worker before the synchronous `put`. A launch that fails at once calls `slot.end(id, Failed)` on the `Starting` entry. `put` then finds `None` and the reply is `stopped()`, not the failure's reason.
- **The link reader after replacement (`auth.rs:247`, `272`).** The link reader sends `Ok(waiting)` even when `current` was false. `begin` then answers `Ok(Some(waiting))`, a live-looking link, for a sign-in that was replaced and whose agent was aborted. The end-to-end tests read this reply (the reason given for keeping it).

## 7. A folder of your own is labelled "kept for this project", and the settings' doc still says relative homes are the project's

- **The label.** `src/packs.rs` passes any home (`kept`, `~/…`, absolute) as `entry.home`. `driver.rs` passes it to `keeping_home`, and `profile.rs:60-62` reports every `kept_home` as `HarnessHome { kept: true }`. A home such as `~/homes/claude` is shown on the harness's line as "Its home …, kept for this project: /Users/…/homes/claude", though the project keeps nothing there (not in `.nah/harness/`, not gitignored). The README describes it as "a folder of your own … used as it is".
- **The doc.** `src/packs.rs:44-45` documents `home` as "a folder (the project's when relative)". Every relative home that doesn't climb out with `..` is now refused by line 117. In the user's own `harnesses.toml`, a relative home is joined to each project, so it is refused in every project.

# Review of fcb0865..HEAD (logins), tests

Problems are ranked with the most severe first. Paths are relative to `meta-harness/`.

## 1. "A wrong paste keeps the login waiting" is only tested for pastes refused before the agent sees them

- **Where:** `crates/harness-acp/tests/acp_login.rs:65`, with `crates/harness-acp/src/auth.rs:164-178` and `crates/cockpit-ui/src/chat/actions.rs:247`.
- **What the test covers:** three pastes that `hand_back_callback` rejects locally: a bare code, a Google URL, and the wrong port.
- **What it misses:** the likely wrong paste is the right port with a wrong or partial address. Examples are a truncated `http://127.0.0.1:<port>/?code=4/ab`, or the address of another page. This paste goes to the agent's listener.
  - In the mock (and probably in Antigravity), that request gets a 400 and the `authenticate` fails.
  - `finish_agent` has already taken `Running::Agent` (auth.rs:171) and returns "the login failed: …".
- **How it goes wrong in the cockpit:**
  1. `AuthFinished(Err)` keeps the `Login`, with `ran_out` false and `finishing` false.
  2. `waiting` stays true, so "Finish signing in" is still the header button, and `again` is false, so "Sign in" is not on the top line.
  3. The user corrects the paste and tries again. They get "no sign-in is waiting: sign in again".
- **Effect:** the test name claims a guarantee the code does not give.
- **Fix:** add a case where the mock rejects the callback. Assert either that the login is still waiting (if that is the intended rule), or that the cockpit offers "Sign in" again.

## 2. No test of the login-file status with the default (native) home on macOS, where it is wrong

- **Where:** `crates/harness-acp/src/control.rs:175` and `packs/harness/antigravity-acp/pack.toml` (`kept = "{gemini}/antigravity-acp/acp_token.json"`; the platforms include `macos-arm64`).
- **Background:** `AGY_ACP_FORCE_FILE_STORAGE=1` is only set through `[provides.settings.home]`, which applies only when the home is kept. The plan says that on macOS the login otherwise goes to the Keychain.
- **How it goes wrong:**
  1. A macOS project server uses the default native home and signs Antigravity in. The token goes to the Keychain.
  2. `probe` finds no `acp_token.json` and reports `SignedOut`.
  3. The cockpit shows "Not signed in" and puts "Sign in" on the header forever.
- **What the test misses:** `status_comes_from_its_login_file_when_it_cant_say` (acp_login.rs:103) uses an absolute temp path, so it cannot catch this.
- **Fix:** add a test that runs the shipped pack's settings with no kept home and checks what status is reported. Given the bug, that test would currently fail.

## 3. Untested: a home given as an existing project folder is chmod'ed to 0700 and ignored by git

- **Where:** `src/startup.rs:42` (`path => project.join(path)`) and `crates/harness-acp/src/driver.rs:44`.
- **How it goes wrong:** with `[home] antigravity = "."` (or `"src"`), `make_home` chmods the project root (or `src/`) to 0700. If that folder has no `.gitignore`, it writes one containing `*`, so git ignores the whole project (or all of `src/`).
- **What the tests cover:** `a_harness_whose_home_the_project_keeps_is_started_with_it_there` (tests/packs_tests.rs:153) uses only fresh folders. No test pins what happens with a folder that already holds project files, or with a path outside the project (`../x`, or an absolute path).

## 4. The home test can flake: it races the driver's background first refresh on the same file

- **Where:** `tests/packs_tests.rs:153-169`, with `crates/harness-acp/src/driver.rs:91`.
- **How it goes wrong:**
  1. `driver.create` spawns `first.refresh()`, which starts `/bin/sh -c "env > env.txt"`.
  2. The test then calls `instance.refresh()` itself (line 174), which starts a second shell writing the same file.
  3. The test awaits only its own refresh. If the background shell starts later, its `>` truncates `env.txt` before it writes.
  4. A `read_to_string` in that window sees an empty file, and the `GEMINI_HOME=` assertion fails.
- **Fix:** write to a file named per run (for example with `$$`), or wait on the snapshot's status from the first refresh instead of starting a second one.

## 5. A misspelled harness key in `.nah/harnesses.toml` is silently ignored, and no test covers it

- **Where:** `src/startup.rs:25-45`.
- **How it goes wrong:** the keys are instance ids (`antigravity`), but users install the pack as `antigravity-acp`. `[home] antigravity-acp = "kept"` parses fine and matches nothing. The harness runs native, and its login lands in the user's own `~/.gemini` with no word said.
- **What the tests cover:** `a_harness_whose_home_the_project_keeps_is_started_with_it_there` writes `other = "homes/other"`, which has no instance, and expects it to be accepted.
- **Fix:** add a test that an unknown harness name is reported (as a problem on the snapshot, or at start).

## 6. The login tests run on a hand-copied Antigravity config; the contract test duplicates the first one

- **The copy:** `crates/harness-acp/tests/acp_login.rs:20` (`GOOGLE`) duplicates the link pattern, `within_s` and `on_session` from the pack. If the pack's pattern changes, or `on_session` is dropped, all five acp_login tests still pass.
- **The duplicate:** `tests/harness_contract.rs:226` (`a_login_it_runs_itself_shows_its_link_and_says_how_long_it_waits`) runs the same begin-and-see-the-link check against the shipped settings. The filter `!auth_terminal` leaves only Antigravity.
- **What that contract test asserts:**
  - `within_s.is_some()` checks the pack's contents, not behaviour.
  - `challenge.within_s == within_s` checks a field copied through.
- **Its other weaknesses:**
  - It forces the Antigravity loopback mock and the method `oauth-personal` on any future non-terminal pack, whatever that pack's real login does.
  - If no pack passes the filter, it passes vacuously.
- **Fix:** cut the contract test. Have acp_login build its profile from the shipped `antigravity-acp` settings (as `shipped()` does) instead of `GOOGLE`.

## 7. The cockpit tests skip the real paste path, and one assertion echoes its own input

- **Where:** `crates/cockpit-ui/src/chat/tests.rs:323` and `crates/cockpit-ui/src/agents.rs:442-450`.
- **What is skipped:** the test injects replies through `sign_in_began_for_test` and `sign_in_finished_for_test`, and never goes through `Act::FinishSignIn` (agents.rs:288). So these are untested:
  - setting `finishing`;
  - hiding "Finish signing in" while the paste is checked (actions.rs:247);
  - clearing `refused` on a new paste.
- **The echo:** line 342 asserts that `refused` equals the exact string the test posted one line earlier. It tests the copy, not a behaviour.
- **Fix:** drive the paste through the command (`ChatCommand::CompleteLogin`) with a `NetClient` pointed at nothing. Assert that `finishing` hides the button, then post the reply.

## 8. Tests pin wording where a variant or status exists, including one coupling across crates

- **`crates/harness-acp/tests/acp_login.rs:98`:** asserts that `err.to_string()` contains `"authentication"`, which is the substring the cockpit's `needs_login` searches for (`chat/render.rs:136`). The two sides of that string contract are tested in different crates, and the cockpit side is not tested at all. The adapter also sets the status to `SignedOut`, which the test already checks through `seen`. Match `HarnessError::AuthRequired { .. }` instead.
- **Other assertions on message text** where matching the error variant would hold:
  - `acp_login.rs:72`: `"isn't the address"` (the variant is `Refused`).
  - `acp_login.rs:87`: `"no sign-in is waiting"` (the variant is `Conflict`).
  - `acp_login.rs:56`: the exact `expects` sentence.
  - `tests/packs_tests.rs:193`: `"native, kept, or a folder"`.
  - `chat/tests.rs:326`: `"Not signed in"`.
- **Merges to make:**
  - Fold `a_paste_with_no_login_waiting_says_so` (acp_login.rs:82) into test 1 as a second `complete` after the successful one. That also proves a finished login is cleared.
  - Fold `a_harness_home_setting_left_empty_is_refused` (packs_tests.rs:188) into the home test.

## 9. No test that a session opens once the loopback login is done

- **Where:** `crates/harness-acp/tests/acp_login.rs:48` and `:91`.
- **What exists:** the refusal (`a_session_opened_signed_out_is_refused_at_once`) and the sign-in are tested separately.
- **What is missing:** nothing shows that, once signed in (the mock's `signed_in` set), `open_session` with `on_session = true` succeeds. If `asks_login` matched a link said for some other reason, every session would be refused.
- **Fix:** open a session at the end of test 1.

## 10. Several `begin_agent` and `finish_agent` branches have no test

All of these are in `crates/harness-acp/src/auth.rs:92-178`:

- **No link within `wait_s`:** "the login showed no link within N seconds".
- **A login that finishes at once:** for example, a key in its environment, giving "Signed in." with no link.
- **A second `begin`:** it should abort the first login. Untested are that the first link's port is now refused and that the first agent process is killed.
- **A paste after the agent's 300 s ran out on the server:** the connect fails, giving `Conflict` "isn't waiting any more". This is the server half of the plan's "says clearly when … the time ran out". Only the cockpit's own clock is tested.
- **`END_WAIT`:** the case where the login does not end after the callback.

## 11. Untested against the real wire and the real process

- **Error output from a real process:** `ProcessLauncher` forwarding stderr to `said` (`crates/harness-acp/src/launch.rs`) is only exercised through the mock's in-memory channel.
  - The plan's step 1 depends on it ("an agent's error output reaches what the driver asks of it").
  - A `/bin/sh -c 'echo … https://accounts.google.com/x >&2; sleep 1'` launch, read with `says`, would cover it cheaply.
- **The real link format:** `callback_port` (auth.rs:217) is only tested against the mock's own link (`testing.rs` `loopback_link`, with uppercase `%3A`/`%2F` and `127.0.0.1`).
  - Nothing covers `localhost`, lowercase escapes, or the actual link captured live in step 5.
  - The test's own `port_of` (acp_login.rs) parses the mock's format again.
- **The challenge on the wire:** the server serializes `harness_core::AuthChallenge` and the cockpit deserializes its own `net::AuthChallenge`; these are two structs.
  - No test round-trips one into the other.
  - A renamed `withinS` would silently turn off the cockpit's timer, because a missing `Option` field deserializes as `None`.
- **The route:** no route test covers `/auth/complete` returning 400 for `Refused` with its reason in `{ "error" }`.

## 12. The cockpit's clock never moves in the tests

- **Where:** `crates/cockpit-ui/src/chat/tests.rs:323` and `:351`.
- **What happens:** `egui::Context::default()` stays at time 0, so `left > 290.0` is always 300. Running out is tested only by the degenerate `within_s = 0`.
- **What is missing:** nothing tests that 300 s later the login runs out. Nothing tests that clicking "Sign in" after running out starts a fresh login and clears `ran_out`.
- **Order dependence:** the test needs two `tick`s only because `ran_out` is computed before the replies are drained.

## 13. The full `ProviderSnapshot` literal is copied in five places

- **Where:**
  - `crates/cockpit-ui/src/chat/tests.rs:152`
  - `crates/cockpit-ui/src/chat/tests.rs:278` (new)
  - `crates/cockpit-ui/src/agents.rs:469`
  - `crates/nah-notify/src/rules/tests.rs`
  - `tests/common/mod.rs`
- **Effect:** adding `home` meant editing each copy.
- **Fix:** build the new signed-out fixture from one shared snapshot helper, changing only `auth` and `home`.

## 14. `BROWSER=true` is pinned as a TOML literal in two tests

- **Where:** `tests/packs_tests.rs:76` and `:133`.
- **The problem:** both assert that `launch.env` equals exactly `[("BROWSER","true")]`. That is the pack's contents, not behaviour.
- **Effect:** any later variable the pack adds breaks both tests, while nothing checks that the harness actually opens no browser.

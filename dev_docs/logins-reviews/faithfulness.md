# Review of fcb0865..HEAD (logins), faithfulness

Problems are ranked most severe first. Paths are relative to `meta-harness/`.

## 1. The home setting went into a file another reader already owns, with a format that doesn't fit it. Setting any home disables every harness.

- Plan, step 3: "`.nah/harnesses.toml`: each harness's home, `native` (default), `kept`, or a path."
- Rule: "One mechanism per need: … extends the existing system … never a parallel one."
- Code: `src/startup.rs:30-51` (`harness_homes`) reads `.nah/harnesses.toml` as `{ home: {…} }` with `deny_unknown_fields`.
- `src/packs.rs:46-60` (`own_launches`) already reads the same file (and `nah/harnesses.toml` in the config folder) as `BTreeMap<instance, OwnLaunch>`. `OwnLaunch` also has `deny_unknown_fields` and requires `program`. README.md:203-215 documents that format: `[antigravity] program = …`.

How it goes wrong:
1. Following README.md:824, the user writes `[home]\nantigravity = "kept"` into `.nah/harnesses.toml`.
2. `own_launches` reads `home` as an instance with no `program` and an unknown field `antigravity`, and fails.
3. At `src/packs.rs:87` every harness pack gets `HarnessLaunch::Unavailable { reason: "<file>: …", fix: "fix the file it names" }`. No harness can start, Claude Code included.

The other way round:
1. A project that already has an instance launch override (`[antigravity]\nprogram = "/opt/agy/…"`, as README.md:211 shows) is started.
2. `harness_homes` fails with "unknown field `antigravity`".
3. `startup::harnesses` passes the error up with `?` (`src/startup.rs:184`, `src/main.rs:207`), so the whole project server doesn't start.

Nothing tests the two readers on one file: `tests/packs_tests.rs:153` writes `[home]` and calls only `harness_homes`.

## 2. Native is the default home, but on macOS the native status always reads "signed out", even straight after a successful sign-in.

- Plan, decided: "native (its own folders; the default)".
- Plan, step 1: "Status where the agent can't say: present when its login file (`login.kept`) is there."
- Plan, found: "on macOS in the Keychain unless `AGY_ACP_FORCE_FILE_STORAGE=1`".
- Code: `packs/harness/antigravity-acp/pack.toml:139` (`kept = "{gemini}/antigravity-acp/acp_token.json"`) and `crates/harness-acp/src/control.rs:174-178` (status from whether that file exists).
- `AGY_ACP_FORCE_FILE_STORAGE=1` is set only in `[provides.settings.home]` (`pack.toml:144`), so only when the home is kept. The pack ships for `macos-arm64` (`pack.toml:20`).

How it goes wrong:
1. A project server on a Mac uses the default (native) home.
2. The user signs in through the cockpit. `authenticate` succeeds, `on_auth(Some(true))` runs, then `instance.rs:88` refreshes and runs `probe`.
3. The token went to the Keychain, so `acp_token.json` is missing and the status is set back to SignedOut.
4. The cockpit shows "Not signed in" and the "Sign in to Antigravity" banner (`render.rs:92`, `:378-379`), although sessions open.

A user already signed in natively on that Mac sees the same.

## 3. After a wrong paste, Claude Code's login is gone, but the cockpit keeps offering to paste again.

- Plan, step 4: "takes the paste, says clearly when the paste is wrong or the time ran out". This applies to every harness ("every harness's browser login must work").
- Code: `crates/cockpit-ui/src/agents.rs:289` ("Kept until it's taken: a wrong paste can be tried again") and `chat/actions.rs:247` (Finish signing in stays the header button after a refusal).
- On the server, `auth.rs:150-158` `take_command` takes the `LoginRun` out. `login.rs:99-111` `finish` consumes it and waits for the process to exit.

How it goes wrong:
1. With Claude Code, the user pastes a mistyped code.
2. `claude auth login` exits non-zero and the cockpit shows "Not taken: the login failed: …". It still shows the link and the "Finish signing in" button.
3. The user pastes the right code. `complete` finds no command, falls to `finish_agent` (`auth.rs:164-167`) and gets "no sign-in is waiting: sign in again".

Retrying a paste was built only for the loopback (Antigravity) login. The cockpit applies it to every login.

## 4. The login under way exists only in the cockpit tab that began it. A reload, or a second device, loses it, and restarting the login kills the one the user is finishing.

- Plan, decided: "A harness's login belongs to the project server running it; the UI server only relays the login's steps to the browser."
- Plan, goal: logins from "a laptop, a phone, iPad".
- Code: the challenge (link, `within_s`, refusal) lives in `Agents.login` (`crates/cockpit-ui/src/agents.rs:135`, `Login` at `:65-91`), filled only from the HTTP reply to `begin_auth`. Nothing about a waiting login is in the project server's snapshot or event stream.

How it goes wrong:
1. On a phone, the user starts the sign-in, switches to the browser and signs in, and the cockpit tab is reloaded.
2. `login` is `None`. `needs_login` is true, so the banner shows "Sign in to Antigravity" with Sign in as the header button, and no link or time is shown.
3. The user presses Sign in. `begin_agent` aborts the waiting login (`auth.rs:96-99`), whose callback port is the one in the address the user copied.
4. The user pastes that address. It is refused as "isn't the address…" because the port is wrong (`auth.rs` `hand_back_callback`).

A cockpit on a second device never sees the link or the time left.

## 5. For the default (native) home, the snapshot and the cockpit say nothing about where the home is.

- Plan, step 3: "The snapshot says where each harness's home is."
- Plan, step 4: "the harness's line says signed in or not, and where its home is."
- Code: `crates/harness-acp/src/driver.rs:81` and `instance.rs:78` set `home` only for a kept home. `crates/harness-contracts/src/snapshot.rs` documents `None` as "its own folders". `crates/cockpit-ui/src/chat/render.rs:383` shows a line only when `home` is `Some`.

With the default every harness has, the harness's line doesn't say where its home is (for example `~/.gemini` or `$GEMINI_HOME`). It says nothing at all, not even "its own folders".

## 6. Home settings are keyed by instance id, and an unknown key is silently ignored, so the harness stays native.

- Plan, decided: "kept (… `.nah/harness/<harness>/` …)". README.md:824 says "by harness".
- Code: `src/startup.rs:37-50` accepts any key. `crates/harness-acp/src/driver.rs:81` looks the home up by instance id (`antigravity`, not the pack name `antigravity-acp`). No check matches the keys against known instances.

How it goes wrong:
1. The user writes `antigravity-acp = "kept"` (the pack's name, which `pack install`/`which` use).
2. The server starts without a word, and Antigravity signs in to `~/.gemini`.
3. The cockpit shows no home line, which is also what native shows (finding 5).

Apart from that, finding 1 makes the file unusable anyway.

## 7. Claude Code's sign-in shows no time left and can never say the time ran out.

- Plan, step 4: "shows the link to open and the time left … says clearly when … the time ran out".
- Code: `packs/harness/claude-code/pack.toml:65-67` sets no `within_s`. `auth.rs` `begin_command` passes `settings.within_s` (here `None`). `agents.rs:100-102` sets `until = None`, so `ran_out` is never set.

Only Antigravity's login gets step 4's time.

## 8. The countdown starts when the cockpit gets the reply, not when the login started waiting, so it shows more time than is left.

- Plan, step 1: "the challenge says how long the login waits (`login.within_s`)".
- Code: `auth.rs` returns the pack's fixed `within_s` (300). `crates/cockpit-ui/src/agents.rs:100-102` sets `until = now + within_s` on the cockpit's clock when the reply arrives.

How it goes wrong:
1. Antigravity's 300 s starts when its listener opens, before it prints the link.
2. The driver then waits up to `wait_s` (30 s) for the link, and the reply crosses two machines.
3. The cockpit shows "0:xx left" after the agent has already stopped. A paste in that window gets "the sign-in isn't waiting any more (its time ran out?)", not the planned clear "time ran out".

## 9. A user-given home path has its permissions and git status changed. The plan didn't ask for this.

- Plan, step 3: "Kept: the folder made (the server's user only)". This describes making the kept folder.
- Code: `crates/harness-acp/src/driver.rs:37-56` (`keep_home`/`make_home`) does the same to any path given, including an existing folder:
  - `set_permissions(0o700)` on the folder itself;
  - writes a `.gitignore` containing `*` if none exists.

How it goes wrong:
1. The user gives `home.antigravity = "/Users/u/.gemini"` to share the native home, or a project folder such as `"tools/agy"`.
2. That existing folder is chmodded to 0700.
3. A project folder becomes wholly git-ignored without being asked for.

The `.gitignore` file is not in the plan at all.

## 10. A bad `.nah/harnesses.toml` stops the whole project server instead of the harness.

- The README's rule for harness settings: "wrong or unknown ones stop the harness, saying why". The plan's setting belongs to the harnesses.
- Code: `src/startup.rs:33,41` return errors (bad TOML, an empty value), and `src/startup.rs:184` / `src/main.rs:207` pass them up with `?`.

How it goes wrong:
1. On a remote project server, the user mistypes `antigravity = ""`.
2. The server exits at start: no code analysis, no terminals, no harnesses.
3. The cockpit can't show why. Compare `own_launches`, which marks the harness Unavailable with the reason (`src/packs.rs:87`).

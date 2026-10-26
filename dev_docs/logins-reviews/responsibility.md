# Review of fcb0865..HEAD (logins), responsibility

Paths are relative to `meta-harness/`. Problems are listed most severe first.

## 1. `.nah/harnesses.toml` now has two readers that expect different formats, so neither the home setting nor a launch override can be used without breaking the other

- `src/startup.rs:20-47` (`harness_homes`) reads `{project}/.nah/harnesses.toml` as `struct Settings { home }` with `deny_unknown_fields`.
- The same file is already the harness-overrides file. It is listed in `packs/index.toml:34` (`overrides = [..., "{project}/.nah/harnesses.toml"]`). `src/packs.rs:44-60` (`own_launches`) reads it as `BTreeMap<instance, OwnLaunch>`, and `OwnLaunch` also has `deny_unknown_fields` and a required `program`.

What goes wrong:

- **Following the README.** A user writes `[home]\nantigravity = "kept"`. `own_launches` parses `[home]` as an instance called "home" and fails (unknown field `antigravity`, missing `program`). `harness_launches` (`src/packs.rs:77`) then marks every harness `Unavailable` with "fix the file it names". No agent starts.
- **A project that already has an override.** The README's own example, `[antigravity]\nprogram = "..."`, is in this file. `harness_homes` now rejects it as an unknown field. `startup::harnesses` returns it with `?` (`src/startup.rs:184`, called at `src/main.rs:207`), so the server no longer starts.

Responsibility problem: a second, separate settings loader was added for a file that `packs.rs` already owns, instead of extending the overrides format. The two readers also disagree on scope and timing:

- Overrides are read from the user's config folder and the project's; homes only from the project's.
- Launches are looked up again on every re-probe; homes are read once and frozen into `AcpDriver::new`. Changing a home needs a restart; changing a launch in the same file does not.

The test (`tests/packs_tests.rs`, `a_harness_whose_home_the_project_keeps_is_started_with_it_there`) calls `harness_homes` and the driver directly. It never runs `harness_launches` over the same file, so the conflict goes unnoticed.

## 2. Finding the login link and timing the login now exist twice in the driver, with separate defaults and separate settings

Command logins (`crates/harness-acp/src/login.rs:19-27, 48-94`) still use their own pieces:

- their own line reader (stdout and stderr) and pattern match;
- their own default pattern `ANY_LINK = r"https://\S+"`;
- a fixed `LINK_WAIT` of 30 s and `FINISH_WAIT` of 120 s.

The new agent-run path has a parallel set:

- `launch::says` over the new `Said` broadcast (`launch.rs:516-536`);
- a second default pattern in `LoginSettings::link_pattern` (`settings.rs:159-161`);
- a new pack setting `login.wait_s`, defaulting to 30 (`auth.rs:94`);
- a separate `END_WAIT` of 60 s (`auth.rs:184`).

What goes wrong: the README documents `login.wait_s` as how long "its login" is given to show its link. A pack that sets `wait_s = 90` for a login command still gets the fixed 30 s, because `LoginRun::start` never reads it. Similarly, a pattern fix or a change to the default has to be made in two places (`login.rs:49` compiles `settings.link` again, with its own error text).

Waiting for a link in an agent's output lines should be one helper, used by both kinds of login.

## 3. The cockpit decides on its own clock whether a server-owned login is still waiting

The plan says the login belongs to the project server and the UI only relays its steps. Instead:

- `cockpit-ui/src/agents.rs:100-103, 363-371` computes `until` from the pack's `within_s`, counted from when the reply reached the browser, and sets `ran_out` itself.
- `chat/actions.rs:243-247, 271` and `chat/render.rs:102-105` hide or show the paste action, the page link and "Sign in again" from that flag.

Meanwhile the server holds the login's real state, `Running::Agent { ended }` (`auth.rs:31, 133`), and never reports it. `within_s` is the pack's claim, not a deadline the server measured: the agent's 300 s starts before it prints the link, and that can take up to `wait_s` of 30 s.

Concrete cases:

- **The agent's login ends early** (the provider errors, or the agent process dies). `on_change(Some(false))` makes the snapshot say signed out. The cockpit's `Login` is independent, so it still shows "4:10 left" with "Finish signing in" as the primary button. The paste then comes back "the sign-in isn't waiting any more".
- **The login finishes some other way**, for example a browser on the server's own machine reaches `127.0.0.1:<port>`. The snapshot turns signed in, but the cockpit keeps the countdown and the paste prompt until its own timer expires.
- **"Stop signing in"** (`Act::StopSigningIn`, `agents.rs:298`) only drops the cockpit's copy and says "Nothing changes". `AuthController` has no way to cancel, so the server's `Running::Agent` keeps the agent process and its loopback listener alive for the rest of the 300 s.

## 4. Opening a session now writes the instance's sign-in status through a side channel, and only for one kind of harness

- `adapter.rs:32-37, 324-326`: `AcpAdapter` now takes the auth controller's `AuthChanged` callback and calls `(self.signed_out)(Some(false))` itself.
- `instance.rs:92` wires the instance's `on_auth` into it.

Sessions already report sign-in state to their instance through `AdapterEvent::CatalogUpdated { sign_in }`, which `harness-core/src/sessions.rs:53-54` applies via `apply_catalog`. Opening a session also already returns `HarnessError::AuthRequired` to its caller (`session.rs:61-62`).

What goes wrong: the adapter (whose job is opening sessions) now also owns auth state, through a second path. The same fact is also handled differently by harness:

- An agent with `on_session` that prints its link marks the instance signed out.
- An agent that refuses `session/new` with ACP `auth_required` (the normal way) returns the same `AuthRequired` error, but its status stays "signed in" or "unknown".

Turning `AuthRequired` into a status change belongs in one place, where the open error is received, for every harness.

## 5. The agent-run login's wording and what it accepts are hard-coded in the driver, and the pack's `login.expects` is ignored there

`auth.rs:121-122` fixes the text and what the paste must be: `expects = "the address of the page that failed to load"`, plus a fixed loopback instruction. `callback_port` (`auth.rs:217-224`) assumes the link carries an OAuth `redirect_uri=` pointing at `127.0.0.1` or `localhost`.

The README and `settings.rs` describe `login.expects` as the pack saying what the page gives back, but only command logins read it (`auth.rs:78`).

What goes wrong: a harness pack whose agent-run login ends with a code sets `expects = "the code the page shows"`.

- The cockpit still says "the address of the page that failed to load".
- Every paste is refused for not starting with `http://127.0.0.1:<port>/`.
- If the link has no `redirect_uri`, `begin` succeeds, `port` is `None`, and every paste fails later with "the login's link names no address to hand back to".

Whether a login takes a loopback address or a code should be the pack's to say, not the driver's guess from the link's shape.

## 6. The variable that moves a harness's home is declared twice in each pack, and `login.kept` silently relies on `home.env`

- **Antigravity.** `packs/harness/antigravity-acp/pack.toml` names `GEMINI_HOME` both in `dirs.gemini.env` (line 129) and in `home.env` (line 144).
- **Claude Code.** `packs/harness/claude-code/pack.toml` names `CLAUDE_CONFIG_DIR` in `dirs.claude`, `dirs.claude_json` and `home.env` (lines 71, 75, 80).
- **What `home` should be.** `home` should name the `dirs` entry it moves, not repeat the variable. If the two copies drift, `{gemini}` (the skills paths, the `kept` login file) points somewhere other than where the harness actually keeps its home.

The status check also depends on settings in the wrong place. `control.rs:175-177` treats "the `login.kept` file exists" as signed in. For Antigravity that file only exists when `AGY_ACP_FORCE_FILE_STORAGE=1` is set, and only `home.env` sets it, in kept mode only.

What goes wrong: a project server on macOS in `native` mode (the default) keeps the token in the Keychain. The probe finds no `acp_token.json` and reports signed out, so the cockpit shows "Not signed in" and a Sign in button even when the agent is signed in.

## 7. "Open no browser on the server" is set as launch environment, in two copies, and any own launch drops it

`pack.toml:96` (`install.launch.env`) and `pack.toml:103` (`provides.launch.env`) both set `BROWSER = "true"`, because a platform's install launch replaces the provides launch whole (`nah-packs/src/engine.rs:460`). An instance's own launch from `harnesses.toml` (`src/packs.rs:83`) also replaces the pack's launch whole.

What goes wrong: a user who points `[antigravity]` at their own program gets no `BROWSER=true`. Antigravity's login then runs `xdg-open` on the server's machine again. This is a property of the harness's login, not of one launch line. It belongs in a setting the driver always applies; the driver already merges `home.env` into whatever launch it has (`driver.rs:37-42`).

## 8. The home folder is split across the binary and the ACP driver, and keys that name no harness are silently ignored

- **Where the folder is.** `src/startup.rs:35-46` (the binary) decides it (`.nah/harness/<key>`, relative paths).
- **Making it.** `harness-acp/src/driver.rs:37-58` creates it, sets mode 0700 and writes a `.gitignore`. Project-folder policy in a driver crate would have to be copied by any second driver.
- **Matching keys.** Nothing checks the keys against the instances. README line 824 says "by harness", and the README calls the packs `antigravity-acp` and `claude-code`. The lookup is by instance id (`driver.rs:81`, `self.homes.get(id.as_str())`), and Antigravity's instance id is `antigravity`, not the pack name.

What goes wrong: `antigravity-acp = "kept"` is accepted without a word. The harness keeps its native home, and the snapshot's `home` is `None`.

## 9. The sign-in reply type is defined twice and was extended in both copies

`harness-core/src/lib.rs:106-118` (`Serialize`) and `cockpit-ui/src/net.rs:60-69` (`Deserialize`) each define `AuthChallenge`, and this change added `within_s` to each. The wire type for `/auth/begin` should live in `harness-contracts`, which the cockpit already depends on, so that one definition is shared. As it is, a field renamed or added on one side only shows up at run time as a missing field.

## 10. The cockpit's one sign-in is shared across the whole desk, not tied to the harness it belongs to

`agents.rs:138` holds a single `login: Option<Login>` with no instance. `chat/render.rs:78-91` and `chat/actions.rs:240-272` show it in every chat buffer, whatever that chat's harness is. `CompleteLogin` (`chat/actions.rs:419-422`) sends the paste to the active chat's instance, not to the instance that started the login. `Reply::AuthFinished` (`agents.rs:75, 408`) carries no instance either.

This was already the case before the change. The change builds the new countdown and the "not taken" state on top of it.

What goes wrong:

- **Pasting in another chat.** Sign-in is started in the Antigravity lead chat. The paste is made in a Claude Code sub-slot chat, which shows the same banner. The paste goes to Claude Code and comes back "no sign-in is waiting".
- **A late reply.** A late `AuthFinished(Ok)` from one paste clears a sign-in started meanwhile for another harness.

# Review of fcb0865..HEAD (logins), consistency

Paths are relative to `meta-harness/`. Most severe first.

## 1. `.nah/harnesses.toml` already exists, with a different shape: using either feature breaks the other

`src/startup.rs:25` (`harness_homes`), `README.md:824`; the existing reader is `src/packs.rs:46` (`own_launches`), and `packs/index.toml:34` lists `{config_home}/nah/harnesses.toml` and `{project}/.nah/harnesses.toml` as the instances' own launches (`README.md:203`).

The existing file holds one table per instance (`[antigravity] program = …`), read as `BTreeMap<String, OwnLaunch>` with `deny_unknown_fields`. The new reader opens the same file and expects only a `[home]` table, also with `deny_unknown_fields`.

- If a project writes `[home]\nantigravity = "kept"`, `own_launches` reads `home` as an instance's launch. It fails on the missing `program` field and the unknown field `antigravity`. Every harness then becomes Unavailable with ".nah/harnesses.toml: missing field `program`", and the fix offered is "fix the file it names".
- If a project already has the documented `[antigravity]\nprogram = "/opt/agy/…"`, `harness_homes` fails with "unknown field `antigravity`". `startup::harnesses` passes that error on with `?`, so the server doesn't start at all.

The new reader also skips the user's `nah/harnesses.toml` layer, which the existing file reads first. The README now describes the same file twice, in two incompatible ways. The new test (`tests/packs_tests.rs:153`) calls `harness_homes` directly and never runs `harness_launches` on the same file, so it doesn't catch the clash.

The change should extend the existing per-instance file (or its reader), not open the same file through a second, incompatible reader.

## 2. A second way to keep `.nah/` folders out of git, and a `.nah/` folder made by the wrong crate

`crates/harness-acp/src/driver.rs:44-56` (`make_home`).

`.nah/` already has one mechanism for this. `arch_service::store::ensure_nah_gitignore` (`crates/arch-service/src/store.rs:124-150`) keeps `.nah/.gitignore` listing `NAH_IGNORED` (`cache/`, `state/`). It writes with `write_atomic` and fills in entries an older file lacks. `state_dir` and `cache_dir` are the helpers that make `.nah/` folders.

The driver instead writes its own `.gitignore` containing `*` into each harness's folder, with a plain `std::fs::write`. So `.nah/harness/` is made from inside `harness-acp`, and kept out of git by a rule that `.nah/.gitignore` doesn't show.

The default folder `.nah/harness/` should be added to `NAH_IGNORED` and made through an arch-service helper like `state_dir`. Only a custom folder outside `.nah/` needs a rule of its own.

## 3. Unknown or misspelled harnesses in `[home]` are silently ignored, unlike every other setting

`src/startup.rs:37-46`.

`deny_unknown_fields` is on the outer struct, but `home` is a free `BTreeMap<String, String>`, and the keys are never checked against the instances. The key is the instance name (`antigravity`), not the pack name (`antigravity-acp`). The pack name is what `meta-harness pack install`/`which` take, and what the README's `.nah/harness/<harness>/` suggests.

Concrete case: `antigravity-acp = "kept"` makes no folder, gives no error, and Antigravity keeps signing in to `~/.gemini`. The user believes the login is kept with the project.

This breaks the rules the code base follows elsewhere: "wrong or unknown ones stop the harness, saying why" (README, pack settings), `.nah/terminals.toml` refusing bad values, and the overrides reader refusing `programme`.

## 4. The cockpit treats every refused paste as retryable, but a command login (Claude Code) is used up by its first paste

`crates/cockpit-ui/src/chat/actions.rs:158` and `crates/cockpit-ui/src/agents.rs:289-295` keep the sign-in open after an error ("a wrong paste can be tried again"). That matches only the agent-run login, where `finish_agent` leaves `Running::Agent` in place.

For a command login, `take_command` (`crates/harness-acp/src/auth.rs:150`) removes the `LoginRun` before `finish`. A wrong code therefore ends that login. The sequence:

1. Sign in to Claude Code and paste a wrong code.
2. The banner shows "Not taken: harness failed: the login failed: …" and "Finish signing in" is still a header button.
3. Paste again: the reply is "no sign-in is waiting: sign in again" (`auth.rs:167`).

The two login kinds follow different rules behind one control.

## 5. The link pattern and "first line that matches" are built twice

- `LoginSettings::link_pattern` (`crates/harness-acp/src/settings.rs:159`) has the default `https://\S+` and the message "the login link pattern is wrong". `LoginRun::start` (`crates/harness-acp/src/login.rs:27,49`) keeps its own `ANY_LINK` constant and compiles its own `Regex` with the same message. `begin_command` (`auth.rs:69`) still passes the raw `settings.link` string down, so the new method is used for one login kind only.
- `launch::says` (`crates/harness-acp/src/launch.rs:32`) scans an agent's lines for the first match. `LoginRun::start` (`login.rs:70-79`) does the same over the command's streams with a `watch` channel. That is two mechanisms for "find the login's link in what it prints".

The project rule is clean cuts: one should replace the other.

## 6. The login's waits: a setting for one kind, constants for the other; and a default written in code, not in serde

- How long a login has to show its link is `login.wait_s` for agent-run logins (`auth.rs:94`, `unwrap_or(30)`) but the constant `LINK_WAIT = 30 s` for command logins (`login.rs:21`).
- How long a login has to finish after the paste is `END_WAIT = 60 s` (`auth.rs:184`) for one kind and `FINISH_WAIT = 120 s` (`login.rs:22`) for the other.
- `sign_in.wait_s` gets its default through `#[serde(default = "default_wait")]` (`settings.rs:118,131`). The new `login.wait_s` is an `Option` whose default 30 is buried at the use site (`auth.rs:94`), and the README repeats it ("`wait_s`, 30").

## 7. URL parsing, percent-decoding and an HTTP request built by hand

`crates/harness-acp/src/auth.rs:217-250`.

`callback_port` finds `redirect_uri=` by substring and decodes only `%3A` and `%2F` with `replace`. It goes wrong in two ways:

- A link with any parameter whose name ends in `redirect_uri` (for example `post_redirect_uri=…`) placed before the real one gives the wrong port, or none. With none, every paste is refused with "the login's link names no address to hand back to".
- A `redirect_uri` with any other escape is misread.

`hand_back_callback` takes the pasted address apart with `strip_prefix` and `split_once`. It then writes a raw `GET … HTTP/1.1` and checks the reply with `starts_with("HTTP/1.1 2")`. `read_to_string` has no time limit, so a listener that keeps the connection open makes `complete` hang.

Maintained crates already do all of this:

- **URLs:** `url` 2.5 is in `Cargo.lock` (`Url::parse(link)?.query_pairs()`, then `.port()`). `urlencoding` is already used by `cockpit-ui` (`net.rs:197`).
- **HTTP:** the workspace's `hyper`/`hyper-util` client (`src/home/pass.rs:13-15`) or `ureq` (`crates/nah-packs/src/engine.rs:391`), with a timeout.

The test helper `port_of` (`crates/harness-acp/tests/acp_login.rs:42`) is a third hand-written parser of the same link.

## 8. A new error kind parallel to `InvalidConfig`, and one user mistake reported as two kinds

`crates/harness-core/src/error.rs:23-26`, `src/server.rs:28`.

`Refused` maps to the same 400 as `InvalidConfig`, which the code base already uses for "what was given isn't acceptable". Examples are `slots.rs:337-344` ("a slot can't be named …") and the old bad-paste message this change replaced. Nothing else moves to the new kind, so there are now two kinds for one need.

In the same flow, a wrong Claude Code code comes back as `Provider` (502, "harness failed: the login failed: …", `login.rs:110`). A wrong Antigravity address comes back as `Refused` (400). The cockpit can't treat "the paste was wrong" the same way for both.

## 9. The cockpit shows sign-in errors in two different ways

`crates/cockpit-ui/src/chat/render.rs:106,125`.

The existing pattern for an error is its own banner: `banner(ui, colors().error_bg, colors().error, …)` with an `X_CIRCLE` label and the text in `colors().text` (`render.rs:85-89`, fed by `Agents::errors`). A failed *start* of sign-in (`Reply::AuthStarted(_, Err)`) still goes there.

A refused paste ("Not taken: …") and a login that ran out are now drawn instead as error-coloured lines inside the blue info banner. So the same sign-in shows its errors in two places and two styles, depending on which step failed.

## 10. Words on screen

- "login" and "sign-in" are mixed in messages that end up in the same banner. `auth.rs` says "the login failed", "the login stopped", "the login didn't finish within 60 seconds of the address", and "the login's link names no address" (130-178). It also says "no sign-in is waiting", "the sign-in was started again meanwhile", and "the sign-in isn't waiting any more" (167, 172, 240). The cockpit says "The sign-in stopped waiting", "Sign in", "Finish signing in".
- Raw protocol text reaches the user: "Not taken: harness failed: callback rejected: HTTP/1.1 400 Bad Request" (`auth.rs:248`).
- `NOT_SIGNED_IN` (`crates/harness-acp/src/adapter.rs:42`) is wrapped by `AuthRequired`'s Display, so the user reads "authentication required: not signed in: sign in, then try again". That says the same thing twice.
- The banner repeats the server's instruction. Claude Code's challenge already says "…then paste the code the page shows here." (`auth.rs:84`). Antigravity's says "…Copy its address and paste it here." (`auth.rs:121`). `render.rs:116` then adds "Then paste {expects} with Finish signing in." Both instructions also say "here", but there is nowhere to paste in the banner.

## 11. `kept` means three things, and `{kept}` is filled outside the template system

- `login.kept` is a template of the login *file* (`settings.rs:151`), filled by `Places::path`.
- `home.env`'s `{kept}` is the home *folder*, filled by a separate `value.replace("{kept}", …)` in `HomeSettings::env_for` (`settings.rs:195-196`). It doesn't go through `Places::fill`, the one mechanism every other template (`{home}`, `{project}`, `dirs`) uses.
- `"kept"` in `.nah/harnesses.toml` is a mode.

A pack author who writes `login.kept = "{kept}/…"` gets the literal text `{kept}`, because that template is filled by `Places`, which doesn't know `{kept}`.

## 12. `home.env` repeats what `dirs` already says

`packs/harness/claude-code/pack.toml:70-81`, `packs/harness/antigravity-acp/pack.toml:122-148`.

`dirs.claude` and `dirs.claude_json` already say `CLAUDE_CONFIG_DIR` moves them, and `dirs.gemini` says `GEMINI_HOME`. `[provides.settings.home] env` names the same variables again, so the same fact lives in two places in each pack. A pack that changes one and not the other has skills and tool files looked up in one place while the agent runs from another.

`BROWSER = "true"` is also written twice in the Antigravity pack (`install.launch` and `provides.launch`, lines 96 and 103). A comment ties the two together, nothing else does.

## 13. Stale doc on the setting this change widened

`crates/harness-acp/src/settings.rs:38`: the `login` field is still documented as "Logins by the agent's own login command." It now also covers logins the agent runs itself (`within_s`, `wait_s`, `kept`, `on_session`).

## 14. Test helpers copied instead of shared, and a name reused

- `crates/harness-acp/tests/acp_login.rs:13-18` copies `PROJECT` and the TOML-to-`Profile` helper from `tests/acp_adapter.rs:13-22`, not from `harness_acp::testing`, where the mock and the new `loopback_link` already are.
- `MockAgent::signed_in` (`crates/harness-acp/src/testing.rs:160`, "its loopback login went through") has the same name as `MockSetup::signed_in` (`testing.rs:49`, who it reports being signed in as). The mock's `session/new` checks one and ignores the other.
- `crates/cockpit-ui/src/chat/tests.rs:276` writes out a full `ProviderSnapshot` again. `agents.rs:469` already has a private `snapshot()` builder; `queue.rs:488` shows the pattern of making such a builder `pub(crate)` for other tests. Because the literal is repeated, this change had to add `home: None` to five copies.

## 15. `begin_agent` re-implements `replace`

`crates/harness-acp/src/auth.rs:96-99` takes and aborts the waiting agent login by hand. `AcpAuthController::replace` (`auth.rs:60`) exists for exactly that, and the same function calls it again at line 133.

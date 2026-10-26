# Review of fcb0865..HEAD (logins, round 2), faithfulness

Paths are relative to `meta-harness/`. Each point compares something the
plan or `logins-findings.md` says against the current code. Ranked most
severe first.

## 1. Pastes are not tied to a sign-in by id on the server (Fixed 6 and 9)

Findings, Fixed 6: "pastes are tied to it by id (a late answer about an
older one is dropped; one about a sign-in that's over is said as an
error)". Fixed 9: "It acts on the sign-in it read, by id".

Code: the cockpit reads the id (`crates/cockpit-ui/src/agents.rs:313-321`)
but only uses it to read the reply. `NetClient::complete_auth`
(`crates/cockpit-ui/src/net.rs:627`) sends only `{ "response": … }`.
`AuthController::complete` takes no id, and `AcpAuthController::complete`
(`crates/harness-acp/src/auth.rs:300-313`) acts on whatever sign-in is under
way when the request arrives. A command login is `take()`n whatever its id.

How it goes wrong (Claude Code):
1. Device A shows sign-in 3. The user signs in on a phone and pastes the
   code into A.
2. Before the paste reaches the server, device B (or A from the palette)
   starts sign-in 4. `begin` kills login 3 and holds login 4.
3. The paste arrives. `complete` takes login 4 and writes code 3 into its
   input (`login.rs:103-105`). Login 4 fails, sends `Waiting(None)` and
   `Status(Some(false))`, and B's sign-in, still in progress, is gone.

The server can't refuse "that was for a sign-in that's been replaced"
because it never gets the id. The cockpit's id check only decides how the
reply is shown.

## 2. A login command is not held from its start, so a second sign-in doesn't replace it (Fixed 8 and 11)

Findings, Fixed 8: "the server holds a sign-in from its start, so a new one
replaces it at once". Fixed 11: "It's held from its start and shown when its
link comes."

Code: only the agent's own login is held from its start
(`auth.rs:222`). `begin_command` (`auth.rs:130-157`) awaits
`LoginRun::start`, which can take up to `wait_s` (30 s). Only after that
does it store the login, with a plain assignment at `auth.rs:152`. That
assignment doesn't stop what it replaces, and it sends no event for it.

How it goes wrong:
1. Device A taps Sign in for Claude Code (id 1). `stop_under_way` finds
   nothing, and `claude auth login` starts.
2. Within those seconds, device B taps Sign in (id 2). `stop_under_way`
   again finds nothing, because id 1 isn't held yet, so a second
   `claude auth login` starts.
3. Id 2 shows its link first and is stored, and the snapshot says id 2.
   Then id 1 shows its link and overwrites it at `:152`. Login 2's process
   is killed (`kill_on_drop`), and the snapshot now says id 1. The newer
   request lost to the older one. B's user may already have opened link 2.

If the overwritten entry is an agent login (a harness offering both kinds),
dropping `Running::Agent` only drops its `JoinHandle`. The login task keeps
running detached, can't be cancelled, and still sends `Status` when it ends.

## 3. "Starting the sign-in" exists only in the tab that tapped it (Fixed 6; commit 12fb861)

Findings, Fixed 6: "The sign-in under way lived in one browser tab … Now
it's the server's (`AuthState.waiting`, per harness, every device)".

Code: the server holds the agent's login from its start (`auth.rs:222`),
but the snapshot shows nothing until the link comes (`auth.rs:201-203`).
For Antigravity that is 14 s measured, and up to 60 s by its pack. The
"Starting the sign-in…" state added in 12fb861 comes from
`Agents.signing_in`, a set in one tab's memory
(`crates/cockpit-ui/src/agents.rs:304-311`, `chat/render.rs:98`).

How it goes wrong:
1. Device A taps Sign in for Antigravity, which shows "Starting the sign-in…".
2. Device B, or A after a reload, sees the plain "Sign in to Antigravity"
   banner. Because `needs_login()` is true, Sign in is the header button
   (`chat/actions.rs:246`).
3. Tapping it calls `begin`, and `stop_under_way` aborts A's login. A's
   request then fails with "the sign-in ended before it showed a link"
   (point 6). This is the "a second device lost it; starting again killed
   it" case that Fixed 6 says is closed, now during the starting phase.

## 4. The README says a kept home is closed to the agent's file access; only the agent's own kept home is (Fixed 4)

README (`README.md:302-307`): "`"kept"`, `.nah/harness/<instance>/` (made
for the server's user only, kept out of git and never served, nor to the
agent's file access)".

Code: `Profile::workspace` (`crates/harness-acp/src/profile.rs:182-184`)
closes only `self.kept_home`, the home of the harness the session belongs
to. Fixed 4 words it the same way ("refuses its kept home"), but the README
promises more, and other harnesses' kept homes stay open.

How it goes wrong: Claude Code and Antigravity both have `home = "kept"`.
An Antigravity session (`client_fs = true`) calls `fs/read_text_file` on
`.nah/harness/claude-code/.credentials.json`. That path is inside the
session's root and isn't Antigravity's kept home, so it is read and the
Claude Code login goes to the Antigravity agent. `.nah/state/` is open to
the agent's file access in the same way, though `/api/fs/read` refuses it.

## 5. A sign-in that runs out or is turned down sets "Not signed in" even when the harness is signed in (Fixed 5, plan step 4)

Findings, Fixed 5: "Status from the login file overrode what the agent
said … Now the file says 'signed in' only while nothing else has said."
Plan step 4: "the harness's line says signed in or not".

Code: when the agent's login ends for any reason other than being stopped,
it sends `Status(Some(result.is_ok()))` (`auth.rs:219`). Its time running
out (`auth.rs:209`) or a callback the agent turns down is therefore a
"signed out". Once the status is `SignedOut`, the login file is never
consulted again (`instance.rs:160,168`), and no check runs after "signed
out" (`instance.rs:82`). The test
`the_server_stops_a_sign_in_when_its_time_is_up` asserts this
`Status(Some(false))`.

How it goes wrong: Antigravity is signed in, either by its kept
`acp_token.json` or by a sign-in earlier in this run. The user taps Sign in
(to switch account, say) and leaves it. Five minutes later the server ends
it, and the harness line says "Not signed in" with Sign in as the header
button. Its stored token is untouched and sessions still open. The status
stays wrong until another full sign-in or a server restart.

## 6. A stopped sign-in is reported as something else (Fixed 14)

Findings, Fixed 14: "'Ended before it showed a link' lost how it ended. It
waits for the end (5 s) and says it."

Code: aborting the agent's login task, by Cancel or a new sign-in, drops the
`ended` watch sender. `how_it_ended` then returns `None` at once
(`auth.rs:274-279`), and both callers turn that into a wrong reason:

- While the address is being checked (`finish_agent`, `auth.rs:261-267`): a
  Cancel or a new sign-in from another device makes the paste fail at once
  with "the sign-in didn't finish within 120 seconds of the address".
  Nothing waited 120 s; it was stopped.
- While starting (`begin_agent`, `auth.rs:231-237`): the starting request
  fails with "the sign-in ended before it showed a link". It was replaced or
  cancelled (the case in point 3).

## 7. The hand-back's connect error is still dropped (Fixed 33)

Findings, Fixed 33: "an error replaced without its source (the hand-back's
connect) carries it now."

Code: `let Ok(mut stream) = … connect(…).await else { return Ok(false) };`
(`auth.rs:377`) throws away the `io::Error`. The user sees "the sign-in
isn't waiting any more: sign in again" (`auth.rs:257`) whatever the connect
failure was. In the same block, a connect that hits the 10 s
`HAND_BACK_WAIT` lands in `Err(_) => Ok(true)` (`auth.rs:384-388`) and
counts as "delivered" though nothing was sent. `finish_agent` then waits the
full 120 s `FINISH_WAIT` and ends the sign-in with "didn't finish within 120
seconds of the address".

## 8. The chat's status line still looks the instance up four times a frame (Fixed 24)

Findings, Fixed 24: "Per-frame lookups in the chat's status line
(performance 6). One." Performance 6 asked for "One `instance()` borrow at
the top of the function".

Code: `ChatStreamState::status` (`crates/cockpit-ui/src/chat/render.rs`)
still calls `self.instance()` at lines 384, 385, 390 and 401. Each call is a
`RefCell` borrow and map lookups, every frame, for every chat panel.

## 9. The home isn't "always" in the snapshot (Fixed 28)

Findings, Fixed 28: "The snapshot gives its home always: the folder, and
whether it's kept."

Code: `Profile::home` (`profile.rs:53-56`) returns `None` when the pack has
no `[provides.settings.home]`, and the contract's comment says "when its
pack says" (`crates/harness-contracts/src/snapshot.rs:33-36`). An ACP pack
without a `home` table shows no home line, native or not. The two shipped
packs both have the table.

## 10. Override files are not merged "field by field" (Fixed 1)

Findings, Fixed 1: "read by the one reader, from both files (the user's,
then the project's), field by field."

Code: `own_settings` (`src/packs.rs:60-85`) treats the launch as one unit.
A later file's `program` replaces the earlier `args`, `env` and `env_remove`
wholesale. A later file that gives `env` or `args` without `program` is
refused, which makes every harness unavailable (`src/packs.rs:77`). Only
`home` and the launch are separate.

How it goes wrong: the user's `~/.nah/harnesses.toml` sets `[antigravity]
program = …`, and the project's file adds
`[antigravity] env = { X = "1" }`. Every harness is unavailable with "a
launch's args or env without its program", where field-by-field merging
would have added `X`.

## 11. Words (Fixed 30)

Findings, Fixed 30: "'Sign-in' throughout; … 'authentication required: sign
in, then try again'".

Code:

- A session refused for not being signed in reads "authentication required:
  not signed in: sign in, then try again" (`adapter.rs:152` under
  `HarnessError::AuthRequired`'s "authentication required: {reason}").
- A login command's failures still say "login": "the login failed: …",
  "the login didn't finish", "the login no longer takes input"
  (`crates/harness-acp/src/login.rs:103,108,113`). A wrong Claude Code code
  reaches the cockpit as "harness failed: the login failed: Invalid code",
  while the agent's own path says "the sign-in failed".

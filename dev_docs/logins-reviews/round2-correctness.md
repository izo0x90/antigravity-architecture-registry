# Review of fcb0865..HEAD (logins, round 2), correctness

Ten problems, most severe first. Paths are relative to `meta-harness/`.

## 1. A paste isn't tied to its sign-in on the server: an old code ends a newer Claude Code sign-in

`src/routes/harness.rs:102-110`, `crates/harness-acp/src/auth.rs:300-321`,
`crates/cockpit-ui/src/agents.rs:313-321`

Findings 6 and 9 say pastes are tied to their sign-in by id. That holds only
for the cockpit's display. `POST /auth/complete` carries only `{response}`,
and `complete` acts on whatever sign-in is under way when the request
arrives. The cockpit takes the id from its snapshot when Finish is tapped,
not from the sign-in the user actually opened, so even its own `Pasted.id`
is the new one.

How it goes wrong (Claude Code, whose login takes one answer):
1. Device A shows sign-in 1. The user opens its link and gets code 1.
2. Device B taps Sign in. `begin` replaces sign-in 1 with sign-in 2 (a new
   `claude auth login` with its own PKCE state). A's snapshot now shows
   sign-in 2.
3. On A the user pastes code 1 and taps Finish. The cockpit records
   `id = 2`. The server's `complete` takes run 2 out of the slot, sends
   `Waiting(None)`, and runs `run.finish("code 1")`, which fails.
4. Sign-in 2 is over (`Status(Some(false))`). B's link is dead and B gets
   no reason, because the error goes only to A.

The same thing happens with no second device: a `begin` that lands between
the user copying the code and tapping Finish.

## 2. When an agent sign-in fails after its paste, the reason can disappear from the cockpit

`crates/harness-acp/src/auth.rs:215-220`,
`crates/cockpit-ui/src/agents.rs:433-447`,
`crates/cockpit-ui/src/chat/render.rs:92`

The agent's task does three things in this order: it sets `ended`
(line 215), then sends `Status`, then calls `end()`, which sends
`Waiting(None)`. `finish_agent` wakes on `ended` and replies at once. The
reply and the snapshot event take separate paths (HTTP, and the event
stream through the UI server), so the reply can arrive first.

How it goes wrong:
1. On Antigravity, the user pastes an address whose code Google refuses.
   The callback is delivered and `authenticate` fails.
2. The `AuthFinished` error reaches the cockpit while its snapshot still
   has `waiting.id == id`, so `still` is true. The error goes into
   `pasted.refused`, not into `errors`, and `finishing` becomes false.
   Meanwhile "Finish signing in" is offered again for a sign-in that is
   already over.
3. `Waiting(None)` arrives. `sign_in_now()` returns `None`. The refusal
   banner is drawn only inside `now` (`render.rs:92`), so it disappears.
   `errors` never had the reason.
4. The user sees only "Sign in to Antigravity", with no word of why it
   failed.

The "didn't finish within 120 seconds" path (`auth.rs:265`, `end()` and
then the reply) has the same race.

## 3. "Not signed in" is set by sign-ins that say nothing about the login, and then it sticks

`crates/harness-acp/src/auth.rs:219`, `auth.rs:319`,
`crates/harness-acp/src/instance.rs:82,168`

The agent's login task reports `Status(Some(result.is_ok()))` however it
ended. That includes "its time ran out" (the server's own limit), a launch
that failed, and a refused callback. A wrong paste to a command login does
the same (`Some(false)` at line 319). Cancel reports nothing. For
Antigravity, which never reports its status, nothing afterwards clears
`SignedOut`:
- `Status(Some(false))` skips the re-check.
- The file hint applies only while the status is `Unknown`.
- A session that opens doesn't set `SignedIn`.

How it goes wrong:
1. Antigravity is signed in. Its token file is there, so the status is
   `SignedIn`.
2. The user starts "Sign in" from the actions (it is always offered), for
   example to switch accounts, and leaves it. After 300 s the task times
   out and reports `Status(Some(false))`.
3. The snapshot says signed out. The cockpit shows "Not signed in" and the
   "Sign in to Antigravity" banner. Sessions still open, and the status
   stays wrong until another sign-in through the cockpit succeeds. A server
   restart does not help while the snapshot persists in memory.

A second sequence: a session is refused (the status becomes `SignedOut`).
The user then signs in some other way, or puts a token file into the kept
home, and presses Retry. `refresh` ignores the file because the status
isn't `Unknown`, so it stays "Not signed in" while sessions work.

## 4. The user's `harnesses.toml` breaks every harness in a project that lacks one of its names

`src/packs.rs:71-73`, `packs/index.toml:34`

The override files are `{config_home}/nah/harnesses.toml` (shared by every
project) followed by `{project}/.nah/harnesses.toml`. Every name in either
file must be one of this project's harness instances, and harness packs can
come from `{project}/.nah/packs`.

How it goes wrong:
1. Project A has a project pack giving the instance `my-agent`. The user
   puts `[my-agent] home = "kept"` in the user-level file.
2. The user opens project B, which has no such pack.
3. `own_settings` returns `Err("there's no harness named my-agent")`, and
   every harness in B becomes `Unavailable` ("fix the file it names"),
   Antigravity and Claude Code included.

The same happens if a distribution pack is renamed or removed.

## 5. Two begins at once: the older can win, and a replaced agent login keeps running unseen

`crates/harness-acp/src/auth.rs:130-152`, `auth.rs:222`

`begin` calls `stop_under_way()` before `begin_command` starts waiting on
`LoginRun::start`, which can take up to `wait_s`. The command run is put
into the slot only after that wait, with `*lock(&self.under) = Some(...)`,
and it overwrites whatever is there. Overwriting an `Agent` entry drops its
`JoinHandle` without aborting it.

How it goes wrong (a harness offering both a command login and an agent
login, or two devices):
1. Device A begins a command login (id 1). The slot is empty while the
   command starts.
2. Device B begins an agent login (id 2). The slot holds agent 2, whose
   link is shown.
3. Command 1 prints its link. Line 152 overwrites agent 2 and
   `Waiting(1)` is shown. B's newer request lost.
4. Agent 2's task still runs, with its agent process and its callback
   port, until `within_s` (300 s). It then reports `Status(Some(false))`,
   which marks the harness signed out while sign-in 1 may just have
   succeeded. Its `end(2)` does nothing.

With two command begins, the one whose command prints its link last wins,
not the last one asked for.

## 6. Making the instance anew in the middle of a sign-in: an orphan login, and reused ids show an old refusal

`crates/harness-core/src/registry.rs:76-83`, `auth.rs:66,99,290`,
`crates/cockpit-ui/src/agents.rs:304-311`

Retry calls `refresh_instance`. When the harness's config has changed (a
`home` edited in `harnesses.toml`, say), it calls `reconfigure`, which
builds a new instance. The old controller is dropped, but its agent login
task holds its own `under` and its process, so it is never aborted. The
new controller's ids start again at 1.

How it goes wrong:
1. Antigravity sign-in 1 is waiting. On device A the user pastes a wrong
   address. `pasted = {antigravity, id 1, refused: "that isn't the
   address…"}`.
2. The user edits `home` and presses Retry. A new instance appears with no
   `waiting`. The old login's agent process stays alive for up to 300 s,
   holding its port (on the sandbox, an emulated agent).
3. The user taps Sign in. The new controller hands out id 1. `Act::SignIn`
   doesn't clear `pasted`, so `pasted(antigravity, 1)` matches and the
   fresh link appears with "Not taken: that isn't the address…" above it.

A server restart (the cockpit only reconnects) reuses ids the same way. A
late `AuthFinished` about the old id 1 is then also taken as being about
the new one.

## 7. The callback is always handed to 127.0.0.1, though `[::1]` and `localhost` are accepted

`crates/harness-acp/src/auth.rs:340-356`, `auth.rs:377`

`callback_port` and the paste check accept an IPv6 loopback or `localhost`
(`[::1]` is in the test), but `hand_back_callback` always connects to
`("127.0.0.1", port)`.

How it goes wrong:
1. An agent's link has `redirect_uri=http://[::1]:4001/`. Or it says
   `localhost` and its listener is bound to `::1` only, as Node does by
   default since v17.
2. The user pastes `http://[::1]:4001/?code=…`. The paste passes the check,
   the connect to 127.0.0.1:4001 is refused, and the function returns
   `Ok(false)`.
3. The cockpit says "the sign-in isn't waiting any more: sign in again"
   while the login is still waiting on its port.

## 8. Cancel or a new sign-in is reported as the wrong failure

`crates/harness-acp/src/auth.rs:231-237`, `auth.rs:261-267`

When the task is aborted (by Cancel or by a new begin), its `ended` sender
is dropped, so `how_it_ended` returns `None` at once.

How it goes wrong:
- Device A pastes and `finish_agent` waits on `how_it_ended`. Device B
  cancels or starts a new sign-in. A gets "the sign-in didn't finish within
  120 seconds of the address", immediately. The login that was being
  finished was killed, and nothing says so.
- Device A begins (the agent takes 14 s to show its link). Device B begins
  too. A's begin gets "the sign-in ended before it showed a link". A's
  device shows that error banner above B's live link.

## 9. The snapshot shows a native path as the kept home when the harness can't start

`crates/harness-acp/src/driver.rs:66-76`, `crates/harness-acp/src/profile.rs:52-55`

When `launch` is `Err` (the pack isn't installed yet, or `make_home`
failed), `Profile::new` gets `None` and `Places` falls back to the server's
`GEMINI_HOME` or `{home}/.gemini`. `keeping_home(home)` still marks it
kept.

How it goes wrong: with `home = "kept"` and Antigravity's pack not
installed, the harness line says "Its home …, kept for this project:
/home/u/.gemini". That is the user's own home, not `.nah/harness/antigravity`.

## 10. `home = "~/…"` lands inside the project

`src/packs.rs:95`

`~` isn't expanded, so `project.join("~/agy")` gives `<project>/~/agy`.
That passes the "not the project or above it" check.

How it goes wrong:
1. The user writes `home = "~/.agy-home"`, meaning their own home folder.
2. The folder is made in the project's tree: not in `.nah/.gitignore`, and
   not in `private_dirs`.
3. After sign-in, `acp_token.json` is in the working tree. It shows up in
   the changes view, can be committed, and `/api/fs/read` serves it.

# Logins: every harness's browser login, from any device

## Why

A harness's login has to work when the browser (a laptop, a phone), the
UI server and the project server running the harness are three machines,
with nothing installed on the device. Claude Code's login does (a link,
then a code pasted back). Antigravity's doesn't: its link is thrown away,
its status is "unknown", and opening a session while it's signed out
starts a login nobody sees and waits five minutes.

## Decided (with the user, 2026-10-23)

- A harness's login belongs to the project server running it; the UI
  server only relays the login's steps to the browser.
- First: the browser logins solid for every harness.
- Where each harness keeps its home (login, settings, sessions) is a
  setting per project server: native (its own folders; the default) or
  kept (a folder of the project's, `.nah/harness/<harness>/` unless a
  path is given). Each pack says how its home moves; the server knows no
  harness by name.
- No hacks: no logging in through another program (the `agy` CLI) and
  carrying its login over.

## Found, checked live (2026-10-23, Antigravity ACP server 1.1.0 here)

- It offers `oauth-personal` (Google), `oauth-business`, `gemini-api-key`,
  `agent-platform`. The two Google logins only finish by a callback to
  `127.0.0.1:<random port>` on its machine (a listener answers there;
  it waits 300 s). No code-to-paste mode (its code: loopback only).
- It prints "Open the following link to authenticate the ACP server:
  <url>" on its error output, after trying to open a browser. With
  `BROWSER=true` it opens none (no tab on the user's Mac through the
  sandbox's `xdg-open`) and still prints the link.
- `session/new` while signed out starts that login itself and doesn't
  answer (40 s and more): a session can't be used to ask the status.
- It keeps its login at `<home>/antigravity-acp/acp_token.json`, `<home>`
  being `$GEMINI_HOME`, else `~/.gemini`; on macOS in the Keychain unless
  `AGY_ACP_FORCE_FILE_STORAGE=1`.

## The path (three machines)

1. The cockpit asks the project server (through the UI server) to begin.
2. The harness starts its login; the project server reads its output for
   the link (the pack's pattern) and answers with it, how long it waits,
   and what to paste back.
3. The user opens the link on any device and signs in.
4. Claude Code: the page shows a code. Antigravity: the browser goes to
   `127.0.0.1:<port>` on the device, which fails; its address is copied.
5. Pasted in the cockpit, it goes to the project server: the code into
   the login command, or the address replayed to the harness's listener.
6. The harness trades it with its provider for a token, kept in its home.

## Steps

1. **The driver.** An agent's error output reaches what the driver asks
   of it (lines, while it runs). A login the agent runs itself waits (up
   to `login.wait_s`) for its link (`login.link`) and answers with it;
   the challenge says how long the login waits (`login.within_s`). A
   session opened while the agent shows its login link fails at once,
   said: not signed in (the status set). Status where the agent can't
   say: present when its login file (`login.kept`) is there. Tests with
   a fake agent that does as Antigravity does.
2. **The packs.** Antigravity: its link pattern, 300 s, its login file,
   `BROWSER=true` (it opens no browser on the server's machine), its home
   folder (`GEMINI_HOME`) as a dir the skills use. How each pack's home
   moves (`home.env`): Claude Code `CLAUDE_CONFIG_DIR`; Antigravity
   `GEMINI_HOME` and `AGY_ACP_FORCE_FILE_STORAGE=1`.
3. **The setting.** `.nah/harnesses.toml`: each harness's home, `native`
   (default), `kept`, or a path. Kept: the folder made (the server's user
   only), the pack's `home.env` given to the harness. The snapshot says
   where each harness's home is.
4. **The cockpit.** The login says what to do in plain words, shows the
   link to open and the time left, takes the paste, says clearly when
   the paste is wrong or the time ran out; the harness's line says
   signed in or not, and where its home is.
5. Tests, lints, snapshots; live in the user's tab up to Google's page
   (link shown, a wrong paste refused, the time shown); reviewers on the
   whole; fixes. The user's part: one real Google sign-in.

## Reviewed (2026-10-24)

Eight reviewers; findings and what was done in `logins-findings.md`. The
biggest changes from the steps above: the home is a field of the
instance's table in the existing `harnesses.toml` (not a `[home]` table);
the sign-in under way is the server's, in the snapshot, for every device
(Cancel and the time limit on the server); the login file says "signed in"
only while nothing else has said. Two points are the user's to decide.

## Not in this

- Token logins (an API key or a long-lived token pasted in the cockpit).
- Getting logins across machines in other ways (a helper on the device).
- A login carried between sandboxes.

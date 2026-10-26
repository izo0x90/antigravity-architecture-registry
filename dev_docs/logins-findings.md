# Logins: review findings

Eight reviewers (the seven of `process/reviewers.toml` and anti-patterns)
read `fcb0865..0be8482`. Reports word for word: `logins-reviews/`. They
found 92 problems, many the same one seen from several angles. Each point
below: **Fixed**, **Kept** (with why), or **Decide** (the user's).

## Fixed

1. **The home setting went into `.nah/harnesses.toml` through a second
   reader, clashing with the launch overrides already there** (correctness 1,
   permissions 6, faithfulness 1 and 10, responsibility 1, consistency 1).
   Writing `[home]` made every harness unavailable; a project with a launch
   override stopped the server from starting. Now `home` is a field of the
   instance's own table (`[antigravity] home = "kept"`), read by the one
   reader, from both files (the user's, then the project's), field by
   field. A bad file stops the harnesses saying why, never the server.
2. **A name that isn't a harness was ignored silently** (correctness 13,
   faithfulness 6, consistency 3, tests 5, responsibility 8;
   `antigravity-acp` for `antigravity`). Refused, naming the harnesses.
3. **Any folder given as a home was chmodded and git-ignored** (correctness
   12, faithfulness 9, tests 3, consistency 2). A home is made only when
   missing (for the server's user only); one that's the project's folder or
   above it is refused; `.nah/harness/` is kept out of git by `.nah/.gitignore`
   (arch-service's list), not by a file written into the folder.
4. **A kept login was readable** (permissions 1, 3, 4). `/api/fs/read`
   refuses `.nah/state/` and `.nah/harness/` (tested with `..` in the path);
   the agent's file access refuses its kept home; out of git, so out of the
   changes view, the timeline's snapshots and the changes tool.
5. **Status from the login file overrode what the agent said** (correctness
   2 and 3, faithfulness 2, anti-patterns 1, tests 2). On a Mac with its own
   folders (the Keychain) it always read "not signed in"; with a stale
   file, a refused session's "signed out" flipped back. Now the file says
   "signed in" only while nothing else has said; no file says nothing.
6. **The sign-in under way lived in one browser tab** (faithfulness 4,
   responsibility 3 and 10, correctness 9, anti-patterns 9 and 14,
   performance 2). A reload or a second device lost it; starting again
   killed the login being finished; Cancel never reached the server; a
   paste could go to another harness. Now it's the server's
   (`AuthState.waiting`, per harness, every device): Cancel stops it on the
   server (`/auth/cancel`), the server ends it when its time is up, and
   pastes are tied to it by id (a late answer about an older one is
   dropped; one about a sign-in that's over is said as an error).
7. **The countdown started when the reply reached the browser** (faithfulness
   8, anti-patterns 10). The server says when it stops waiting (`until_ms`,
   counted from the login's start) and stops it then.
8. **Two taps started two logins** (correctness 4, performance 3). The
   cockpit asks once per harness until answered; the server holds a
   sign-in from its start, so a new one replaces it at once.
9. **A paste could finish a newer sign-in** (correctness 5, anti-patterns 8).
   It acts on the sign-in it read, by id; a login ends only itself.
10. **The callback's reply decided the outcome; its read had no limit**
    (correctness 6, performance 4, anti-patterns 4 and 10, permissions 5).
    Handing back takes at most 10 s and reads at most 64 KB; once delivered,
    how the login ended says whether it was right (a redirect is fine).
11. **A begin request that went away left the login running unseen**
    (correctness 7). It's held from its start and shown when its link comes.
12. **The 30 s for the link counted the emulated start-up** (correctness 8).
    Measured live: 14 s. Antigravity's pack gives 60 s.
13. **After a wrong Claude Code code the cockpit still offered to paste**
    (faithfulness 3, consistency 4, anti-patterns 3). A login command's
    sign-in is over after its paste; the cockpit says why and offers Sign in.
14. **"Ended before it showed a link" lost how it ended** (correctness 10,
    anti-patterns 7). It waits for the end (5 s) and says it.
15. **A link with no callback port was accepted** (correctness 11,
    anti-patterns 12, consistency 7). Parsed with `url`; refused at begin.
16. **A variable set by the launch and the home resolved differently**
    (correctness 14). The setting replaces the launch's own of that name.
17. **Only one way of refusing a session set "signed out"** (responsibility 4).
    Any session refused for not being signed in sets it, in one place.
18. **Two ways to find a login's link and two sets of waits**
    (responsibility 2, anti-patterns 5, consistency 5 and 6). One `says` for
    both kinds; one pattern, checked when the settings load and compiled
    once; `wait_s` (a serde default) for both; one wait to finish.
19. **A bad pattern was ignored at session open; `on_session` with any link**
    (performance 7, anti-patterns 6 and 13). Checked at load: a bad pattern,
    or `on_session` without `link`, stops the harness saying why.
20. **`home.env` repeated the dirs' variables; `{kept}` was a template of
    its own** (responsibility 6, consistency 11 and 12). `home.dir` names the
    dir whose variable moves it; `{kept}` is gone.
21. **`BROWSER=true` was written into two launches, and an own launch lost
    it** (responsibility 7, anti-patterns 15, tests 14). `env` in the pack's
    settings, added by the driver to whatever starts it.
22. **Every refused session started a second agent to check again**
    (performance 1). No check after "signed out".
23. **The whole desk repainted every second during a sign-in**
    (performance 5). Only while the countdown is drawn.
24. **Per-frame lookups in the chat's status line** (performance 6). One.
25. **File-system calls on async threads** (performance 8). `tokio::fs`, and
    the folder made on the blocking pool.
26. **"Needs sign-in" decided by searching error text** (anti-patterns 2).
    By the status only.
27. **The sign-in reply defined twice** (responsibility 9, tests 11). One
    contract type, `SignInWaiting`.
28. **A harness's own (native) home wasn't shown** (faithfulness 5). The
    snapshot gives its home always: the folder, and whether it's kept.
29. **A refused paste shown in another style than errors** (consistency 9).
    In the error banner.
30. **Words** (consistency 10). "Sign-in" throughout; the paste instruction
    said once; "authentication required: sign in, then try again"; no raw
    HTTP status line.
31. **A stale doc comment, a duplicated replace, test helpers copied**
    (consistency 13, 14, 15, tests 13). `ProviderSnapshot::new` for every
    snapshot built; the mock's flag renamed `login_done`.
32. **Tests asked for** (tests 1, 4, 6-12): a callback the agent turns down;
    the shipped pack's settings instead of a copy (the contract test that
    duplicated it cut); the cockpit's paste through its real path with
    answered calls; variants, not words; a session after signing in; a new
    sign-in replacing one, Cancel, the time running out; a real process's
    error output; the live link's format; the env file written atomically.
33. **ast-grep**: an error replaced without its source (the hand-back's
    connect) carries it now.

## Kept, with a reason

- **Claude Code's sign-in shows no time left** (faithfulness 7). How long
  `claude auth login` waits isn't known (not measured); its pack sets no
  `within_s`. When it ends, the cockpit says so.
- **The callback is handed back by a hand-written GET** (consistency 7). One
  request to a loopback listener, exactly as the browser sends it; a client
  crate adds redirects and pooling it must not do. Bounded (10 s, 64 KB).
- **`Refused` beside `InvalidConfig`** (consistency 8). A paste that's
  wrong (the sign-in still waits) isn't a wrong setting; the cockpit acts
  on it differently (paste again).
- **A login the agent runs itself is taken to call back to a loopback
  port** (responsibility 5). The only kind ACP agents run (Antigravity's);
  a link without one is refused at begin. A pack setting when a second
  kind appears.
- **A home given as your own folder isn't protected** (permissions 3). Used
  as it is; the README says to keep it out of git.
- **The countdown is the server's deadline on the device's clock.** A device
  whose clock is off shows the time off by as much.

## Decide

Decided by the user (2026-10-25): both left as they are for now. It's the
same exposure as running a harness natively with access outside the
project folder. To be addressed at some point (TODO.md).

1. **Agents can change `harnesses.toml`** (permissions 2). An agent that
   writes the project's files can change a harness's launch (already) and
   now its home, and plant a login in a kept folder. Close the project's
   `.nah/` settings to agents' file tools, or move what's per machine out of
   the project?
2. **Agents with a shell can read a harness's login wherever it is** (native
   or kept): they run as the server's user. Only the server's own routes
   and file access are closed here.

## Round 2

Four reviewers (correctness, permissions, faithfulness, responsibility)
re-read `fcb0865..12fb861` after the fixes above. Reports word for word:
`logins-reviews/round2-*.md`. 32 problems, many the same.

### Fixed

1. **The server didn't check which sign-in a paste or Cancel was for**
   (correctness 1, faithfulness 1, responsibility 1). Both name the sign-in
   (`id` in the request); the server acts only on that one. A stale Claude
   Code code no longer ends a newer sign-in.
2. **The cockpit guessed how a sign-in ended from clocks** (responsibility 2,
   correctness 2). The server says it (`AuthState.ended`: signed in, failed
   and why, ran out, stopped); every device shows the same. A failure's
   reason no longer vanishes when its answer and the snapshot cross.
3. **Two time limits, and running out set "Not signed in"** (correctness 3,
   faithfulness 5, responsibility 3). One deadline for every kind, counted
   from the start, ending it as "ran out". A sign-in that fails, runs out or
   is stopped says nothing of the login there is: only success sets a status.
4. **A login command wasn't held from its start; the older of two begins
   could win and an overwritten agent login ran on** (correctness 5,
   faithfulness 2). Every sign-in is held from its start ("Starting the
   sign-in…" in the snapshot, for every device: faithfulness 3); a new one
   stops it at once; a start that finds itself replaced gives up.
5. **A stopped sign-in was reported as something else** (correctness 8,
   faithfulness 6). "That sign-in was stopped (cancelled, or a new one
   started)".
6. **An instance made anew left its login running, and ids repeated**
   (correctness 6). A controller dropped stops its sign-in; ids never repeat
   (from the clock); a new Sign in clears the last paste's refusal.
7. **The hand-back always went to 127.0.0.1** (correctness 7, permissions 5).
   It goes only to the host the link named (`localhost`: either loopback);
   a connect that fails or hangs isn't counted as delivered, and says why
   (faithfulness 7).
8. **Another harness's kept login, and `.nah/state/`, were open to an
   agent's file access; `.nah/.gitignore` could be rewritten to let logins
   into git** (permissions 1 and 2, faithfulness 4, responsibility 4). One
   list (arch-service `private_paths`: `.nah/state`, `.nah/harness`,
   `.nah/.gitignore`) for `/api/fs/read` and every agent's file access; the
   kept folder named by arch-service too (`kept_harness_home`).
9. **Links** (permissions 3 and 4). Writing through a link to something not
   there is refused; a home that's a link isn't used; `/api/fs/read` checks
   the path as asked and as found.
10. **`..` through a missing folder escaped the agent's workspace** (found
    writing the test for 9; older than this change): `missing/../../x` was
    written outside the root. `..` is taken out of the path first.
11. **A home given as a folder in the project, or `~/…`** (correctness 10,
    responsibility 4). A home in the project is `kept` (`.nah/harness/`); a
    folder of your own is outside it; `~/` is the home folder.
12. **The user's own `harnesses.toml` broke projects without one of its
    names** (correctness 4). Only the project's file must name its harnesses.
13. **A harness that can't start showed its own folder as the kept home**
    (correctness 9). The kept folder is shown.
14. **The pattern compiled twice** (responsibility 5). Compiled once as the
    settings are read (`Pattern`); no pattern means any `https://` link,
    found without one.
15. **`says` exported for a test** (responsibility 6). Not exported.
16. **Words; four lookups a frame** (faithfulness 11 and 8). "sign-in"
    throughout the command login too; "authentication required: sign in,
    then try again"; the status line reads its harness once.

### Kept, with a reason

- **After "signed out", a login file put there by hand isn't believed until
  a sign-in or a restart** (correctness 3, second case). A file that's there
  says nothing a refused session hasn't; this is the stale-file case.
- **Overrides merge the launch as a whole, and the home** (faithfulness 10).
  A launch's parts without its program are refused, not merged.
- **The home shows when the pack says how it moves** (faithfulness 9). Both
  shipped packs do; one that can't move it has no home to show.
- **The begin reply also carries the sign-in** (responsibility 6). Callers
  other than the cockpit (the end-to-end tests) read it; the cockpit reads
  the snapshot.

## Round 3

Two reviewers (correctness, permissions) read the round-2 commit (40c2464).
Reports: `logins-reviews/round3-*.md`. 10 problems.

### Fixed

1. **What's said of an older sign-in could wipe a newer one from the
   snapshot** (correctness 1). It's said while the sign-in is held, so in
   order; and the snapshot passes over anything about a sign-in older than
   the one it shows (ids only grow).
2. **While a Claude Code paste was being taken, the sign-in was nobody's:
   Cancel did nothing, a new one ran beside it** (correctness 2). Taking
   the paste is a task held as the sign-in; Cancel or a new one stops it.
3. **A request that went away left a sign-in stuck, or killed a login
   mid-paste** (correctness 5). Every step runs in a task of its own; the
   request only waits for its answer.
4. **Replies misnamed how a sign-in ended** (correctness 6). A request
   about a sign-in that's over is told how it ended (failed and why, ran
   out, stopped, done), from the server's record; a link shown for a
   sign-in already replaced isn't answered as live.
5. **A link above the home, or a link into the project, put a login where
   git sees it** (correctness 3, permissions 1). No folder on the way to a
   kept home may be a link; a home of your own is compared with the
   project as found on disk too.
6. **A failed or run-out sign-in said nothing unless the harness was
   signed out; an old reason could come back later** (correctness 4). How it
   ended is shown whatever the status, until put away on that device
   (Dismiss).
7. **A folder of your own was labelled "kept for this project"; the doc
   still said relative homes are the project's** (correctness 7). "Set for
   this project"; the doc says a folder of your own is outside it.
8. **A committed `!harness/` let kept logins back into git** (permissions 2).
   The server's own `.gitignore` drops lines that undo its entries, at start.

### Kept, with a reason

- **A `localhost` callback is tried at 127.0.0.1 first, then `[::1]`**
  (permissions 3). `localhost` names both; Antigravity's link names
  127.0.0.1 itself. Taking the code needs a process listening on that port
  on the other address, which an agent with a shell could do in worse ways
  (Decide 2).

## Round 4

Two reviewers (correctness, permissions) read the round-3 commit (0dea926).
Reports: `logins-reviews/round4-*.md`. 13 problems.

### Fixed

1. **A failed sign-in's banner stayed though signed in since, and offered no
   Sign in** (correctness 1). Not said once the harness reads signed in;
   Sign in beside it otherwise.
2. **Ids weren't taken in the order sign-ins started; two at once could
   hide the live one** (correctness 2). The id is taken as it starts, held.
3. **A login command starting wasn't held: Cancel didn't stop it**
   (correctness 3). Its start is held as the sign-in (Cancel or a new one
   stops it and its process).
4. **What's said was said while held: an instance dropped then could
   deadlock** (correctness 4). It's queued while held and said after, by one
   sayer at a time, in order.
5. **"the sign-in failed: the sign-in failed: …"** (correctness 5). Once.
6. **How the agent's login ended was published before it was recorded**
   (correctness 6). Recorded first.
7. **A login command's link could be answered as live after it was stopped**
   (correctness 7). Held and shown in one step; not answered if it wasn't.
8. **A second paste while the first is taken was told to wait for a link**
   (correctness 8). "busy: wait for its answer".
9. **`.gitignore` lines letting the logins back in by a glob; a `.gitignore`
   that's a link** (correctness 9, permissions 2 and 3). When any line lets
   something back in, the server's entries go last (git's last match
   wins); a linked one is replaced by a file.
10. **A link inside a kept home** (permissions 1). Refused.

### Kept, with a reason

- **The `.gitignore` is put right at start only** (permissions 4). A pull
  bringing in a line that undoes it while the server runs takes effect
  until the next start; the snapshots' own exclusion of `.nah/harness/` is
  the TODO's (the timeline's), not this plan's.
- **A file a repository commits inside `.nah/harness/` stays tracked**
  (permissions 1, second case). Git keeps tracking what's committed; the
  server can't tell a repository's files from a login's.
- **Put away is per device and per session** (correctness 1). Another
  device or a reload shows it again until signed in or a new sign-in.

## Checked live (the sandbox, Antigravity 1.1.1 emulated)

- Signed out, a session is refused in 13 s (as not signed in) instead of
  waiting on a login nobody sees.
- In the user's Chrome tab (the lead switched to Antigravity for it, then
  back to Claude Code): the sign-in shows Google's link and the time left;
  no browser tab opens on the Mac; a wrong paste is refused, naming the
  port it should start with; a reload brings the sign-in back (it's the
  server's); at its time the server ends it and the harness reads "Not
  signed in"; Cancel ends it on the server and the agent's process exits;
  the harness's line shows where its home is.
- Found live and fixed (12fb861): nothing was on screen for the 14 s before
  the link (now "Starting the sign-in…"); a sign-in ended at its time went
  back to "Sign in" without a word (now it says its time ran out).
- Not checked: Google's page, the paste of the real address and the token
  kept. That's the user's one real sign-in.

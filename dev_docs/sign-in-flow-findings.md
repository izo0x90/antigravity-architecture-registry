# Sign-in flow: findings

Built: 24660e6 (keys, Copy link, the box), dd6c5dd and cc6b530 (found
live), then the review fixes. Four reviewers (correctness, consistency,
tests, faithfulness) read `ee6d6d3..cc6b530`; reports word for word in
`sign-in-flow-reviews/`.

## Found live (2026-10-24, the user's tab)

- **A draft in the box would have gone to the sign-in as its code.** The
  user's unsent "@asdfsaklsja" was in the lead's box when a sign-in
  started. Now the box is emptied for the sign-in and the draft comes back
  after (dd6c5dd, reworked below).
- **Cancel didn't stop Claude Code's login, and 72 had piled up.** Its
  login command is a wrapper (`claude-agent-acp --cli auth login`) starting
  the real `claude` login; only the wrapper was stopped, for ten days. The
  login now runs in a process group of its own, stopped whole, by the same
  piece that stops an agent (cc6b530; not in the plan: recorded here). The
  72 were stopped by hand. Still open (TODO): what the server started is
  left running when the server itself is killed.

## Fixed (reviews)

1. **Enter in the box sent the pasted code to the agent** (correctness 1).
   The box's own Enter (and Ctrl+Enter) sent straight to the agent; only
   the Send action knew the box was the sign-in's. Every way of sending now
   goes through one place (`send_typed`).
2. **A second draft store, which a slot change or a late draft load could
   put into the sign-in's box, then erase** (correctness 2, consistency 1).
   Gone: while a sign-in has the box, the box is empty, nothing is
   reported and the saved draft isn't put back; when it ends, the saved
   draft is taken again (untrimmed, as written: correctness 5).
3. **What a login command started survived once the command had exited**
   (correctness 3, consistency's note). The group's id is kept from the
   start, so it's stopped after the command was waited for too.
4. **Send while a paste is checked threw the text away** (correctness 6,
   consistency 2, faithfulness 2). It stays, and the status line says to
   send again once it's answered.
5. **Two "Finish signing in" with different guards; no top-line button**
   (consistency 3 and 4, faithfulness 1). One: `auth.complete` (`Space a c`,
   a top-line button while the box is the sign-in's) sends what's pasted,
   or puts you in the box to paste. The box's own button says "Send to the
   sign-in".
6. **Tests** (tests 1-8, faithfulness 3 and 4): app-level tests now press
   the keys (`Space a y`, `Space a c`, `Space a x`), type and press Enter,
   check the server calls (the paste named by id, nothing to the agent),
   the saved draft (never the paste), Cancel naming the sign-in and the box
   going back to the agent after; a login command's own child stopped
   whether the login waits or has exited (Linux only, polled). The weak
   chat-level tests went.
7. **Copy link's weight** (consistency 5): kept as it is, said in the code:
   here it's one of the two ways to the page.

## Kept, with a reason

- **A sign-in started elsewhere takes the box between two keystrokes**
  (correctness 4). What was written is saved first and comes back after,
  and the status line says the box is the sign-in's now; the rest typed
  after goes to the sign-in when sent. Taking the box only when the user
  isn't typing would leave two owners for one box.
- **The key sheet snapshot has no sign-in keys** (faithfulness 5): its app
  has no harness signing in; the keys are pinned by the app test pressing
  them.

## Re-read (two reviewers, 225bf33): fixed

1. **The draft given back was the one read at start, not the one written
   since; a sent message could come back** (correctness 1, consistency 1).
   The drafts service answers the slot's draft as it is now (what's to be
   saved, else what was); the box hands back that.
2. **Before the drafts were read, taking the box lost what was in it**
   (correctness 2). The box isn't taken until the slot's drafts are read;
   then what's in it is reported as the draft first.
3. **Two answers to "whose is the box"** (consistency 3). One: the box's
   owner as tended each frame; sending, the offered actions and the hint
   read it.
4. **Finish signing in offered while a paste is checked** (consistency 2).
   Not then.

Kept: **a kept process-group id could, in theory, name another group** if
every process in it ended and the pid came round again within the seconds
before the drop (correctness 3, low). Pids come round far slower here; the
alternative (not stopping a group whose leader was waited for) is the
leak this fixed.

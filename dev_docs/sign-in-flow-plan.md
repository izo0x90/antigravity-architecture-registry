# Sign-in flow: quick by keyboard and mouse

## Why

The first real Antigravity sign-in (2026-10-24) worked, but the link
had to be selected with the mouse to take it to another browser, and
selection is broken. The steps' keys are partly "Space , number", which
moves with whatever else is on the chat's top line.

## Decided (with the user, 2026-10-24)

- Every sign-in step has a fixed key under Account (`Space a`) and a
  button on the chat's top line:
  - Start: `Space a l`, "Sign in" (as now)
  - Copy the link: `Space a y`, "Copy link"; the status line says it's copied
  - Open the link here: `Space a p`, "Open the sign-in page"
  - Paste back: `Space a c`, "Finish signing in" (as now)
  - Cancel: `Space a x`, "Cancel sign-in"
  - Put away a failed or timed-out sign-in's notice: `Space a d`, "Dismiss"
  - Sign out: `Space a o` (as now)
- While a sign-in waits for its answer, the chat's typing box is the
  sign-in's: its hint says what to paste (the failed page's address, or
  the code the page shows), and Send (Enter) finishes the sign-in instead
  of messaging the agent. Cancel, or the sign-in's end, gives it back.
- The link is shown as text (the whole link on hover), not an editable
  box: Copy and Open replace selecting it.
- Built from what the chat already has, nothing new to look at: actions
  (keymap file, top-line buttons), the chat's banners, its typing box and
  Send, copying as the code view and terminal do (to the clipboard, said
  on the status line).
- Not in this: a QR code (TODO, possible enhancements); selecting, copying
  and pasting in general (TODO, to investigate).

## Status

Done 2026-10-24: steps 1-3 (24660e6, dd6c5dd, cc6b530, 225bf33, 0c78461); reviewed and re-read; findings in `sign-in-flow-findings.md`; checked live in the user's tab (keys, copy, the box taken and given back with the user's draft, Cancel ending the login process).

## Steps

1. Keys and buttons: `auth.copy`, `auth.page`, `auth.cancel`,
   `auth.end.dismiss` get their keys in `config/default.toml`; Copy link
   copies and says so. Tests: each key runs its action; copy puts the link
   on the clipboard and says it.
2. The typing box: while a sign-in waits for an answer, its hint says what
   to paste and Send finishes the sign-in (one id, as Finish does); typed
   text never reaches the agent then; it's the chat's again once the
   sign-in ends. The link shown as text. Tests: Send during a sign-in
   finishes it (the agent gets nothing); after Cancel, Send messages the
   agent.
3. README (keys), key sheet snapshot, suite, lints; live in the user's
   tab: start, copy, open, paste back by Enter, cancel; then reviewers on
   the change, fixes, findings in `sign-in-flow-findings.md`.

## Reviews (agreed 2026-10-24)

Reviewers: `process/reviewers.toml` (seven) and anti-patterns (its brief in
`TODO.md`). Checkpoint reviews: three reviewers on that step's changes only.
End reviews: all eight on the whole plan. After fixes, two reviewers re-read
the fixes once; what they still find goes in the findings file, not another
round. Reports word for word in `<plan>-reviews/`.

- End: correctness, consistency, tests, faithfulness (this plan is small:
  no checkpoint).

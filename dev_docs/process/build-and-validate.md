# Building and validating a plan

How a plan is built so it ends up built the way it was planned, not just
"working". Agreed with the user 2026-10-17. Each plan has a **Process**
section naming its stopping points; this is what happens at them and
between them. The reviewers' briefs are in `reviewers.toml` (here, in
`.nah/reviewers.toml`'s shape, so the meta harness can run them later).

## While building each step

- Build the step as planned. A step that turns out costlier than
  planned, or a check pointing at the very thing the step was meant to
  fix: stop and ask the user. Never swap in something cheaper and call it
  done.
- Tests: behaviour a user or an agent relies on, each saying what it
  protects. No tests of wording, none that only test themselves. Every
  problem found live becomes a test first, then gets fixed.
- Check it live in the running cockpit as the user would (below), then
  commit the step on its own.

## Testing live, as a user

Ask "would a person using this be annoyed, confused or slowed down?",
not "did what I built happen?":

- **Look and feel**: flicker, jumps, delays, anything left behind on
  screen, focus going somewhere unexpected.
- **What happens next**: after a reload, a server restart, a dropped
  connection, switching servers, in the quick look, in a second panel.
- **Every way in**: keyboard only, mouse, touch, a phone-width window.
- **Big and long**: lots of output, many items, running for a while.
- **Failing on purpose**: kill the server mid-action, drop the socket,
  slow the network, reload mid-action. What does the user see, and can
  they recover?
- **Measured, not eyeballed**: rapid screenshots for blank or jumping
  frames, timings for anything slow.
- **Does it serve the plan's intent**, not just its letter: the higher
  goal the user stated.

## At each stopping point

1. Linters: clippy (desktop and browser builds), the cockpit's lints,
   ast-grep (read what it flags: fix it, or say why it stays).
2. The whole test suite (one package at a time; disk is tight).
3. The live pass above, across everything built so far.
4. The reviewers (`reviewers.toml`): fresh agents, each given only the
   plan's goal and rules, the short system description below, and the
   changed code (never this session's reasoning, so they don't share its
   blind spots). Each reports concrete problems: file, line, and how it
   goes wrong.
5. Fix what's real. Where a finding is disputed or would change the
   plan, take it to the user.
6. Record in the plan what was found and decided; commit.

At the end: every reviewer on the whole change, and a full "day in the
life" live run through everything the plan built.

## What reviewers are told about the system

- The server (`meta-harness`, crates under `crates/`) runs a project:
  its code analysis, agent harnesses in slots, terminals, notifications,
  a state database; routes check a capability (`Needs<can::X>`), agent
  tools declare what they need, roles are in `nah-access`.
- Home relays the cockpit to every project server; the cockpit reads one
  event stream and calls through `/s/{server}/…`.
- The cockpit (`crates/cockpit-ui`, egui, in the browser) is a shell
  holding buffers (one `Buffer` trait) in panels and a quick look. A
  buffer reads what the desk shares (`Shared`, services as `Held<T>` read
  through `Reader<T>`) and reaches past itself only by asking (`Ask`).
  Hosts draw controls; keys come from the keymap config only. Platform
  pieces are handed in at start-up (`web/`).
- One mechanism per need: a new need extends the existing system or
  replaces it outright, never a parallel one.

## Decisions a plan leaves open

Read `decision-heuristics.md` (next to this file) before making one, and
again after a compaction. Decide the way it leans, keep it reversible,
record it under "Decide". When the user settles something new, add the
lesson there.

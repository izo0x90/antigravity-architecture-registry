# How the user decides: heuristics for decisions made while they're away

Distilled from the user's decisions and corrections (through 2026-10-25).
Read this before making any decision a plan doesn't settle. When it
doesn't settle it either, decide the way it leans, keep it reversible, and
record it under "Decide" for the end summary.

## Architecture

- **One mechanism, not a second one beside it.** If something like it
  exists (an asking path, a diff, a parser, a list, a copy action, an
  overlay), extend or replace it. Never add a parallel version. Shared
  concepts get one abstraction: menus, overlays, an agent's state.
- **Derive, don't duplicate.** Never keep a side copy of data that
  already persists (tags, files, the harness's history). Make data
  self-describing at its source and rebuild from it. Never match things
  up by hashing or comparing whole texts.
- **One owner per format.** If text carries meaning, the code that writes
  it and the code that reads it live in one module and are tested
  together.
- **Couple at interfaces.** Views draw content, hosts draw controls;
  buffers ask, the shell does. Don't reach into another part's internals.
- **Facts belong where they're known.** The server stamps what it knows
  (when, who caused it); a harness's quirks go in its pack; the cockpit
  doesn't guess.
- **Clean cuts, no shims.** A replaced thing is deleted in the same
  change, tests included. No compatibility layers.
- **Zero-cost abstractions are expected.** In Rust a good abstraction
  shouldn't cost anything at run time; that's not an excuse to skip one.

## Performance

- Cost follows what's on screen, not the size of the session or file.
  Nothing scans everything when it loads; work happens for what's shown,
  once, and is remembered.
- Think at scale: thousands of messages, long files, many panels. A test
  with one message and one link proves nothing about that.
- Small, measured costs that happen once are fine (a quarter second of
  CPU per browser). Repeated per-frame or per-event waste isn't.

## Behaviour and UX

- **Vim-style keyboard first, touch and mouse fully supported.** Keys
  stay consistent everywhere (`c` copies in every view). Anything
  tappable is a real, finger-sized target. Pop-ups close by tapping
  outside, as well as with Escape.
- **Consistency wins.** The same thing behaves the same way everywhere. A
  difference is fine when it's deliberate and explained (`:` reaches
  other views; menus show the focused one).
- **Useful power is welcome.** Running an action in a side panel without
  moving focus: yes. Being shown honestly what's happening (the live
  line, "waiting on you", no session running): yes.
- **Settings by how often they change.** Things changed while working
  (how much a chat shows) sit on the panel. Look-and-feel choices (the
  activity animation, fonts) go in config and theming.
- **Defaults stay conservative until a deliberate pass.** Keep egui's
  font; leave a design call for a look-and-theming review rather than
  guessing.

## Scope and risk

- **Leave hard, 50/50 calls as they are** when the cost of the current
  behaviour is low (sessions don't reopen on their own; quick keys
  exist).
- **Security trade-offs the same as running natively are acceptable for
  now**; write them down as a TODO, don't block on them.
- **Don't blow up scope.** A nice idea that isn't needed now goes under
  "Possible enhancements" in TODO.md, ideally in its cheapest correct
  form (Vim through the existing terminal, not an editor of our own).
- **Investigate before building.** Check what tools and harnesses really
  do (probe them) before designing around a guess.
- **Keep generous budgets that are already safe** (the 16 KB attachment
  cap); prefer the agent fetching more itself to pushing more at it.

## Working style

- The user is often away. Run plans end to end without stopping; decide
  in line with this file; record every deviation and open question for a
  realistic end summary. Never stop to wait, and never quietly swap in a
  cheaper stand-in.
- Talk plainly. Write for a person in plain American English, short
  sentences, the full context, no internal shorthand. No silver linings:
  report problems as they are.
- Own mistakes directly, fix them properly, and record the lesson.
- Don't make changes when the user is only asking a question.
- Commit TODO.md and `dev_docs/` as they change (since 2026-10-26); not
  `.nah/plans`. A plan that's fully built is deleted from `dev_docs/`
  (git keeps it): the code is its truth. Only what still has something
  open stays: unticked steps, open decisions or questions.
- Don't touch the user's chat draft. Drive only the user's own Chrome tab.

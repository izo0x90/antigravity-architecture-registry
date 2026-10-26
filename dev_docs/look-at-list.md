# Things to look at

Added by the user with a `++++` prefix; looked at when they say so.

- **Resuming a session is slow to load** (2026-10-26). In a fresh
  cockpit, Space s l resumed the test session (claude-code 1), but it
  took noticeably long. Not very long, but enough to notice.
- **A long message in the chat's typing box disappears below it**
  (2026-10-26). Once the text is taller than the box's room, the extra
  lines go out of sight below, and the box doesn't scroll at all.
- **No quick way to clear the typing box** (2026-10-26). One action
  should empty it. The user's idea: Escape again once already back in
  normal mode.
- **Reconsider the tests** (2026-10-26). There are too many, they cost
  a lot of time and tokens, and many may not be useful. The end review
  found tests that pass without the fix they claim to protect, and tests
  that only test themselves. When the same agent writes the code and its
  test, the test tends to confirm what was built. Ideas: cut duplicates
  and self-testing tests; keep a small set that checks behaviour; have
  someone other than the builder write them; enforce rules with types and
  lints instead of new tests.

The first three and the tests note were looked at on 2026-10-26: see
`rules-and-subagents-plan.md`, Part 5 (the tests note is "Not in this
plan").

## Still to discuss (2026-10-26)

1. **The three end-review leftovers.** Each was put in
   `rules-and-subagents-plan.md`, but not yet talked through:
   - the server resends a sub-agent's whole prompt and answer on every
     state change;
   - link chips are worked out again on every frame;
   - pickers and the action list don't use the shared entry piece.
2. **The ~40 open "Decide" items** across the findings files: chat 10,
   discussions 14, menus and lists 5, chat cleanup 5, tags 3, fonts 2,
   logins 2.

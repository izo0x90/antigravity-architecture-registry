# Tags review: the batch of fixes

The decisions the user made on 2026-10-21 going through
`.nah/plans/tags-everywhere-review.json` in the cockpit, built in one go.
Each item's decision text is on its item in the plan; this is the order
of work and the checklist. One commit per item; the plan item is set to
`done` as each lands. Findings in `tags-review-batch-findings.md`;
reviewers at the end, reports in `tags-review-batch-reviews/`.

Rules carried over: no shims, no compat layers; one mechanism per need;
nothing committed from `dev_docs/`, `TODO.md`, `.nah/plans/`; the user's
chat draft untouched; the user's host Chrome (relay 9222) is the only
browser driven, and only to read.

## Order

- [x] 1. #41 + #60: the `#` trigger generalized (typing box reports a
      trigger character at a word start; the host maps it to an action;
      the palette opens the action with the text set aside, its search
      box empty); the choice mark generic; the caret helper out of theme.
- [x] 2. #48: one "draw a choice" (mark, label, detail) for palette rows
      and header drop-downs.
- [x] 3. #49: the palette ranks only when the text or the list changes.
- [x] 4. #39: the `?` key sheet shows each action's description.
- [x] 5. #37: the Tags view keeps prepared rows, draws visible ones only.
- [x] 6. #6: Tags group `Space k` → `Space t`; talk keeps plain `t`;
      keys snapshot; README; findings item.
- [x] 7. #9: terminal's `keep` goes; a version per buffer kind on place
      and kept blob (one buffer resets, the rest survive); the chat draft
      out of the layout into its own saved thing keyed by slot.
- [x] 8. #17: jumps and marks name a terminal line as a tag does and are
      moved by the server's exact moves; no client-side shifting.
- [x] 9. #33: a lock per terminal; buffered log writes.
- [x] 10. #59: the terminal test observes "half" before producing the rest.
- [x] 11. #3: delete the two test tags `jl`, `testtagforla` (data only).
- [x] 12. Full suite, clippy (native + wasm), ast-grep; snapshots.
- [x] 13. Live in the user's cockpit (host Chrome, relay 9222): each
      item tried as the user would; the lead connected for at most one
      or two turns where an agent is needed (`#` in the chat, a tag's
      name handed on). What was tried and seen in the findings.
- [x] 14. (Self-review instead, at the user's ask.) Adversarial reviewers on the whole batch, one per angle, each
      told to break it: the user's list (`TODO.md` first item: faithfulness
      to the decisions, anti-patterns incl. ast-grep-able global ones,
      logic correctness, the custom lints) plus the angles used before
      (responsibility, performance, permissions, tests, consistency) plus
      angles for this batch (the layout's compatibility across the version
      change; the line identity under resize and restart; locks and
      ordering in the per-terminal writer). Reports word for word in
      `tags-review-batch-reviews/`; every real finding fixed, re-reviewed
      until nothing real is left; what's kept and why, in the findings.
- [ ] 15. Plan items set to done; findings file complete.

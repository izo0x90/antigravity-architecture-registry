Faithfulness review of the discussions plan against `772aa54..0e24374`. All paths below are relative to `meta-harness/`. Problems are ranked, most severe first.

**1. A question an agent asks you can't be settled in the cockpit. High.**
- Where: `crates/cockpit-ui/src/discussion.rs:424-429`. The Settle action is only available when the status is Answered.
- What should happen: the engine lets you settle an Open discussion asked of you (`crates/nah-discussions/src/lib.rs:468`). The 6f7b5b5 fix and the README ("An agent may ask you: say your answer, then settle it", `README.md:552`) both promise this.
- What happens: an agent calls `discuss` with `ask: user`. You reply in the discussion buffer, but it stays Open, so Settle stays greyed and `a` does nothing. The cockpit has no delete action either.
- Effect: the discussion goes back and forth forever. On a plan component it blocks that component's approval for good, and the only way out is a raw HTTP call.
- Missed by tests: the cockpit tests never cover a discussion asked of you; only the server route test does.

**2. `discussions` and `discussion_read` never show a non-lead slot what's waiting on it. Medium-high, breaks F.1 ("what's waiting on the agent first").**
- Where: `src/mcp/sets/discussions.rs:125-126` and `:169`.
- Cause: `mine` compares `whose_move()`, which gives a slot's *name* (`discussing::asked` returns `s.name`), with `caller.slot`, which is the slot's *id*.
- Effect: the lead's id and name are both `lead`, so the lead works. Every other slot has an id like `sub-1a2b3c4d` and a different name. When you pick a sub-slot under "Who answers?", that sub-slot sees the discussion under "The rest", never "Waiting on you", and `discussion_read` says "`<name>`'s (asked)" instead of "yours".
- Missed by tests: `tests/discussions_tests.rs:51,53,75` test only the lead.

**3. A slot can be asked about something it isn't allowed to read, and then it can never answer. Medium.**
- Where: `src/discussing.rs:16-23` (`asked`) and the cockpit's "Who answers?" step (`crates/cockpit-ui/src/app/starting.rs:55`). Neither checks whether the picked slot can read the thing.
- Example: you start a discussion on the lead's chat turn and ask a sub-slot. The sub-slot is told about it and invited to answer, but `discussion_read` and `discussion_answer` refuse it (`reach.rs`: "a chat this session can't read"). It stays on that slot's move for good.
- This is separate from Decide 8 (a closed slot).

**4. Built differently from the plan, recorded as "Fixed" instead of asked as "Decide". Medium.**
The plan's process says such changes go to the user as Decide; each of these changes behaviour the plan states.
- **Whose move:** D.1 says "the agent's when it's open". The code says it's the starter's move once the slot asked has spoken last (`crates/harness-contracts/src/discussions.rs:141`).
- **Asking a slot other than the lead:** this now needs `slots.message` (`src/mcp/sets/discussions.rs` `discuss`). F.2 says starting needs `discuss` only, and F.3 says a discussion "goes to the lead unless it names an agent slot". In effect, sub-slots and workers can't name another slot.
- **Slot names:** no two slots may share a name, and none may be named "user" or "you". Slot creation now refuses these (`crates/harness-core/src/slots.rs:333-341`). This is a new rule on slots that the plan doesn't ask for.

**5. Rule 5 is bypassed for change requests asked of you. Low-medium.**
- Where: `crates/nah-discussions/src/lib.rs:468` applies to both kinds.
- Effect: a `change_request` an agent asks of you can be settled while Open, with no answer naming what changed ("a change request's answer names what changed"). Your answer route was removed, so this is the only way such a request closes.

**6. Searching discussions' text with FFF is built but nothing can reach it. Low.**
- Where: `Discussions::find` (`crates/nah-discussions/src/lib.rs:302`) is only called from that crate's own tests. No route, tool or cockpit view uses it.
- Plan: the Decided list says discussions' text is "searched with FFF". As built, neither you nor an agent can search it.

**7. A.5 is unfinished.**
- `.nah/ui/state.json` is still on disk and `.nah/.gitignore` still lists `ui/`.
- `ensure_nah_gitignore` (`crates/arch-service/src/store.rs:130-150`) only adds missing lines, so the file is not "written for it" as A.5 says.
- Step 1 is still unticked because of this. It is recorded as Decide 1 in `discussions-findings.md`, but Decide 1 is not on your list of open items in the brief, so I'm noting it.

**8. H.1: the review plan misstates and omits a few things. Low.**
- **#6 shows as `open`** (item `i7`) in `.nah/plans/tags-everywhere-review.json`. The plan records it as decided (`Space k …`), and step 8's notes say "#6 was already in". `tags-everywhere-findings.md:44` was never marked, though the plan's checklist says that file is updated as each finding closes.
- **The reports are named, not linked.** Each item's text lists report paths as plain text, which can't be opened from the card. H.1 says "linked from each". It's ambiguous whether naming them is enough.
- **The plan file is not committed.** The step 9 notes cite "your rule: `.nah/plans/` stays out". I found no such rule in the repo or your memory files, and the plan's Decided list says `plans/` is "committed by default". You can confirm which is right.

**9. Small gaps.**
- **G.7 test:** it asks for starting a discussion from "a plan item" in the cockpit. The cockpit test starts one on a plan section (`crates/cockpit-ui/src/app/tests/discussing.rs:13-21`). Only the server test uses an item (`tests/discussions_tests.rs:40`).
- **C.3 plan tree:** a component's row counts only discussions on the component itself. Discussions on its relations are listed under it as rows but not counted in "1 waiting on you…" (`crates/cockpit-ui/src/tree/plan.rs:64-91`).

**Areas with nothing real found:**
- **E (moving plans' comments):** the field, `CommentAnchor` and the old tools are gone, and `clean_cut` checks it.
- **A.1–A.4:** `nah-things` is split out, the FFF walk reaches the logs, the 64 MB per-project cap is in, and the layout 10 conversion is reduced to an empty step.
- **D.2:** activity is recorded in the same transaction.
- **F.2:** the `discuss` capability is given to the lead, sub-slots and workers.
- **F.4:** no old tool names are left in packs or prompts.
- **G.1:** the chat and discussions draw with one conversation component.
- **G.3–G.5:** keys come from the keymap and the host draws the controls.
- **I (README):** matches the code, apart from the claim in item 1.

The stopping-point-3 fixes I checked hold (one-step change request, the inbox notifying each time, a refused send coming back to its box), except the "settle a question asked of you" fix, which works only through the route (item 1).

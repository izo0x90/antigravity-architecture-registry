I reviewed `772aa54..0e24374` read-only, without building. Below are the problems I found, worst first.

**1. High: a discussion an agent asks of the user can never be settled from the cockpit, and it holds plan approvals for good.**
- Where: `crates/cockpit-ui/src/discussion.rs:424-429`. The Settle action is only available when the status is `Answered`.
- The engine allows settling it while Open when `asked == USER` (`crates/nah-discussions/src/lib.rs:468`). Nobody can answer it, because `answer` requires `d.asked == by` (lib.rs:447) and the user has no answer path.
- Sequence: the lead calls `discuss on:"plan p component a" ask:"user"` → the inbox says "asks you" → the user replies → Settle, Reopen and Change can't settle it (status stays Open). The lead can only reply, which hands the move back to the user, and so on forever.
- Component `a` can't be approved (`startup.rs` blocker). The only way out is a raw HTTP POST to `/settle`.
- Related: in the engine, the `Open && asked==USER` arm comes before the change-request arm. So a change request asked of the user settles with no answer naming what changed, which breaks rule 5 as written.

**2. Medium: "Waiting on you" and "yours" never work for any slot except the lead.**
- Where: `src/mcp/sets/discussions.rs:125-126`.
- `me` is the caller's slot id (`sub-1a2b3c4d`), but `Move::Agent(..)` carries `d.asked`, which is a slot name (`discussing::asked` returns `s.name`, e.g. `sub 1`). Only the lead matches, because its id and name are both `lead`.
- Sequence: the user starts a discussion asking `sub 1`. `sub 1` calls `discussions`: the discussion is listed under "The rest", and `discussion_read` tells it "sub 1's (asked)" instead of "yours". F.1's ordering (what's waiting on the agent first) is broken for sub-slots and workers.

**3. Medium: after a session reload, every note on a chat turn or message reads as gone. #16 isn't achieved.**
- Where: `src/chats.rs:133` and `:308`. `settle` stores the turn's `turn_id`, and once an id is stored, `turn()` matches by id only, never by number.
- Replayed turns get fresh uuids (`crates/harness-acp/src/state.rs:277`), and `Chats::moved` can't map the old id because `numbers` doesn't have it.
- Sequence: start a discussion on `chat lead turn 3`, restart the server, reopen the lead's session. Turn 3 is there under the same number, but `there()` is false. The cockpit shows it as kept-not-source, `discussion_read` says "…is gone from where it was", and the tag gutter treats it as gone.
- No test covers a turn noted, then the session reloaded.

**4. Medium-low: an agent's question asks itself by default and reaches nobody.**
- Where: `src/discussing.rs:16-22`. `asked()` rejects user→user but allows lead→lead. `discuss` with no `ask` defaults to the lead.
- Sequence: the lead calls `discuss on:"plan p component a" text:"is this ok?"`. The result is `by=asked=lead` and whose move is the lead's. The user gets no inbox item, and `tell` skips the lead's own message.
- It stays Open and blocks approval of `a` until the lead answers its own question.

**5. Medium-low: a change request can be "answered" without touching what it's on.**
- Where: `crates/nah-discussions/src/lib.rs:436-455` and `src/plan_things.rs:183-196`.
- Naming `plan p` passes `touches()` for any component, because a plan contains its components. `changed(Plan)` is true whenever any plan-level field changed since the last review.
- Sequence: the plan's title was edited, then the user files a change request on component `a`. The agent answers `changed:["plan p"]` and it's accepted with `a` untouched.
- Separately, "changed" means changed since the last review, not since the request. A component with an unreviewed edit made before the request satisfies it immediately.

**6. Low-medium: text that failed to send can come back in another discussion's box and be sent there.**
- Where: `crates/cockpit-ui/src/discussion.rs:130, 245-250`. `show_target` (389-399) doesn't clear `unsent`.
- Sequence: reply in d1, open d2 before the response arrives, the d1 POST fails. d1's text and purpose land in d2's box, and Enter posts it to d2.

**7. Low: a discussion that fails to start throws away what was typed.**
- Where: `crates/cockpit-ui/src/app/starting.rs:99-125`. `discussing` is `take()`n before the POST, and on error only a note is shown.
- Sequence: the server refuses (for example the terminal lines are no longer there), and the user's message is lost.

**8. Low: comments in a plan file that arrives after start-up block approvals until a restart, and can be silently dropped.**
- E.1 says comments move "as each plan is read", but moving runs once, at start (`crates/arch-service/src/store.rs:325`). A file brought in later by `git checkout` or `pull` blocks that plan's approvals until a restart.
- At that restart, de-duplication by `from` ("plan p comment c1") skips a different comment that reuses the id `c1` from another branch, and the file is rewritten without it, so it is lost.

**9. Low: `#` can open the tag picker by mistake.**
- Where: `crates/cockpit-ui/src/conversation.rs:166-170`. `names_tag` only checks that the text ends with `#`.
- Sequence: the box holds `see #`, the caret moves to the start, the user types `x`. The picker opens, and `put` appends the picked tag at the end.

**Areas with nothing real found:**
- **Engine locking and races:** re-checked under the lock, the message file is removed when keeping fails, numbers are never reused, and a failed start uses no number.
- **Bringing comments in:** all or none, no duplicates, and it runs before `tell_the_asked` subscribes, so agents aren't flooded.
- **Start-up order:** `forget_unused` runs after every note kind and owner is registered.
- **Code snapshots:** the `handed` set covers the race between noting and keeping.
- **Notifications:** ids per (discussion, length), ended when it's no longer your move.
- **Approval gate:** sits under the plan lock, with no deadlock path into the engine.
- **Stopping point 3 fixes:** the asked-back case in `whose_move`, the single-step `ask_for_change`, and `start` not using up numbers all hold.

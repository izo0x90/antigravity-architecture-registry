Permissions review of 772aa54..0e24374 (meta-harness). I read the code only; nothing was built or run.

**1. `discussion_reply` lets any agent with `discuss` message other slots without `slots.message`. Medium.**
- Where: `src/mcp/sets/discussions.rs:227-239` with `src/discussing.rs:50-69`.
- `discuss` refuses to ask a slot other than the lead or the user unless the caller has `slots.message` (lines 215-221).
- `discussion_reply` has no such check. It only checks that the caller reaches the subject, and every subject that isn't a chat is reachable. Every reply produces a `DiscussionUpdated`, and `tell` then starts a turn in both the asked slot and the starter's slot (`slots.tell` → `start_turn`).
- Sequence: the lead (or the user) starts a discussion on `plan p` asking sub-slot B. Worker W, which has no `slots.message` or `slots.report`, calls `discussion_reply {id, text}`. B's session gets "Discussion dN on plan p: W said: …" as a new prompt.
- The same works for any discussion started by agent A and asked of the user: replying there prompts A.
- Effect: one agent can push prompts into another agent's session, which the capability model is meant to prevent.
- Fix: replying should need the caller to be the asked slot or the starter, or else need `slots.message`.

**2. Queue briefs are expanded with full reach, even though agents write them. Low-medium.**
- Where: `src/queue.rs:64`, `slots.expanded(ticket.brief, None)`.
- `None` means "the user's own prompt": every `#tag` expands, chats included. But `render_brief` (`crates/arch-service/src/queue.rs:101`) is built from the plan title, the component name and the contract, all of which agents set with `plan_edit` (`plan.edit`).
- This change made `tell` and `send` expand only as far as the receiving slot may read. This path was left open.
- Sequence: a worker puts `#notes` in a component's contract, where `notes` is a tag the user or lead put on a lead chat turn. When the user queues that component, the new worker's brief holds the lead's chat text. It can then copy it back into the plan with `plan_edit`.
- Fix: expand with the worker slot as the reader (`Some(slot)`).

**3. Slots are matched by name, and names are reused after a slot closes. Low-medium.**
- Where: `crates/harness-core/src/slots.rs` (`named` at 292, `next_name` at 363) and `crates/nah-discussions/src/lib.rs:445-448`.
- A discussion stores `asked` and `by` as slot names. The answer check is `d.asked == by`, and `tell` finds the slot with `slots.named(name)`.
- `close` removes the slot, so `next_name` hands out "sub 1" again.
- Sequence: the user asks sub-slot "sub 1" in d4, then closes it. The lead creates a new sub, which is named "sub 1".
  - The new slot is told every later user reply in d4. User text is expanded with full reach (`by_user`), so tagged chats are included.
  - The new slot may also `discussion_answer` d4.
- This is related to the open "asked of a closed slot" question, but here an unrelated new slot inherits the right to answer and receives the messages.

**4. Agents can be shown "the user" when it was another agent. Low.**
- Where: `crates/harness-core/src/slots.rs:334` refuses only the names "user" and "you". `by_to_agent` (`crates/harness-contracts/src/discussions.rs:156`) shows the real user as "the user".
- Sequence: a slot created with the name "the user" (the lead can do this with `slots.manage`) replies in a discussion. The other agents see "Discussion d3 …: the user said: …" in their prompt and in `discussion_read`. They can't tell it apart from the real user.
- Fix: reserve "the user" too, or show agents something unambiguous.

**5. `discuss ask=user` reaches the user's notifications without `user.notify`. Low.**
- Where: `src/mcp/sets/discussions.rs:219`, with `crates/nah-notify/src/rules.rs` `discussion_news`.
- Any caller with `discuss` can ask the user, and that opens a notification.
- The shipped roles all have `user.notify`, so this only matters for a project role in `.nah/access.toml` that grants `discuss` without `user.notify`.

**6. An agent can block a component's approval in a way only deleting fixes. Low.**
- Where: `crates/nah-discussions/src/lib.rs:463-473`.
- A sub-slot can `discuss` a question on `plan p component c`, asking the lead. While the lead doesn't answer, the user can't settle it ("isn't answered yet") and can't approve `c`. Their only way out is deleting the discussion.
- This touches the open "deleting discussions" question; I'm noting it because an agent can cause it.

**7. Plan files are still read in the old comment shape, so authorship can be faked. Low.**
- Where: `crates/arch-service/src/store.rs:192-201` and `src/plan_comments.rs:79-85`.
- Any plan file holding `comments` is held as "waiting" and imported at the next start. Its `Author::User {}` entries become messages by `user`, with whatever status the file says.
- An agent editing `.nah/plans/*.json` with its own file tools could therefore leave "the user said…" or "settled" history in the discussion record.
- Agents already write those files (approvals included), so this adds little. It does go against E.1's "nothing reads the old shape after".

**Areas where I found nothing:**
- **Routes:** all eight `/api/discussions*` routes take `Needs<…>` first. HTTP is user/home-only anyway (`principal_of`: cookie or bearer, otherwise anonymous). The old comment routes are gone.
- **Agent tools:** all five discussion tools declare a capability (`view.read` or `discuss`). Settling, reopening, making a change request and deleting are only reachable through user routes; the engine also checks `by == "user"`, and no slot can be named "user". No agent tool wraps a user-only action.
- **Reading:**
  - `Reach` now covers `ChatSession`, and follows a discussion that is on another discussion.
  - Code subjects go through `project_file`: canonicalised, inside the project, with hidden and ignored files (`.nah/`, `.env`) refused.
  - Chats are read only from this server's slots, so nothing from other servers is reachable.
  - Discussion ids use AUTOINCREMENT, so ids aren't reused and a discussion can't end up on itself in a loop through `Reach`.

**Outside the brief (a real bug at the seam between steps):** `mine` in `src/mcp/sets/discussions.rs:126` compares `Move::Agent(name)` with the caller's slot id. Ids and names only match for the lead. For sub-slots and workers, `discussions` never lists anything under "Waiting on you", and `discussion_read` never says "yours". The tests only cover the lead (`tests/discussions_tests.rs:51,75`).

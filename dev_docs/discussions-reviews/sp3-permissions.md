Permissions review of c7f8a98..d537e31, most severe first:

**1. `discuss` lets any agent message any slot, which bypasses `slots.message` and `slots.report`.** (src/discussing.rs:16-31 `asked`, :73-87 `tell_the_asked`; src/mcp/sets/discussions.rs:218-235; harness-core slots.rs `tell`)
- `asked()` accepts any slot id or name. `tell_the_asked` then starts a turn in that slot's running session with `slots.tell`, or holds the text until its next session.
- Workers have no `slots.message` and no `slots.report`, and sub-slots may only report to the lead. Today `slot_message` can only reach sub-slots, even for the lead.
- With `discuss`, a worker can do `discuss {on:"plan p", text:"…", ask:"worker-abcd1234"}` and then repeat `discussion_reply`. Each message starts a new turn in that other slot.
- Impact: agents can drive each other and spend each other's turns outside the role model. The plan's F.3 wants the asked slot told, but nothing ties who may be asked to `slots.message`/`slots.report`. Needs your decision.

**2. Tag names in discussion text leak chats past the read limits.** (harness-core slots.rs `tell` → `expanded` → nah-tags lib.rs:422-462)
- Every `#name` in what a slot is told is expanded to the full text of what that tag is on, with no Reach check.
- Sequence: the user tags a lead turn `#notes`. Worker A runs `discuss ask:<worker B> text:"see #notes"`. B's session receives the lead's chat text, which B's own `tag_read` and `discussion_read` would refuse.
- Before this change only the user, the lead's `slot_message` and sub-slot reports to the lead went through this expansion, and all of those senders could already read the chats.

**3. Any agent can answer any discussion, not just one asked of it.** (crates/nah-discussions/src/lib.rs:423-445; discussions.rs:237-247)
- `answer` never checks that `by` is the slot asked, though the tool description says "asked of you".
- Case one: the user asks the lead for a change (d1). A worker calls `discussion_answer d1` and names a file that is dirty anyway. d1 becomes Answered and the user is told "worker answered d1: settle or reopen". The lead is never told, because `news()` returns None for an answer (discussing.rs:44-48).
- Case two: an agent can "answer" its own question to the user (`asked == "user"`).

**4. An agent's question to the user can't be closed by the user, and on a plan component it blocks approval.**
- The `/answer` route was removed. `settle` refuses a question that is still Open (lib.rs:450-455). The cockpit offers only reply and change for an Open discussion, and reply leaves it Open (cockpit-ui discussion.rs:400-417). The cockpit offers no delete.
- Sequence: a sub-slot (no `plan.edit`) runs `discuss {on:"plan p component a", ask:"user", text:"?"}`. It stays in your inbox for good, and `approvals_ask` (startup.rs) blocks approving `a` until an agent "answers" it or someone calls DELETE through the API by hand.
- To confirm: in the existing test, after d2 (`ask:"user"`) try a reply then settle through the routes. Settle should return 400.

**5. The lead can settle and reopen as you through the cockpit.** (src/mcp/sets/cockpit.rs `cockpit_run`/`cockpit_click`; cockpit-ui discussion.rs:404-425)
- The new `discussion.settle`, `discussion.reopen` and `discussion.change` actions call the routes as you (`by = "user"`), and the lead has `view.drive`.
- Rule 5 says only the user settles. This widens the deferred finding #63, which the plan says must not be widened. It only applies while you let agents drive the cockpit.

**6. Authors are identified by slot name, and names aren't unique.** (discussing.rs:80; nah-discussions keeps `by` = `caller.label`; harness-core slots.rs:324-328)
- `create` refuses only the names "user" and "you". A sub-slot can be named "lead", or two slots can share a name; `asked()` even handles that case.
- Two effects:
  - What a same-named slot says is never told to the asked slot (`by == slot.name` is skipped).
  - The discussion record shows "lead answered" for something a sub-slot wrote.
- Low severity.

**Areas checked with nothing found:**
- **Routes:** every discussions route takes `Needs<…>` first. Reading needs `view.read`, start and reply need `discuss`, settle/reopen/change/delete need `plans.approve`. The `/answer` route is gone. No new socket.
- **Tool capabilities:** all five tools declare one (`discussions` and `discussion_read` need `view.read`; `discuss`, `discussion_reply` and `discussion_answer` need `discuss`). No tool settles or reopens, and the engine's `user_only` guards it.
- **Chat reach:** `Reach` covers `ChatSession`, `ChatTurn`, `ChatMessage` and discussions on discussions, and is applied in all four places a tool reads or starts one.
- **Files:** `file <path>` goes through `project_file`, which canonicalizes the path, keeps it under the project root, and excludes hidden and ignored files. So `.env`, `.nah/state` and paths outside the project are refused.
- **Notifications and search:** agents can't read notifications or the discussion text search.
- **Home relay:** it is the user, by design.

Touch and phone width: no permissions issues there.

## Review of 53cd6b5..c7f8a98: responsibility and coupling, most severe first

1. **Plan files are migrated inside the plan store, and old comments get imported again.** `crates/arch-service/src/store.rs:187` (`read`), `:201` (`move_waiting`), `:239` (`save`).
   - `FsPlans`, the plans' storage layer, now runs the discussions migration. It reads `comments` out of every plan read, forever, keeps a `waiting` map, calls the mover and rewrites files. That contradicts E.1 ("nothing reads the old shape after") and the no-shims rule.
   - **Trigger:** `plans/` is committed, but discussions live in uncommitted `state/`. Check out an older branch whose plan file still has `comments`. The watcher (`service.rs:597`, then `changed`, then `move_waiting`) brings them in again as new discussions, duplicating every earlier one, and rewrites the file, which dirties the old branch.
   - **Race:** `move_waiting` copies `waiting` and calls the mover without holding a lock across the call. If the watcher's `changed()` and start-up's `move_comments` run at the same moment, the same plan is imported twice.
   - **Severity:** high. The user sees duplicated threads, and approvals are blocked by the copies.

2. **Agents can't answer anything in this commit.** `src/mcp/sets/review.rs` removes `review_comments`, `answer_comment` and `reply_comment`, and adds no replacement. `discuss` is granted to lead, sub and worker (`access.toml`) but no tool uses it.
   - **Trigger:** the user opens a change request on a component. Only an answer can unblock it, so approval stays blocked until the user answers it themselves or deletes it.
   - **Severity:** high if this commit is run on its own (step 6 is meant to add the tools).

3. **The discussion engine knows about plans** (rule 2: "tags and discussions know no owner").
   - `crates/nah-discussions/src/lib.rs:550` `blocker(&PlanId, &ComponentId)` holds the approval policy and its wording ("answered: settle or reopen it"). The crate depends on `arch-graph` only for this.
   - Plan containment (section, relation and plan touching a component) is hard-coded in the wire contract, `harness-contracts/src/tags.rs:254` `Subject::touches`, not asked of the owner (`PlanParts`).
   - **Better split:** the plan asks the engine for discussions on a subject that aren't settled and formats its own refusal; containment is a `Resolve` question for the owner.
   - **Severity:** medium. Every new plan part means editing the contract and the engine.

4. **The plan's structure is worked out again in four places**, besides `touches`:
   - `review.rs:86` `marks`
   - `spec.rs:122` `section_of`
   - `tree/mod.rs:577` `TreeBuffer::things` (section → component → plan)
   - `src/plan_things.rs:183` `changed`

   They already disagree: `blocker` counts a discussion on the whole plan, but `section_of` and `marks` leave it out.
   - **Trigger:** a discussion on `Subject::Plan`. Approving any component is refused, citing a discussion the card and the graph badges don't show.
   - **Severity:** medium.

5. **The generic tree buffer special-cases plan items.** `tree/mod.rs:438` (the Enter hint), `:662` (`enter` returns `GoTo::Spec` for `PlanPart::Item`) and `:577` (plan containment in `things`).
   - What a row's Enter does, and what things a row is, belong to the `TreeSource`. Instead the buffer that hosts every source matches on one source's targets.
   - **Severity:** low-medium. Each new source with its own rows means editing the generic buffer.

6. **There is no desk-held plan, so it is fetched twice; discussions are kept in three copies.**
   - `PlanTree::ask` (`tree/plan.rs`) and the item card (`spec.rs:1667`) each call `net.arch_plan` and cache it themselves, each refetching on `ArchEvent::Changed`. After the shell's `plan.item_state`, the tree and an open item card can show different states until both requests return.
   - Discussions:
     - `Shared::discussions` (a `Held`).
     - A copy in `ReviewState.discussions`, which the shell fills (`app/views.rs:70`).
     - A third copy in `spec.comments` (`spec.rs:1299`).

     The plan's discussions should be read through `Reader<Discussions>`, and the plan should be one desk service.
   - **Severity:** medium. This is a parallel path beside `Held`/`Reader`.

7. **"Only the user" is decided by a string, beside the capability system.**
   - `user_only` (`lib.rs:173`) compares `by == "user"`, while the routes are gated by `Needs<can::Discuss>`, a capability every agent role now holds. Before, these routes needed `plans.approve`, which no agent role has.
   - Today HTTP callers can only be User, Home or Anonymous, so agents can't reach the routes; the risk is two policies for one rule that can drift. Step 6's MCP tools will pass a slot name as `by`; the only guard is the slot-name check in `harness-core/src/slots.rs:321`.
   - **What would confirm it:** whether any non-user principal will ever hold `discuss` over HTTP.
   - **Severity:** low-medium.

8. **The user is named two ways.**
   - Discussions sign the cockpit user as the literal `"user"` (`routes/discussions.rs`).
   - Approvals and `mark_reviewed` sign them by the name sent in the request (`arch.rs` `user(by)` → `Actor::User{name}`).
   - The cockpit hard-codes `"user"` again in `spec.rs:996` `said_by`, a third copy beside `nah_discussions::USER` and `slots.rs`.
   - **Effect:** the activity log names one person two ways, and nothing in contracts ties the cockpit's copy to the server's.
   - **Severity:** low.

9. **A change request on a review item needs no actual change.** `lib.rs:408`: `PlanParts::changed` returns `None` for `PlanItem` (`plan_things.rs:183`), so `answer` accepts any item named as "changed".
   - **Trigger:** an agent answers a change request on item `i3` naming `plan p item i3` without editing it. It is accepted, and the user is asked to settle it.
   - **Severity:** low-medium. Rule 5 isn't enforced for review lists, which are the main use.

10. **Minor:**
    - `lib.rs:288` `pub fn things()` exposes the engine's `Things` and nothing calls it.
    - `nah-state/src/discussions.rs:42` reads the engine's serialized shape (`["kept"]["file"]`), as tags do. Renaming `Stored::kept` would silently make `forget_unused` delete logs that live discussions still use.

**Clean:** the event stream (`DiscussionUpdated`/`Removed`, hub cache, cockpit `apply`), the module/route wiring and the shared things layer follow the existing tag mechanisms; I found no parallel system there. Item ops in `ops.rs` and the shell's item-state action (found through `view.things`, the interface) are also clean.

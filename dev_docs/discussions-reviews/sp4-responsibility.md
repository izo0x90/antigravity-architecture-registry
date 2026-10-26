## Responsibility and coupling review, 772aa54..0e24374

Ranked from most to least severe.

**1. Agents are identified by a reusable display name in one place and by slot id in another. High.**
- **Where:**
  - The engine and `discussing.rs` store `asked`/`by` as the slot's *name* (`discussing.rs:22`, `:56-59`; `whose_move` returns `Move::Agent(name)`).
  - The agent tools compare that name to the caller's *SlotId*: `src/mcp/sets/discussions.rs:125-126` (`me = caller.slot…to_string()`), and again at line 169.
- **What goes wrong:**
  - For every slot except the lead (the lead's id and name are both "lead"), the `discussions` tool never shows "Waiting on you". `discussion_read` says "sub 1's (asked)" to sub 1 itself.
  - The only test (`tests/discussions_tests.rs:51,75`) uses the lead, so it doesn't catch this.
  - Auto-names are reused (`SlotManager::next_name` takes the first free "sub N"). Close "sub 1" and start a new sub-slot: it is named "sub 1" again and inherits the old one's discussions. `tell` (`discussing.rs:56`) messages it, and only it can `discussion_answer` them (`nah-discussions/src/lib.rs:447`).
- **Fix direction:** key `asked`/`by` on SlotId and show the name.

**2. Who may be asked and who may read are decided in two separate places, so a discussion can get stuck. High-medium.**
- **Where:** asking is decided in `discussing::asked`, reading in `mcp::sets::reach::Reach`.
  - The route `routes/discussions.rs:235-238` and the `discuss` tool (`mcp/sets/discussions.rs:209-222`) never check that the slot asked can reach the subject.
- **Trigger:** the user (or the lead, which has `slots.message`) starts a discussion on `chat lead turn 3` and asks "sub 1".
  - sub 1 is told the text (`discussing.rs:64`).
  - Then `discussion_read` and `discussion_answer` refuse it (`mcp/sets/discussions.rs:165`, `:233`, NOT_YOURS).
  - The discussion stays "waiting on sub 1" for good, and holds a plan's approval if it's on a plan's part.

**3. The plan store keeps a shim for old comments, and moves them only at server start. Medium.**
- **Where:**
  - `FsPlans` keeps the old `comments` JSON in `Held.waiting` (`arch-service/src/store.rs`).
  - It writes it back into every `save`.
  - Approval checks `plans.comments_waiting` (`service.rs` ~896).
  - `move_comments` runs once (an AtomicBool), at start-up only.
- **Trigger:** a plan file that still has comments arrives while the server runs (a `git checkout` or pull). `changed()` holds them, and approvals stay blocked "until the server starts again".
- E.1 says comments move "as each plan is read". As built, the persistence layer carries a permanent second path, against the no-shims rule.

**4. The approval rule is optional and split across layers. Low-medium.**
- `ArchitectureService.discussed: OnceLock<Blocker>` fails open: an unset blocker means discussions never block approval.
- The rule and its wording live in start-up glue (`src/startup.rs:333-345`), not in the engine or arch-service.
- An embedding or test that doesn't call `approvals_ask` approves through open discussions.

**5. The MCP read policy is reused outside MCP, with a hand-written fallback that differs. Low.**
- `src/modules.rs:315-327` builds prompt expansion from `crate::mcp::sets::reach::Reach`.
- When the slot is gone, the fallback filters only `Chat*` subjects, so `#tag`s on a `Subject::Discussion` about a chat expand. `Reach` would refuse those.

**6. A second, unchecked copy of how every owner names its things. Low.**
- `mcp/sets/mod.rs:96-101` (`names_of_things`) hard-codes each owner's naming in one string, beside each owner's own `Resolve::named` parser.
- Nothing ties the two together; a change to an owner's naming leaves the tools' descriptions wrong.

**7. The cockpit holds the same discussions several times, and says whose move it is four ways. Low.**
- **Copies:** the desk has `Shared::discussions` (a `Held`/`Reader`). `ReviewState.discussions` re-holds a filtered copy (`review.rs:38`, `:59-69`). SpecState copies that again (`spec.rs:1056`), and so does PlanTree (`tree/plan.rs:149`). Spec and the plan tree don't read through the Reader.
- **Whose move:** it is turned into words and colours separately in:
  - `tree/plan.rs:38-44` ("on lead")
  - `discussion.rs:471-476` ("waiting on lead")
  - `app/notes.rs:75-91`
  - `conversation.rs:178-184`
- The literal "user" is also hard-coded in `harness-contracts/discussions.rs:139`, `conversation.rs:155,174` and `whose_move`, beside `nah_discussions::USER`.

**8. The shell special-cases one kind of target and passes state on a side field. Low.**
- `app/going.rs:161-166`: `land` hard-codes `Target::Discussion` → `BufferKind::Discussion` in the quick look, instead of a property of the buffer kind.
- `desk.discussions_here` passes state from a gutter click (`app/notes.rs:341`) to an action (`app/starting.rs:39`). It is cleared in `draw/panels.rs:92`.

**9. A view decides which things change with the architecture. Low.**
- `discussion.rs:269` lists Subject variants to decide when to re-read what a discussion is on; that is owner knowledge inside a view.
- A discussion on another discussion is never re-read when that one changes.

**10. Smaller items. Low.**
- `Discussions::find` (`nah-discussions/src/lib.rs:302`) has no caller: the plan's "discussions' text searched with FFF" isn't reachable from any route or tool.
- `PlanParts::code` (`plan_things.rs:29-33`) calls `code_tags::project_lines` directly instead of asking the things registry for the code's text.

**Checked, no problems found:**
- The engine/tags split over `nah-things`: one registry, and tags keep no naming of their own.
- The conversation component, used by the chat, the spec view, the discussion buffer and the palette.
- No keys or buttons outside the keymap in the new cockpit code.
- The discussion buffer reads through a `Reader`, and the shell names no buffer type except as in item 8.
- The notify rules for discussions.
- Steps 6-8's fixes appear to hold, apart from items 1 and 2, which cross steps.

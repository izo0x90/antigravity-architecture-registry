## Faithfulness review: 772aa54..53cd6b5 (Parts A and B)

Ranked, most severe first.

**1. A.1 "one registry, used by tags and discussions alike" can't hold the recorded decision on where discussions' text goes.** (nah-things/src/logs.rs:43-49, src/startup.rs `things()`)
- The decision says discussions' text lives in `state/discussions/`, "through the same log layer as the tags' logs".
- `Things::open` takes exactly one folder, which startup sets to `state/tagged`. File names are hard-coded to `tagged-N.log`, and the header the module documents is "`· tagged by`".
- So Part D has two ways out, and both break the plan. It can open a second `Things`: a second registry, every owner registered twice, a parallel mechanism. Or it can write discussion text into `tagged/`, which contradicts the decision.
- No user-visible bug today. It is a design gap in the layer that was built specifically to serve discussions.

**2. A.5 is half done and the gap isn't mentioned.** (crates/arch-service/src/store.rs:102-130; nah-state/src/lib.rs:89-123)
- The plan says the cockpit's layout moves from `ui/state.json` into `state/ui.json`, once. No code does this. The code keeps the layout in the database (`ui_state` table) and reads no `ui/` folder at all, so the plan's premise looks wrong. The plan says such cases are asked about first; this one wasn't.
- This project still has `.nah/ui/state.json`, and nothing removes it.
- ".nah/.gitignore written for it" didn't happen. `ensure_nah_gitignore` only appends missing lines, so this repo's file still lists `ui/`, `index.sqlite` and `terminal-hooks`.
- The commit message mentions only the hooks move.
- Minor: the "once" hooks cleanup (startup.rs:271) actually runs on every start.

**3. B.1/B.2: `PlanItem` is declared but not owned, and the commit says it is.** (src/plan_things.rs:110, 133, 139-167, 180)
- `owns` claims `PlanItem`, and `Subject::name` gives it a name ("plan v item x").
- But `named` rejects those words; its error lists only component, section and relation, even though its own doc at line 138 says "… item <id>". `now` returns None and `there` returns false.
- Result: the one name it has can't be named back, which breaks #25. A `PlanItem` sent as JSON to `/api/tags` is stored and always reads "gone".
- The commit message says items are owned and named back. Leaving this for Part C is reasonable, but it should be said.
- Related: going to an item gives "Not a part of a card." (cockpit-ui/src/spec.rs:1413), and there's no plan-tree row target. Both are C.3's to build.

**4. #16 is only partly closed.** (harness-acp/src/state.rs:410-413)
- Two of your messages in a row are told apart only when the agent sends message ids; the fallback `_ => true` merges them.
- Trigger: with an agent that sends no ids, cancel a prompt before any reply, send the next one, then reload. The two become one turn, every later turn number shifts, and tags point at the wrong turn.
- To confirm: check whether the ACP adapter in use sets `messageId` on replayed user chunks.

**5. A.3 "tags behave exactly as before": `tag_find` now matches differently.**
- Before, it used `rank_text`: every word, in any order, with operators. Now it matches the whole phrase as one substring (nah-things lib.rs:162 `has`, and FFF `containing`).
- Example: "flaky build" no longer finds "build … flaky". The tool's description wasn't changed to say so.
- The same thing reads differently depending on where it's kept. Lines from the logs come back trimmed (arch-search/src/files.rs:167 `trim_whitespace: true`); lines still at their owner don't.
- FFF's `partial` is ignored (nah-things lib.rs:468). If the 2-second budget cuts a search short, the agent is told "Nothing tagged says X."

**6. #18 and #20 are fixed only for new tags.**
- Tags stored before this change have no `command`. At the first `refresh`, `Things::renamed` (lib.rs:265) rewrites their `what` without "(of command N)", even while the shell is still running. That is exactly the loss #20 describes, now applied to every existing tag.
- Component tags stored before this change have `was: None`, so a deleted definition still reads back as nothing (#18).

**7. The command-tagging delay is moved, not removed (#34; #33 still open).** (src/terminals.rs:534-541)
- `follow` now builds a running command's output so far and writes all of it to disk while holding the single `followed` mutex.
- Every terminal's output delivery needs that mutex (`tell_followed`, line 231). Tagging a running command with a large scrollback stalls output delivery for every followed command, in any terminal.

**8. Rule 2 leak.** (nah-things lib.rs:340)
- The layer that "knows no kind of note" writes "tag_read's `full` gives them" into folded text. Discussion reads will inherit that wording.

**9. Built without being asked:**
- Steering now splits the live turn (session.rs:215, `steered`): an added message completes the running turn and starts a newly numbered one. That's a visible chat change chosen to close #16; the plan doesn't describe it.
- `Palette::drawn` is a production API used only by a test (app/tests/tags.rs:400). It replaces the test's read of an internal id with new non-test code.

**10. #32 is partly closed.** `Logs::count` (logs.rs:154) still reads a whole log on its first count after each server start; the cache lives in memory only.

**Checked, nothing real found:**
- #1, #5, #7, #19, #21, #22 (end-race handling), #4 (real restart test), #29/#50 (one fold), #31, #40, #51-53, #60.
- B.1/B.2 for plan, component, section and relation: names, `now`, `there` and named-back match.
- B.3 spec-card and graph go-tos.
- A.2 one path through `app/notes.rs`.
- Rule 1 (`CommentAnchor` still exists) belongs to Part E, so it isn't counted here.

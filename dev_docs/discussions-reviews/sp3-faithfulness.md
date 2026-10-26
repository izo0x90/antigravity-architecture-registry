Faithfulness review of c7f8a98..d537e31 against the discussions plan, Parts F and G. Ranked, most severe first.

1. **High: an agent's question to the user can't be closed.** `discuss` takes `ask: "user"` (src/discussing.rs:16-31), and `whose_move` makes it the user's move (harness-contracts/src/discussions.rs:139).
   - The cockpit has no way to answer it: the `/answer` route was removed (src/routes/discussions.rs).
   - A reply leaves it Open. `settle` refuses an Open question (nah-discussions/src/lib.rs:450-455).
   - Result: it stays "waiting on you" in the inbox for good. Each reply re-raises the notification with new detail (nah-notify/src/rules.rs:157-173).
   - The asking agent is never told what the user said, because `tell_the_asked` skips `asked == user` (src/discussing.rs:72).
   - The only exit is delete. Agents asking the user is not in F.3 ("goes to the lead unless it names an agent slot"), so this is unplanned and half built.

2. **Medium-high: `discuss` makes a false promise.** Its reply says "you're told when something's said in it" (src/mcp/sets/discussions.rs, `discuss` arm). Only the slot asked is told (src/discussing.rs:73-85). Example: a sub-slot asks the lead; the lead's replies and answer never reach the sub-slot.

3. **Medium: the user can't pick who is asked.** The decision says "the lead, unless you pick another agent slot when you start a discussion". `NetClient::discussion_start` sends no `asked` (cockpit-ui/src/net.rs:874-876), and the G.3 flow has no step for it (app/starting.rs). The route supports it; the UI doesn't.

4. **Medium: `#` does nothing in the first message (decision #10).** `discuss.write` is labelled "What it asks (# names a tag)" (app/starting.rs:39). The palette's Writing stage ignores `typed.names_tag` (palette.rs:234-238), so typing `#` there opens nothing. The tag-note box (#27) has the same gap.

5. **Medium: on a plan's graph, cards don't carry plan discussions (#14, G.3 "in any view").** `GraphState::drawn_at` matches only `Subject::Component` (graph/select.rs:606-614), and `things()` returns `Component` even when the graph shows a plan.
   - Discussions on `PlanComponent` (the ones that hold approval) get no mark on cards.
   - `C` on a plan card starts a discussion on the code component, which doesn't hold approval.
   - For a component not built yet, the server refuses with "it isn't there" (nah-things/src/lib.rs:422; code_tags.rs `there`).

6. **Medium (touch): the gutter is too small for a finger.**
   - The mark's hit area is `gutter_mark = 11` pt; a tag bar's is about 4 pt wide (config/default.toml:416-418; app/notes.rs).
   - The phone test taps the mark's exact centre.
   - With several discussions on one thing, a click opens only the most urgent (notes.rs, `first.id`). The others are named on hover or long press but can't be opened from the mark.

7. **Medium: an item card from another plan can't show its contents.** The item card now reads only the desk's review plan; before, it fetched its own. Going to a plan item of a plan the desk isn't showing (for example Enter on "what it's on" in a discussion opened from the inbox) shows "it isn't the plan shown now" (spec.rs, `share`, about line 1058).

8. **Medium-low: multi-line selection (#13) is pointer-drag only.**
   - It works only by dragging down the line numbers (code.rs: `Sense::click_and_drag` on the numbers, about lines 789-870).
   - There is no keyboard way to select lines.
   - On touch, a swipe that starts on the numbers selects lines instead of scrolling.

9. **Low-medium: G.2's top block is thinner than planned.** The plan asks for "code with its lines highlighted … saying when it changed since". `draw_on` prints plain monospace text with no highlighting or surrounding code (discussion.rs:132-153). The status uses `Held::words`, which says "as it was when tagged: changed or gone since": wrong word for a discussion, and no "when" (harness-contracts/src/tags.rs:387-393).

10. **Low-medium: a cancelled start leaves a stale action.** `desk.discussing` is never cleared when you Escape out of the picker (app/starting.rs:38, 62-66). "Question on X" stays in the palette, and running it later starts a discussion on the old thing.

11. **Low-medium: `discussion_answer` doesn't check who is answering.** Any slot with `discuss` (a sub-slot, a worker) can answer a discussion asked of the lead, or of the user, and move it to Answered (nah-discussions `answer` checks only status and what changed; src/mcp/sets/discussions.rs). The tool's own description says "asked of you".

12. **Low: making a discussion a change request misleads the slot asked.** On an answered question it is told "reopened it … discussion_answer it", without "naming what you changed": the news is built before `kind_to_change` runs (src/discussing.rs `news`; nah-discussions:470-490). On an open question it is told nothing at all.

13. **Low: #36 is only half done.** The gutter still walks every tag's things and every discussion, calling `drawn_at` on each, every frame (app/notes.rs `draw_notes`). The cache only saves rebuilding the lists.

14. **Low: #44 has a second date format.** The cockpit writes times as `2026-10-20 22:11` (conversation.rs:24-27); the logs and agent tools use RFC 3339.

15. **Low: G.7 test gaps.** No test starts a discussion from a plan item: it starts from a spec section (app/tests/discussing.rs:13). Nothing tests #10 in the palette's writing box.

16. **Low: stale docs.**
    - net.rs:883 still lists `answer` as an action.
    - The routes header says the user can ask "no one but you", but `asked()` refuses the user asking the user.

**Built but not asked for:**
- Agents asking the user (item 1).
- Settle, reopen, change and delete now need `plans.approve`.
- The `/answer` route is removed.
- `Things::pass` (a per-thread cache) and the chats' turn-number map.
- A change to how the logs search counts lines per file.
- The plan tree and item card now read only the desk's review plan.

**F.4:** no agent file or pack names the old tools, and none names the new ones either; the agent files' instructions never mention discussions.

**Phone width (unconfirmed):** the discussion buffer reserves a fixed `box_height` (discussion.rs:182). If the box's label or the error note wraps at about 400 pt, the typing box may be pushed out of the quick look. A 390-pt frame with the error note showing would confirm it.

**No problems found:**
- F.2 reach and capabilities.
- F.5 #26.
- The G.1 chat on one component.
- The spec composer is gone.
- G.5 inbox.
- Keys: `C`, `Space k …`.
- #24, #38, #42, #46, #47.

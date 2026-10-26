I reviewed c7f8a98..d537e31 for separation of responsibility and coupling, reading only. Most severe first:

1. **A discussion an agent asks of the user can never be closed (high).**
   - `src/mcp/sets/discussions.rs:98` lets `discuss` take `ask: "user"`. `whose_move` (`crates/harness-contracts/src/discussions.rs:139`) then says it's the user's move.
   - The user's answer route is gone (`src/routes/discussions.rs`, `/answer` removed), and `DiscussionView::actions` (`crates/cockpit-ui/src/discussion.rs:389-417`) offers no answer.
   - The user can only reply, and a reply leaves it Open. Settle refuses it ("isn't answered yet"); reopen refuses it ("open already").
   - Result: it sits in the inbox and notifications as "waiting on you" forever. The asking agent is never told the user replied either: `tell_the_asked` skips it at `src/discussing.rs:72` (`if d.asked == USER { continue }`).

2. **The agent that starts a discussion is never told what's said in it (high).** `tell_the_asked` (`src/discussing.rs:62-100`) only tells `d.asked`. Yet `discuss` replies "you're told when something's said in it" (`discussions.rs:234`).
   - Sequence: a sub-slot runs `discuss` on code, asking the lead. The lead answers. The sub-slot gets nothing and has to poll `discussions`.

3. **An agent's question back stalls the discussion (medium-high).** `discussion_reply` is described as the way "to ask back" (`discussions.rs:109`), but a reply changes neither status nor `asked`. `whose_move` keeps it on the agent.
   - So `discussion_news` (`crates/nah-notify/src/rules.rs:159`) makes no notification, and the inbox, gutter colour and "waiting on you" never show it. The lead waits on the user while the cockpit says it's the lead's move.
   - "Whose move" is decided only from status plus `asked`; nothing records who spoke last.

4. **No one checks who may answer (medium).** `Discussions::answer` (`crates/nah-discussions/src/lib.rs:423`) never checks that `by` is the slot asked, and neither does the tool.
   - Any slot with `discuss` (sub-slots, workers) can answer a discussion asked of the lead, or one asked of the user, turning it Answered. The rule that the slot asked answers isn't enforced in the engine.

5. **`#` does nothing while writing a discussion's first message (medium-low).** The palette's `Stage::Writing` (`crates/cockpit-ui/src/palette.rs:234-238`) draws the shared `TypingBox` but ignores `typed.names_tag`.
   - The `discuss.write` hint promises "(# names a tag)" (`app/starting.rs:39`). The tag-note edit is the same.
   - The second user of the "one conversation component" drops half its contract. `tag.insert` only targets a view's `typed_in`, not the palette.

6. **One record names slots two ways, and the who-wording is copied in many places (medium-low).**
   - `asked` holds a slot id (`sub-1a2b3c4d`, `discussing.rs:23-28`), while `by` and `said.by` hold the slot's name (`caller.label`).
   - So the cockpit shows "waiting on sub-1a2b3c4d" (`discussion.rs:456`, `tree/plan.rs:41`) next to "reviewer said".
   - `tell_the_asked` filters out the asked slot's own messages by name (`discussing.rs:78`). With two slots sharing a name (which `asked` allows), one's message isn't told to the other.
   - The literals `"user"` and `"lead"` are re-declared or used bare in contracts (`discussions.rs:139`), `nah-discussions` (`USER`, and a second `LEAD`), `nah-notify/rules.rs:161` and `conversation.rs:17,69,87`.
   - "Who said it" is worded separately in `discussing.rs:35`, `mcp/sets/discussions.rs:39`, `mcp/sets/tags.rs:81`, `rules.rs:161` and `conversation.rs:15`. Finding #45 is closed only inside the cockpit.

7. **Discussion policy borrows plan capabilities (low).** settle, reopen, change and delete require `can::PlansApprove` (`routes/discussions.rs`). So settling a discussion on a terminal command depends on a plan permission.
   - A second user-only check (`user_only`, `by == USER`) sits in the engine. Since the route always passes `USER`, anything holding `plans.approve` acts as the user.
   - The plan's component-approval blocker text now lives in startup glue (`src/startup.rs:583-594`) rather than with plans.

8. **The shell hard-codes where discussions open (low).** `land` (`app/going.rs:159-166`) names `Target::Discussion` and `BufferKind::Discussion` and calls `view_mut(...).show_target` directly.
   - That path skips `after_jump(from)`, so Back doesn't return after opening one from a mark, the inbox or the plan tree.

9. **Two lookups of a slot by id or name (low).** `discussing::asked` (`src/discussing.rs:16-31`) repeats `Slots::find` (`src/mcp/sets/slots.rs:120-131`) with different error wording.

10. **The speech bubble skips the theme's solid-icon font (low).** `fill::CHAT_CIRCLE` is drawn with `FontId::proportional` (`app/notes.rs:292`) and `.size()` (`hosted.rs:139`) instead of `theme::solid()`, the way `tag_mark` does it.
    - I couldn't confirm what this renders: the egui_phosphor source wasn't on disk. If phosphor's fill and regular glyphs share code points, the "solid" bubble draws as an outline. The tests match only the glyph string, so they wouldn't catch it.

11. **A cancelled start leaves stale state (low).** `desk.discussing` is cleared only when a message is sent (`app/starting.rs:66`). Esc out of the writing box leaves "Question on X" listed in the palette with no `when`; running it later starts a discussion on the old thing.

12. **The cockpit can't choose which slot is asked (gap).** This was a decided plan item. `net.discussion_start` (`net.rs:874`) sends no `asked`, though the server takes one. `net.rs:883`'s doc still lists `answer`.

**Touch and phone width (about 400pt):**
- The discussion mark is `gutter_mark = 11.0` points square. That's far below a finger target, and tag bars beside it are 2.5pt.
- A mark covering several discussions opens only the most urgent. The others can only be listed (hover or long press), not opened, from the mark.
- Line selection now drags on the line-number labels (`code.rs:789`, `Sense::click_and_drag`). A vertical finger drag there likely selects lines instead of scrolling. To confirm: a touch drag over the numbers in the code view at phone width.
- Picking the next thing at the cursor when starting a discussion relies on `Tab`/cycle; I didn't verify a touch path for it.

**Areas with no problem found:**
- The cockpit's `Discussions` desk service and its `Reader`/`Held` use.
- `Reach` (shared by the tags and discussions tools, one place).
- The chat's move onto `conversation::message`/`TypingBox`.
- Shared `ItemBody::said`.
- `HeldReport`/`tell` generalised in `harness-core`.

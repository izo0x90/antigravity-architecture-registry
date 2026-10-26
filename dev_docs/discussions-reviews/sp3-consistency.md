Review of c7f8a98..d537e31 for consistency with existing patterns, most severe first. I only read code; nothing was built or run.

1. **`#` does nothing in the palette's writing box** (`palette.rs`, the `Stage::Writing` arm). It calls `self.writing.draw(...)` and checks only `typed.sent`. `typed.names_tag` is dropped. But `discuss.write` tells the user "What it asks (# names a tag)" (`app/starting.rs:376`), and the tag note now uses the same box.
   - Trigger: press `C`, pick a question, type `see #`. No tag picker opens.
   - The plan's #10 and #27 say every typing box behaves the same way. The chat and discussion boxes route `names_tag` to `Ask::NameTag`; the palette doesn't.
   - Severity: medium. The on-screen hint is wrong, and the plan rule isn't met.

2. **An abandoned start leaves a live action pointing at the old thing** (`app/starting.rs:375`, `desk.discussing`). The state is cleared only by `DiscussWrite`.
   - Trigger: press `C` on line 12, pick a kind, press Escape. "Question on line 12 of …" stays in the palette's action list from then on. Running it later, from anywhere, starts a discussion on line 12.
   - `tag_editing` (`tag.edit.picked`) has the same problem.
   - The existing pattern that avoids this is `tag_level`, which only counts within one palette opening (`palette.opening()`). These should do the same.
   - Severity: medium (a discussion lands on the wrong thing).

3. **Filled icons drawn without the solid-icon font** (`app/notes.rs:292`, `hosted.rs:139`). `egui_phosphor::fill::CHAT_CIRCLE` is drawn with `FontId::proportional` / `.size()`. But only the `icons-solid` family has the fill font (`theme::font_definitions`). `theme::solid()` and `tag_mark()` exist for exactly this.
   - Result: the gutter and header marks draw as the outline glyph, or a missing-glyph box, depending on whether phosphor shares codepoints across weights. The tests only compare the text string, so they pass either way. A screenshot would confirm.
   - Severity: medium (visual; it's the plan's "solid bubble").

4. **Slot ids on screen where names are used everywhere else.** `d.asked` is a slot id (`discussing::asked` returns `slot.id`, e.g. `sub-3fa2b1c9`), and it is shown raw in:
   - `conversation::heading` ("asks sub-3fa2b1c9")
   - `DiscussionView::status` ("waiting on sub-…")
   - `tree/plan.rs` `whose` ("on sub-…")

   Messages in the same view show the slot's name (`said.by` = `caller.label`), and the chat names slots via `slot().name`. Trigger: the lead asks a sub-slot with the `discuss` tool. Severity: medium-low.

5. **Key letter hardcoded in a message** (`spec.rs:1171`): `Step::Nothing("C asks about it, …")`. Keys are supposed to come only from the keymap, and nowhere else in the cockpit hardcodes a key in a message. If `discuss.start` is rebound, the message is wrong. Severity: low.

6. **The cockpit can't choose which agent slot answers.** The decided plan says the lead answers "unless you pick another agent slot when you start a discussion". The route accepts `asked`, but `net.discussion_start` never sends it and the `C` flow has no slot step. Severity: medium (a decided feature is missing, not just inconsistent).

7. **One idea written several ways:**
   - **Who wrote something** (finding #45 asks for one way). It's done four ways on the server, and they already differ on an empty author (`"?"` vs `""`):
     - `mcp/sets/tags.rs:79` `who`
     - `mcp/sets/discussions.rs:39` `who`
     - `discussing.rs:34` `said_by`
     - the closure in `nah-notify/src/rules.rs` `discussion_news`
   - **Whose move it is, in words**, three ways that read differently: "on lead" (`tree/plan.rs` `whose`), "waiting on lead" (`discussion.rs` `status`), and `notes.rs` `discussions_said`.
   - **Status and said-kind words** are written in both `conversation.rs` (`standing`, `said`) and `mcp/sets/discussions.rs` (`status`, the `how` match). `DiscussionKind::words()` already lives in contracts; `DiscussionStatus` and `SaidKind` have no equivalent.
   - **`capitalized`** is copied verbatim in `spec.rs:873` and `app/starting.rs:99`.
   - Severity: low, but these are already drifting.

8. **The naming grammar is hand-written twice in tool descriptions**: `mcp/sets/tags.rs` (the `subject` description) and `mcp/sets/discussions.rs` (`thing`). They already differ: only the tags one mentions `chat lead@<id's start> …` and `terminal_read`. The plan wants naming as one shared layer, which `nah-things` could own. Severity: low.

9. **Kind choice parsed and labelled by hand** (`app/starting.rs`):
   - `kind == "change_request"` is a string compare, and any other value silently becomes a question. `DiscussionKind` already has serde snake_case.
   - The choice is labelled "A change asked for", while every other place says "change request" (`DiscussionKind::words`, "Make it a change request", the heading).
   - Severity: low.

10. **The two typing boxes don't have the same controls.** The discussion buffer adds `discussion.cancel` (a "Stop" button, plus Escape in `default.toml`); the chat's box has none.
    - In Insert mode the global `back` key already takes Escape (`keys.rs`, the `InputMode::Insert` arm), so the Escape binding can never run the action.
    - Severity: low. Rule 4 (one conversation component) is only half met.

11. **The thing's text isn't drawn like code** (`discussion.rs` `draw_on`). It's one plain monospace label. The plan (G.2) says "code with its lines highlighted", and `CodeView::coloured` already does syntax-coloured lines; it isn't reused. Severity: low-medium.

12. **Discussions get their own branch in `land`** (`app/going.rs:159`), while the other quick-look openers (`talk`, `pop_terminal`) live in `look.rs`.
    - Trigger: the spec card is open in the quick look and you press Enter on one of its discussions. The discussion replaces the card, and closing it returns to the panel, not the card.
    - Severity: low.

**Touch and phone width (about 400pt):**
- **Gutter hit areas are too small for a finger.**
  - The discussion mark is 11pt (`gutter_mark`) and a tap opens it.
  - Tag bars are 2.5pt wide; long-press only finds them if it lands within about 4pt (`mark.contains(p)` in `draw/host.rs:262`).
  - Finger targets elsewhere in the cockpit are far larger.
  - Severity: medium on touch.
- **Card marks overlap the corner** (graph, `in_place`). They're centred on the card's top-right corner, so the bubble and the tag dots sit over the card's border and any corner badge. A narrow screenshot would confirm.
- **The palette's writing stage has no button to send.** It only sends on the keymap's Enter. The discussion buffer gets a host-drawn Send button; the palette doesn't. Whether a phone keyboard's return reaches egui as `Key::Enter` in a multi-line box needs checking on a device. If it doesn't, a discussion can't be started from a phone.

Nothing wrong found in: the reuse of `nav::list_moves` and `Move::Enter.offer`, the gutter sizes coming from the theme, the `Reach` extraction (the old copy in `tags.rs` is gone), `SlotManager::tell` replacing `report`'s delivery path, or `ItemBody::said` replacing `text_of`.

# Re-review of 225bf33 (sign-in flow), consistency

Three problems, most severe first. Line numbers are at 225bf33.

## 1. Giving the box back takes the draft read when the desk opened, not the one that was written

`crates/cockpit-ui/src/chat.rs:265-267` (`tend_sign_in_box`, giving back: `unsent_for = None`), `crates/cockpit-ui/src/chat/nav.rs:52-66` (`take_unsent`), `crates/cockpit-ui/src/drafts.rs:50-52` (`Drafts::saved`), `README.md:300`.

**The draft pattern.** `Drafts::saved` returns `loaded`. That is the map read from the server once, when the desk opens. It is set only in `tick` on `Reply::Loaded` (`drafts.rs:76`) and never updated after that. What the chat reports afterwards (`Ask::Draft` → `changed`) goes into `pending` and `sent` and never into `loaded`. The module doc says so: "Read once when the desk opens; a chat view takes its slot's when it's empty". `take_unsent` was built for that one first read. It isn't a way to get back the current draft.

**What the commit does.** To give the box back, it clears `unsent_for` so that `take_unsent` runs again "and the saved draft [is] taken again". The draft it takes is the one from desk-open time. Sequence:

1. The desk opens. The lead has no saved draft, so `loaded` is `{}`.
2. The user writes "half a message" in the lead's box. It is reported and saved to the server (`sent`). `loaded` is still `{}`.
3. A sign-in starts. `tend_sign_in_box` reports (no change), empties the box and says "what you were writing is kept and comes back after".
4. The sign-in ends. The box is emptied and `unsent_for = None`. `take_unsent` reads `saved(lead)` = `Some(None)`, so nothing is restored and `unsent_said` is empty.
5. The box stays empty. The next thing typed is reported as the lead's draft and overwrites "half a message" on the server.

If the desk-open read did have a draft for the slot, step 4 puts that older text back instead.

A second way to lose it: when the drafts were never read (the read failed, `drafts.rs:79-81`; it is asked again only after a reconnect), `unsent_for` is never set. `report_unsent` then saves nothing, and step 3 empties a box that nothing kept, while the note says it was kept.

The app test `a_sign_in_has_the_typing_box_while_it_waits_enter_and_keys_included` misses this. It seeds the draft with `arrived(...)` before anything is typed, so the draft is in `loaded`. Both the note and README.md:300 ("what was being written to the agent is kept, and comes back after") promise something the code only does for a draft the desk read at start-up.

## 2. "Finish signing in" stays on the top line while a paste is checked, and then says to paste

`crates/cockpit-ui/src/chat/actions.rs:254-255` (gate: `sign_in_box().is_some()` only) and `actions.rs:426-432` (`CompleteLogin`).

The commit gave `Send` a note for a paste being checked (`send_typed`, `chat.rs:282`). `auth.complete`'s other branch doesn't check for it. Sequence:

1. Paste the code and press Enter. The paste is taken out of the box and is now being checked, and the banner says "Checking what you pasted…".
2. The top-line "Finish signing in" button (or `Space a c`) is still offered.
3. Press it. The box is empty, so you're put in the box with the note "Paste what the page gave back here, and send it." This contradicts the banner.
4. Paste again and press Enter. Only now are you told it's being checked.

The view's existing pattern is that a control that can't do its job isn't offered, or that it says why on the status line. Before the review round, the header button was hidden while a paste was checked. The empty-box note is also generic ("what the page gave back"), while the box's own hint for the same moment names what's expected (`render.rs`, `waiting.expects`). The status line and the hint describe one step in two different ways.

## 3. Who has the box is decided in two places, and they disagree for a frame

`crates/cockpit-ui/src/chat.rs:254-270` (`sign_in_has_box`, set only in `render`) versus `chat.rs:276-292` (`send_typed`), `actions.rs:218` (`to_agent`) and `actions.rs:254` (`auth.complete`), which all ask `sign_in_box()`.

The drafts follow the flag. Sending, the actions offered and the hint follow the predicate. The flag changes only in `Buffer::render`. Within a frame, `update_lifecycle` takes events and then runs `handle_keys` (`app/lifecycle.rs:83`) before any panel is drawn. So in the frame where a sign-in ends (an `InstanceUpdated` saying it was finished on another device, or `ran_out` turning true on the clock), the predicate already says "the agent's box" while the box still holds what was pasted for the sign-in. Sequence:

1. A code is pasted but not sent.
2. A turn is running with steering.
3. The sign-in ends.
4. In that same frame, the last key of `Space s a` arrives. `chat.inject` is offered because `to_agent` is now true and the box isn't empty.
5. `AddToTurn` → `send_typed(true)` → `(None, _)`, and the pasted code goes into the agent's turn.

The window is one frame, so this is narrow. It is still the same kind of leak the commit set out to close: "Every way of sending goes through one place" is true, but that one place doesn't ask the same owner as the draft code. Having `send_typed` and `to_agent` read `sign_in_has_box`, or settling ownership before keys are handled, would give one answer.

## Review of c7f8a98..d537e31: discussions (most severe first)

**1. A discussion notifies the user only once, ever (high).**
- Where: `harness-contracts/src/notifications.rs` (`NotificationId::discussion` is always `disc:{id}`), `nah-notify/src/rules.rs` `discussion_news`, and `nah-notify/src/hub.rs:224-226`.
- Cause: `Change::Open` is dropped when a notification with that id exists in any state, and nothing ever deletes notifications.
- Sequence: the user asks the lead in d1. The lead answers and the inbox shows "lead answered d1". The user reopens it, which ends the notification (Done). The lead answers again. `derive` emits Open(`disc:d1`), the hub finds the Done row and drops it.
- Result: the second answer never reaches the inbox (G.5 is broken). The title/detail "same one" check in `discussion_news` is dead code for the same reason.
- The test covers `derive` only, not the hub.

**2. An agent's question to the user is a dead end, and whoever starts a discussion is never told what's said in it (high).**
- The cockpit's answer route was removed (`routes/discussions.rs`).
- The `DiscussionView` offers Reply only: Reply keeps it Open, Settle is unavailable until it is Answered, Reopen is unavailable while it is Open. The engine's `settle` also rejects an open question.
- `tell_the_asked` skips `asked == "user"` (`discussing.rs`, the `continue` in the event loop) and only ever tells the slot asked, never the starter.
- Sequence: lead calls `discuss on "plan p" ask "user" text "Complete?"`. The inbox says "lead asks you". The user replies "Yes". It stays Open, it is still the user's move, the notification never ends, and the lead never hears.
- Delete is the only way out. Even with no dead end, the starter is not told: when the lead asks sub-1, sub-1's answer is never sent to the lead (`news` returns None for an Answer). Yet `discuss` replies "you're told when something's said in it".

**3. Any agent can answer a discussion asked of someone else (medium).**
- Where: `mcp/sets/discussions.rs` (the `discussion_reply`/`discussion_answer` branch) and `nah-discussions` `answer`. Neither checks the caller against `asked`.
- Sequence: a worker calls `discussion_answer d1` on the user's change request asked of the lead. It becomes Answered and the user is asked to settle the worker's answer.
- An agent can also "answer" its own question to the user.

**4. News to the agent asked can be lost (medium).**
- Where: the `tell_the_asked` loop in `discussing.rs`.
- Cause: the loop awaits `slots.tell` (`expanded` plus `start_turn`) one event at a time. The hub holds 4096 events and is shared with streaming session events and terminal output.
- On `Lagged`, `seen` is reset to now, so everything said in the gap is never told (the code comment admits this).
- Restarts: a crash between `keep()` and the tell has the same effect, because `seen` starts at the current lengths after a restart.
- Result: the lead never learns of the user's change request, and the discussion waits silently.

**5. `ask_for_change` misinforms the agent and isn't atomic (medium-low).**
- Where: `nah-discussions` `ask_for_change`.
- On an open question, only the kind changes. `said.len` is unchanged, so the slot asked is never told it is now a change request.
- On an answered one, the reopen is published while the kind is still Question. The news says "discussion_answer it" without "naming what you changed", and the agent's answer without `changed` is then rejected.
- If the agent answers between the two steps, `kind_to_change` fails with "changed meanwhile" after the reopen was already stored. A retry adds a second "why".

**6. A discussion asked of a slot that closes is stuck and blocks approval (medium).**
- It stays `Move::Agent(sub-3)`, `slots.get` returns None so nothing is told, and nothing lets the user ask another slot.
- If it is on a plan component, `unsettled` blocks the approval. The user can't settle an open discussion, so the only way out is to delete it.

**7. The cockpit can't pick which slot is asked (medium-low).**
- `starting.rs` (`DiscussWrite`) and `net.rs` `discussion_start` never send `asked`.
- This misses the decided rule "the lead, unless you pick another agent slot".

**8. Typed text is lost when sending fails (low-medium).**
- Where: `discussion.rs` `send` and `starting.rs` (`discussing.take()`).
- `writing.take()` and the reset of `purpose` to Reply happen before the server answers.
- Sequence: the user writes a reopen reason while the agent re-answers. The request gets "is open already" and the reason is gone.

**9. `#` doesn't open the tag picker when writing a discussion's first message (low).**
- In `palette.rs`, the Writing stage ignores `typed.names_tag`.
- `discuss.write` still says "# names a tag", so #10 is not met here.

**10. "As it is now" can stay stuck on "Reading what it says…" (low).**
- Where: `discussion.rs` `tick`.
- Sequence: d1 is open in the quick look and the user moves to d2 within a round trip. d1's late reply overwrites `on_now`, and `asked` already matches d2, so it is never fetched again.

**11. A stale start action stays in the palette (low).**
- `desk.discussing` isn't cleared when the user presses Esc in the writing step.
- "Question on X" stays in the palette, and running it later starts a discussion on the old thing.

## Rules checked
- **Rules 1–3 (one pointer, shared layer, one engine):** hold.
- **Rule 4 (one conversation component):** holds for drawing. The palette's box lacks `#` (item 9).
- **Rule 5:** "only the user settles" holds. The "who answers" part is not enforced (item 3).
- **Rule 6 (no keys or buttons of a view's own):** holds.
- **Reach, path traversal for `file …`, the held-reports migration:** nothing found.

## Touch and phone width
- The discussion mark is 11pt and a tag bar's hit area is about 4pt wide (`config/default.toml` `gutter_*`). Both are far below finger size.
- A long press on a shared column is hard to land. Tapping a mark opens only the most urgent discussion; the others are reachable only through the hover or long-press text.
- `code.rs` line numbers are now `Sense::click_and_drag`. A vertical finger drag that starts on the number column selects lines instead of scrolling.
- I didn't verify whether the gutter's `fill::CHAT_CIRCLE`, drawn with the proportional font rather than `solid()`, renders the outline glyph. Checking it in the running cockpit would settle it.

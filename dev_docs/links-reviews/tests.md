Review of the tests in 0dfa7ff..HEAD, worst first. I only read the code; I built and ran nothing.

1. **The budget and Auto rules are never run.** `crates/nah-things/src/attach.rs` has no tests of its own. No server test produces a non-empty note: `tests/terminals_tests.rs:498` only checks a small thing (`{"notes":[]}`). The cockpit test makes up the note through `net.answer`. So all of these could break with every test still passing:
   - the 4096-byte / 100-line limit for Auto;
   - the `text.len() <= room` check against the 16 KB budget;
   - adding the note when a Whole link goes over budget.

   This is the plan's whole "budget per message" rule. Fix: tests in attach.rs with a fake owner (a thing at exactly 100 lines vs 101 under Auto; a 20 KB thing linked Whole becomes a pointer with one note; two Whole things where the second goes over). Add one `/api/links/check` test that returns a note.

2. **Some checks can never fail.** `terminals_tests.rs:488`, `:511` and `discussions_tests.rs:172` check that the text does not contain "What's linked above". If that heading's wording changes, or the `#word` scan comes back under another heading, they still pass. Check for the linked content instead (`!contains("tagged-line")`) or compare the exact text handed over.

3. **The server is never tested with a file, lines or symbol link.** Only tag links and terminal-command links reach the server. The `@` test (`app/tests/tags.rs`, last test) stops at the POST body.
   - `path:10-20` is only tested through the `lines_of` parser.
   - The `CodeLines` link built at `app/linking.rs:154` has `snapshot: ""`. Whether `code_tags.rs` `now_at` handles that (it should fall to `Err => At`) is unconfirmed. A server test with a lines link would confirm it.

4. **Links in discussions are untested on the server.** Nothing checks that links in the user's discussion message reach the asked slot (`src/discussing.rs:80`), or that `StoredSaid.links` survives a save and reload.
   - Real gap: `nah-discussions/src/lib.rs:576` writes `links: Vec::new()`, so a copied discussion loses its links.
   - In the cockpit, replying with links in the discussion view box (`discussion.rs`) is untested; only starting one from the palette is.

5. **`follow_edit` guesses wrong when a character repeats (`conversation.rs:238`).**
   - Text `#flaky now`, caret at `#flak|y`, type `y`: the diff sees an insertion after the link, so the link is kept. The plan says editing inside a link drops it.
   - Type `#` just before `#flaky`: the link is dropped, though the edit was outside it.

   The unit test only uses edits that can't be misread. Using the TextEdit's caret range would settle it.

6. **Giving typed text back is only half tested.** The `#` test presses Escape only with an empty filter (`tags.rs:506`, `:519`). These are untested:
   - Escape after typing something (`#fla` then Escape should leave `#fla`);
   - giving back into the palette's own writing (`put_back().put`);
   - the `@` picker giving back.

7. **The wait before the check isn't protected (`layout.rs:211`).** The test draws frames at 10.0 and 10.5 and then counts one request, which would also pass if the check fired at once. It also doesn't cover dropping a stale answer (`linking.rs:55`) or clearing the warning when the link is cycled off Whole. Fix: assert no request after the first frame, one after 0.4 s, then change the message and check the old answer is ignored.

8. **ctrl+i is never pressed in a test.** The unit test sets the private `caret` and calls `cycle_link` directly. If the TextEdit started taking the key first, or the caret stayed `None`, nothing would fail. One app-level test pressing ctrl+i would cover it.

9. **`thing_read`'s access check is untested.** Its `Reach` refusal (for example a sub-slot reading `chat lead`) and the `lines` range errors have no test. The plan now relies on this as the only reader-side check, since expansion no longer checks reach.

10. **Bad link offsets can underflow in `Message::trimmed` (`harness-contracts/src/links.rs:69`).** A link with `start >= lead` but `end < lead` makes `l.end - lead` underflow. In debug builds that panics; in release it wraps and the link is dropped. Example: a reply `{"text":"  hi","links":[{"start":2,"end":1,…}]}`. No test sends a malformed span. Low severity.

**Cut or merge:**
- `nah-tags/src/lib.rs:646` checks nah-things' pointer wording through tags. Move the pointer and size checks into the attach.rs tests from item 1. Keep the tags test to the heading, the note and the missing-tag case.
- `terminals_tests.rs:495` checks the same pointer wording; one place is enough.
- `tags.rs`'s `"no tags yet"` text check is wording; `is_open` already shows the behaviour.
- The draft-with-links checks in `layout.rs` and `nah-state/src/tests.rs` are different layers; keep both.

Nothing real in the mechanical updates (`slots_tests` `.0`, the snapshots, the role tool count).

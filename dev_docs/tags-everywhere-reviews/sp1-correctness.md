# Stopping point 1: correctness reviewer

## Review of ca36e61..b29fbfc (palette, action descriptions, terminal jump place)

Ranked, most severe first.

**1. Saved terminal lines point at the wrong line after any width change.** Medium.
- Where: `terminal/mod.rs:442` (`place()` stores `(shell, line)`) and `go_where_asked` at 163-179.
- Line numbers are not kept for life. `lines.rs:23-25,61-68`: when the width changes, the lines are laid out again and renumbered by `by`. The cursor is shifted at `mod.rs:244`, but places already stored in the jump list and marks are not.
- Sequence: in t1, `] t` to line 1010 (jump recorded). Change the terminal's width (`layout.2`, `panel.add`/`panel.fewer`, browser resize, phone rotation), so the server re-lays and the update's `cols` differs. Then `ctrl+o`.
- Result: line 1010 now names old line `1010-by`. The cursor lands on a different line with no message. If `by < 0` it can land past `end` and is clamped with the false note "its lines went".
- Marks behave the same way.
- The `nav.line` description added in `commands.rs` ("numbers they keep for life") states the same false premise.
- Fix: store the width with the place, or shift stored places on `Relaid`.

**2. The line is lost from the jump list if you move on before the terminal's lines arrive.** Low-medium.
- Where: `mod.rs:442` together with `nav.rs:233-258` (`back`/`forward` overwrite `entries[at]` with `here`).
- While a go-to is pending (`show()` reset `cursor = None` and `shell == 0`), `place()` is `{id, at: None}`.
- Sequence: cursor in t1 at line 1010. `g T` to a tag in t2 (records A = t1:1010). `ctrl+o` switches the buffer to t1 and waits for its socket. Press `ctrl+o` or `ctrl+i` again before t1's first screen arrives (a round trip through Home).
- Result: `back`/`forward` write `here = (t1, None)` over A. From then on, ctrl+o/ctrl+i to that entry opens t1 with no line.
- Any new jump made during that window also records `(t1, None)`.

**3. A jump or mark to a closed terminal does nothing, with no message.** Low-medium.
- Where: `mod.rs:463-465`. `let _ = self.show_target(...)` discards `Err("Terminal t1 is closed.")`.
- Sequence: `] t` in t1 (jump recorded with a line), `terminal.close`, then `ctrl+o` (or `'a` for a mark set there).
- Result: `restore_jump` has already focused the terminal panel. It keeps showing whatever terminal it had, no note is shown, and the jump step is used up.
- This is inconsistent in two ways:
  - A place without a line still goes through `show(id)`, which switches to the closed id.
  - `g T` to a closed terminal says so ("said, and nothing moves").

**4. A full-screen program moves the cursor onto its rows, with a false note.** Low.
- Where: `mod.rs:168-176`. While `alternate()` is true, `first()` is `screen.top()` (`lines.rs:131`).
- Sequence: jump list holds t1:1010. Run `less` in t1. `ctrl+o` from elsewhere.
- Result: 1010 is clamped onto a row of less's screen, with "its lines went". The lines still exist in the scrollback.
- Before this change it only said "gone" and didn't move the cursor.

**5. Rule 6 doesn't hold at runtime, and its check has gaps.** Low.
- Empty descriptions from harness data:
  - `chat/actions.rs:229`: `command.description.clone()`. `SlashCommand.description` is a plain `String` (`harness-contracts/src/snapshot.rs:116`), so a harness that sends `""` gives the action an empty description.
  - `chat/actions.rs:163`: `unwrap_or_else` only covers `None`, so a config control with `Some("")` also ends up empty.
- The check, `every_action_says_what_it_does` (`tests/bindings.rs:94`):
  - It is a test of one state, not a lint, as G.3 asks.
  - It only sees the actions the full fixture offers with each kind focused after two Escapes.
  - Actions offered only in other states are never checked: `terminal.restart` (ended), `tree.clear_filter`, `goto.pick`, `tag.goto.pick`, `server.shutdown_confirm`, `chat.inject`, request answers, `quicklook.*`, `attention.updates`.
  - `Action::new(.., "", ..)` still compiles. Test helpers do this at `actions.rs:677,713` and `keymap/mod.rs:562`.
- Wire change: `outside.rs:64` now always emits `"detail": ""|"…"`, where it used to emit `null` when there was none. This only matters if any cockpit-tool consumer checks for null.

**6. The palette still changes size while open.** Low.
- Where: `palette.rs:275-280`. The Text stage has no `set_min_height`.
- Sequence: open `:` (the list is held at `list_height`), pick "Link another server" or "Go to line or row". The palette shrinks to its title, input and one hint line.
- G.2 says "no shrinking back while one is open".
- Within one stage the title label still wraps. For example "Go to which thing <long tag name> is on?", or the long `panel.item_buttons` label as a Toggle/Choose title. That changes the height between pickers.
- The new test only compares frames inside one stage, so it doesn't catch this.

**7. Escape now records a jump in the terminal.** Low; possibly intended.
- Where: `mod.rs:590-594` together with `commands.rs:498`.
- `Leave` already ran `after_jump`. Since `place()` now includes the cursor, Escape back to the live screen (and `G`, `gg`, `:N`) are now recorded as jumps; before, the terminal's place never changed, so none were recorded.
- Sequence: cursor on 1010, Esc, type for a while, `ctrl+o`. The cursor goes back to 1010 and stops typing (`landing`).
- The plan lists go-tos and `] t` as jumps, not leaving the cursor. Please confirm this is intended.

**Areas checked with nothing real found:**
- Row truncation and wrapping (`max_rows = 2` survives `Label`'s wrap handling in egui 0.33.3; `space_m` equals `item_spacing.x`, so the keys never overlap the label).
- Centring.
- The serde default for old saved `TermPlace`.
- `kept_place` in the saved layout and fresh buffers: a reload doesn't bring the cursor back.
- Search cancel restoring the line.
- `landing` getting stuck.

**What would confirm the uncertain items:**
- #1: check that the server really renumbers on a width change (the doc in `lines.rs` says so).
- #2: a test that sends ctrl+o twice through a terminal switch before calling `arrive`.

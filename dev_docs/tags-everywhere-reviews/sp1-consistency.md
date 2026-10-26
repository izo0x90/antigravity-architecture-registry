# Stopping point 1: consistency reviewer

I reviewed `ca36e61..b29fbfc` and found 8 problems, ranked most severe first. All paths are under `meta-harness/crates/cockpit-ui/src/`.

**1. `ctrl+o` back to a place with no cursor in the same terminal does nothing (medium).** `terminal/mod.rs:453-469`: `restore` with `at: None` calls `self.show(id)`. When that terminal is already shown, `show` does nothing, so the cursor and scroll stay where they are.
- **To trigger:** in t1, press Esc (live screen, no cursor). Then `g T` to a tag on a t1 line, or `G`, or `:1010`. The jump recorded is `TermPlace{id:t1, at:None}`. `ctrl+o` then leaves the cursor on the line, and the press is spent because the forward list moves.
- The plan says "back to the exact spot left". Here that spot is the live screen with no cursor.
- The test hides this: `app/tests/terminal.rs:122-124` calls `leave()` before `restore(&kept)` to get `None`.
- **Fix:** `at: None` on the same id should clear the cursor and selection and go live, as `leave` does.

**2. Going back to a line in a closed terminal fails silently (low-medium).** `terminal/mod.rs:465` does `let _ = self.show_target(...)`, which throws away "Terminal X is closed."
- **To trigger:** `ctrl+o` or a mark that points at a line in a terminal closed since. Nothing moves, nothing is said, and the jump is spent.
- The `at: None` branch behaves differently: `show(id)` switches to the closed terminal.
- The existing pattern is the go-to path (`show_where_shown`), which puts the error in the note.

**3. `Navigable::kept_place` is a second mechanism for state kept across a reload (design; violates "one mechanism").** `nav.rs:685` adds it beside the existing `Buffer::keep`/`restore_kept` (`buffer.rs:74`), which the terminal already uses to keep its id (`terminal/mod.rs:377`).
- The terminal id is now saved twice in `SavedUi` (`kept` and `views`), and `restore_saved` (`app/keeping.rs:60-70`) calls `show(id)` twice.
- `kept_place` is also what a new buffer starts from (`app/layout.rs:23`). That is not "across a reload", so its doc and name say less than it does.
- One mechanism would do: `keep` and `restore_kept` own what survives a reload, or `views` stores `place()` and strips the line in one spot.

**4. The new descriptions only show in the palette (low-medium).**
- The key sheet (`app/draw/bars.rs:215`) still shows only `action.label`. Yet `nav.keys` now says "Every key here and what it does".
- Hover tips on controls (`actions.rs:594`, `draw_control`, whose doc says "hovering it says what it does") show only the label and key.
- The palette's search text (`palette.rs:~188`, `format!("{} {} {group}", label, id)`) leaves the description out, so typing a word from it finds nothing.

**5. The shared move descriptions contradict the views' labels (low).** `nav.rs:351-372` gives every `Move` a generic line, and no view overrides it with `.detail()`. Examples:
- Queue: label "Hide why it failed", description "Into what you're on, or opens it" (`queue.rs:262-263`).
- Terminal: "Start its shell again" or "Open a terminal" with that same line, and "Back to the live screen" with "Out of it, back, or up".
- `Motion::Left` reads "in a tree, fold…" in views that have no tree.

So rule 6 is met on paper but the text is wrong where you read it.

**6. The "lint" is a test that only sees the states `full_app` reaches (low).** `app/tests/bindings.rs:94-104`, and `Action::new` accepts `""`.
- Never checked: `terminal.restart` (only once the shell ended), `tree.clear_filter` (only while filtering), `chat.inject` (only during a running turn), request answers.
- Harness `command.*` uses `command.description`, which the harness may send empty.
- `config.*` gets an empty line when `control.description` is `Some("")`.
- The plan says "a lint fails without one". A `debug_assert!` in `Action::new`, plus a fallback for strings that come from the harness, would actually enforce it.

**7. The palette row hand-builds what egui already has (low, maintenance).** `palette.rs:329-340` measures the key text with `layout_no_wrap`, then uses `allocate_ui_with_layout`, `set_max_width` and `truncate`, so the label is cut short beside right-aligned keys. egui 0.33.3 has `egui::Sides::new().shrink_left().truncate().show(ui, left, right)` for exactly this; the code base doesn't use it yet. The hand-built measure also uses its own copy of the key font, so it drifts if the key style changes.

**8. One description hard-codes another action's key (low).** `nav.mark_set`: "A letter to come back here with (' and the letter)" (`app/commands.rs`). `'` is `nav.mark_jump`'s binding in `config/default.toml:194`. If that is rebound, the line is wrong, and keys are meant to come only from the keymap config.

Checked and fine:
- The palette's fixed place and size while open (`set_min_height` after the rows), and the description under the label wrapped to two lines.
- `#[serde(default)]` on `TermPlace.at`, so layouts saved before this change still read.
- The wording of the "nearest line" message, which matches the existing "It's gone from the terminal…" message.
- Callers of `.detail()` (request answers' warnings, alerts) override the description consistently.
- `outside.rs` now always sends a `detail` string (it could be null before); harmless.

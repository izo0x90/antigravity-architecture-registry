# Stopping point 3: consistency reviewer

I reviewed the d4c5b67..HEAD diff (3 commits) against the existing cockpit patterns. I edited nothing and built nothing. Findings, most severe first:

**1. "Name a tag" (`tag.insert`) shows in every view's palette, and running it from a view with no text box switches that view to typing mode.**
- Where: `crates/cockpit-ui/src/app/tagging.rs:199`. The condition is `typing = Insert || self.palette.is_open()`. The action is only ever listed while the palette is open, so the guard always passes.
- Trigger: in a terminal, open the palette, pick "Name a tag", pick a tag. `typed_in` does nothing there (the default in `nav.rs:767`), but `run_tag` still sets `InputMode::Insert` (tagging.rs:~272). The terminal is now in typing mode and the next keys go to the shell.
- Severity: medium. Unexpected keystrokes reach a live shell.
- Fix: the existing pattern is to offer an action only where it applies. Tie it to the buffer having a text box, as chat's typing-only actions do with `on_typing`.

**2. `#` in the chat box opens the modal palette, and Esc leaves you out of typing mode.**
- Where: `crates/cockpit-ui/src/chat/render.rs:326-329`.
- The existing prefix mechanism in the same box is `/`. It shows matching commands as `on_typing` buttons under the box, filtered by what follows the `/`, and the box keeps focus (`chat/actions.rs:223-234`). `#` is a second, different mechanism that takes the keyboard away.
- Closing the palette without a pick sets `InputMode::Normal` (`app/draw/panels.rs:81-84`).
- Trigger: type "fixes #", press Esc, type "12". The "12" is read as cockpit keys, not prompt text.
- The trigger is also fixed in code, so it can't be turned off. A markdown "# heading" or any "#123" pops the palette.
- Severity: medium. Typing is interrupted and keys go astray.

**3. The Tags view lost keyboard access to its edits.**
- Before: as the view's own focused actions, they got `Space , 1-6` (the removed `tags tags.rename [Space , 1]` … lines in `tests/snapshots/keys.txt`).
- Now: the shell's `tag.rename`, `tag.recolour`, `tag.note`, `tag.note_clear`, `tag.delete` and `tag.remove` are all bound `[]`. `config/default.toml:200-203` binds only `tag.add`, `tag.goto`, `tag.next` and `tag.prev`.
- Result: renaming, recolouring, writing a note or deleting needs the palette or a mouse/finger click.
- Severity: medium for a keyboard-first cockpit.
- Fix: add keymap entries in `default.toml`, the existing pattern for shell actions.

**4. A long press on a gutter bar uses a second long-press detector.**
- Where: `app/draw/host.rs:116` uses egui's `Response::long_touched()`.
- The cockpit already has its own recogniser (`touch.rs`, `touch_tick` and `on_gesture` in `host.rs:236-315`), timed by the `[touch] long_press_ms` config. The gutter ignores that setting and uses egui's own timing.
- Both run on the same finger. Elsewhere a long press puts the cursor on the item and opens the palette; on a bar it only posts a note.
- Severity: low-medium (inconsistent touch behaviour, config ignored).
- Fix: handle the gutter in `on_gesture`.

**5. A note can't be cleared when the cursor's thing has several tags.**
- Where: `tagging.rs:97` (the edits table has 4 entries) and `tagging.rs:141`.
- `tag.note_clear` is unavailable whenever no single tag is picked, and there is no "pick which tag" step for it. The `ClearNote` branches in the "which" labels are dead code.
- The id mapping in `TagEditWhich` (`tagging.rs:275-285`) repeats the ids in the edits table, so the two can drift apart.
- Severity: low (a missing path).

**6. Some tag edits fail silently.**
- Where: `tagging.rs:289` (`let Some(tag) = self.tag_to_edit(here)… else { return }`) and the `TagInsert` branch at `tagging.rs:268`.
- The same function's existing pattern says why it can't, through `self.note` (for example "The tag isn't on that any more."). The old Tags view said "The cursor isn't on a tag."
- Trigger: open Rename and start typing. Meanwhile another client deletes the tag, or takes it off the thing. Press Enter: the typed name is dropped with no message.
- Severity: low.

**7. The pre-filled text field may put the cursor in the wrong place (unconfirmed).**
- Where: `palette.rs:98-104` puts `start` into `filter`. The `TextEdit` at `palette.rs:201` has no explicit id, so it shares egui's stored text-field state (including the cursor) across stages and openings.
- Possible trigger: type "ren" in the list, press Enter on Rename. The cursor would stay at 3, inside the name, instead of at its end.
- To confirm: a test that opens `tag.rename` and reads the palette text field's stored cursor from egui's memory.
- Fix if real: set the cursor to the end when `start` is put in.

**8. Colour choices don't follow the rule in every picker.**
- F.2a asks for a swatch before a coloured choice "in every picker". The palette draws a tag icon instead (`palette.rs:236-238`).
- The dropdown picker drawn for settings controls (`actions.rs:656-684`) ignores `Choice::colour` entirely. No tag choice reaches it today, so this is latent.
- Severity: low.

**9. Gutter sizes are fixed pixel numbers.**
- `host.rs:22-25` (`BAR_W` 3.0, `BAR_GAP` 2.0) and the ±1.0 insets at `host.rs:106`.
- The same file otherwise uses the configured `theme::look()` sizes (`look.line_thin`, `space_s`, the radius sizes).
- Severity: low.

**10. Log-folder permissions.** `crates/nah-tags/src/logs.rs:47-54` only logs a warning when it can't restrict the folder and carries on. `nah-state/src/lib.rs:195`, setting a file's permissions, returns an error instead. This is a pattern difference only; I see no user-visible failure.

**Areas with nothing found:**
- Folds in the Tags view reuse `nav::fold_here` and `nav::fold_all`.
- "When tagged" reuses `attention::age`.
- Row buttons go through `Action::on_item`, drawn by the host.
- There is one "take the tag off", the shell's.
- The new buttons in the Tags view are drawn by the host, as the existing check that views draw no controls requires.
- Every new action has a description.
- No crate already provides something that was built by hand here. Loading egui-phosphor's filled icon style as a second font family has to be done by hand.

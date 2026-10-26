# End review: correctness (eda7934..03c4a3e)

Paths are under `meta-harness/crates/cockpit-ui/src`. Problems are ranked, most serious first. Each one gives the sequence that goes wrong. Items the findings file records as fixed or knowingly left are not repeated. The step-1 fixes I checked (the waiting Enter, back to the top, the graph's find tended off screen, the setting popup's size, rounds dropped) hold.

## High

### 1. A tap on the which-key menu also lands on the panel under it, and the action runs on that panel (app/draw/panels.rs 27-33; app/draw/which_key.rs 149-155; app.rs 561, 590)

- `render_panel` focuses a panel on any `primary_clicked()` inside its rect. It doesn't check which layer took the click.
- The which-key menu is a Foreground area anchored at the bottom centre, over the panels. It reads its tap later in the same frame (`render_which_key_hud` runs after the panels).
- The tap goes to `leader_key`, which reads `bound_actions()`. That list was cached for the frame by the top bar, before the focus moved, so it is the old panel's list. Shell commands then act on `desk.panels.focused()`, which is now the panel under the menu.

Sequence (two or three panels, 1278 px wide):
1. Tap "Panels l" on the rightmost panel's top line. That panel is focused and the menu opens at `l`. The menu is 760 wide and centred, and its first entry, "x Close this panel" (Host layer, listed first), has its key pill at about x = 275. That's over panel 1.
2. Tap the "x" pill.
3. `render_panel(0)` focuses panel 1. `leader_key('x')` runs `panel.close`, and `ClosePanel` closes `focused()`, which is panel 1. The panel whose menu was opened stays.

The same happens for:
- "Buffer b › b Open buffers": it shows the buffer in the other panel;
- "Space › / …" (nav.hint and nav.search): they act on the other view;
- tapping a part of the path ("Space ›"): focus moves under it, and the menu shows the other panel's entries from the next frame.

The palette's rows have the same cause: a tap on a row over a panel focuses that panel before the palette runs the row. On touch, which this step makes the main way to walk the menus, this is the normal case.

### 2. The quick look's own menu can't be used by tapping: "Close" reopens it, "Open it in a panel" does nothing (app/look.rs 154-158, 39-46, 95-96; app/draw/which_key.rs 151-152; app.rs 581, 590)

Step 3 removed the quick look's header buttons. Its own actions are now only in its "Quick look q" group, plus its view's menu.

- `render_quick_look` closes the quick look on any primary click outside its area. The quick look spans 10-90 % of the height. The which-key menu's entry rows sit about 48 px above the bottom, so they're outside it.
- The quick look's close runs before `render_which_key_hud`, in the same frame.

Sequence:
1. Open the quick look (`q` on a component).
2. Tap "Quick look q" on its top line. The menu opens at `q`.
3. Tap "c Open it in a panel". The click closes the quick look (`quick_look = None`). Then `leader_key` runs `quicklook.to_panel` from the cached Look list. `quick_look_to_panel` returns at `let Some(look) = self.quick_look.take() else { return }`. Nothing opens.
4. Or tap "q Close the quick look". It's closed by the click, then `toggle_quick_look` finds none open and opens it again, on the focused panel's target. If there's no component, it says "Nothing here to look at".

Entries of the quick look's view menu (for example the spec's) run on `desk.view(Spec)`. After `close_quick_look` → `use_buffer(look.before)`, that is the buffer used before, not the card that was shown. With keys it works, because no click is involved.

### 3. A notice-row button (or any control) in a panel not focused does nothing on its first tap when the action is the view's `focused()` one (app/draw/host.rs 298-303; app/commands.rs 254-265, 316-325; app/draw/panels.rs 27-33; app/draw/top_bar.rs 150)

- The control sets `asked`, and `render_panel` then focuses that panel. `run_asked` → `run_by_id` → `palette_actions()`.
- `palette_actions` starts from `bound_actions()`. That list was cached this frame by the top bar for the old focused view, and nothing drops it when a click moves focus. (`on_gesture` does drop it, host.rs 262-265. The click path and the group tap don't.)
- It then adds the `Scope::Focused` actions of every kind except `focused_kind()`, and that is now the clicked panel's kind.
- So a `.focused()` action of the clicked view is in neither list. `find` gets `None`, and nothing runs or is said.

Sequence: two panels, the graph focused and the chat in panel 2 showing a sign-in.
1. Tap "Copy link" (or "Open the sign-in page", "Cancel sign-in", "Finish signing in") on panel 2's notice row.
2. Panel 2 is focused, and the link isn't copied.
3. A second tap works.

The same happens for:
- the chat's Dismiss, Retry and "Dismiss" of an error or of a sign-in's end (all `.focused()`, chat/actions.rs 255-298);
- the graph's "Return" and "Show N levels" (graph's `f` is `.focused()`, graph/actions.rs 81, 116-119);
- the code view's "Deleted lines" checkbox.

With two panels of the same kind (two chats), the action is found in the old list. It then runs on `desk.view(kind)`, which after `desk.drawn()` is the used buffer, the other chat. So "Dismiss" on panel 2's error dismisses panel 1's.

### 4. Under the graph's chips (any touch, or a canvas under 480 px), the "View" setting disappears from the graph's settings row (graph/draw.rs 880-886; app/commands.rs 198-203; hosted.rs 175-180)

- `GraphState::placed()` gives every id in `[graph.chips] actions` a `Control::new(At::Header, Style::Button, "", None)` while `chips_shown()`. That's `self.fingers || canvas < narrow_below`, and `fingers` is true after any touch event.
- The default list has `graph.view`. `gather` replaces the action's own control (`header(Setting, "View")`) with that button.
- Since step 3, `hosted::header` draws only `At::Notice` buttons and `At::Header` settings. Header buttons are drawn nowhere on a panel.

Sequence:
1. On a phone, or after one touch on a laptop, look at a graph panel.
2. Its settings row has "Source" and the others but no "View" dropdown, and no chip (zoom, fit, flip, labels) is drawn.

`[graph.chips]` now only takes the View setting away.

## Medium

### 5. Tapping a group while the palette is open enters the which-key menu with the palette still open: keys go to both (app/draw/host.rs 51-56; app/draw/which_key.rs 43-46; app/keys.rs 117-145; palette.rs 333-336)

- `open_leader_at` sets `InputMode::Leader` whatever the mode was.
- The palette doesn't close on a click outside it. It is at y = 90 (wide) or at the bottom (narrow), so every panel's top line stays tappable.

Sequence:
1. Press `:`.
2. Tap "Buffer b" on a panel. Now `input_mode` is Leader and `palette.open` is still true.
3. Type `n`.
   - `handle_keys` (Leader) takes it as the "New buffer" group: path [b, n].
   - It isn't a completed action, so `typed_away` isn't called, and the palette's field (focus requested every frame) also gets "n". Its filter becomes "n".
4. Press Escape. The leader pops to [b], and the palette's `back()` closes it too: one key, two things undone.
5. `render_palette`'s clean-up (panels.rs 82: `input_mode == Palette`) doesn't run, so `palette_from` isn't restored. A palette opened from typing doesn't go back to typing.

### 6. Tapping a group during a capture (a mark, a search, hint labels) leaves the capture behind in Normal mode (app/draw/which_key.rs 43-46; app/keys.rs 34-37; app/draw/bars.rs 103-120, 31-43)

- `open_leader_at` sets Leader and doesn't end `self.capture`.
- `leader_key` (or Escape at the top of the menu) then sets `InputMode::Normal`, with `capture` still `Some`.

Sequence:
1. Press `'` (jump to a mark). The "marks" pop-up shows.
2. Tap "Buffer b" on the panel, then tap any entry, or press Escape twice.
3. Now `input_mode == Normal` and `capture == Some(MarkJump)`:
   - `render_prefix_popup` keeps drawing "marks", because `marking` reads only `capture`;
   - the key bar shows "Marks: a letter the mark · Esc gives up" (`mode_keys` reads `capture` first);
   - a letter runs a Normal-mode command instead;
   - Escape doesn't clear it.
4. It stays until another capture is started and finished. With `/` (search) the key bar keeps saying "Searching".

### 7. Keys stand aside for a setting's pop-up only in Normal mode (app/keys.rs 84; app/keys.rs 87-115; actions.rs, the Choice/Setting arm)

The guard is `InputMode::Normal if egui::Popup::is_any_open(ctx)`. A setting pop-up can be opened by a click in any mode, because nothing ends Leader or Capture on a click.

Sequence:
1. Press `/` and type "foo" (Capture: Search).
2. Click the graph's "View" dropdown, press ctrl+J, then Enter.
3. `handle_keys` runs first and reads Enter with `ctx.input` (not consumed): the search is sent and jumps to the match.
4. The pop-up's `keys.picked(i)` then reads the same Enter and switches the view: one Enter, two actions.

In Leader mode, Escape both closes the pop-up (egui) and pops the menu.

### 8. The code view's "Re-read it" button is gone from its panel (code.rs 1049-1055; hosted.rs 175-180)

- `CodeView::placed()` still puts `index.file` at `At::Header, Style::Button` when the file is out of date with the index, and nothing draws that any more.
- The spec's same case (`spec.reread`) was moved to the notice row in step 3. The code's wasn't.

Sequence:
1. Open a file that changed since it was indexed.
2. Nothing on the panel says so or offers to re-read it. It's only in the menus and the list.

`tree.clear_filter` (tree/mod.rs 469) and `terminal.restart` (terminal/mod.rs 756) are also `.header(Style::Button, ..)` and are drawn nowhere now. They're placed as if on the top line, a dead placement.

### 9. A group tapped in another panel: for that frame, the menu and the buttons of what you're on are built from the old panel's list (app/draw/host.rs 51-56, 159; app/draw/which_key.rs 69-70; app.rs 581-590)

- `render_view` calls `desk.panels.focus(panel)` but doesn't drop `frame_actions`, as `on_gesture` does.
- In the same frame:
  - `render_item_controls` draws the old view's `Layer::On` buttons at the new view's cursor item (its `kind` and spots are read fresh);
  - `render_which_key_hud` lists `leader_entries(<old list>, [key])`.

Sequence:
1. Graph focused.
2. Tap "Chat actions ," on the chat panel.
3. For one frame the menu is titled "All chat actions" (`focused_kind()` is fresh) but lists the graph's `,` entries. A group with no entries in the old view, such as "Account a" from a code panel, shows its path and no entries.
4. The next frame corrects it. This is the flicker this plan set out to remove.

## Low

### 10. A long press on any top-line control is also a gesture on the view: it focuses the panel and, in the graph, can select a card (app/draw/host.rs 210-215, 250-266; graph/draw.rs 216-218, 380-385)

- `touch_tick` skips a finger only when `view_at` is `None`, which means only over Foreground layers. The top line is in the panel's Background layer, so a long press on a group, notice or setting button fires `Gesture::LongPress`.
- `on_gesture` focuses that panel. In the graph, `own_pointer`, it calls `on_gesture` → `hit(pos)`, which tests card rects against the screen with no clip to the canvas.

Sequence:
1. Graph panel not focused. Pan so a card sits under the panel's top line.
2. Long-press "Graph actions ," to read its tip.
3. The panel takes focus and the hidden card is selected. Hover, which this is meant to mirror, does neither.

### 11. A discussion's notice row is there with nothing to do (discussion.rs 424-436; hosted.rs 175; actions.rs 646-650)

Settle and Reopen are always pushed with `.notice(..)`, each `When::Unavailable` when it doesn't apply. `header` shows the notice row when any notice-placed action exists, and `control` draws an unavailable one greyed.

Sequence:
1. Open a discussion that is open and waiting on the agent (`asked` isn't the user).
2. The notice row shows two greyed buttons, "Settle" and "Reopen", though nothing needs you. The plan has the row only "while there is something".

### 12. `?` never lists the keys of the mode it was opened from (app/commands.rs 447-452; app/draw/bars.rs 31-43, 172-174; config default.toml 226)

- `?` and `Space ?` are read only in Normal and Leader. In Insert, Capture and the palette, `?` is typed.
- `leader_key` sets Normal before it runs, and the palette sets Normal before it invokes.
- So `mode_keys()` is `None` whenever `Keys` runs from the user. The "<mode>: its keys" section of the plan's `?` never appears.
- `sheet_mode` is never cleared. If it was set once (an agent running `nav.keys` while the user types: "Typing: its keys"), it shows again in every later `:` → Tab, whatever mode the user is in.

### 13. In `?`, a faded row that takes a value does nothing when picked, and says nothing (sheet.rs 153-158; pick_list.rs 187, 197; app/commands.rs 343-345)

- `kind` is `Inform` when the action doesn't apply and has a param, so Enter and a tap pass it by.
- For `When::Unavailable` (most such rows: tagging, links, terminal, queue, discussion), `needs()` is `None`, so the row doesn't say why either.
- Its key would say "<label>: nothing here it applies to." (`run_bound`).

Sequence:
1. Put the cursor on an item with no tags.
2. Press `?` and type "take a tag off". "Take a tag off what you're on" (`tag.remove`, `Param::Choice`, app/tagging.rs 94-96) is faded, with no reason given.
3. Enter or a tap on it does nothing, and nothing is said.

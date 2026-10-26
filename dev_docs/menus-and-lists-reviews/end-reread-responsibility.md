# End re-read: responsibility (03c4a3e..1694b22)

Paths are under `meta-harness/crates/cockpit-ui/src`. Line numbers are at 1694b22. I only read the code; nothing was built or run.

## 1. The end review's findings

### Responsibility (`end-responsibility.md`)

1. **One meaning per `At`; no view-placed buttons drawn nowhere.** Partly fixed.
   - Done: `At::Header` is split into `Settings` and `TopBar` (`actions.rs:132-145`). `Action::header` is replaced by `Action::setting`, which always uses `Style::Setting` (`actions.rs:451-455`). The graph's chips and their config are deleted. `index.file`, `tree.clear_filter` and `terminal.restart` have been moved.
   - Left: a view can still place a control that no host draws. `Hosted::placed` (`hosted.rs:77`) returns a bare `Control`, and `Control::new(at, style, …)` is public (`actions.rs:199`). So a view can still return `At::TopBar` (the top bar draws it only for an Everywhere-layer action, `app/draw/top_bar.rs:144`, so a view's action is dropped). It can also return `At::Settings` with `Style::Button` (the settings row filters on `At` only, `hosted.rs:185`). The type still doesn't make these unrepresentable.
2. **What needs you now on what it's about; no `At::Notice`.** Fixed. The banners are a place: `here()` names it (`chat/render.rs:407-412`) and `drawn_at` says where it is. Their actions are item controls. `At::Notice` and `Action::notice` are gone. The inbox's Previous and Next, the queue's Start and Stop, and the chat's Stop are now in the menus only. One new problem: the "which banners show" condition is written twice (see 2.6).
3. **The menu has one entry; focus is moved properly; the group button is a control.** Partly fixed.
   - Done:
     - `open_menu` is the one entry. It goes through `leader_key` (`app/draw/which_key.rs:69-78`).
     - A tap asks through `RunCx::asked` (`hosted.rs:195-203`).
     - The click-to-focus path drops the frame's stale list (`app/draw/panels.rs:28-39`).
     - `header` no longer returns `Option<char>`.
   - Left:
     - The group button is still a type of its own (`hosted::GroupButton`, `hosted.rs:123`), not a `Bound` drawn by `actions::control`. Its tip is formatted in `which_key.rs:45` and `:62`, apart from `draw_control`'s.
     - The top bar's Menus button uses this second kind of control too (see 2.3).
     - `open_menu` replaces the capture state directly (see 2.2).
4. **One judgment of "applies", and one run policy.** The judgment is fixed; the run policy is not.
   - `CockpitApp::applies` (`app/views.rs:59-65`) judges each action in its own view. The key bar, the which-key list, the groups, the item controls, swipes and the sheet all use it.
   - The parallel `applies: Vec<bool>` is gone.
   - The run policy is still split three ways, and the findings' claim that controls go "through `run_bound`" is false. See 2.1.
5. **`sheet_mode`; the sheet built while closed.** Fixed. The mode's keys come from `mode_keys(self.palette_from)` (`app/draw/bars.rs:180`). `render_palette` returns before gathering anything when the list is closed and nothing asked for it (`app/draw/panels.rs:55-58`).
6. **A second long-press recognizer, and the gestures listed by hand.** Partly fixed.
   - Done: `tip_on` and `long_touched` are gone. `on_gesture` ignores a gesture that isn't on a view's content or a note mark (`app/draw/host.rs:252-255`).
   - Left:
     - The tip on a held finger appears after egui's hover-tooltip delay, not after `[touch] long_press_ms`.
     - The "Pointer and touch" section is still a hand-written list in `app/draw/bars.rs:185-192`, apart from `touch.rs` and `on_gesture`, which handle those gestures. Only its wording was updated.
7. **The row's kind is found by building the row.** Fixed. `PickList::show` takes a separate `kind` closure (`pick_list.rs:134`), and `Listed::kind` builds nothing.
8. **The dead `keyed` parameter; hosts rewriting `Bound`s.** Fixed. `keyed` and `TopLine.keyed` are gone. The host still strips keys on a panel that isn't focused, but now through `Bound::unkeyed` and `GroupButton::unkeyed` (`app/draw/host.rs:31-32`) rather than by writing fields.
9. **A search's lifetime handled in three places.** Fixed. There is one driver, `CodeSearch::drive`. The palette's `answered` is gone. `render_palette` sees the search stage end and tells its owner (`app/draw/panels.rs:44-52`, `:124-133`). A hidden view's search is tended by `Buffer::tend_search`. Still there: the shell side dispatches on the action's id string (`LINK_THING`, `app/draw/panels.rs:129`), next to the string-keyed `desk.shell_searches`.
10. **egui's popup state decides who has the keys; two sources for the list keys.** Partly fixed.
    - Done: `Popup::is_any_open` is gone. The new `InputMode::Choosing` now has the keys.
    - Left:
      - The mode is set and cleared inside the widget function `draw_control` (see 2.2).
      - The list keys still come from two sources. The setting control reads `keymap::local().list` (`actions.rs:782`). The palette is handed `&self.config.keymap` (`app/draw/panels.rs:66`, `palette.rs:310`).
11. **The view menu's name decided twice.** Fixed. There is one `group_name(path, kind)` and one `menu_path` (`app/draw/which_key.rs:9-23`), and `BufferKind::words` replaces the copied title-casing. `leader_title` is gone. `Sheet.groups` carries the view menu's name for matching.
12. **Layer titles matched by position.** Fixed: `Sheet::title(Layer)` is a match (`sheet.rs:79-86`).
13. **The `:N` row's offset arithmetic in the palette.** Fixed. `Item::Line` is an entry of `Listed`, found by its command (`app/draw/bars.rs:195`), not by its id.

### Anti-patterns (`end-antipatterns.md`)

1. **Dead top-line placements and chips.** Fixed, except that a view can still place a control no host draws (responsibility 1 above).
2. **`At::Notice` beside the plan's mechanism.** Fixed (responsibility 2).
3. **Two search drivers, two encodings of a picked row, `before`.** Fixed:
   - `drive` is the one driver;
   - a picked row carries its `Hit`, or a `Picked { subject, label }` (`app/linking.rs:20-23`);
   - `before`, `hit()` and the label lookup are gone.

   Still there: the shell-side search state stays in the string-keyed `shell_searches`.
4. **`sheet_mode`.** Fixed.
5. **`Sheet`'s parallel arrays and tuples; the special case for other views.** Fixed. `Offered`, named `Inform`/`Informs`, `Sheet::title`, and `applies` by the action's own view.
6. **The `everything` flag.** Fixed: `Stage::Actions { reach }`, `open_list(reach)`, and `Listed` has sections in both reaches. The cycle key still has its own branch in `render` (`palette.rs:366-370`), apart from the value-stage `cycle`.
7. **The view menu's name special-cased twice; title-casing copied.** Fixed.
8. **Patched flags for panels that aren't focused.** Fixed (responsibility 8).
9. **The setting popup taking the keyboard outside the input modes.** Partly fixed. It's an input mode now, but the widget sets it (see 2.2), not the app's own transitions. The fix the review suggested (the list going through the palette's mode, or a mode the app enters) wasn't taken.
10. **A faded row's runnability is a second copy of `run_bound`'s rule.** Fixed for the list: every action row is `Pick`, and `Offered::refused` decides. For controls it isn't (see 2.1).
11. **`answered`.** Fixed.
12. **Stale comments and leftovers.** Mostly fixed. Left or new:
    - `app.rs:258-261`: the `gathered` doc line ("How many times the actions were gathered") now sits above the new `searched` field, and `gathered` has none.
    - `kind.rs:56-57`: `name()`'s doc line ("Lowercase name used in action ids") now sits above the new `words()`, and `name()` has none.
    - `app/tests/audits.rs:82`: "its key sheet".

## 2. Problems introduced by the fix pass, or still there

Ranked, most serious first.

### 2.1 What may run where it doesn't apply is still decided in four places, and controls skip the "one rule"

- The findings say `Offered::refused` "is asked by the keys, the controls (through `run_bound`) and the list". The controls don't go through `run_bound`:
  - `actions::control` puts the id on `RunCx::asked` (`actions.rs:668`);
  - `run_asked` calls `run_by_id` (`app/draw/host.rs:294-297`);
  - `run_by_id` (`app/commands.rs:323-331`) opens the picker or invokes, and never asks `refused`.
- So the rules are:
  - `actions::control` (`actions.rs:656`) greys only `When::Unavailable`;
  - the settings row is filtered by layer only, not by `applies` (`app/draw/host.rs:32`). A setting whose `When` doesn't hold is drawn enabled, and when used, runs or opens its picker with nothing said;
  - `render_palette`'s view-request filter is a fourth copy: `when != When::Unavailable` (`app/draw/panels.rs:64`), not `applies`;
  - `refused` lives in `sheet.rs` ("what the action list lists", `sheet.rs:36-51`). `run_bound`, the key runner, builds a `sheet::Offered` (cloning the `Bound`) just to ask it (`app/commands.rs:358`).
- The run policy belongs beside `run_bound`/`invoke`, or on `Bound`, not in the list module. Every run entry (`run_bound`, `run_by_id`, the palette's pick) should ask it.

### 2.2 Two new ways into a mode change `input_mode` and `capture` directly, skipping the leave path of the mode they replace

- **`open_menu`** (`app/draw/which_key.rs:69-78`):
  - it sets `self.capture = None` and `InputMode::Leader` from any mode;
  - a tap on a group button (or `menu.open` from the action list) during a search being typed drops `Capture::Search` without what `on_capture`'s leave path does (`app/capture.rs:69-73`): `on_search(SearchEvent::Cancelled)`, `restore(before.place)`, `restore_view()`;
  - the view is left mid-search (terminal: `found` kept; views: cursor and camera left where the search moved them).
- **`InputMode::Choosing`**:
  - `draw_control` (`actions.rs:795-806`) is a widget-drawing function in the action registry. It writes the app's `input_mode` through `RunCx`.
  - It replaces any mode but `Palette` (Insert, Capture, Leader) with `Choosing`. On close it sets `Normal`, not the mode it replaced. `capture` and `leader_path` are left as they were.
  - Leaving the mode is decided in two places: in `draw_control` on close, and in `keys.rs:60-66` when the list vanishes.
  - It rebuilds egui's private ComboBox id derivation (`ui.make_persistent_id(egui::Id::new(combo_salt))`, `actions.rs:795`, "its salt made an id, in this ui"). That couples the mode to egui's internals.
  - It has to `request_repaint` because the bars were drawn before it decided.
- Fix: mode changes belong to the app. The control reports "my list opened or closed" (as it reports use, through `asked`), and the app enters or leaves the mode through the same transitions keys use. `open_menu` leaves a capture through the capture's own leave path.

### 2.3 The top bar's Menus button bypasses the top bar's own placements

- The top bar has its own placement mechanism: `top_bar_placements`, then `At::TopBar` controls, drawn by `host_controls` (`app/draw/top_bar.rs:81-125`, `:144`).
- `menu.open` is a shell action. Placed there, its control would be one more `At::TopBar` button.
- Instead, `every_menu()` builds a `hosted::GroupButton` (`app/draw/which_key.rs:60-63`). The top bar draws it with `crate::hosted::groups` (`app/draw/top_bar.rs:182-184`): the panel top line's function, with a hand-built `RunCx`.
- That's two ways to put a button on the top bar, and a second control type, with its own caption and tip format, outside "every button is a bound action".
- The panel group buttons are the same second type (responsibility 3). They carry `asks: (MENU, key)` rather than being the `menu.open` action bound with that value.

### 2.4 `Offered`/`applies` vs `actions::control`'s greying: a control and its key disagree

- This follows from 2.1 but shows on screen on its own.
- A setting or item control is drawn enabled unless `When::Unavailable` (`actions.rs:656`).
- The key bar and the menus hide or fade the same action by `applies` (the full `When`, judged in its own view).
- So a `When::Target` setting with nothing under the cursor is enabled on the settings row while it's faded in `?` and refused from its key with "it needs …". The greying should read the same judgment as `applies`.

### 2.5 The key bar reads a copy of the palette's key line stored the frame before

- `Palette.keys` (`palette.rs:55`) is set inside `render` (`palette.rs:534`). The key bar reads it (`app/draw/bars.rs:89-92`), but the key bar is drawn before the palette in `draw_frame` (`app.rs:587` vs `:625`).
- So the key bar always says last frame's keys. On the frame the list opens, `Palette::open` has reset `keys` to empty (`..Self::default()`), so the key bar says nothing.
- "One key line" is done by caching render output for another surface, instead of the key bar asking `keys_said(stage, …)` (`palette.rs:561`) of the current stage.

### 2.6 Which banners show is written twice

- `render_banners` (`chat/render.rs:85-160`) draws a banner on each of five conditions.
- `banners_shown` (`chat/render.rs:183-186`) restates them by hand, for `here()`.
- If they drift, `here()` names a place that isn't drawn, or a banner is drawn without its buttons. Both should come from one list of the banners showing now.

### 2.7 The per-frame situation cache is dropped only where callers remember to

- `frame_situations` (`app.rs:255`) is new and is dropped by `forget_frame`.
- The click-to-focus path (`app/draw/panels.rs:37`) and `on_gesture` (`app/draw/host.rs:259`) call it.
- The click-to-move path doesn't: `pointer_on_content` (`app/draw/host.rs:111`) moves the cursor with `move_to` and calls nothing. For the rest of that frame, `applies`, the item controls and the groups are judged against the old target.
- The same holds for `frame_actions`, which isn't new. The new cache widens it.

### 2.8 Shell-side search ownership is a branch on an id string

- `search_done` for `Origin::Shell` (`app/draw/panels.rs:124-133`) removes the id from `desk.shell_searches` and, if it's `LINK_THING`, clears `desk.finding`.
- A view's search is told through `ActionProvider::search_done`. The shell's searches have no owner type: each new shell search needs another `if action == …` here.

### 2.9 A per-host content registry added beside the others

- `CockpitApp.contents` (`app.rs:327`, filled at `app/draw/host.rs:64`) exists only so that `on_gesture` can tell "on content" from "on the top line" (`app/draw/host.rs:252-255`).
- It's a fourth per-frame rect list beside `panel_rects`, `item_bars` and `note_marks`. The top line's own rect, now returned by `hosted::header` (`app/draw/host.rs:41`), is used only for the inspector, not for this.

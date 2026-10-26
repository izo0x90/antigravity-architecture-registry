# End review: responsibility (menus and lists, eda7934..03c4a3e)

Paths are under `meta-harness/crates/cockpit-ui/src`. Ranked, most important first. The checkpoint findings recorded as fixed were checked against the code. Where a fix only partly holds, it is item 9.

## 1. Views still place top-line buttons, but no panel draws them any more; `At::Header` now means "the top bar" or "the settings row" depending on the layer

- Step 3 cut `hosted::header` down to the notice buttons and the header settings (`hosted.rs:175-179`). Nothing draws `At::Header` + `Style::Button` on a panel now.
- The top bar draws `At::Header` controls only when the action's layer is `Layer::Everywhere` (`app/draw/top_bar.rs:150`).
- So where a placed control ends up depends on which host's filter it happens to match, not on its `At`. Three cases:
  - `code.rs:1049-1055`: the code view places `index.file` ("Re-read it", shown when the file is out of date with the index) as `At::Header, Style::Button`. It's a Panel-layer action, so it's now drawn nowhere. Step 3 moved the spec's twin of it (`spec.reread`) to `notice`. This one was missed.
  - `graph/draw.rs:880-885`: when the chips show (touch, or a canvas under 480 wide), the graph places every `[graph.chips] actions` id as `At::Header, Style::Button`.
    - Its own actions (`graph.fit`, `graph.zoom_in`, `graph.zoom_out`, `graph.flip`, `graph.view`) are Panel layer, so they're drawn nowhere.
    - `gather` replaces an action's own control with the placed one (`app/commands.rs:198-203`). So `graph.view` loses its `Setting` control, and the graph's View picker disappears from the settings row exactly on touch and narrow screens.
  - The shell's `nav.back`, `nav.forward` and `nav.hint_open`, placed by the same graph code, are Everywhere layer. They go to the global top bar while the graph is focused.
- These docs still say "on its top line": `hosted.rs:73-76` (`Hosted::placed`), `graph/draw.rs:876-879`, `code.rs:1048`.
- `Action::header(style, …)` (`actions.rs:449`) still accepts `Style::Button`. The type lets a view ask for a place that no longer exists, and nothing warns.
- Fix: one meaning per `At` (a top-bar place of its own, or no `Header` + `Button` for views). Make the placements a view can give match what the hosts draw.

## 2. "What needs you now" got a new place (`At::Notice`) instead of the plan's mechanism; not recorded

- The plan (Decided 1, and "Step 3: how it's built") says a sign-in's buttons stay on its banner. It names the means: the banners are a spot of the chat's, `here()` names it, and their actions are item controls ("the mechanism that puts a request's answers on its card").
- What was built is a panel-wide row:
  - `At::Notice` (`actions.rs:131-135`);
  - `Action::notice` (`actions.rs:454-458`);
  - drawn under the top line (`hosted.rs:175-177`).
- So "what needs you now" has two mechanisms:
  - item controls on a spot (a request's answers, e.g. `inbox.rs` `answer.place(…)`);
  - the notice row (the chat's Connect, Resume, Stop, sign-in steps, Dismiss and Retry; the discussion's Settle and Reopen; the inbox's Open, Put away, Previous and Next; the graph's Return and "Show N levels"; the queue's Start and Stop).
- How it goes wrong:
  - The chat draws its banner text in the body (`chat/render.rs:75-77`: "its ways out … are its actions, on its top line"), but the buttons sit on the panel's top area, away from the banner they answer.
  - One inbox request's actions are split: answers under the card (`At::Item`), its other actions on the notice row.
  - The notice row also holds things that aren't "needs you now" (the inbox's Previous and Next).
- The plan's Status says step 3 is done, and its "how it's built" section still describes the banner spot. The findings file has no entry. `process/build-and-validate.md` says: a step built another way than planned goes to the user. This one hasn't.

## 3. A tapped group opens the which-key menu by a second path, with the previous panel's actions for a frame

- `app/draw/host.rs:51-56` focuses the panel with `self.desk.panels.focus(panel)`, then calls `open_leader_at(vec![key])` (`app/draw/which_key.rs:43-46`). That sets `leader_path` and `InputMode::Leader` directly.
- The key path is different:
  - it enters Leader only from `InputMode::Normal` (`app/keys.rs:153-157`);
  - it walks into a group through `leader_key` (`app/keys.rs:26-33`), which checks that the entry exists in `bound_actions()`.
- `host.rs:53` focuses without dropping the frame's cached list. `on_gesture` does drop it on the same change (`host.rs:262-265`: "Where you are moved: the list is gathered again").
- Later in the same frame, three things read the cached list of the panel focused before the tap (`app.rs:581-590`): `render_item_controls`, `take_asks` and `render_which_key_hud`.
- For that frame:
  - the menu at `[key]` lists the old view's group, or nothing (the view menu `,` shows the other view's actions);
  - the item controls of the old view are drawn in the newly focused panel.
- A tap from `InputMode::Insert` also goes straight to Leader, a route keys can't take.
- The group button isn't a bound control either.
  - `hosted::header` returns `Option<char>` (`hosted.rs:139`, `186-195`), a second "a control was used" channel beside `RunCx::asked`, which every other host-drawn control uses.
  - Its look and tip are built apart from `actions::control`.
- Fix: one entry to the leader menu (the leader, then `leader_key(key)`), with where you are moved the way `on_gesture` moves it.

## 4. Whether an action applies, and whether one that doesn't may run, is decided in several places with different rules

- Applies:
  - `app/draw/bars.rs:159-165` (`sheet_of`, for `:` and `?`): another view's action applies unless it's `When::Unavailable`; the rest are judged by `situation()`.
  - `app/draw/which_key.rs:19,27` (`groups_of`, for the top-line groups): every action in the list, other views' global ones included, is judged by `situation_of(here.kind)`. So one view's `When::Code` is judged against another view's target.
  - The key bar and the which-key list use `situation()` for all (`bars.rs:23`, `124`).
- The answer reaches the list by position:
  - `Sheet::applies: Vec<bool>` is parallel to the action slice (`sheet.rs:19`);
  - it's read with `applies.get(i).copied().unwrap_or(true)` (`sheet.rs:55`). An index that doesn't line up silently reads as "applies".
- May run when it doesn't apply:
  - `run_bound` (`app/commands.rs:341-346`) refuses only a `When::Unavailable` action that takes a value, and says why. An action that takes a value but merely misses its `When::Target` still opens its picker from its key.
  - `sheet.rs:157` restates the rule another way: a row that doesn't apply is picked only if `Param::None`. Otherwise it's an Inform row: Enter and a tap do nothing and nothing is said.
  - The palette then runs the pick, or enters its next stage, itself (`palette.rs:476-482`), not through `run_bound`.
- So the same action, where it doesn't apply, opens from its key but is a dead row in `?`.
- One judgment of "applies" (on `Bound`, or one function the surfaces share) and one run policy (`run_bound`) would remove both splits.

## 5. The `?` list's mode keys are app state that outlives the opening; the sheet is built every frame while closed

- `ShellCommand::Keys` stores the mode in `CockpitApp::sheet_mode` (`app/commands.rs:449`, `app.rs:304`). Nothing ever clears it. `sheet_of` reads it whenever it builds the sheet (`bars.rs:172-174`).
- So: open `?` while a search is being typed, close it, later open `:` and press Tab. "Searching: its keys" is listed though nothing is being searched.
- The mode belongs to the palette's opening. `Palette::open` resets everything else of an opening (`palette.rs:141-143`); this one piece lives on the app.
- `render_palette` builds the sheet every frame, open or closed (`app/draw/panels.rs:48-49`): `situation()`, a `When` judgment per action, the layer titles, the pointer strings. `Palette::render` then returns at once when closed (`palette.rs:315-317`). The key-sheet window it replaced did this work only while open.

## 6. A second long-press recognizer, and the gestures said apart from where they're handled

- `actions::tip_on` (`actions.rs`, added in step 3) uses egui's `Response::long_touched`. That fires after egui's `max_click_duration` (0.8 s), not after the cockpit's `[touch] long_press_ms = 550` (`config/default.toml:39`).
- `touch.rs` says the host runs one recognizer for the whole cockpit, with "thresholds from `[touch]`".
- A long press on a control inside a panel runs both:
  - `touch.rs` emits `Gesture::LongPress` at 550 ms, and `on_gesture` (`app/draw/host.rs:256-296`) focuses the panel and hands own-pointer views the gesture;
  - egui shows the tip at 0.8 s.
- The `?` list's "Pointer and touch" section is a hand-written list in `bars.rs:175-177`. It still says a long press is "what can be done with it (the palette)" and doesn't name the tip step 3 added. Gestures are described in one place, handled in two others.

## 7. The pick list learns a row's kind by building the whole row

- `PickList::show` calls the `row` closure to learn only `kind`:
  - `pick_list.rs:136-137`: skipping a heading;
  - `pick_list.rs:153`: every row of the list, on each key move, to find the stops;
  - `pick_list.rs:181`: hover;
  - `pick_list.rs:197`: Enter.
  - It calls it once more per visible row to paint (`pick_list.rs:186`).
- In the action list the closure is `Listed::row` (`sheet.rs:147-163`). Each call formats `bound.ways().join(" · ")` (with `?`) or `key_hint()`, plus `format!("Needs …")` for a faded row.
- So:
  - a key move in `?` formats the keys of every action listed (the whole list, not the rows in sight);
  - a pointer move builds each visible row twice.
- A row's kind is structural (`Line::Title`, `Item::Inform`) and cheap to ask. The list's interface mixes "what is this row" with "draw this row".

## 8. "A panel not focused says no keys" is done by host code rewriting `Bound`s; `gather`'s `keyed` is dead

- Both callers of `gather` now pass `keyed = true` (`app/commands.rs:155`, `167`). The `false` branch (`commands.rs:222-224`) can't run.
- The job moved to `app/draw/host.rs:31-40`, which reaches into each `Bound` and blanks its `keys` and `leader`.
- `hosted::TopLine` has `focused` and `keyed` (`hosted.rs:125-130`). The one caller always sets them equal (`host.rs:47`).
- The test counter in `gather` is now tied to `here == self.here()` (`commands.rs:180`) so it keeps counting the same thing. That's test plumbing following the dead parameter.

## 9. A search's life across openings is handled in three places (re-read responsibility 4 only partly holds)

- `@`: `app/linking.rs:179-183` clears its search on every frame the picker isn't open.
- The graph's find:
  - `graph/actions.rs:252` clears it only on a pick. Escape leaves its round and rows;
  - `find_choices` is still built in every gather (`graph/actions.rs:110`).
- The palette makes up for the rest: `rows_of` and `answered` (`palette.rs:50-55`, `510-515`) hide rows until they're of what's typed in this opening.
- So "a search opened afresh starts empty" is handled per user (on close; on pick), plus a guess in the palette from the provider's state. That's the kind of palette-side inference the checkpoint removed (`rows_of_nothing`). The re-read asked for one place, so that neither user has to remember.

## 10. Who has the keys is now partly decided by egui's popup state

- `app/keys.rs:84`: `InputMode::Normal if egui::Popup::is_any_open(ctx) => {}`. The shell stands aside for any open egui popup, not for a mode it owns.
  - Any popup does it (another `ComboBox`, a context menu).
  - Nothing in `InputMode`, the key bar or `mode_keys` (`bars.rs:31-43`) knows the setting list has the keys.
- The setting control reads the list keys itself (`actions.rs:788`, `keymap::local().list`). The palette gets them through the `KeymapConfig` it's handed (`palette.rs:331-333`). That's two sources for one table.

## 11. The view menu's name is decided twice; the `?` and `:` search doesn't know it

- The menu's path and its entries use `group_label` (`app/draw/which_key.rs:7-12`): "All {focused view} actions".
- The top-line button uses `groups_of` (`which_key.rs:29-31`): "{View} actions", from `here.kind`, with its own case-folding. That copies the casing code of the removed `tabs()`.
- `Listed::haystack` (`sheet.rs:110`) asks the config's `group_label`, which has no entry for `,` (`config/default.toml:84-102`). So typing the view menu's name in `:` or `?` doesn't match its actions by group.
- `leader_title` (`which_key.rs:49-60`) is kept only for the top bar's mode label (`top_bar.rs:205`): a third way of writing the path ("LEADER > X", beside the menu's "Space › X ›").

## 12. The sheet's layer titles are matched by position

- `Sheet::layers: [String; 4]` (`sheet.rs:20`) is zipped with `sheet.rs`'s own `LAYERS` order (`sheet.rs:28`, `63`).
- The app builds the array in that order by hand (`bars.rs:170`).
- If a `Layer` is added or reordered, every section gets the wrong title silently. A title per `Layer` (a map or a function) keeps the coupling at the type.

## 13. What the action list lists is split between `sheet.rs` and the palette

- Step 2 made `sheet.rs` "what the action list lists".
- `palette.rs:458-470` still makes its own first row (`:N`, "Go to line N"):
  - it finds `nav.line` by its id string;
  - it shifts the sheet's lines by `first` for both drawing and picking (`palette.rs:463-476`).
- A row added by the sheet, or a change in how lines map to entries, has to be matched in the palette's offset arithmetic.

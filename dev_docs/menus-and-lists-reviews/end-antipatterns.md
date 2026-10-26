End review of the menus-and-lists plan (eda7934, a7f349c, c464f49, 856f36c, 17d4757, 03c4a3e), looking for anti-patterns and bolt-ons, most severe first. Paths are under `meta-harness/crates/cockpit-ui/`. I read the code only; nothing was built or run.

1. **Top-line button placements are still declared, but nothing draws them any more** (dead code and dead config; on touch, the tap targets are lost).
   - Step 3 changed `hosted::header` (`src/hosted.rs:139-180`) to draw only `At::Notice` buttons and `At::Header` + `Style::Setting`. Only the top bar (`src/app/draw/top_bar.rs:81-82,150`) still draws `At::Header` + `Style::Button`. These view placements are now dropped without any warning:
     - `src/graph/draw.rs:876-890` (`placed`): `nav.back` and `nav.forward`, plus the touch/narrow-screen chips. The doc comment above it still says "On its top line: the jump list's back and forward, and … the chips".
     - The whole chips setup feeds only that dead placement: `[graph.chips]` in `config/default.toml:715-720` (`show`, `narrow_below`, `size`, `actions`), `graph_config::ChipMode` and its `size` validation (`src/graph_config.rs:11-21,84`), `chips_shown` (`src/graph/draw.rs:388`), the "clear of the chips" note on the legend (`:720`), and `GraphState.fingers` as far as chips go. With a finger on the graph, fit, zoom, back, forward and flip no longer have a button. The user's rule is that touch is first-class.
     - `src/code.rs:1049-1055`: "Re-read it" for an out-of-date file. The spec's equivalent (`spec.reread`) was moved to `.notice(...)` in the same commit; the code view's was not.
     - `src/tree/mod.rs:469`: `tree.clear_filter`, still `.header(Style::Button, "", X)`.
     - `src/terminal/mod.rs:756`: `terminal.restart` ("Start again" once the shell has ended), still `.header(Style::Button, …)`. This is what-needs-you-now and has no button any more.
   - Fix:
     - Move each one to wherever it now belongs (a notice, or the view's menu only), and delete the graph's `placed` header entries and the chips config, type and code if chips no longer exist.
     - Make the combination unrepresentable. `At::Header` currently means two places, "a panel's settings" and "the top bar's buttons" (see its doc at `src/actions.rs:129-131`). Split off the top bar's own placement, or have `Action::header` take only `Style::Setting`, so a view can't place a button that silently never shows.

2. **`At::Notice` is a second placement beside the mechanism the plan chose for what-needs-you-now** (`src/actions.rs:132-135,454-458`, `src/hosted.rs:175-177`).
   - The plan's step 3 design (`dev_docs/menus-and-lists-plan.md`, "What needs you now, on what it's about") says the chat's banners become a spot of the chat's, `here()` names that spot, and their actions are item controls: the same mechanism that puts a request's answers on its card.
   - What was built:
     - A new host-drawn row under the top line.
     - Every former `.header(Style::Button, …)` call was renamed to `.notice(…)` wholesale.
     - The banners (`src/chat/render.rs:75-110`) are still not a spot, and `here()` (`src/chat/render.rs:381`) still names only the pending item.
   - So the buttons sit apart from what they are about, and the "notice row" is the old header-button row moved down a line. It now holds things that aren't notices at all:
     - the inbox's Previous/Next with empty captions (`src/inbox.rs:356-357`);
     - the queue's Start (`src/queue.rs:235`);
     - the graph's "Show N levels" (`src/graph/actions.rs:116`).
   - The plan was not updated to say the design changed, and its status calls step 3 done.
   - Fix: do what the plan decided. Make the banners a spot, have `here()` name it, and put their actions in `on_item`. Then delete `At::Notice` and `Action::notice`. If a whole-view button row is kept on purpose, record that decision in the plan and limit it to actions that really are about the whole view.

3. **The one code search has two drivers, two encodings for a picked row, and a one-frame workaround** (`src/code_search.rs`, `src/app/linking.rs:177-210`, `src/graph/mod.rs:389-413`, `src/graph/actions.rs:237-252`).
   - The per-frame driver is copied in two places: take the answers, `answer()` each one and say the note, `typed`, `tend`, say its note, send `net.search` for each of `kinds()` into an inbox, then `request_repaint_after(ROUND_LIMIT_S)` while `is_out()`.
     - `@` is driven by the shell, through a string-keyed side map (`desk.shell_searches["link.thing"]`), and notices it has closed with `palette.picking() != Some("link.thing")`.
     - The graph's find is driven from the graph's own `tick` through `ActionProvider::search`. That forced a second `tick` call site for views that aren't drawn (`src/app/draw/panels.rs:65-71`, `Desk::shown_now`).
   - A picked row is encoded two ways:
     - The graph uses the string `"{round}:{kind}:{index}"`, split and parsed back (`graph/actions.rs:246-250`). Its target is short-lived state, so `CodeSearch` keeps a second answered round, `before` (`code_search.rs:50-53,146-149`), only so a pick made in the frame the next round arrives still resolves. That is a one-frame workaround.
     - `@` puts the `Subject` as JSON in `Choice.value`, then finds the label by re-running `rows()` and deserialising every row's value (`linking.rs:271`). `rows()` reads only `shown`, so in the same race `before` covers for the graph, the `@` link gets the label "@something".
   - Fix:
     - One driver: `CodeSearch` takes the answers and sends its own rounds through a closure for the net call (per kind, with the caller's limits). It is cleared the same way, when the picker closes, for both pickers.
     - One value: a picked row carries what it stands for (the hit, or a `Subject` together with its label), so neither `before` nor the label lookup is needed.

4. **`sheet_mode` is state stashed on the app and never cleared** (`src/app.rs:302-304`, `src/app/commands.rs:448-452`, `src/app/draw/bars.rs:172`).
   - `?` copies `mode_keys()` into `CockpitApp.sheet_mode`. Nothing resets it.
   - Open `:` later and press Tab to see everything: it lists the keys of whatever mode the last `?` was opened from.
   - `mode_keys()` (`bars.rs:31`) is now called from this one place only. Opened from the palette (the "Keys for this buffer" action), it records "The palette" keys, which describe the list itself.
   - `open_palette` already records the mode it was opened from (`palette_from`, `src/app/commands.rs:328-331`).
   - Fix: `sheet_of` derives the mode's keys from `palette_from` (and the capture) when it builds the sheet. Delete `sheet_mode`.

5. **`Sheet` holds parallel arrays and tuples, and special-cases other views' actions** (`src/sheet.rs:18-28,55,63`, `src/app/draw/bars.rs:157-176`).
   - `applies: Vec<bool>` is matched to `actions` by index, with a silent fallback (`sheet.applies.get(i).copied().unwrap_or(true)`). A length mismatch lists everything as applying.
   - `layers: [String; 4]` is zipped with a separate `const LAYERS`, so the order lives in two places.
   - `Informs = (String, Vec<(String, String)>)` and `mode_keys`' `(&'static str, Vec<(String, &'static str)>)` are anonymous tuples copied into owned strings each frame.
   - `sheet_of` judges another view's actions with `Origin::View(kind) if kind != here.kind => when != Unavailable` (`bars.rs:162`). Step 3 added `situation_of(kind)` (`src/app/views.rs:44`), which judges any view's `When` properly. The special case should use it.
   - Fix: give `Sheet` a function `applies(&Bound) -> bool` (using `situation_of(origin's kind)`), a title per `Layer` (a match or a map keyed by `Layer`), and a small named struct for an informing key.

6. **"Everything" is a flag kept beside the stage and threaded through every function** (`src/palette.rs:47-49,168-172,372-375,393,458,551`, `src/sheet.rs:47,56-60,122,152`).
   - `Palette.everything` only means something in `Stage::Actions`. Opening it takes two calls, `open_palette(None)` then `palette.show_everything()` (`commands.rs:450-451`, again in the test helper `src/app/tests.rs:230-236`).
   - The Tab key gets its own branch in `render` (`palette.rs:372`) and its own key-line text, separate from the action `cycle` mechanism that the same key drives in the other stages.
   - `Listed` carries the same bool and branches on it in `of`, `lines` and `row`.
   - Fix:
     - Make it `Stage::Actions { everything }`, opened as `open_actions(everything)` (or a `Scope` enum).
     - Give `Listed` sections in both scopes: `:` becomes one untitled section, and a section with no title draws no heading. Then `lines` has no early return, and `row` picks the keys text from the section, not from a flag.

7. **The view menu's name is special-cased twice, with two different results, and the title-casing is copied** (`src/app/draw/which_key.rs:7-12,28-33`, `src/app/commands.rs:260`).
   - `group_label` returns `"All {focused_kind} actions"`. It reads the focused view, so it is wrong for any other panel, which is why `groups_of` special-cases the same key again with `"{Title} actions"`.
   - That title-casing (`&title[..1]`, `title[1..].to_lowercase()`) is a copy of the one in `palette_actions` (`commands.rs:260`). It also copies the `tabs()` code step 3 deleted.
   - The leader path's names are built twice: `leader_title` ("LEADER > SESSION", `which_key.rs:49-59`, still used by the top bar) and the which-key header's tappable path (`which_key.rs:75-76`), each looping over `group_label` itself.
   - Fix: one `group_name(path, kind)` that takes the view's kind, one helper for a kind's display title, and one path builder that both callers format.

8. **Patched flags around binding keys for panels that aren't focused** (`src/app/commands.rs:177-180,206-230`, `src/app/draw/host.rs:31-48`, `src/hosted.rs:122-130`).
   - `gather(here, keyed)`: both callers now pass `true` (`commands.rs:155,167`). The `keyed` parameter and its `else` branch (`vec![(None, Vec::new(), Vec::new()); …]`) are dead.
   - The test counter was patched to `keyed && here == self.here()` so other panels' gathers don't count.
   - Keys are then bound for an unfocused panel and hidden two different ways:
     - `render_view` blanks `b.keys`/`b.leader` on every `Bound`;
     - `TopLine.keyed` hides the group buttons' keys.
   - `TopLine.focused` and `TopLine.keyed` are always set to the same value (`host.rs:48`).
   - Fix:
     - Remove the `keyed` parameter.
     - Keep one `focused` field. `header` and `controls` read it to decide whether keys are shown, instead of the caller rewriting each `Bound`.

9. **The setting popup takes the keyboard through "is any egui popup open", outside the input modes** (`src/app/keys.rs:83-84`, `src/actions.rs:770-800`).
   - The palette owns the keys through `InputMode::Palette`. The setting popup reads the list keys itself from the global `keymap::local()`, keeps its `PickList` in egui temp data, and the app's key routing stands aside whenever `egui::Popup::is_any_open(ctx)`. That condition is true for any popup, not only this list.
   - Fix: the popup's list goes through the same mode as the palette's (an input mode, or the palette opened at that action's `Choose` stage, anchored at the control), not a second ownership check.

10. **Whether a faded row can be run is a second copy of `run_bound`'s rule, with a different condition** (`src/sheet.rs:157`, `src/palette.rs:466-474`).
    - `run_bound` (`src/app/commands.rs:341-346`) refuses to open a picker for `When::Unavailable` and says why.
    - The `?` list instead decides per row: it can be picked if it applies or if `param == Param::None`, otherwise it informs only. The palette then builds the `Invocation` directly, so `run_bound`'s check and note never run for it.
    - Fix: a picked action row goes through `run_bound`'s check, which says why it can't run. Every action row stays `RowKind::Pick`, and `Inform` is kept for keys that run nothing.

11. **`answered` hides rows left over from a search that was closed without a pick** (`src/palette.rs:50-55,510-515`).
    - The flag exists because the graph's find is cleared only on a pick (`graph/actions.rs:251`), not when its picker closes. A reopened find therefore shows the last session's rows until its first answer arrives.
    - `@` is cleared on close (`linking.rs:181-184`). The two clear differently, and the palette carries a "has this search answered since it opened" boolean to cover the gap.
    - Fix: clear every search when its picker closes (part of 3). Then `answered` goes, and `rows_of` alone decides when the highlight resets.

12. **Comments and leftovers that no longer match the code.**
    - `src/app/draw/host.rs:1-8`: the module doc still says "the host's own (tabs, closing, sending the quick look's card to a panel) on the host's line". Those are gone.
    - `src/hosted.rs:1-7`: still says "its header buttons first … its settings on a row under". It doesn't mention the groups or the notice row.
    - `src/graph/draw.rs:876-879`: see 1.
    - `src/pick_list.rs:203-207`: `go`'s doc speaks of "`len` rows". The parameter is now `rows`, and the two paragraphs read as one.
    - `src/app/tests.rs:229-230`: `sent`'s doc line ("The calls sent to the server since last asked") now sits on top of the new `everything` helper, and `sent` has none.
    - `src/chat/tests.rs:293-295`: the old doc line on `on_top` is left above the new one.
    - "Key sheet" is named but gone:
      - `src/actions.rs:5`, `:215`, `:316`;
      - `src/queue.rs:238`;
      - `src/remote.rs:43,466`;
      - the `nav.keys` action, "Keys for this buffer … on one sheet" (`src/app/commands.rs:96`), which now opens every action everywhere.
    - `src/spec.rs:908,923,933` (the `spec.pin` ×2 and `spec.mark_reviewed` builders): the removed `.header(...)` lines left blank lines of trailing whitespace inside the builder chains.
    - `src/actions.rs:819-850`: the new test is placed above `use super::*;` in the tests module, after an empty line.
    - `src/hosted.rs:175-176`: `placed(At::Notice, Style::Button)` filters on a style that `Action::notice` always sets.

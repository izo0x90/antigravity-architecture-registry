# End review: consistency (anti-patterns), menus and lists (eda7934..03c4a3e)

Paths are under `meta-harness/crates/cockpit-ui/` unless they start with `README.md`. Ranked, most important first.

## 1. Views still place top-line buttons that no host draws any more (graph back/forward and touch chips, the code view's re-read)

- Step 3 changed `hosted::header` to draw only `At::Notice` buttons and `At::Header` settings (`src/hosted.rs:175-180`). The top bar draws only `Layer::Everywhere` header controls (`src/app/draw/top_bar.rs:150`).
- Two `Hosted::placed` implementations were left placing `At::Header, Style::Button`, so their buttons now show nowhere:
  - `src/graph/draw.rs:880-891`: the jump list's back and forward, and on touch or narrow screens the `[graph.chips]` buttons (`config/default.toml:715-720`). That whole config table now does nothing, and a phone loses the graph's on-screen fit, zoom, view and flip.
  - `src/code.rs:1048-1055`: "Re-read it" when the file is out of date with the index.
- The same state on the spec card uses the new row: `spec.reread` with `.notice("Re-read", None)` (`src/spec.rs:949`). The code view's equivalent should be the same (`At::Notice`). The graph's buttons belong in its menus, or in a placement a host still draws.
- `src/hosted.rs:73-76` still documents "the graph's back and forward on its top line".

## 2. The action list goes by six names on screen, and its two actions describe each other's job

- `src/app/commands.rs:38`, `palette.open` (`:`): "Command palette", described as "Every action there is here, found by typing". Since step 2, `:` lists only what applies; everything is `?`.
- `src/app/commands.rs:96`, `nav.keys` (`?`): "Keys for this buffer", "Every key here and what it does, on one sheet". `?` now lists every action everywhere, and the sheet is gone.
- Other names on screen:
  - the top bar's "Actions" (`src/app/draw/top_bar.rs:106`);
  - the mode badge "PALETTE" (`top_bar.rs:206`);
  - the key bar's mode name "The palette" (`src/app/draw/bars.rs:40`);
  - the `?` pointer row "what can be done with it (the palette)" (`bars.rs:176`);
  - the list's titles "Run an action" and "Every action and key" (`src/palette.rs:396-397`).
- `README.md:715-716` lists "the action list (`:` and `?`)" and "the palette" as two separate things. `README.md:732` and `README.md:843` say "palette".
- Config and module docs say the same: `config/default.toml:43` ("the palette, its pickers"), `src/keymap/local.rs:2,23`.
- The README's word since step 2 is "action list". Use it in the labels, the descriptions, the badge and the mode name.

## 3. Two key lines describe the same list at once, in different words, and only one says Tab

- While the list is open, the key bar shows the mode's keys from `ListKeys::keys(&back, "closes")` (`src/app/draw/bars.rs:40`), for example "Escape closes".
- The list's own footer (`src/palette.rs:543-563`) builds the same set with "goes back", and adds "Tab: everything" / "Tab: what applies here", which the key bar never says.
- `ListKeys::keys` (`src/keymap/local.rs:75-87`) is meant to be the one place a list's keys are said. The `:`/`?` switch should be said there, so both lines read it.
- The cycle key's own words still describe only the old meaning, "While a value is picked: what it's for, the next of its kinds": `src/keymap/local.rs:36-37`, `config/default.toml:52-54`.
- `README.md:843` doesn't mention the switch either.
- Smaller: the footer says "Enter runs" on rows that only inform (`?`) and on choices, while the README says "`Enter` or a tap picks" (`README.md:677`).

## 4. A second long press, beside the cockpit's own, mostly redoing what egui does

- `actions::tip_on` (`src/actions.rs:691-711`) keys on egui's `Response::long_touched()`. That is egui's own threshold (`max_click_duration` 0.8 s, `max_click_dist` 6). The cockpit's long press is `crate::touch`, with `[touch] long_press_ms = 550` and `tap_slop = 12` (`config/default.toml:36-41`, `src/app/draw/host.rs:233`). One finger, two definitions of a long press: the button one is neither configurable nor the same length.
- egui already shows a widget's tooltip while a finger is held on it (egui 0.33 `containers/tooltip.rs:381-387`: "we do want tooltips while holding down on an item on a touch screen").
- What `tip_on` adds is the 2.5 s linger after lifting (`LONG_TIP_S`), which isn't in `[touch]` either.
- The existing "a long press says it" pattern is the gutter's: a `touch` long press, said in the key bar's note (`host.rs:285-288`, `src/app/notes.rs:314`).
- The README claims more than is wired. `README.md:702` says "a long press on any button says what it does and its keys", but:
  - the key bar's buttons use plain `on_hover_text(&label)` (`bars.rs:97`);
  - a which-key entry's label uses plain `on_hover_text(detail)` (`src/app/draw/which_key.rs:136-139`);
  - the path parts and the Escape button have no tip at all (`which_key.rs:87, 91`).

## 5. Group buttons are a second kind of button, outside "every button is an action"

- `hosted::group_button` (`src/hosted.rs:186-194`) hand-builds `draw_control`'s caption-and-key job (`src/actions.rs:738-757`).
- It drops that pattern's rule of showing the key only when there's room (`KEY_SHOWN_ABOVE`): a group's key always shows, however narrow the panel.
- It shows the bare key ("Account  a"). On a focused panel, `a` alone runs the view's own key (`discussion.settle`, `queue.approve`, `config/default.toml:351, 357`), not Account. `README.md:713-717` says "Every button is an action … so a key is shown wherever it runs and runs what it shows". The plan chose "Account a", so either the README's rule or the button's key has to change.
- Tips are written three ways:
  - controls: "Label (Space a c)" then the detail (`actions.rs:727-729`);
  - groups: "Account: its actions (Space a)" (`which_key.rs:35`);
  - which-key entries: label then detail, with no key (`which_key.rs:131`), though the README says the tip gives its keys.

## 6. The notice row's rule ("what needs you now, or is happening now, acted on at once") is applied unevenly

- **Inbox, Previous and Next** (`src/inbox.rs:356-357`): moves, not something that needs you. Every other view's item steps (`terminal.next`/`prev`, `chat.next_slot`, `spec.next_section`, all `J`/`K`) are only in the menus.
- **Inbox item actions split across two rows.** A request's answers go on the item (`inbox.rs:366`, `answer.place(…, here)`, `At::Item`). "Open it", "Open its chat", "Open its terminal" and "Put away" go on the notice row (`inbox.rs:369-375`). The plan's own example is "a request's answers on its card".
- **Discussion: a row that never goes away.** `discussion.settle` is a notice even when `When::Unavailable` (`src/discussion.rs:426-429`). `actions::control` draws it greyed (`src/actions.rs:647-651`), and `header` doesn't skip unavailable controls. `discussion.reopen` is a notice whenever it's not open (`discussion.rs:432-435`). So every discussion has a notice row, not one "while there's something".
- **Queue Start** (`src/queue.rs:235`): shown whenever the queue isn't running or done. That is a standing action.
- **The code view's out-of-date re-read** isn't on the row at all (see 1). The spec's is.
- **Chat: the buttons are not with their banners.** The banners now only say things, and their buttons sit on the panel's notice row (`src/chat/render.rs:75-77`, `src/chat/actions.rs:73-298`).
  - The plan says those buttons stay on what they're about. Decided 1: "a sign-in's buttons on its banner". Step 3: "the banners are a spot of the chat's … their actions are item controls".
  - That is the existing pattern for a request's answers (`request_card::Answer::place`, `At::Item`).
  - The plan and `README.md:698-700` now describe two different designs.

## 7. The view's own menu, and "where it acts", are named four ways; `?` groups by a different set than the menus

- The same menu (`Space ,`) is named:
  - "Graph actions" on the top line (`which_key.rs:29-31`). Its hand-written capitalising duplicates `src/app/commands.rs:260`.
  - "All graph actions" in the which-key path and entry (`which_key.rs:9`).
  - "GRAPH" as the which-key's own heading (`which_key.rs:101`).
  - In `?`, its section is the view's status title, for example "<file glyph> lib.rs · Navigate" (`bars.rs:170`, via `Hosted::title().text`).
- The `:` matcher reads the config's group labels (`src/sheet.rs:937`), so typing "graph actions" matches nothing in that menu.
- `?` sections are action `Group`s, "Navigate / Go to / Act / View" (`sheet.rs:890-899`). The menus group by the leader's groups (Buffer, Session, Account…). Plan decision 5 is "browsing (menus) and searching (the list) read one set" and show it the same way.

## 8. The leader path is written two ways on screen

- The which-key menu: "Space › Account ›" (`which_key.rs:76-86`).
- The mode badge in the top bar at the same moment: "LEADER > ACCOUNT", from the older `leader_title` (`which_key.rs:48-60`, `top_bar.rs:205`).

## 9. Notice captions rename the action they run

- "Connect" runs "New session" (`src/chat/actions.rs:73`). The Session menu and `:` say "New session".
- "Cancel sign-in" runs "Stop signing in" (`chat/actions.rs:281`).
- The same "put it off the list" act is "Dismiss" in the chat (`chat/actions.rs:289, 292, 298`) and "Put away" in the inbox (`src/inbox.rs:375`).
- With the notice row and the menus side by side, one action reads two ways (plan decision 5).

## 10. `?`: Enter on a faded row that takes a value does nothing and says nothing

- `sheet.rs:984` makes a non-applying action with a parameter `RowKind::Inform`, and `pick_list.rs:196` ignores Enter on it.
- The pattern for a key that can't run is to say why in the note: "Nothing to go back to." (`src/app/commands.rs:443`).
- The checkpoint findings ("Recorded, not changed") said step 2 would "say why when Enter lands on a row that doesn't apply".

## 11. The spec's pin lost its state, and the edit left blank lines

- The "Pinned"/"Pin" button was the only place the pinned state showed. It was removed (`src/spec.rs:905-910, 920-925`), and the title and status don't say "pinned".
- The plan's rule is that the settings row stays because each setting shows a state. The pattern is `Param::Toggle` with `.header(Style::Setting, …)`, as `code.deleted` (`src/code.rs:965`), `changes.timeline` and `graph.only_changed` do.
- Deleting `.header(…)` left whitespace-only lines inside builder chains (`spec.rs:908, 922, 933`).

## 12. The panel's top line lost its inspector region and its hover

- 17d4757 removed `register_inspectable(…, "panel_N:header", …)` and the number's hover "Panel N: kind" (old `src/app/draw/panels.rs`).
- `hosted::header` registers nothing in their place.
- Each region registering itself is the pattern: `src/tree/mod.rs:357` (`{host}:tree:header`), `src/app/draw/top_bar.rs:135-190`.

## 13. "The keys of the mode you were in" in `?` is written but never reached

- `sheet_mode` is taken only when `ShellCommand::Keys` runs (`src/app/commands.rs:448-452`).
- That happens from normal or leader mode, where `mode_keys()` is `None` (`bars.rs:31-42`). In typing, search and the list itself, `?` is typed, not run, and Tab from `:` doesn't take the mode.
- So the section `bars.rs:172-174` builds doesn't show, yet `README.md:672` and the `?` doc comments promise it.

## 14. Comments still describe the removed sheet, tabs and buttons

- `src/app/draw/host.rs:1-8`: "the host's own (tabs, closing, sending the quick look's card to a panel) on the host's line".
- `src/actions.rs:5, 113, 215, 316, 527`, `src/app/commands.rs:146`, `src/queue.rs:238`: "key sheet" / "the `?` sheet".
- `src/app/draw/panels.rs:1`: "and the palette".
- `README.md:666` "leader menu" and `README.md:701` "which-key menu" name one thing two ways, two paragraphs apart.

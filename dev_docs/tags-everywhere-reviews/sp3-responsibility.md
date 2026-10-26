# Stopping point 3: responsibility reviewer

Review of d4c5b67..HEAD for separation of responsibility and coupling. I only read the code; I built and ran nothing. Ranked, most severe first.

**1. The shell lays out one buffer's buttons, beside the existing placement mechanism.** `crates/cockpit-ui/src/app/tagging.rs:330-333,373-375,399`
- `tag_edits` works out "this is a tag's own row" as `view.tag_at(k).is_some() && view.things(k).at.is_empty()`. It then gives the shell's tag edits `on_item(Style::Button, …)` there, and `untag_action` does the same for "Take off".
- That logic depends on how `TagList` happens to work. It is the only view whose `tag_at` returns something, and the only one whose tag rows have no things.
- `Hosted::placed()` (`hosted.rs:73-79`, used by graph, code and spec, collected in `commands.rs:222`) already lets a view give controls, at Item or Header, to actions by id, the shell's included. `TagList` could return `("tag.rename", Control::new(At::Item, Style::Button, …))` and the rest the same way.
- So two ways now decide where a shell action's button goes. Layout for the Tags view is decided in the shell, which contradicts F.2 ("declares its actions (the host draws their buttons)") and the module doc in `tag_list.rs:1-9`.
- What it triggers: any future view that returns `tag_at` and an empty `things` on some row gets five edit buttons under that row, whether it wants them or not.
- Severity: structural, medium.

**2. The shell decides "Name a tag" is available without asking whether the view has a typing box.** `app/tagging.rs:199-207,270-274`, `nav.rs:767`
- Availability is `typing = Insert{..} || palette.is_open()`. While the palette is open that is always true, so "Name a tag" shows in every view's palette once any tag exists.
- Running it calls `typed_in`, whose default does nothing, then forces `input_mode = InputMode::Insert { at: here.at }` anyway.
- Sequence: in a terminal, open the palette, pick "Name a tag", pick a tag. Nothing is typed. The terminal is now in Insert mode with `Field::Keys`, so the next keystrokes (`j`, `k`, …) go to the shell process.
- In the Tags or code view it enters Insert mode with no field to type in, and only the back key gets the user out.
- Severity: medium; it's a visible wrong action.
- The existing `Hosted::typing_rect()` / `field()`, or a capability the buffer declares, should decide this, not the shell's guess.

**3. The chat buffer names a shell action by its string id.** `chat/render.rs:607-609`
- `self.pending_palette = Some("tag.insert".to_owned())` goes through `take_palette_request`. That hook's doc (`actions.rs:520`) and its earlier use (`chat/session.rs:67`) are for a buffer's own actions.
- The chat buffer now knows a shell action's id, and it decides which character triggers tags, while the shell decides what gets inserted.
- If `tag.insert` is renamed, `#` silently stops working: `render_palette` finds no action by that id (`draw/panels.rs:58-62`). No compile error and no note.
- The way the brief describes for a buffer to reach past itself is an `Ask`, and there is none for "open the tag picker".
- Severity: low to medium (silent breakage later).

**4. Text is written into the chat's string, not through its text field.** `chat/nav.rs:76-79`, `app/tagging.rs:271`
- `typed_in` does `input_buffer.push_str(text)` directly. egui's `TextEdit` keeps its own caret in its saved state, and nothing here moves it.
- Likely result: after "look at #" plus picking `flaky`, the caret stays just after `#`. Typing `x` then gives `#xflaky `.
- I'm not sure of this. The test (`app/tests/tags.rs:438-452`) only checks the buffer string. To confirm: type `hi #`, pick a tag, type one character, and check where it lands.
- Severity: medium if real (it corrupts the prompt).

**5. Tag editing guesses the palette's internal counter.** `app/tagging.rs:313,485`
- `TagEditWhich` stores `palette.opening() + 1`, and `tag_to_edit` accepts `opening` or `opening + 1`. This assumes `run_by_id → open_palette → Palette::open` adds exactly 1 to `opened` (`palette.rs:70-77`).
- The tag being edited is not carried in the invocation's value. `TagEdit` works it out again at run time from cursor state and `desk.tag_editing`.
- Sequence: pick "Rename a tag here", pick A, press Escape. `tag_editing` keeps `(N+1, A)`, and while the palette stays closed at opening N+1, A still counts as picked.
  - Nothing uses it today (no keys are bound to `tag.*`, and edit buttons only show where there is one tag).
  - It becomes live as soon as a key or button for `tag.rename` exists. A confirmed edit would then do nothing with no note, because `tag_to_edit` returns `None` and `run_tag` just returns (`:497`).
- Severity: low now; this design breaks easily.

**6. Which tag to take off is decided twice.** `app/tagging.rs:385-400` (`untag_action`) and `:460-474` (`run_tag` Untag)
- Each works out "the row's tag, else the only tag there" on its own. They agree today, but a change to one leaves the label ("Take A off this") and the actual act out of step.
- Severity: low.

**7. Two trait methods have to agree.** `nav.rs:756-779`
- `drawn_at` and `tag_gutter` must match. The old chip drawing worked for any view with `drawn_at`. The new gutter (`draw/host.rs:174-189`) only draws for views where `tag_gutter()` is true.
- A chat or code owner that implements `drawn_at` but forgets `tag_gutter` has its tags drawn nowhere, silently. Only the terminal implements either today.
- Severity: low (risk for the next owners the plan adds).

**8. The generic picker draws every colour as a tag.** `actions.rs:52-55`, `palette.rs:750-752`
- `Choice.colour` is a general field, but `palette_row` always draws it as `theme::tag_mark`, a filled tag glyph.
- Any future coloured choice that isn't a tag would show a tag icon. The plan asks for "a swatch before its name, in every picker".
- Severity: low.

**Areas with nothing real found:**
- `hosted::header` taking the cursor's tags: the host draws them, as rule 4 says.
- `theme.rs` loading the filled fonts and `tag_mark`.
- `terminal/mod.rs` `drawn_at`: it stays inside the terminal.
- `tag_list.rs` fold, enter and leave: internal to the buffer, through the shared `Fold`/`Move`.
- `nah-tags/src/logs.rs` tightening the folder's permissions on open: correct layer, no other part reaches in.
- No server part reaches into another's state, and the UI decides no server policy in this diff.

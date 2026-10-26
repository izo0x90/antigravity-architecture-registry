# Chat checkpoint after steps 2-3: correctness (6dd0d2f, 32dc365)

Ranked, most serious first. Paths are under `meta-harness/crates/cockpit-ui/src/`.

## 1. Ten seconds after the last update, the live line says "No word for N min", where N is the turn's age

`chat/live.rs:27-28`, `:35`.

`note_updates` drops every update older than `TRACE_S` (10 s). `activity()` takes `last = updates.back().or(running_since)`. Once the queue is empty, "the last word" becomes the moment the turn started.

Sequence: a turn has run for 5 minutes, with updates every few seconds. The agent then goes quiet (a long build, say).
- From 1.5 s to 10 s after the last update: "Running a command… 9 s since anything", which is right.
- At 10 s the last update is pruned, so `since_last` is about 300 s. That is ≥ `QUIET_S`, so the line turns amber and says "No word for 5 min".

The waiting state only ever lasts from 1.5 s to 10 s. After that the line reports the turn's age, not the silence. For a turn under a minute old, the seconds shown count from the turn's start.

The test `a_running_turn_says_what_its_doing_and_how_live_it_is` can't see this. Its only update arrives in the same frame the turn is first seen running, so `running_since` equals the last update's time.

## 2. A call that fails after you opened it, or opened its run, folds shut

`chat/rows.rs:109-117`.

Folded or open is `default XOR toggled`. When a call's status turns `Failed`, its default flips to open, so the toggle you set now means *folded*.

Sequences (normal):
- A run of three calls is folded. You press Enter on it (toggle stored on `i1`), then `j` to the third call, which is still running. It fails: `run_open` is `true != true`, which is false. The run collapses back into one line, and the failed call is hidden.
- You open a running command's output to watch it (`z o`, which toggles its id). The command fails, and its output folds shut.
- `z R` (open everything) stores toggles for every run and output. Every later failure folds that call and its run.

This breaks "a failed call stays open" (Decided 3). In the first sequence the cursor is also left inside a folded run on an index that isn't a row (see 3).

## 3. The cursor can sit on an item that is no row: nothing is highlighted, and no buttons or hints are under it

`chat/actions.rs:476-477`, `chat/nav.rs:192-204`, and 2 above. The consequences are in `chat/render.rs:130`, `:304` and `app/draw/host.rs:143-145`.

Rows, highlights and spots are keyed by `row.first`, and `cursor_key()` is the cursor's item index. Only `fold()` snaps the cursor back to its row (`nav.rs:279-284`). Three other paths leave it inside a folded run:
- **Changing the verbosity.** In normal, the cursor is on call 5 of an open run (or on a lone call). You set "Shows: quiet": the folds are replaced wholesale and the cursor stays at 5, now inside the folded run 3..6.
- **`restore`** (Backspace or the jump list, or a restored layout). A saved place on call 5, which you had reached through an opened run, comes back after the run has been folded (by `z M`, a verbosity change, or a reload, which starts with fresh folds). `cursor = 5` is set as is.
- **A default flipping under a toggle** (2).

Then:
- `item_frame` fills nothing, because no row has `first == 5`.
- No spot "5" exists, so `host.rs:145` finds no rect. The `fold_here` button, the Enter or Open controls and the cursor's anchor for hints all vanish, exactly where the fold button is needed.
- The `/` search status (`app/capture.rs:181`) still counts from item 5. The search moves from a place you can't see.

## 4. Jumping into a folded run scrolls by the old, folded layout's stops, and never corrects

`chat/render.rs:346-359`, `viewport.rs:302-308`.

`put_cursor` opens the run, recomputes the rows, and asks `scrolling.show(row)`, where `row` is an index in the *new* rows. On the next frame `Measured::area` applies it straight away, using last frame's stops (the folded layout) whenever `row < stops.len()`, and clears `show`.

Sequence:
- The rows are `[0 prompt, 1 answer, 2 run(2..7), 3 answer(8, 400 px), 4 prompt(9), 5 answer(10, 600 px), …]`.
- A `/` search (or `f` on a link, or a tag's `show_target`) lands on item 5 inside the run. The run opens, and item 5 is now row 5.
- The next frame centres old stop 5, which is item 10's 600-px answer, far below where item 5 now is. The cursor's item is off screen.
- `show` is spent, so nothing scrolls it back.

The tests check rows and cursor, never where the view scrolled after a jump that opened a run.

## 5. The live line widens the stream past the panel: rule 1 broken again

`chat/render.rs:315-329`, `chat/live.rs:87-88`.

The turn's line is not laid out in an item box (`item_frame`). In the turn line's `ui.horizontal`, the words are a `truncate()` label. When they are longer than the space left, the label takes the whole remaining width. Then `allocate_exact_size(64×12)` for the indicator's spot, and the "N files changed" label, are placed past the right edge. egui's region grows its `max_rect` to include them, so every row after the running turn's line gets a wider `available_width` (`item_frame`'s `set_width`). The rows are laid out about 70 px or more wider than the column and are cut off at the panel's edge.

Sequence: a 300-px panel. "Turn 12" + "Reading parse_long_module_name.rs… 34 s since anything" fills the line, and the running turn's items (the ones you are watching) drift right.

The step-1 width test covers items only, not a running turn's line.

## 6. In an open run where every call has output, no fold key folds the run again

`chat/rows.rs:177-180`, `chat/nav.rs:271-276`.

`at()` gives an item's own fold before its open run. On a call with output (every Read, Search and Execute that returned something):
- `z c` with the output folded does nothing (`(Open) != open` is false).
- `z a` only toggles the output.
- The `fold_here` button reads "its output".

Sequence: normal. Enter on "Read 3 files" opens it, and you `j` onto the second read. `z c` does nothing, `z a` opens its output, and `z c` folds the output again. The run stays open unless you `z M` (which folds everything) or find a call without output. In "everything" mode runs are open by default, so no run of reads can be folded one at a time at all. This contradicts "each folds and unfolds on its own" (Decided 3).

## 7. The liveness is counted per view and only while drawn, not from when updates arrived

`chat/live.rs:18-31`.

`updates`, `seen_activity` and `running_since` live in the `ChatView` and change only in `draw`. Nothing ties them to the thread.
- **A fresh chat opened mid-turn.** `seen_activity` is `None`, so nothing is pushed, and `running_since = now`. The line says "Receiving" ("Reading x…") for 1.5 s and then counts the seconds from when you opened the chat. This happens even though the agent may have said nothing for 5 minutes.
- **Coming back to a chat.** The chat sat in an unshown panel for 3 minutes, and one update came at minute 1. On the first frame back the count differs: one blip is drawn *now*, it says "Receiving", and then it counts from now.
- **Several updates between two frames** count as one blip. That always happens while not drawn, and during bursts. "Blips once for each update" (Decided 5) does not hold.
- **Switching the slot in the same view** (`show_target` with `Target::Agent`, or a slot picker). The lead's turn has run for 5 minutes and worker-1's for 3 s. The switch draws a spurious blip, because the two threads' counts differ. `running_since.or(Some(now))` keeps the lead's start time, so with 1 above the worker's line soon says "No word for 5 min".
- **A reconnect.** The resync replays the thread's events, and any event bumps `activity`, including the echo of your own answer (`RequestUpdated`) and title changes (`ThreadUpdated`). Each reads as "Receiving" from the agent.

## 8. With a turn queued, the live line describes the queued turn

`chat/live.rs:20`, `:49`; `store.rs:36-37`.

`running_turn()` is the *last* turn that is `Queued` or `Running`. `doing()` reads that turn's items, but the live line is drawn on the running turn's line.

Sequence: turn 3 is running ("Reading render.rs"), and you send a prompt, which queues turn 4. Turn 3's line now says "Starting…" (turn 4 has no items yet) or "Working…" (its latest item is your message). It stays wrong until turn 3 ends.

Also, `running` stays true from turn 3 into turn 4, so `running_since` is never reset. Through 1, turn 4's quiet spells are reported against turn 3's start.

## 9. Repaints are asked for while the live line isn't shown, and in a long turn it isn't shown

`chat/live.rs:90-97`, `chat/render.rs:286-293`.

`render_live` runs inside the stream's `ScrollArea` closure, which lays out every row, on screen or not. It asks for a frame every 0.1 s (heartbeat) or 1 s whether or not the turn's line is in view.

The line is drawn at the top of its turn. In a long turn, while you follow the latest, that top is scrolled out of view. The one live line is out of sight for most of the turn (the plan's live check, "a long agent turn"), and the 10 Hz repaints continue the whole time.

The plan says "drawn only while it's shown" and "not at all while the panel … is hidden"; this does neither. Also, `next` starts at 1.0, so `Next::OnChange` is never honoured: the chat repaints every second while quiet, while disconnected, and with "none".

## 10. The heartbeat's breathing dot is drawn outside its own clip: never visible, but it keeps 10 Hz repaints

`indicator.rs:204-210`, `chat/live.rs:93`.

The spot painter is `ui.painter_at(spot)`, which is clipped to the 64×12 spot. The dot is centred at `rect.right() + space_sm` (6 pt) with a radius of `line_bold + breath`, at most 3 pt. All of it lies outside the clip.

While waiting with no blip in sight, the heartbeat returns `In(0.1)` only for that dot: 10 frames a second for nothing visible. The unit test checks the `Next` values only, not that anything is drawn.

## 11. The indicator's stage is not what the interface says

`chat/live.rs:92-93`.

- `Stage.items` is `item_rects.clone()` at the moment the turn's line is drawn. `item_rects` was cleared at `render.rs:71`, so it holds only the rows *before* the running turn, including the ones scrolled off screen, and none of the running turn's own rows. An indicator that walks "along the messages" (the lemmings) gets the wrong set.
- `over` is `ui.painter()` of the turn line, on the same layer and painted before the rows that follow. Those rows, and the selected row's fill, paint over it, so it is not "a see-through layer over the panel".
- `render_live` runs once per non-terminal, non-queued turn line. If two turns are shown as running (a terminal update missed, with a newer turn started), `draw(&mut self)` is called twice a frame with two stages. That breaks any indicator that keeps state.

## 12. "Quiet too long" doesn't say whether the server can still reach the agent

`chat/live.rs:84`.

The plan (Decided 5): "no word for a long while … and whether the server can still reach the agent." `Liveness::Quiet` says only "No word for N min". Nothing reads the slot's or instance's status (`SlotStatus`, `ProviderStatus`) to say whether the agent is reachable.

## 13. A call you opened folds away when the next call arrives

`chat/rows.rs:139-143`.

In normal, a lone call is its own row, and you open its output (toggle on its id). When the agent's next call arrives, the two become a run, which is folded by default, so the output you were reading disappears into "Read 2 files".

While following a running turn this repeats with every call. Items arriving into a folded run hide what was opened by hand.

## 14. The new tests don't protect what they claim

- `how_much_the_chat_shows_is_its_setting` (`app/tests/chat_view.rs:235-250`):
  - It asserts `quiet <= normal`, which passes even if quiet changes nothing.
  - Its doc says "switching starts what was folded by hand afresh", but it never folds anything by hand.
  - `set("normal") == normal` holds trivially.
  - The cursor after a switch (3) isn't checked.
- `a_running_turn_says_what_its_doing_and_how_live_it_is`: its only update arrives in the same frame the turn is first seen running, so `running_since` equals the last update's time. This hides 1. No sample falls between 10 s and 60 s after the update with an older turn.
- `the_heartbeat_moves_only_while_there_is_something_to_move` checks only `Next`, so 10 goes unseen.
- Nothing tests:
  - a call failing under a toggle (2);
  - the cursor after a verbosity change or a `restore` (3);
  - the scroll position after a jump opens a run (4);
  - a running turn's line in a narrow panel (5);
  - folding an open run from a call with output (6);
  - a queued turn (8).

## 15. Smaller

- `chat/actions.rs:476`: picking the verbosity already set (normal → normal) wipes every hand fold.
- `chat/nav.rs:189` vs `:226`: the place in words says "message {item index + 1}", but `Motion::Line(n)` counts rows. `:5` lands on a row that is then called "message 9".
- `chat/render.rs:49`: in quiet a run of one says "1 calls, folded", and reasoning counts as a call there.

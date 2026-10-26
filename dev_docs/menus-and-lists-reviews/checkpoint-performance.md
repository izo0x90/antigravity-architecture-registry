# Checkpoint review: performance (commit eda7934, menus and lists step 1)

Scope: the changes in eda7934 only (pick_list.rs, palette.rs, app/linking.rs,
keymap/local.rs), read against the code they run in. Each finding says whether
this commit added the cost or it was already there. Ranked most important first.

## 1. Added: Enter and the `@` rows now wait for the slowest of several searches that are never debounced, cancelled or timed out

- `palette.rs:331-338` holds Enter (`enter_waiting`) for as long as the Search
  stage's action says `Param::Search { waiting: true }`. Before this commit,
  Enter waited one frame (the frame the query changed). Now it waits until every
  answer of the newest round is in.
  - `@` picker (`app/linking.rs` `Finding::waiting`/`settle`): both the Files and
    Symbols answers.
  - `graph.find` (`graph/actions.rs:274`, unchanged but now subject to the new
    hold): all three kinds, including the Text grep. That grep runs up to its
    150 ms server budget (`arch-service/config/limits.toml:40`), plus the
    round trip.
- `@` rows: `Finding::settle` replaces the rows only when both kinds have
  answered. So the list now updates when the slower answer arrives, not the
  faster one. If you type faster than that slower answer comes back, the list
  shows an older query's rows until you pause.
- The requests behind this, already there before this commit:
  - Each key sends 2 GETs (`linking.rs` `find_links`, the `for kind in [Files, Symbols]` loop) or 3 (`graph/mod.rs:391` `send_find`).
  - Nothing waits for typing to pause, and nothing aborts a round that a newer
    key has replaced.
  - Every replaced round still costs a `spawn_blocking` search on the server
    (`src/arch.rs:114`), a JSON encode, and a `serde_json` parse in the client
    (`net.rs:298-302`). The client drops the result only afterwards
    (`linking.rs` `round != self.desk.finding.round`).
  - Example: typing at 4 keys/s in `graph.find` gives 12 requests/s, and 9 of
    those are thrown away. The browser runs at most 6 requests at once per
    origin over HTTP/1.1 (I didn't check which HTTP version the server uses).
    The newest round's requests can then queue behind stale ones, so the wait
    from the first point grows.
- No upper bound:
  - `net.rs:307-336` (`call_bytes` via `ehttp`) sets no timeout.
  - If one answer of the newest round never arrives (a dropped connection, a
    server stall), `waiting` stays true. The rows never change, and an Enter
    pressed meanwhile is held with no end.
  - Nothing on screen says it is held. The "Looking…" note shows only when
    there are no rows (`palette.rs` Search stage, `if found.is_empty()`), and
    while waiting the rows are the previous query's.
- What it costs: Enter-to-pick latency for `graph.find` goes from one frame to
  at least the slowest kind's round trip (≥150 ms with a busy grep). For `@`, it
  is the slower of the files/symbols round trips. On a stalled request it never
  completes.

## 2. Added: about 5 allocations and 3 galley-cache lookups per visible row per frame in `PickList::paint`, partly duplicated

- `pick_list.rs:151`: `row.keys.to_string()` copies the `Cow` even when it is
  already `Owned`. In the action list it always is: `palette.rs:441` builds it
  from `bound.key_hint()`, which itself allocates (an `Rc` clone of
  `keymap::local()`, a `Vec` of labels, a `join`). So each row's keys are built
  and then copied again, every frame.
- `pick_list.rs:154`: `row.label.to_owned()` (or `format!`). Then
  `actions::choice_text` (`actions.rs:645-658`) copies that string again into a
  `RichText`.
- `pick_list.rs:159`: `detail.to_owned()`.
- Three layouts per row (`layout_no_wrap`, `into_galley`, `layout_job`). Each
  frame, each one hashes its whole `LayoutJob` to find the cached galley.
- What it costs: these run every frame the palette is drawn, once for each row in
  sight. At the 360 px list height that is about 9 rows: roughly 45 small
  allocations and 27 hashed lookups per frame. It is small next to findings 3
  and 4.
- Fix: move the owned key string into the layout (`into_owned()`), and lay the
  label out from `&str` without the intermediate `String`.

## 3. Already there: the action list's per-frame cost grows with the number of actions, and this commit leaves it all in place

Every frame (`app.rs:584`), open or closed:

- `render_palette` calls `palette_actions()` (`app/draw/panels.rs:55`,
  `app/commands.rs:278-289`). That clones the frame's cached `Vec<Bound>`
  (`commands.rs:149-160`). It then gathers every unfocused view's actions and
  builds a new label for each with `format!`.

Every frame while the action list is open:

- `palette.rs:426-427` collects all actions into `listed` and sorts them by
  `(layer, label)`.
- `palette.rs:429` builds a 4-tuple fingerprint of all of them.
- `Ranked::of` (`palette.rs:85-96`) hashes the filter and every id, label,
  detail and group string, just to learn that nothing changed.
- `palette.rs:432` collects the ranked `Vec<&Bound>`.

What it costs: with a few hundred actions that is about 2-3k string compares
for the sort, and hashing tens of KB of text, every frame. The rows drawn are now
bounded by the rows in sight, so these whole-list passes are what the palette
spends per frame. Fix: build the sorted list and its key once per change, not
once per frame. For example, key the `Ranked` cache on a generation number of
the action set, not on hashing its contents.

## 4. Already there: Choose and Search stages copy their whole list several times per frame; tag pickers re-rank every frame

Every frame while a Search or Choose stage is open:

- `palette.rs:329`: `**action = fresh.clone()` (the whole `Bound`, including
  `found`).
- `palette.rs:339`: `self.stage.clone()` (again the whole `Bound`).
- `palette.rs:481`: `found.clone()`.
- `palette.rs:463`: `param.choices()` (a clone of every choice), plus the
  fingerprint and hash.

Where the lists come from, rebuilt on every gather. A gather runs once for where
you are, and once more for each unfocused panel via `shown_at`
(`commands.rs:166-167`, `draw/panels.rs:30`, `draw/host.rs:29`):

- `app/linking.rs:190` `self.desk.finding.shown.clone()` while `@` is open.
- `app/tagging.rs:159-176` `tag_found`: while a tag picker is open, it builds
  `Item`s for every tag, fuzzy-ranks them (`nav::rank_matches`) and makes a
  `Choice` for each.
- `app/tagging.rs:127` `tag.goto` builds a `Choice` for every tag on every
  gather, whether the palette is open or not.
- `graph/actions.rs:250-275` `find_choices` formats a detail string for every
  hit.

What it costs: a tag picker over a few hundred tags means one full fuzzy rank,
plus four or more full copies of a few hundred `Choice`s (each with three or four
`String`s), every frame it is open, times the number of panels.

## Outside this brief (seen while reading)

- `pick_list.rs:119-132`: when the pointer moves down from the highlighted
  row j to a row i > j, row j is painted highlighted before row i takes the
  highlight. Both rows show highlighted for that frame. This is a one-frame
  blink in the list that was meant to stop blinking.
- `linking.rs` `Finding::clear`: this keeps the round number, so a late
  answer of the round just picked passes the round check in `find_links` and
  is settled into `shown`. It is not dropped, though nothing shows it (it is
  replaced on the next opening's `ask("")`).

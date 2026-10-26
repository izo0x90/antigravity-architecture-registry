# End review: performance (menus and lists, eda7934..03c4a3e)

Scope: the plan's six commits, read against the code they run in (cockpit-ui;
nothing built or run). Costs are estimates from the code at the sizes this
cockpit has: about 150 actions where you are, about 250 in the palette's list,
about 18 top-level leader groups (`[keymap.groups]` plus the view menu), and
3 panels, maybe with a quick look. Each finding says whether this plan added
the cost or it was already there. Ranked, most important first.

## 1. Added (step 3): the per-kind key cache thrashes while the quick look shows a kind a panel also shows, so all actions are bound again twice every frame

- `app/commands.rs:206-221`: the cache has one entry per `BufferKind`. An
  entry is used only if the whole `KeyedActions` (kind, then every action's
  origin, id and aliases) is equal to the one stored.
- Step 3 made every host's gather keyed (`shown_at`, `commands.rs:166-167`,
  `keyed: true` for panels that aren't focused).
- A quick look and a panel can show the same kind but don't offer the same
  actions:
  - `host_actions` (`commands.rs:47-80`) gives the quick look
    `quicklook.toggle`/`quicklook.to_panel`;
  - it gives a panel `panel.close`, `buffer.*`, `buffer.list`,
    `buffer.delete` and `buffer.new.*`.
- So in each frame:
  - `bound_actions()` (here = the quick look) finds the panel's set in that
    kind's entry and binds everything again;
  - then `render_view` of the panel of that kind (`draw/host.rs:29`, here is
    not where you are, so `gather(.., true)`) finds the quick look's set and
    binds everything again.
  - The same happens in the next frame, and in every frame while the quick
    look is open.
- When this happens. All of these are ordinary layouts:
  - the lead's chat in the quick look (`talk.open`, `app/look.rs:27`) with a
    chat panel open;
  - the pop-up terminal (`look.rs:66`) with a terminal panel open;
  - the card quick look (Spec) with a spec panel open.
  - The chat and the terminal redraw on every streamed chunk, so in a busy
    session this is every frame.
- What one rebind costs (`commands.rs:213`, then `keymap/mod.rs:450-498` and
  `commands.rs:236-250`):
  - `actions.clone()`: a deep copy of every action, with their `Param::Choice`
    lists (every tag for `tag.goto`, every open buffer for `buffer.list`,
    saved sessions);
  - a `binding_for` lookup per action, with a `format!("{head}.*")` for each
    dot level of an id that isn't bound;
  - linear scans of `taken`;
  - for every `#` entry (`config.*`, `command.*`, `request.*`: the harness's
    commands can be dozens), a `parent.clone()` and a scan of `taken` for
    each key it tries;
  - then the whole `Vec<Bound>` built and dropped, and `keys.clone()` to
    store it.
  - Roughly 2-3k allocations and on the order of 10^5 short-slice compares
    per bind. Two binds per frame is likely around a millisecond of a 16 ms
    frame in wasm (an estimate; not measured).
- Before step 3 the cache held one entry, and only where you are was keyed,
  so this case couldn't happen.
- Fix: key the entry by `Here` (host and kind), or keep a small entry per
  distinct `KeyedActions` (at most the hosts on screen plus one).

## 2. Partly added (step 2): the action list's whole input is built every frame while it's closed

- `app/draw/panels.rs:39` and `:49` run every frame, open or closed:
  - `palette_actions()` (`commands.rs:254-265`): a full clone of the frame's
    `Vec<Bound>`, then `actions()` of every other kind of view (each builds
    its `Vec<Action>`: formatted labels, choice lists), each relabeled with a
    `format!`;
  - `sheet_of(&bound)` (`draw/bars.rs:157-179`, new in step 2): a `Vec<bool>`
    over all of them, a `situation()` (see 3), four layer titles, the
    pointer's 5 rows as 10 owned `String`s, and a copy of `sheet_mode`'s keys.
- `Palette::render` then returns at once when closed (`palette.rs:315`), so
  all of it is thrown away.
- The only other use of the list is the lookup for `take_palette_request`
  (`panels.rs:41-48`). That needs it only when a view has asked, which is
  rare.
- Cost: about 250 actions copied or built (about 1-2k allocations) plus about
  20 more for the sheet, every frame, for nothing.
- The checkpoint's performance report raised the `palette_actions()` half (its
  item 3, "Every frame ... open or closed"). The findings file records only
  the per-frame work while the list is open as knowingly left. Nothing
  covers the closed case. Step 2 added the sheet on top.
- Fix: when the palette is closed and no view has a palette request, return
  before gathering. Build the sheet only when the palette is open.

## 3. Added (steps 2 and 3): `situation()` is worked out 3 + (hosts) times a frame, and each time builds every go-to's whole candidate list to test if it's empty

- `app/views.rs:45-61`. `situation_of` calls `reaches_of`. For the target's
  component, `reaches_of` calls `goto::candidates` for all 7 relational
  go-tos (`goto.rs:24-46`), and keeps only `!is_empty()`.
- Each `candidates` call builds a full `Vec<Candidate>`:
  - one `ComponentId` clone per reference (Uses);
  - two clones per relation (the candidate, and the `BTreeSet` of seen
    ones).
- Callers per frame now:
  - `bar_keys` (`bars.rs:18`) and `render_item_controls` (`host.rs:158`),
    both there before;
  - `sheet_of` (`bars.rs:158`), every frame, added in step 2 (see 2);
  - `groups_of` (`which_key.rs:19`), once per host per frame, added in step 3.
    Hosts that aren't focused work theirs out for their own view's target.
- With 3 panels that is 6 a frame where there were 2. On a well-used
  component (say 300 references and 100 relations) each is about 500
  allocations and set inserts, so about 2k extra allocations a frame. It
  lasts as long as the cursor stays on it.
- Fix:
  - keep the situation per kind for the frame (as `frame_actions` keeps the
    actions);
  - have `reaches_of` ask whether any candidate exists (an `any` over the
    relations) instead of collecting them.

## 4. Added (step 3): each host's group buttons cost a sort whose key scans every action, every frame

- `app/draw/which_key.rs:18-38` (`groups_of`), run for every panel and the
  quick look each frame (`draw/host.rs:30`), calls:
  - `leader_entries(bound, &[])` (`actions.rs:597-614`). Its
    `entries.sort_by_key(|e| (closest(e, bound, path), ..))` calls `closest`
    twice per comparison.
    - For a group, `closest` (`actions.rs:618-627`) allocates a prefix `Vec`
      and scans all of `bound`.
    - With about 18 top-level entries (a small-slice insertion sort, about
      100 comparisons), that is about 200 `closest` calls × 150 actions:
      about 30k `starts_with` and 200 allocations.
  - then `is_primary` per entry: another `closest` each, about 18 × 150;
  - then an `.any` over `bound` per group;
  - then per button, a `format!` for the tip and `leader_hint` (an `Rc`
    clone, a `Vec<String>`, a `join`).
- Per host per frame that is about 35k scans and about 250 allocations. With
  3 panels plus the which-key menu (`which_key.rs:69-98`, the same
  `leader_entries` and `is_primary` per frame while open), it is over 100k
  scans a frame. The result changes only when the actions or the situation
  change.
- Fix: one pass over `bound` that gives each top-level key its closest layer
  and whether anything in it holds (`sort_by_cached_key`, or a small map).
  Or keep the groups with the keyed actions.

## 5. Added (step 3): a keyed gather copies all ids and all keys every frame, even when nothing changed, and for a panel not focused the keys are then dropped

- `commands.rs:207`: each keyed gather builds `KeyedActions`, one `String`
  clone per action id (plus its aliases), to compare with the stored one.
- `commands.rs:211`: on a hit, `cache[at].1.clone()` copies every `Keys`
  (`Option<Vec<char>>`, `Vec<Vec<Chord>>`, `Vec<String>`): up to 3
  allocations per action.
- Step 3 makes this run once per host per frame (before, only where you
  are). That is about 300-600 allocations per host per frame at 150 actions,
  only to find that nothing changed.
- For a panel that isn't focused, `draw/host.rs:33-41` then clears every
  `keys` and `leader` except what `groups_of` read.
- Fix: compare the actions with the stored ids in place (no new `Vec`), and
  keep the keys behind an `Rc<[Keys]>`. Or key the cache on a generation
  number of the action set once there is one (as the findings file already
  suggests for the palette).

## 6. Added (c464f49): tending a hidden view's search runs its whole tick, which can load the whole graph

- `app/draw/panels.rs:66-71` calls `buffer.tick(net, ctx)` for the view
  being searched when it isn't on screen.
- For the graph, `tick` (`graph/mod.rs:463-527`) does more than the find:
  - `graph_config`, if never asked;
  - `arch_plans`, if `plans_stale` (true at start, `graph/mod.rs:192`);
  - the whole `graph_tree` if `stale` (true at start, `graph/mod.rs:209`,
    and set again by every `Changed` event, `:535`), then `set_tree`
    (`:336`) on the reply;
  - `want_details`.
- So opening the graph's find from a layout where the graph was never shown
  fetches and parses the whole tree, and again after each re-index while
  the find stays open, though only `send_find` was needed.
- A pick of a symbol navigates the graph, which asks for a tree of its own.
  A pick of a file opens code. Either way the tree load done for the find
  is not used.
- Fix: tend only the search (`find_query`, then `send_find` and the found
  replies) for a hidden view.

## 7. Added (step 2): the list builds whole rows to read their kind

- In `pick_list.rs`, `PickList::show` calls the `row` closure for its `kind`
  only:
  - at `:136` every frame;
  - at `:181` on pointer moves;
  - at `:197` on Enter;
  - at `:153`, for every row of the list on every move key.
- In the `?` list each call (`sheet.rs:147-162`) builds:
  - `bound.ways()` (an `Rc` clone of the local keys per leader, a
    `Vec<String>`, a `join`);
  - for a row that doesn't apply, a `format!("Needs ..")`.
- One move key in `?` over about 250 entries is about 1.5k allocations
  before anything is drawn. Ctrl+J held at the key-repeat rate repeats that
  about 30 times a second.
- Fix: a `kind(i)` the list can ask without building the row (`Listed` knows
  it from `Line` and `applies`).

## 8. Small, every frame: setting controls and long-press tips

- `actions.rs:778`: `PickList::fitting` runs for every Choice setting control
  every frame, closed or not. It takes two `fonts_mut` row-height lookups,
  needed only to size a popup that isn't open.
- `actions.rs:800`: every frame, each closed setting does a `data_mut` write
  lock and a remove of its `PickList` entry.
- `actions.rs:696-711` (`tip_on`, now on every control, group button and
  which-key entry): an `Arc` clone of the context, then a `data` lookup per
  widget per frame.
- A long press leaves an `f64` in egui's temp data under
  `response.id.with("long_tip")`. It is removed only if that same id is
  drawn again after 2.5 s.
  - Button ids are positional (`ui.button`), so a button that moves or goes
    away (a group no longer offered, a which-key menu closed) leaves its
    entry for good.
  - The entries grow with long presses, never shrink, and are a few dozen
    bytes each.
- Each of these is small. Together they are a few dozen lock-and-lookups a
  frame on a busy screen.

## Checked and holding

- Checkpoint performance 1:
  - one round out at a time (`code_search.rs:93-110`);
  - a round counts as answered after 8 s (`ROUND_LIMIT_S`), and
    `find_links` asks for a repaint for when that runs out;
  - "Looking…" is on the key line while Enter waits (`palette.rs:551-553`).
- Checkpoint performance 2: the keys are laid out from the owned string
  (`pick_list.rs`, `row.keys.into_owned()`), and the label is copied only
  for the tick.
- The two outside-brief items (two rows lit; a late answer after a pick) are
  fixed (`pick_list.rs:172-189`, `CodeSearch::clear`).
- Knowingly left, and the reason still holds:
  - the open list's per-frame sort, fingerprint and hash;
  - the Search and Choose stages' row copies;
  - the graph's find rows in each gather.

  Step 2's `?` adds 16 layer-by-group passes and their titles
  (`sheet.rs:62-73`) and a sort in `lines` (`:130-131`). That is still tens
  of microseconds at 250 entries.
- The per-kind key cache has at most one entry per `BufferKind`, so it is
  bounded.

## Outside this brief (seen while reading)

- `sheet_mode` (`app.rs:304`) is set when `?` runs (`commands.rs:449`) and
  never cleared. So `:` then Tab (everything) lists "<mode>: its keys" for
  whatever mode the last `?` was pressed from, not the one this list was
  opened from.

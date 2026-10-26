# Stopping point 3: performance reviewer

Performance review of d4c5b67..HEAD (meta-harness). Ranked, most severe first.

**1. Each frame clones whole `Tag`s several times per gather (crates/cockpit-ui/src/app/tagging.rs:100-124, 128-133; app/draw/host.rs:35, 124-126)**
- `tag_actions` already calls `tagged_here` once. The new `tag_edits` adds two more: it calls `tags_here`, and then `tag_to_edit` calls `tags_here` again and `.cloned()`s the target once more. `render_view` adds a fourth through `tags_here_named`.
- Each `tagged_here` call does three things:
  - runs `things_here`, which clones `cursor_key` and, in the terminal, formats 1-2 strings;
  - for each thing at the cursor, calls `Tags::on`, which allocates `list()`, sorts all T tags by name, and scans every `Tagged` with `Subject::contains`;
  - clones each matching `Tag`, including its whole `on: Vec<Tagged>`, where each entry holds 2-3 Strings plus a `Subject`.
- How often: the focused view is gathered once per frame (cached). Every other panel is gathered twice per frame (draw/panels.rs:30 and draw/host.rs:36, both through `shown_at`). With 3 panels that is about 5 gathers per frame, roughly 15 extra `tagged_here` calls plus 3 in drawing.
- Trigger: the cursor on a command tagged `flaky` (300 things) while another terminal streams output, which repaints every frame. That is about 18 × 300 × 3, roughly 16k allocations per frame, or about 1M per second at 60 fps in wasm, plus 18 sorts of the tag list.
- `tags_here_named` only needs the name and colour, but clones the whole tag to get them.
- Severity: medium. Frame time grows with how many things the tags at the cursor are on. You see jank while scrolling a busy terminal on a heavily tagged line.
- Fix: compute the tags at the cursor once per gather and pass ids or refs into `tag_edits`, `tag_to_edit` and `untag_action`.

**2. While the palette is open, the tag picker's rows are built twice per gather, in every panel, whatever the palette is showing (tagging.rs:180 and new 233-234)**
- `picking = palette.is_open()`, so `tag_found("tag.add")` and the new `tag_found("tag.insert")` both run on every gather. That includes unfocused panels, whose `found` is never shown, and frames where the palette is listing something unrelated.
- Each call:
  - builds a `Choice` per tag (id and name clones, a `described()` format);
  - with a query, also clones every tag's name and note into `Item`s and runs `rank_matches`, a fuzzy rank over names plus a text rank over notes.
- `tag.insert` then filters its result through `tags.get`.
- Trigger: 300 tags, the palette open (Ctrl-P), 3 panels. That is about 10 `tag_found` calls per frame and about 3,000 Choices, on every keystroke and on every frame while a terminal streams.
- A query typed into `tag.insert` and then dismissed stays in `shell_searches` (it is only removed on a pick, tagging.rs:270). From then on, every palette frame re-ranks against that stale query.
- Severity: medium-low. Typing in the palette lags with hundreds of tags.
- Fix: build `found` only for the action whose Search stage is open, and only when gathering the focused view.

**3. The Tags view lays out every row every frame, now with more per row (tag_list.rs:157-225)**
- It uses `ScrollArea::show` over all rows, with no row virtualisation; that part is pre-existing.
- This change adds an `age()` format and a `who()` format per thing row, a `first_line` format per tag row, and two `tag_mark` calls.
- Folded by default keeps rows down to one per tag. But `z R` (`Fold::OpenAll`) on 100 tags × 10 things gives about 1,100 rows: about 4,000 labels laid out and about 1,100 `Spot { key: keys[i].clone() }` copies per frame. Thing keys are `"{id} {subject-json}"`, around 100+ bytes.
- Severity: low-medium. It only appears with everything unfolded.

**4. Linear key scans in the Tags view, about 8 per gather (tag_list.rs:273-282 `things`, 364-367 `tag_at`; called from tagging.rs `tags_here` ×2, `tag_edits` ×2, `untag_action`, `things_here`)**
- Each scan string-compares the cursor key against every row key. With about 1,100 rows unfolded and about 5 gathers, that is roughly 40k string compares per frame.
- Severity: low.
- Fix: the view knows its cursor row and can answer `tag_at` and `things` for the cursor directly.

**5. Tag gutter, per terminal panel per frame (draw/host.rs:90-121)**
- It allocates and sorts `tags.list()`. For every tag it collects a `rows` Vec through `drawn_at` (a binary search per thing), even after the 3 columns are full: the `rows` work happens before the `columns == GUTTER_BARS` check.
- Every visible bar gets a `format!("{name}: {note}")` and a `ui.interact` widget each frame, though the text is only needed on hover or long press.
- Cost is O(total tag-things): about 1,000 cheap lookups at realistic sizes.
- Severity: low.
- Fix: check `columns` first; build `said` only when hovered or touched.

**6. `theme::tag_mark` (theme.rs:491-495)**
- Each call allocates two `Arc<str>` (`FontFamily::Name(SOLID.into())`, in both the `contains_key` check and `solid()`) and takes the fonts lock.
- It is called per tag in every panel's header, per tag row in the Tags view, and per coloured palette row, each frame. With hundreds of rows, that is hundreds of small allocations and lock takes per frame.
- Severity: low.
- Fix: the "solid font in" check can be a flag set once.

**Growth**
- `TagList::open` (tag_list.rs:57) never drops ids of deleted tags. The growth itself is trivial, bounded by tags opened in a session.
- It also makes `fold_all`'s `self.open.len() < tags` (tag_list.rs ~430) wrong. Example: delete an unfolded tag, then have exactly one tag folded; "Unfold every tag" shows as unavailable. This is a correctness slip, not a cost.
- `desk.tag_editing` and `shell_searches` are bounded.

**Server**
- `nah-tags/src/logs.rs:47-54`: `set_permissions` runs once in `Logs::open`, called once at startup (nah-tags/src/lib.rs:216). It is blocking, but not on a per-request path. Nothing else changed on the server.

**Network**
- Each tag edit sends one `Ask::Tags`. The Tags view adds no requests. Nothing chatty, nothing re-sent.

**Chat `#` detection (chat/render.rs:377-379)**
- It runs only on `output.changed()` and is O(1) amortised. Not an issue.

**What would confirm items 1-2**
- The `#[cfg(test)] gathered` counter in `gather` only counts keyed gathers.
- Add a counter in `tagged_here` and `tag_found`, then run a 3-panel desk with the palette open over a few frames. Expect about 18 `tagged_here` calls and about 10 `tag_found` calls per frame.

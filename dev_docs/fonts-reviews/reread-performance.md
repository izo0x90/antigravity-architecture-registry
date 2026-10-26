# Re-read of d3f743c (fonts fixes), performance

Checked against egui/epaint 0.33.3, tower-http 0.7.1 and ehttp 0.5.0 sources.
The sizes are for the 11,611,796-byte Iosevka Term face, which gzips to
3.4 MB in 0.24 s of CPU (measured in the first review).

## The fixes, checked

- **1. Download and gzip on every load.** Fixed for repeat loads.
  - `src/routes/cockpit.rs:117`: `ServeFile` streams the file in 64 KB
    chunks.
  - It sets a strong ETag (size plus mtime in ns) and `Last-Modified`.
    `If-None-Match` and `If-Modified-Since` get a 304 with an empty body.
  - `CompressionLayer` leaves the 304 alone, because `SizeAbove` sees an
    exact size of 0.
  - Line 119 adds `no-cache`, and ehttp's fetch uses the default cache mode,
    so the browser revalidates.
  - Zip extraction writes the face with `File::create`, so a reinstall gets
    a new mtime and a new ETag.
  - The first `200` still costs the same (see problem 2).
- **3. Three copies of each face.** Fixed on our side.
  - `fonts.rs:75` turns the downloaded `Vec` into `FontData` with no copy.
  - `theme.rs:374` puts the same `Arc` into the definitions.
  - The one copy left is epaint's `FontVec` (see problem 1).
- **4. The same pack fetched twice.** Fixed. `fonts.rs:43` dedups the
  names into a `BTreeSet`. `font_definitions` inserts the same key and the
  same `Arc` twice, so there is one `FontVec`.
- **5. Two rebuilds.** Fixed.
  - `settle` (`fonts.rs:87-98`) does nothing while `waiting` isn't empty.
  - It calls `set_fonts` only when the leads (by name) changed.
  - The first frame's `want(defaults)` no longer calls `set_fonts`, because
    `built` already holds `(None, None)` from `start`.
- **6. Changing one lead dropped the other.** Fixed. `fonts.rs:44-47`
  keeps faces that are still named and fetches only new names.
- **Per frame.** No new per-frame work.
  - `tend` takes one uncontended lock and calls `mem::take` on an empty
    `Vec`, which doesn't allocate.
  - `want` runs only when `theme_dirty` is set: on the first frame and when
    the config reply lands (`lifecycle.rs:33`, `:193`).
  - `Settings::of`, which runs every frame, doesn't include `fonts`.
  - The other `set_fonts` calls (`code.rs`, `theme.rs`) are all in
    `#[cfg(test)]`.

## Problems, ranked

### 1. One avoidable copy of each face remains in wasm memory: 11.6 MB per face for the page's life

- **Where.** `crates/cockpit-ui/src/fonts.rs:75` builds the face as
  `FontData::from_owned(bytes)`, which is `Cow::Owned`.
- **How it goes wrong.** epaint 0.33.3 `FontsImpl::new` (`fonts.rs:786`)
  parses every font eagerly. `ab_glyph_font_from_font_data`
  (`fonts.rs:207-209`) calls `FontVec::try_from_vec_and_index(bytes.clone(), ..)`
  for `Cow::Owned` data. For `Cow::Borrowed` it uses `FontRef`, with no
  copy.
- **Cost.**
  - Each face is held twice: our `Arc<FontData>`, shared with the
    definitions, and epaint's `FontVec`. That is 23.2 MB committed for one
    Iosevka face, and 46 MB with Iosevka as both mono and a second
    Iosevka-sized text face.
  - Wasm linear memory doesn't shrink, so this stays committed for the
    page's life on every device that opens the cockpit, phones included.
- **Wrong claim in the findings.** `fonts-findings.md` calls this copy
  "egui's own". It follows from our choice of `from_owned`.
- **Fix.** Give egui a `'static` slice.
  - Leak each fetched face once per pack name, into a map that lives for
    the page, and hand egui `FontData::from_static(leaked)`.
  - That removes the second 11.6 MB.
  - It is bounded by the number of installed font packs (7 shipped), so
    step 2's live switching doesn't leak without limit.

### 2. Every `200` still gzips the whole 11.6 MB face on a tokio worker

- **Where.** `src/server.rs:79` (`CompressionLayer::new()`, default
  predicate) wraps the font route.
- **How it goes wrong.**
  - The default predicate doesn't exclude `font/ttf`.
  - On a `200`, tower-http (`compression/future.rs:100`, `:139-140`)
    gzips the `ServeFile` stream inside `poll_frame` on the async worker.
  - It also drops `Content-Length` and `Accept-Ranges`.
- **Cost.**
  - About 0.24 s of runtime CPU per full send, spread over about 178 chunk
    polls.
  - This happens on the first load in each browser profile or device,
    after every `pack install` or `pack update` of a font, in every
    private window, and after the browser evicts its 3.4 MB entry.
  - The fix removed the repeat cost (304), not this one.
- **Fix.**
  - Write `face.gz` at install and use `ServeFile::precompressed_gzip()`:
    no CPU per request, and still 3.4 MB on the wire.
  - Or exclude `font/*` with a predicate, which sends 11.6 MB uncompressed.

### 3. A face asked for again while its first request is still in flight is downloaded twice (step 2 only)

- **Where.** `crates/cockpit-ui/src/fonts.rs:44-56` and `:70`.
- **How it goes wrong.**
  - `want(B)` removes `A` from `waiting`, but `A`'s request keeps running:
    ehttp has no abort.
  - `want(A)` then finds `A` neither in `have` nor in `waiting`, so it sends
    a second request.
  - The first reply is taken. The second is dropped at line 70.
  - Each dropped reply has already been copied from JS into an 11.6 MB
    wasm `Vec`.
  - The second request goes out before the first is in the browser cache,
    so it is a full `200` and another 0.24 s gzip (problem 2).
- **Cost.** Today the config is applied once per load, so this doesn't
  happen. In step 2 ("try each live"), clicking through the faces A, B, A
  costs one extra 11.6 MB download, one extra wasm allocation and one extra
  gzip for each repeat while the earlier request is in flight.
- **Fix.** Track in-flight requests separately from `waiting` (requested
  versus still wanted). A reply for a name that is wanted again is then
  used, not refetched.

## Checked, not a problem

- **Server, per request.**
  - Catalog load, `Lock::read` and `state()` run in `spawn_blocking`
    (`cockpit.rs:109-116`), not on the runtime. `state()` only does
    `exists()` on `holds`, because font packs have no `check`.
  - `ServeFile`'s open, metadata and reads go through `tokio::fs`.
  - That is at most two requests per load (one when mono and text name the
    same pack). A few ms of catalog parsing before each 304 is trivial.
- **`set_fonts`' equality check** (egui `context.rs:1968`).
  - It now runs only when the leads change.
  - `FontData` is `PartialEq`, not `Eq`, so `Arc`'s pointer shortcut
    doesn't apply.
  - At worst it is one memcmp of a kept face (up to 11.6 MB, a few ms),
    once per lead change.
- **Unbounded growth.** None.
  - `have` and `waiting` are bounded by the two names in the config.
  - The inbox is drained every frame.

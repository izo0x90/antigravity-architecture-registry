# Review of 2d03dd5 (fonts), performance

Measured on the face installed here:
`~/.local/share/nah/packs/iosevka-term/1.0.0/IosevkaTerm-Regular.ttf` is
11,611,796 bytes. gzip -6 makes it 3,371,289 bytes and takes 0.24 s of CPU.
The egui code paths were checked in egui/epaint 0.33.3.

## 1. Every page load downloads the face again and gzips it again

`src/routes/cockpit.rs:91-93`: the reply carries only `Content-Type`. It has
no `Cache-Control`, no `ETag` and no `Last-Modified`. ehttp's web fetch uses
the default cache mode, and without a validator the browser has nothing it
can reuse. The router's `CompressionLayer` (`src/server.rs:79`, default
predicate) compresses `font/ttf`, so each request is gzipped again from
scratch.

Sequence: `[fonts] mono = "iosevka-term"`, then reload the cockpit (or open it
on a second device). Each load the server reads 11.6 MB into memory
(`tokio::fs::read`), spends about 0.24 s of CPU gzipping it, and sends
3.4 MB. The browser then unpacks it into an 11.6 MB `Vec` in wasm memory. The
file only changes on a pack install, so all of this repeats with no benefit.
A response that can be cached (an immutable max-age keyed on the pack and
tool version, or an ETag from the lock's sha256) removes it.

## 2. The face arrives after the config round trip, so the first screen is laid out twice and terminals are resized twice

`crates/cockpit-ui/src/app/lifecycle.rs:13` asks for `/api/ui/config` in the
first frame. Until the config arrives, `self.config.fonts` is the built-in
default (empty). `Fonts::tend` (`fonts.rs:33-46`) only asks for the face on
the first frame after `apply_user_config` swaps the config in. The font
request therefore cannot start until the config reply is back, and then it
waits for the 3.4 MB transfer from problem 1.

Sequence: open the cockpit with a terminal panel and `mono = "iosevka-term"`.
1. The first frames draw in Hack. `terminal/draw.rs:60` measures
   `char_w` from Hack, and the grid size is sent to the PTY at once
   (`size_sent == None`).
2. The face lands and `set_fonts` rebuilds the fonts. `char_w` changes
   (Iosevka is narrower) and the column count changes. After
   `SIZE_STILL_S` a second resize goes to every open terminal, and the shell
   or agent TUI in each redraws completely.

Code views and every other text area also lay out again and jump. Fixing
problem 1 shortens the gap but does not remove it, because the request still
waits behind the config reply.

## 3. Each face is kept three times in wasm memory, and four times at peak

- `fonts.rs:62`: the downloaded `Vec` is kept in `Fonts.faces` as
  `Arc<Vec<u8>>` (copy 1). It stays there for the life of the choice.
- `theme.rs:373`: `FontData::from_owned(face.bytes.as_ref().clone())` copies
  all of it into egui's `FontDefinitions` (copy 2). This runs on every
  `font_definitions(&faces)` call.
- epaint 0.33.3 `ab_glyph_font_from_font_data`: for `Cow::Owned` data it calls
  `FontVec::try_from_vec_and_index(bytes.clone(), ..)` when `Fonts::new` runs
  (copy 3).

For Iosevka that is about 35 MB held for one 11.6 MB face. When the second
face (text) arrives, `fonts.rs:73` builds new definitions while the old ones
and the old `FontVec` are still alive, so the mono face briefly exists four
times (about 46 MB). Wasm linear memory only grows, so this peak stays
committed for the page's life.

egui's `font_data` is already `BTreeMap<String, Arc<FontData>>`. If `Face`
held an `Arc<FontData>` and `font_definitions` inserted a clone of that
`Arc`, copies 1 and 2 would be one allocation and the clone at line 373 would
go away.

## 4. The same pack for mono and text is fetched twice and held twice

`fonts.rs:36-45` sends one request per lead, with no check for equal names.

Sequence: `mono = "iosevka-term"`, `text = "iosevka-term"`.
1. Two GETs go to the same URL: two 11.6 MB reads and two gzip runs on the
   server, and two 3.4 MB transfers.
2. Two separate 11.6 MB `Arc<Vec<u8>>`s are kept (`faces.mono` and
   `faces.text`).
3. `font_definitions` clones both, and the second `insert` at `theme.rs:373`
   overwrites the first under the same key. So there are 23 MB in `Faces`,
   plus the clones, plus the `FontVec`.
4. If the two replies land in different frames, there are two rebuilds (see
   problem 5).

## 5. Two faces arriving in different frames cause two full font rebuilds

`fonts.rs:57-74` calls `set_fonts` on every frame where a face arrived. The
mono and text requests are independent, so they usually finish in different
frames.

Each `set_fonts` with new definitions makes egui's `update_fonts_mut` drop
`ctx.fonts` and call `Fonts::new`. That means a new glyph atlas: every visible
glyph is rasterized again and the atlas texture is uploaded again. It also
repeats the 11.6 MB clone at `theme.rs:373` and the `FontVec` copy.

Sequence: `mono = "iosevka-term"`, `text = "geist-mono"`. You get two
rebuilds a few frames apart, and one frame where the mono face is the pack's
and the text is still egui's. Waiting until every requested face has been
answered (arrived or failed) before calling `set_fonts` makes this one
rebuild.

## 6. Changing one lead throws away the other lead's face and fetches it again

`fonts.rs:34` resets both faces (`self.faces = Faces::default()`) whenever
any field of `FontsUi` differs. Line 35 then rebuilds the fonts with egui's
own.

Sequence: `mono = "iosevka-term"` loaded, then the choice becomes
`text = "geist-mono"` with mono unchanged.
1. The fonts are rebuilt with egui's faces (a flash of Hack).
2. The 11.6 MB Iosevka is downloaded and gzipped again.
3. The fonts are rebuilt again for each arrival.

Today the config is read only once per load (`lifecycle.rs:13`, and
`apply_user_config` takes a single reply), so this only shows once step 2's
"try each live" changes the choice at run time.

## Checked, not a problem

- The server reads the catalog on each request (`cockpit.rs:87`):
  `Catalog::load` parses about 112 KB of shipped manifests plus the user's
  pack folders and the lock, inside `spawn_blocking`. That is at most two
  requests per load (one per lead). This cost is trivial next to the 11.6 MB
  read and the gzip.
- `Fonts::tend` runs every frame. When nothing has changed it does one
  `FontsUi` equality check (two short strings) and one uncontended mutex
  lock.
- The redundant `set_fonts(defaults)` on the first frame (`fonts.rs:35`,
  after the same call in `web/mod.rs:56`): egui compares the definitions,
  finds them equal (about 1.5 MB of built-in font bytes compared) and does
  not rebuild.

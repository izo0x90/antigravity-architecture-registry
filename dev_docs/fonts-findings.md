# Fonts: review findings and what was done

The end review of step 1 (2d03dd5) had two reviewers: consistency and
performance. Their reports are word for word in `fonts-reviews/`. Each
finding is listed below with what was done about it.

## Fixed

Consistency:

1. **The face lookup skipped the pack engine.** `nah_packs::font_face`
   now sits beside `harness_launch` and `server_launch` and returns a
   `PackError`. It also asks the pack's state first:
   - not installed: `NotInstalled`
   - installed at another version: the new `Outdated`, which says
     `meta-harness pack update`
   - a file missing: the new `Broken`, which says `pack install`

   The server's own string version is gone.
2. **Every failure was a 404.** The route now has a typed error
   (`FaceFailed`), as the other route modules do:
   - not found or not installed: 404
   - not a font pack: 400
   - outdated: 409
   - the server's own packs or lock broken: 500
3. **The no-project server found packs from another root than `pack`.**
   It now uses `startup::repo_root()`, as the `pack` command does.
4. **Config.**
   - `[fonts]` now comes before the changes comment, not inside it.
   - `FontsUi` is like its sibling sections: no `deny_unknown_fields`, and
     no second default besides `default.toml`.
5. **The plan and README didn't match the build.**
   - The README's pack table has a `font` row, and the fonts paragraph says
     what's served and what's said.
   - The plan says why `[fonts]` is its own section and not a `[theme]`
     token: a face is fetched from the server, not a value.
6. **The manifest rules were tested in the wrong place.** `nah-packs`
   tests now cover:
   - the rules: `[font]` needed, `[font]` refused on another kind, the face
     kept in `holds`, unknown fields refused;
   - the face through install, outdated and broken.

   The server test whose loop could never fail is gone.
7. **The client built its own prefix and asked for JSON.**
   - The prefix is joined in one place (`NetClient::at`).
   - The bytes call takes the `Accept` it sends (`font/ttf, font/otf`).
8. **Two ways to notice a config change, and two owners of the fonts.**
   - The font choice is applied with the rest of the config, in the
     `theme_dirty` block.
   - `Fonts` is the one owner of the context's fonts. The web start calls
     `start` for the first frame's faces and icons.
9. **Manifest data nothing read.**
   - `family` repeated `[tool] name`. It's gone.
   - `bold` was downloaded and never drawn: egui has no bold or italic of
     its own. It's gone, so each pack keeps one face (`[font] face`), and
     the route has no style.

Performance:

1. **Every load downloaded and gzipped the face again.**
   - The route hands the file to tower-http's `ServeFile`, which streams
     it and answers a repeat ask with 304 Not Modified unless it was
     reinstalled since.
   - `Cache-Control: no-cache` makes the browser check each time.
   - Tested with a spawned server: 404 before install, then 200 with the
     bytes, then 304.
3. **Each face was kept three times.**
   - `Face` holds an `Arc<egui::FontData>`, which is handed to egui's
     definitions without a copy.
   - What's left is egui's own copy (epaint makes an `ab_glyph::FontVec`
     from owned data).
4. **The same pack for mono and text was fetched twice.** It's now fetched
   and kept once, by pack name.
5. **Two faces arriving in different frames meant two rebuilds.** Now
   there's one rebuild, once nothing named is still coming, and none when
   what leads didn't change.
6. **Changing one lead threw away the other.** Faces still named are kept;
   only new ones are fetched.

## Not fixed (recorded)

- **Performance 2: the first screen is laid out twice when a pack face is
  set.** The face can only be asked for once the config has said which one
  it is. So the first frames draw in egui's face, and a terminal panel is
  resized again when the face lands.
  - With 304s the second load is quicker, but the order stays the same.
  - Removing it means either remembering the last choice in the browser or
    having the server read the cockpit's config.
  - Under "Decide" below.
- **Consistency 3, second half: `CockpitState.packs_at` holds the same path
  as `HarnessState.root`.** They're separate module states: the
  no-project server has no `HarnessState`, and the cockpit routes can't
  reach into it. So the cockpit's state keeps its own copy.

## The re-read (two reviewers, once)

Reports: `fonts-reviews/reread-consistency.md`,
`fonts-reviews/reread-performance.md`. They confirmed the fixes above are
real, except where noted here. No further review round. The cheap, clear
ones were fixed in the same pass:

- **A server-side manifest fault answered 400** (consistency 1). Asking a
  font of a pack of another kind is now its own error
  (`PackError::NotA`, also used by `server_launch`), which gets the 400.
  Any other manifest fault is the server's (500).
- **A third wording of a pack's state** (consistency 2).
  - `State` has one `Display` in `nah-packs`, used by the `pack` commands
    and the font route.
  - An out-of-range tool is no longer called "installed at another
    version"; it "needs updating".
  - The language servers' own messages (`arch-service`) still say their
    state their own way, with the command that fixes it. Left as they
    are.
- **Tests that didn't pin the fixes** (consistency 3, 4).
  - The route test now starts the server from another folder than the
    project, so the pack root has to be the project's.
  - It also checks outdated (409) and an unreadable lock (500).
  - The assertion that only tested its own setup is gone.
- **One copy of each face left on our side** (performance 1). Each face
  fetched is kept for the page's life and handed to egui borrowed, so
  egui doesn't copy it again: one copy, not two. It's bounded by the
  number of font packs, and a face had is never fetched again.
- **A face wanted again while still on its way was fetched twice**
  (performance 3). What's been asked is tracked apart from what's wanted.
  An answer is kept even if the choice changed meanwhile.
- **Plan text left at two faces** (consistency 5). Fixed.

Left, and recorded:

- **Performance 2: the first send of a face is gzipped on the async
  worker** (about 0.24 s of CPU for the 11.6 MB face, spread over about
  180 polls). It happens once per browser and per reinstall: repeats get
  304. The options are a precompressed file written at install (a font
  special case in the generic installer) or sending fonts uncompressed
  (11.6 MB on the wire). Neither is clearly better, so it's left. Under
  "Decide" below.
- **The browser cache in this setup.** In the user's tab nothing is ever
  revalidated, not even the static page with its ETag. The sandbox bridge
  that routes `sandbox.localhost` appears to bypass the browser cache. So
  the 304 is verified by the server test only.

## Decide (for the user)

- **The relayout on the first load** (above). Options:
  - Leave it.
  - Have the cockpit remember its last face in the browser and ask for it
    at once.
  - Have the server name the configured face. That puts knowledge of the
    cockpit's config on the server, which is the layering the review warns
    about.

  Recommended: leave it until a face is picked as the default (step 3),
  then see whether the jump is noticeable.
- **Compressing a face's first send** (above). Options:
  - Leave it.
  - Write a `.gz` beside a font's face at install and serve that.
  - Send fonts uncompressed.

  Recommended: leave it. It costs a quarter of a second of server CPU
  once per browser.
- ~~Step 3: which face is the default.~~ Decided (2026-10-25): egui's own
  stays for now. JetBrains Mono is the likely default, built into the
  binary when it's chosen. Part of a look-and-theming review in TODO.md.
  The two questions above (the jump on first load, compression) wait for
  it.

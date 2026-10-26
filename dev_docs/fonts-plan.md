# Fonts: a cooler code face, and fonts that can be changed

## Why

The cockpit's monospace font is egui's built-in default (Hack, plus the
icon font): it reads as a corporate IDE. The user wants something with
more of a modern Unix, riced-desktop feel, not overdone. And the font
should be a setting, so trying another is a change of one line.

## Found (2026-10-24)

- `theme.rs` `font_definitions()`: egui's default fonts plus the phosphor
  icons (also added to monospace, so icons show in code). No font of our
  own; no setting.
- The code view and the terminal measure the monospace glyph width at run
  time (`glyph_width`), so a new face changes their columns correctly.
  ~70 places pick `monospace()` at a size from the theme's look; none
  should assume a width. To check: anything sized in pixels for a text's
  width (key pills, gutters, the palette's key column).

## Candidates (all free to ship, SIL Open Font License; latest releases 2026-10-24)

Release files checked downloadable here: JetBrains Mono 2.304 (5.6 MB zip),
Iosevka Term 34.9.1 (177 MB zip: the whole family; one file kept),
Commit Mono 1.143 (0.7 MB), Monaspace 1.400 (static, 56 MB), Geist 1.8.2
(8 MB), Maple Mono 7.9 (1.8 MB).


- **JetBrains Mono**: clean, tall, readable; very common in modern setups.
- **Iosevka (Term)**: narrow, many styles, the classic riced-terminal face;
  fits more in narrow panels.
- **Commit Mono**: neutral, calm, "nothing to notice" modern face.
- **Monaspace Neon / Krypton** (GitHub): a family of matched faces,
  distinctive, texture-healing.
- **Geist Mono** (Vercel): crisp, modern, slightly techy.
- **Maple Mono**: rounded, friendly, a bit of character.

## Status

Step 1 done (2d03dd5). Step 2 done: `dev_docs/fonts/` (specimens and screenshots; all seven installed here). Step 3, decided by the user (2026-10-25): the default stays egui's own for now; JetBrains Mono is the likely default later. The rest goes into a look-and-theming review (TODO.md).

## Steps

1. **Fonts are packs** (one mechanism: everything the server installs is a
   pack). A new kind, `font`: its manifest names its face (one file:
   egui has no bold of its own), and its install downloads the release (checksum
   checked) and keeps those files (`holds`). `meta-harness pack install
   jetbrains-mono` as for any pack. The server serves an installed face's
   files to the cockpit (a route, `Needs<can::ViewRead>`). The cockpit's
   config names the face (`[fonts] mono`, `text`: built as its own section,
   not a `[theme]` token, since a face is fetched from the server, not a
   value); the cockpit loads it at start, the phosphor icons still added
   after it; one not installed leaves egui's built-in face, said once.
   Nothing sizes text by a fixed pixel width (checked).
2. A side-by-side for the user: the same cockpit screen (code, terminal,
   chat, the action list) in each candidate, saved as local images in
   `dev_docs/fonts/`, with the setting to try each live. The user picks.
3. The picked one becomes the default; README (the setting); snapshots.

## Order

After the sign-in flow, before the menus and lists (their visual work is
then done in the new face). Step 3 waits for the user's pick; the rest goes on.

## Reviews

End: consistency, performance (a face's load time and the first frame).

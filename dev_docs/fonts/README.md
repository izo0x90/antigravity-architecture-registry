# Fonts to pick from (2026-10-24)

`compare-specimens.png`: the same code drawn in each face (today's egui
default first). The PNGs named after each font are the cockpit as it
was on screen with that face set (`[fonts] mono = "<name>"`); the faces
show in the monospace text there (keys, the top bar).

Note: the specimens are drawn by an image library that joins some
character pairs (ligatures: `->` as an arrow in JetBrains Mono, Iosevka,
Maple Mono). egui draws each character on its own, so in the cockpit
those stay `->`.

All seven are installed here. To try one live, write
`~/.config/nah/ui.toml`:

    [fonts]
    mono = "jetbrains-mono"

and reload the cockpit. Iosevka Term is about 11 MB a face (the others
0.1–0.4 MB), so it loads slower over a slow link.

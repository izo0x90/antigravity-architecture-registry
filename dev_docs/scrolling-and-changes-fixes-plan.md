# Plan: moving around the views, and fixes to the Changes view

Agreed 2026-10-16. Built, every step: 1 (2fc4b42), the one-path refactor
for actions (`actions-one-path-plan.md`), 2 (b9e85e6), 3 (2a72c63), 4
(4b78533), 5 (69d298f), 6 (5aa7195), 7 (e93f7ae), 8 (a78b6ac), 9
(36eab80).

Touch taps landing on the wrong controls on small screens is in TODO.md,
not in this plan.


## 1. The Changes view stuck at "Reading the changes…"

If you switch the comparison while a list is still on its way, the late
answer is dropped (right), but the view keeps waiting for it and never
asks again (wrong). Switching the comparison will cancel the wait and ask
for the new list straight away. A test switches twice quickly and checks
that the second list shows.


## 2. One way of moving for every view

Today the code view and the outline each work out their own scroll
position, the Changes view, the chat, the spec and the queue let egui
scroll an item into view, and the graph moves its camera its own way.
Half a page means something different in each. The code view flickers at
the bottom of a file because it asks to scroll past the end and egui only
corrects that a frame later.

All views will move the same way:

- Each view says where its cursor stops are and how big they are: a line
  of code, a line in a change, a message, a row in the spec, a card in the
  graph.
- Each view keeps a small record of where it's looking (how far it's
  scrolled, and in the graph also how far it's zoomed), how big the window
  is and how big its content is, updated after every frame.
- One shared set of rules works out the next cursor stop and where to look,
  together, before anything is drawn. It never goes past the edges of the
  content. If nothing would change, nothing is asked for.
- The views only apply the result. No view calls egui's own scrolling.

The lists move up and down, and sideways where their content is wider than
the window (long lines in the code view). The graph moves in both
directions and zooms.


## 3. How moving behaves, in every view

- `j` / `k` move one stop. The view moves only as far as it has to, keeping
  a little room around the cursor, and never past the edges.
- `Ctrl+d` / `Ctrl+u` work like vim: the view moves half a window and the
  cursor moves the same distance. At the end, the view stays put and the
  cursor goes to the last stop. With the cursor already on the last stop,
  nothing happens.
- `Ctrl+h` / `Ctrl+l` do the same sideways: half a window left and right.
  Vim's keys work too: `z H` / `z L` (half a window) and `z h` / `z l` (a
  few columns). In the browser, `Ctrl+h` and `Ctrl+l` are Chrome's history
  and address bar on Windows and Linux: check first that the page can take
  them; if not, the vim keys cover it there. Views that only move up and
  down ignore them.
- `gg` / `G` go to the first and last stop.
- Jumps, marks, search results and opening something from another view put
  the target in the middle, or as near the middle as the edges allow.
- A stop bigger than the window (a long message, a big change, a big open
  box in the graph) is moved through: `Ctrl+d` moves half a window inside
  it instead of skipping to the next stop.
- If you scroll, pan or zoom by hand, the view stays where you left it. The
  next key press first brings the cursor onto the screen, to the nearest
  stop you can see, and moves from there.
- The chat keeps following the newest message until you move the cursor.
  Content that refreshes under you (the changes list, a file reloading)
  keeps the cursor on the same stop.
- Moves made with keys glide for about a sixth of a second. Wheel,
  trackpad and finger moves follow you directly. Holding a key keeps
  moving smoothly instead of in steps.

These rules are tested on their own, without drawing anything: the edges,
content smaller than the window, stops bigger than the window, a manual
scroll followed by a key press.


## 4. The mouse, the trackpad and touch

The same everywhere, the graph included:

- Mouse wheel: up and down. `Shift` + wheel: sideways. `Ctrl` + wheel
  (`Cmd` on a Mac): zoom, around the pointer (the graph; elsewhere it does
  nothing).
- Trackpad: two fingers move in any direction. Pinch zooms around the
  fingers (browsers send it as `Ctrl` + wheel, so it behaves the same).
- Touch screen: one finger drags, and a quick flick glides on briefly. Two
  fingers pinch to zoom around their midpoint and pan as they move. Tap
  selects, double-tap opens.

One wheel notch zooms as much as one press of the zoom keys.


## 5. The graph

**Keys follow the layout and move the way they point on screen.**

- Tree layouts (calls, data flow): these trees grow left to right (each
  level a column, flipped or not), so `h` goes to the parent and `l` to the
  first child, opening it if it's closed. `j` and `k` go to the next and
  previous card in the same column (the same depth), across branches, so
  you don't get stuck at the end of a branch. (Decided 2026-10-16: follow
  the screen.)
- Boxes (contains): `h` `j` `k` `l` go to the nearest box at the same
  depth, its siblings first, then across into neighbouring boxes. `Enter`
  goes into a box (its first child, opening it) and `Backspace` goes out to
  the box holding it.
- In both: `]]` / `[[` next and previous sibling, `gg` / `G` first and
  last, counts work (`3j`).
- `Enter` and `Backspace` go in and out, as in every view: in the boxes,
  into a box and out to the one holding it; in the tree layouts, to the
  first child and to the parent (the same as `l` and `h` there; one meaning
  everywhere beats saving a key).
- Making a card the root moves from `Enter` to `g f`.

**Zoom.**

- `i` / `o` zoom in and out: next to each other, no Shift, right above
  `j` `k` `l`, so moving and zooming stay under one hand. They zoom around
  the selected card if it's on screen, otherwise around the middle.
- `z z` fits the whole tree; `z f` fits the selected card.
- `+` / `-` still show more or fewer levels.
- Zoom goes out as far as the whole tree and in to about twice the normal
  text size. Some of the tree always stays on screen.

**The camera** follows the rules in section 3: it moves only as far as
needed to keep the selected card in view, never zooms by itself, keeps the
top-left of a card bigger than the screen in view, and glides when keys
move it.

Decided: `Enter` / `Backspace` go in and out everywhere; "make it the
root" is `g f`; in the tree layouts the keys follow the tree as it's drawn
(`h` parent, `l` child, `j` / `k` the same depth).


## 6. Moving through the Changes view

The cursor stops on each file's header and on each line inside its
changes, so `j` and `k` move one line at a time, like in the code view.
`]]` and `[[` go to the next and previous change. Lines you can't land on
stay out of the way: in the code view, deleted lines shown in place and a
fold's "… N lines" take up room but the cursor skips them.

Big changes are drawn the way the code view draws big files: only the
lines on screen.


## 7. Folding in the Changes view

- A file folds and unfolds, and so does each change inside it.
- `z a` folds or unfolds what the cursor is in (the change, or on a header
  the file). `z o` / `z c` open and close it. `z M` folds everything and
  `z R` unfolds everything.
- A folded change shows one line: where it is and how many lines were
  added and removed.
- Double-clicking a file's header or a folded change opens it, as
  double-click does everywhere.
- Fold and Unfold also show as a control on the item the cursor is on, so
  they can be found without knowing the keys.

Why folding was invisible, in every view, not only here: the fold actions
were made without saying they belong to the panel you're in, so they
counted as "everywhere" actions, and the key bar never shows those. They
were only in the palette and the `?` sheet. Views also had no way to say
whether folding means anything where the cursor is, or what it would fold,
the way they do for `Enter` ("Open it in the code"). So the actions were
offered on anything, with the same vague labels, and couldn't be put on an
item. The arrow on a file's header is only drawn, not a control.

The fix is in the shared protocol, for every view (code, outline, spec,
graph, Changes): views say whether the cursor's item folds, and what
folding it does ("Fold this change", "Unfold the file"), as they do for
`Enter`. The fold actions then belong to the panel, show in the key bar and
on the item when they apply, and aren't offered where they do nothing.


## 8. More context

"Show more lines around it" will only be offered where there's more to
show: a change in an edited file. Not on new or deleted files, where a
change is the whole file, and not once the whole file is already shown.
Each change says how many unchanged lines are shown around it, so you can
see the effect. A test checks it on an edited file: asking for more adds
10 lines on each side.


## Order

Each step is tested and committed on its own:

1. The stuck list.
2. The shared rules for moving, with their tests.
3. The code view and the outline on them (fixes the flicker; sideways for
   long lines).
4. The Changes view on them, with stops on lines and big changes drawn only
   where visible.
5. The chat, the spec, the queue and the inbox on them.
6. The mouse, trackpad and touch handling, the same in every view.
7. The graph: its keys, zoom, and its camera on the shared rules.
8. Folding in the Changes view: it offers folds for files and changes
   (the shared fix is the one-path refactor).
9. More context.

After each step: the tests, a check in the running cockpit, and the README
where behaviour changed.

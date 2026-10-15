# Plan: views draw content, hosts draw controls

Agreed 2026-10-15. One rule for everything you can click, tap or press.

## The rule

- **A view draws content** (text, cards, code, the chat stream, typing
  boxes) and **declares actions**. It draws no buttons.
- **A host draws every control.** Hosts: the top bar (the shell's), each
  column, the quick look.
- **Every input becomes an action through one path:** a key, a click on a
  control, a tap, a gesture. The app runs it the same way whichever it was.
- **Exception:** the developer feedback and inspector windows (their own
  subsystem).

## Where an action shows (its placement)

Replaces `toolbar` on `Action` (`Control { at, style, caption, icon }`):

1. **Header**: the panel's top line. The view's title, its status (signed
   in as…, loading, stale files, tokens used…), and its view-wide controls
   (Connect, Stop, view pickers, depth…). The host draws it the same way for
   every view. Views give `title()` and `status()`; the controls come from
   their actions.
2. **Item**: the controls of the item the cursor is on, shown only while the
   cursor is on it, as a strip just under the item (full width, so a
   question's answers fit). Accept / Reopen / Reply on a review point;
   Approve / Retry on a job; a request's answers; Go into on a card.
3. **Key bar**: as now (every action with a key).

The top bar is the shell's header: bell, tray, server chip, add/remove
column, hidden views become shell actions placed there.

## Pointer and touch: one grammar, in the host

Every view already reports its items' places on screen (`spots`), the
cursor's item (`cursor_key`) and can move to one (`move_to`, `open_item`).
The host uses that; views stop handling clicks on their items.

| Input | Does |
|---|---|
| click / tap on content | move the cursor there, focus the column |
| double-click / double-tap | open it (Enter) |
| long-press | the item's actions (the palette, at that item) |
| click / tap on a control | run its action |
| swipe left / right | the action bound to it in this view (like a key) |
| two fingers | the view's own (graph: pan and zoom; others scroll) |

Gestures are keymap bindings like keys:
`"chat.next_slot" = { keys = ["J"], gestures = ["swipe_left"] }`.
The gesture recognizer (`touch.rs`, now the graph's) moves to the hosts.

## Steps (each green and committed)

1. **Placement**: `Control { at: Header | Item, … }` on `Action`;
   `title()` and `status()` on views. Hosts draw the header for every view;
   views' own toolbar rows go (chat, graph, spec, queue, inbox, code).
   Spec's section tabs become a header picker. Chat's Send is an action.
2. **Item strip**: the host draws the cursor item's controls under it.
   Per-item buttons go (spec review points, queue jobs, graph Go into and
   levels, code Re-read). A request's answers become its item controls
   (the card keeps the question and form fields).
3. **Pointer grammar**: the host hit-tests `spots` (click: move + focus;
   double: open; long-press: the item's actions). Views' own item click
   handling goes (spec items, tree rows, queue rows, chat items, inbox
   parts). Graph cards keep drag, pan and zoom.
4. **Gestures as bindings**: `gestures = [...]` in the keymap; the
   recognizer runs in the host; swipe bindings per view (chat: slots; graph:
   back and forward).
5. **Top bar** as the shell's header (its buttons become actions).
6. **Lint**: outside the hosts, the shared control and the developer tools,
   no code draws a button, link, checkbox, picker or clickable label.

## Later: the mobile pass (structure ready after the steps above)

- Column count by screen width: one on a phone, two on an unfolded phone
  (`[columns]` widths).
- Tap-size controls on touch (the key bar's narrow mode, for headers and
  item strips).
- Which swipes go where (sessions, views, columns), decided with the user.

## Tests

- Each step's behaviour through the app (a click on an item moves the
  cursor; a double-click opens; a control runs its action; a swipe runs its
  binding), not tests of the drawing code against itself.
- The lint.

## Where you are: one bound list, in layers (agreed after steps 1–6)

The shared protocol every surface reads, built once per frame by one
function: gather (the panel's actions, its host's, the shell's) → place
(the panel's placements, its host's, the top bar's) → bind (keys from the
keymap) → tag each with its layer:

1. **On**: what you're on (the item's, or the typing box's).
2. **Panel**: the focused panel's own, and the moves and go-tos that act
   on its cursor (declared on the action, not listed by prefix).
3. **Host**: what holds it (the quick look's, the column's tabs and close).
4. **Everywhere**: the rest (columns, servers, palette, the bell…).

Readers: key handling; the key bar (On → Panel → Host, by importance);
the `?` sheet (a section per layer, a mode's keys first while typing or in
the palette, every way to run each: keys, leader, gesture); the leader menu
and the palette (closest layer first); every control (drawn from its bound
action: caption and its key; a panel not focused shows no keys, they don't
run it until it is). Views get no key table and never name keys
(`hints`, `key_of`, `key_tip` go). One behaviour test: every key the key
bar, the sheet and the controls show runs the action shown.

# Plan: tags that point, at every level, in every view (and the picker, the jump list)

The terminal-view-and-tags plan built tags with the terminal as their
first owner. Using it (2026-10-19) showed what's wrong with that core
before more owners are added, and a few cockpit problems besides. This
plan fixes the core, then adds chat and code as owners, then the rest.
Built as `dev_docs/process/build-and-validate.md` says.

## What's wrong now (found using it, 2026-10-19)

- A tag copies what it's on into the database (up to 4,000 characters),
  and a tagged command keeps only its output's last 20 lines: a stub of
  the command. A tag should point, and the thing should come back whole.
- `M` on a line of output tags that line alone, with nothing to say which
  command printed it; nothing says what's about to be tagged.
- The terminal keeps 10,000 lines (fixed in code), in the server's
  memory only: a server restart loses all of it, tagged or not.
- Chips are painted over the text they mark.
- The Tags view has its actions only in the palette and `Space ,` (no
  buttons), nothing folds, a note is edited by typing it again, when a
  thing was tagged isn't shown, and it has its own "take the tag off" beside
  the shell's.
- `#` in the chat box offers nothing: the name has to be known.
- The palette grows oddly while typing (wider than its content, pinned
  left, then the whole page), a description runs into its key, and most
  actions have no description.
- `ctrl+o` after a jump inside one terminal goes back to the terminal,
  not the line.

## Agreed (with the user, 2026-10-19)

- **A tag only points.** The owner of a thing gives it back, and knows
  whether it lasts. Things that last are only pointed at (code, the graph).
  Things that don't, or that can't be fetched back one at a time (terminal
  output; chat, since ACP replays whole sessions and has no paging), are
  written by their owner to our own tag logs when tagged: files under
  `.nah/state/`, split by size, each with a header (what, where from,
  date, terminal or session). The database keeps where, never the text.
  Nothing untagged is logged.
- **Things nest, and a tag is on one level.** Terminal: command, then
  lines. Chat: session, then turn, then message. Code: file, then symbol
  (inside symbol), then lines. `M` tags the unit (a command with all its
  output, a message, the innermost symbol); a selection means exactly the
  lines selected. The tag picker's top line says what's being tagged, and
  `Tab` cycles the levels at the cursor, in the same picker as the name.
- **A turn** shows an agent your message, the answer and a line per tool
  call by default; all of it on request. A tag is a set: two messages
  under one tag are connected by it.
- Full sessions stay loaded as they are (how sessions are held is in
  `TODO.md`).
- Keys stay: `M`, `g T`, `] t` / `[ t`, `]]` / `[[` by command.
- **`ctrl+o`** goes back to the exact spot left, in every view (a
  terminal's line too); each go-to and each `] t` is a jump. A line gone:
  the nearest there is. A page reload doesn't bring a terminal's cursor
  back (the live screen).
- Agents with no key get nothing (done: ca36e61); who may connect and do
  what is a later discussion (`TODO.md`).
- The palette: measure what happens first, then decide what reads well.
- A new tag's colour is the theme's least used one (as built).

## Rules (each checked by a test or a lint)

1. A tag never holds what it's on: the database has no subject's text.
2. Each kind of thing has one owner, which names it, gives it back
   (from its source, or from the tag logs) and says what it nests in.
   Tags know no owner (as before).
3. What's tagged comes back whole, for as long as the tag is on it.
4. A buffer only says what levels are at a point and where things are
   drawn; the shell tags, the host draws tags (as before).
5. Nothing is drawn over a view's content to mark a tag.
6. Every action has a one-line description.

## Part A: tags point, owners keep what doesn't last

A.1 **The tag logs** (in `nah-tags`, owner-blind): an owner hands what a
    thing says to the log when it's tagged, and appends while it grows (a
    command still running); the log gives back an extent (file, start,
    end). Files under `.nah/state/tagged/`, a new one past a size (8 MB),
    each with a header (owner, what, terminal or session, date). A file no
    tag points into is removed. Searchable with the file search already
    used (`fff-search`) over that folder.
A.2 **`Resolve`** gains what A.1 needs: whether a thing lasts, and (for
    one that doesn't) writing it to the log when tagged; reading a tagged
    thing gives it whole, from its source while it's there, else from the
    log. `Tagged::said` goes; the state database's layout moves the one
    kept tag over (its text into a log).
A.3 Events carry what changed (a thing added or taken off, a rename),
    not the whole tag twice.
A.4 Tests: a tagged command whose output is longer than the scrollback
    comes back whole after a server restart; the database holds no text
    (rule 1); an untagged command leaves no file; a file nothing points
    into goes.

## Part B: levels

B.1 `Navigable::subject(key)` becomes the levels at a key, narrowest
    first, each a subject with its words ("command 12: `$ ls -la`",
    "lines 104–106 of command 12"), the unit marked. A selection gives
    its lines only.
B.2 The tag picker's top line says what's tagged; `Tab` cycles the
    levels; the rest of the picker is unchanged.
B.3 Tests: on an output line, the command by default; `Tab` to the line;
    a selection is its lines; each level is what's sent.

## Part C: the terminal

C.1 A command is the unit: tagged, its command line and all its output
    go to the log, and keep going until it ends.
C.2 Lines record the command they're in (its number), said with them.
C.3 Scrollback is a setting: `.nah/terminals.toml` `scrollback_lines`
    (10,000).
C.4 Tests against bash: a command with 50,000 lines of output, tagged
    while running, comes back whole.

## Part D: chat

D.1 Subjects: a message, a turn (in a session, by the session's native
    id; how a message is named so it survives a reload is checked first:
    our item ids may be renumbered when a session is replayed).
D.2 Tagged, the owner writes it to the log (a turn: every item of it).
D.3 A turn is said as your message, the answer and a line per tool call;
    `tag_read` with `full` gives everything.
D.4 The chat buffer gives levels (message, turn) and where each is drawn.

## Part E: code and the graph

E.1 Subjects: a symbol (a component: it follows edits, the index tracks
    it); lines of a file at a snapshot, mapped to the files now through
    the changes crate (`on_files`): changed or gone said as such.
E.2 The code view gives levels (lines, innermost symbol, the symbols
    around it); the graph and the outline give the component.
E.3 Nothing is logged: code lasts (git, our snapshots).

## Part F: the cockpit

F.1 **The tag gutter** (rule 5): a strip down the left of every view that
    shows tagged things (the code view's gutter takes it), drawn by the
    host from `drawn_at`: a bar in the tag's colour spanning the thing's
    rows, several side by side. Names: the panel's top line for the
    cursor's line, on hover or a long press on a bar, and in the Tags
    view. Chips over content go.
F.2 **The Tags view:** buttons on the cursor's row (a tag: rename,
    colour, note, delete; a thing: go to it, take the tag off); each tag
    folds to one line (name, colour, count, note) with the `z` keys,
    folded to start, the one gone to opened; when each thing was tagged
    shown; the note opens in its field to edit; one "take the tag off"
    (the shell's).
    The Tags view adds no keys, buttons or pickers of its own: it answers
    the shared moves, folds and search, declares its actions (the host
    draws their buttons), and uses the palette's pickers.
F.2a **How tags look:** where a tag is named (the Tags view, the tag
    picker, a panel's top line), a solid tag icon in its colour (the icon
    set's filled style loaded as a second font family, for other filled
    icons later too); in the gutter, the bars. A choice with a colour
    shows a swatch before its name, in every picker (the recolour picker
    first).
F.3 Tag actions on a tagged thing in any view (rename, colour, note of
    the tags on it).
F.4 **`#` in the chat box** opens the fuzzy tag picker (name, note, how
    many things) and puts the name in.

## Part G: the palette

G.1 Measure first: in the user's Chrome, open each kind of picker and
    type; rapid screenshots with the picker's width and place each frame;
    find which rows make it grow and why past its content.
G.2 Then fix it to what reads well (likely: as wide as its content needs
    within bounds, centred, no shrinking back while one is open); a
    description on its own line under its label, wrapped, never into the
    key.
G.3 Every action gets a one-line description; a lint fails without one.

## Part H: the jump list

H.1 A terminal's place keeps its line for the jump list and marks; the
    layout saved for a reload drops it (rule B.2 of the last plan).
H.2 Tests: `] t` twice then `ctrl+o` twice; a line gone since; a reload.

## Process

Built as `dev_docs/process/build-and-validate.md` says: each step tested
and checked live as a user, then committed. Stopping points (linters, the
whole suite, the live pass, the reviewers in
`dev_docs/process/reviewers.toml`, fixes, findings recorded here):

1. After the palette and the jump list (steps 1–2): consistency,
   correctness; the palette measured again.
2. After tags that point and their levels, with the terminal (3–5):
   responsibility, correctness, permissions, performance.
3. After the cockpit's part (6–7): every reviewer; touch and a phone-width
   window included.
4. At the end (8–10): every reviewer on the whole change, and a full "day
   in the life" live run.

## Steps

1. The palette: measure (G.1), fix (G.2), descriptions and their lint
   (G.3).
2. The jump list (H).
3. The tag logs and `Resolve` (A); the one kept tag moved over.
4. Levels (B), and the picker's top line and `Tab`.
5. The terminal as an owner of levels and logs (C).
6. The tag gutter (F.1); tag actions on tagged things (F.3).
7. The Tags view (F.2, F.2a); `#` in the chat box (F.4).
8. Chat as an owner (D).
9. Code and the graph as owners (E).
10. README.

## Open (★)

- Marks become tags (settled before; still after this plan).

## Checklist

- [x] 1 the palette
- [x] 2 the jump list
- [x] 3 tag logs, tags that point
- [x] 4 levels and the picker
- [x] 5 the terminal
- [x] 6 the tag gutter, actions on tagged things
- [x] 7 the Tags view, `#` in the chat box
- [x] 8 chat
- [x] 9 code and the graph
- [x] 10 README

## Stopping point 1 (after steps 1–2): findings and what was done

Reviewers: consistency, correctness (fresh agents, the brief in
`dev_docs/process/reviewers.toml`).

Fixed:
- A place without a line, gone back to in the same terminal, now goes to
  its live screen (cursor and selection dropped), as leaving does.
- Going back to a closed terminal says so (it was silent).
- A jump's line follows new layouts (a new width): each layout's shift is
  kept per start of the shell, and a place says how many it had seen.
- A place taken while a go-to is under way is where it's going.
- While a program has the whole screen, a go-to waits for it to leave.
- Descriptions show where actions are: the leader sheet and control hover
  tips say them, and the palette's search reads them.
- Where a view's move means something else than the shared words say
  (the terminal's Enter and leave, the queue's, the outline's left and
  right), the view says its own; the shared left/right no longer speak
  of trees only. Setting a mark no longer names a key.
- An action without words fails in debug builds (`Action::new`,
  `.detail`); a harness's command or setting without one gets words.
- The palette's rows use egui's `Sides`; its title is one line, cut; it
  never gets shorter while open (a list to a text field).

Kept, said here:
- `kept_place` stays beside `keep`: a place is where you are; what's kept
  for a reload is that place without what's only for now (a terminal's
  cursor). One hook, defaulting to the place.
- `Escape`/`G`/`gg` in a terminal change its place (as `G` is a jump in
  vim): kept.

## Stopping point 2 (after steps 3–5): findings and what was done

Reviewers: responsibility, correctness, permissions, performance.

Fixed:
- A command followed while it runs wrote a piece, rewrote the whole tag
  in the database and told it on the event stream for every chunk of
  output (quadratic, on the terminal's reading thread, under two locks).
  Now a growing thing gets a log file of its own, read to its end; what
  it adds is appended there by its owner (no lock of the tags, no store,
  no event). One change is kept and told: the tagging.
- Taking the tag off (or deleting it) didn't stop the following; tagged
  again, it was written twice. Following stops once nothing points into
  its file (`More` says when it's no longer wanted).
- `tag()` checked under one lock and wrote under another: a deleted tag
  could come back, a name be made twice, a file just written be removed.
  It now holds the lock throughout (the owner's following needs none of
  the tags'). A failed tag leaves nothing written or followed. Files no
  tag points into are removed at start-up too.
- A command partly scrolled away read back cut short, said to be "as it
  is now": a thing the logs keep is read from the logs (all of it); where
  it is (still there, gone) is the owner's to say (`Resolve::there`).
- Following lost everything after a `clear`, or past a small scrollback:
  it goes on from the oldest line kept, saying how many went before they
  could be kept. Blank lines are kept; a line still wrapping onto the
  cursor's row waits until it's whole.
- Reads (the Tags view, `tag_read`, a prompt's `#name`) read only the
  start and count the lines, never the whole text.
- The logs are the server's user's only (folder 0700, files 0600), as the
  state database is.
- Lines are named "line 5 (of command 3)", as the picker says them; their
  command found without copying every command.
- The tag picker's pick belongs to its opening and is kept by subject (not
  by its place in a list, nor reset by the palette's drawing); the cycle
  action is found by its origin too. "Where it is" words are said in one
  place (`Held::words`). `tag_read` past the end says how many lines
  there are.
- A.1's search: `tag_find` (view.read) searches what tagged things say,
  where it's kept, each line with its tag, thing and line. Not with
  fff-search: its walk keeps to the project's ignore rules (which leave
  `.nah` out) and gives lines by file; searching each thing's text gives
  what it is directly.
- A.3 (events say what changed) isn't needed for size any more: a tag is
  pointers, and following tells nothing.

Kept, said here:
- Headers in the logs can be imitated by what a command prints; the tags
  (the database) are what's tagged, the headers a reading aid.
- A jump's line after a new width moves by how far the end moved, as the
  cursor does (the server moves each line exactly); it's kept per buffer.
- The picker says "command 3 and its output: make"; the tag keeps the
  owner's name ("command 3"), which also names it back.
- Progress bars that redraw rows above the cursor are logged as first
  written.

## Stopping point 3 (after steps 6–7): findings and what was done

Reviewers: all seven. Live: the gutter, the Tags view, a phone-width
window and a long press (headless Chromium, touch emulated).

Fixed:
- A tag edit worked out its tag again when it ran (from the cursor, or a
  pick kept by guessing the palette's next opening): a confirmation could
  delete another tag than it named. Now what runs carries its tag (and a
  take-off its thing); a tag gone since is said, nothing done.
- The Tags view's buttons were laid out by the shell guessing which rows
  are a tag's: now the view places the shell's actions on its rows
  (`Hosted::placed`, as the graph and code do).
- "Name a tag" was offered in every view and left a terminal typing: it's
  offered where the view takes typed text (`Navigable::takes_typed`). The
  `#` picker opens only for a `#` just typed where a word starts (as a
  prompt reads names: after a space or `( [ { " '`), never with no tags;
  Escape goes back to typing (the palette returns to typing when that's
  where it was opened from); the caret goes after the name put in.
- Taking a tag off a Tags-view row took it off the first range containing
  the row's (overlapping ranges): now exactly the row's.
- The gutter dropped a fourth tag silently: four columns, more tags share
  the last one, muted; a bar's hover (and a long press, through the
  cockpit's own gesture recogniser and its timing) names every tag there.
- Search in the Tags view finds things under folded tags (going there
  unfolds them); a deleted tag leaves no "unfolded" behind.
- A note can be cleared with several tags at the cursor (which first);
  delete is the Tags view's only.
- A colour itself shows as a swatch; a tag's colour as a solid tag.
- The tag edits have keys: `Space k r/c/n/x/d/o` (a "Tags" group). ★ keys
  for the user to confirm.
- The tags at the cursor are worked out once per gather; the tag pickers'
  rows only for the picker open.
- The logs folder: never written through, chmodded or emptied through a
  link.
- Tests: the top line's naming checked for real (it passed on the note
  before), colours distinct per tag, Backspace/`zR`/`zM`/Enter in the Tags
  view, a stale confirmation, overlapping ranges, the folder's mode.

Kept, said here:
- `#` opens the palette's fuzzy picker (name, note, how many things, as
  F.4 asks) rather than suggestions under the box as `/` shows commands:
  `/`'s list is short and matched by prefix; tags need the picker.
- The chat asks for the picker by its action's id; if that id changes, a
  test fails.
- A note is edited in the palette's one-line field.
- Agents (same OS user) can read the logs with their own tools: in the
  TODO's access notes, as is an agent driving the cockpit acting as the
  user.

## Stopping point 4 (the end): findings and what was done

Reviewers: all seven, on the whole change (b29fbfc on).

Fixed:
- Critical: tagged code lines were read wherever their path pointed (an
  absolute path, `../`, a link out of the project): any agent with
  `tags.edit` could read the cockpit's token. Now only a file inside the
  project, however named or linked, and not one it leaves out.
- An agent could tag and read any slot's chat (tool output included)
  through tags, past `slots.observe`: now, as `slot_read`, its own slot's,
  or with `slots.observe` a sub-slot's (the user any); `tag_read` says the
  others are a chat it can't read, `tag_find` skips them.
- A growing thing's own log had no limit: past 64 MB it's said and no
  longer followed. A thing's lines are read in a window (`thing_lines`),
  never the whole file for a few lines.
- A chat turn tagged while running was kept cut short: it's followed until
  the turn ends (the rest appended; all of it again, said, if it changed
  rather than grew). A turn's text has its thinking, plans, to-dos, diffs
  and what each tool was asked, as detail.
- Code lines: read where they are now (or, changed or gone, as their
  snapshot has them, said "as it was when tagged"); the same lines are
  one thing whatever snapshot they're kept at (no doubles; an agent takes
  them off by their name); refreshed only when the code's ingested (not
  on plan edits); components named with their id first, named back by it.
- Chat names always carry the session's id start (`chat lead@4f2a91c0
  turn 3`), so a slot's new session doesn't take an old tag's place;
  `chat session <id's start>` names one no slot holds. A chat thing's
  presence is checked without building its text.
- The cockpit's chat worked out where each item is for every item, every
  frame (whole-thread copies): once per change of the thread now.
- A name any owner reads rightly wins over another owner's "that's mine
  but wrong" (code read `terminal t1@5 line 4` as a file's lines).
- `tag_find` matches as every text search here does (`arch_search`), and
  `tag_read` with `lines` numbers lines as `tag_find` does (folding only
  the start it shows; the detail mark made rarer).
- A finished command tagged late says how many of its lines went before.
  "Name a tag" from the palette puts `#name`. `scrollback_lines` under 100
  is refused, not raised. Agents are told how chat and code are named.
- Tests: a restart reads back what the logs kept; nothing untagged is
  logged; tagging again while running is proven followed; more margin for
  the 50,000-line run; a real search in the Tags view; a running chat turn
  kept whole; nothing outside the project.

Kept, said here (for the user):
- A.3 isn't built: a tag's event is the whole tag (pointers only, small).
- A message added to a running turn makes turns number differently after
  a replay (D.1's check); not handled.
- No session-level chat thing, no file-level code thing, no multi-line
  selection in the code view (lines one at a time there; agents name any).
- Snapshots the code tags take aren't pruned when nothing points to them.
- The picker's words for a thing and the owner's name for it differ in
  wording (both said; the owner's is the one stored and named back).
- Layout 10 keeps the first piece of a kept thing logged by this branch's
  first build (pieces existed only there).
- Agents running as the server's OS user can read `.nah/state` with their
  own tools; an agent driving the cockpit acts as the user (both in the
  TODO's access notes).

## As built (deviations and notes)

- Step 3: a tag's event still carries the whole tag (one event per change,
  never two): with `said` gone a tag is only pointers, and a whole tag is
  safe to apply in any order. A.3's "what changed" events aren't built.
- Step 3: the one kept tag isn't moved into a log: layout 9 drops `said`
  from every stored tag (rule 1 holds in the database); a thing of an
  earlier shell start tagged before this says "gone".
- Step 3: A.4's restart is the shell's (`restart`), not the server's: what
  comes back after either is read from the same files and the same rows.
  The test runs 30,000 lines (three scrollbacks) tagged while running.
- Log headers say the date as `Tue, 20 Oct 2026 21:13:57 GMT`.
- Step 4: `Navigable::subject` became `things` (narrowest first, the
  unit marked). Any picker can say what it acts on (`about`, its top line)
  and offer the list's cycle key (`[keymap.list] cycle`, `Tab`) to run
  another action while it stays open (`cycle`); tagging's is `tag.level`.
  Taking a tag off and stepping through a tag look at every thing at the
  cursor, not only the one tagging would pick.
- Step 5: lines say the command they're in through their name (the owner
  names them "lines 5-7 (of command 3)" while it knows; the name is kept
  with the tag), not a field of the subject: the command follows from the
  lines, and a subject stays what identifies the thing. C.1 was built in
  step 3 (a command followed while it runs).
- Steps 6–7: the tag edits (rename, colour, note, clear the note,
  delete, take off) are the shell's, on the tags at the cursor in any
  view; with several there, the tag is picked first and its value asked
  next (the field starts with what's there). A view of tags says which
  tag a row is (`Navigable::tag_at`), and the edits are its rows'
  buttons; it has no actions of its own but its moves and folds. Any
  choice can carry a colour (shown as a solid tag in it): the tag
  pickers, the colours. `#` starting a word in the chat box opens the
  tag picker (`tag.insert`) and the name goes after it
  (`Navigable::typed_in`). Not checked live: the user's own draft is in
  the chat box (covered by a test instead).
- Step 8 (chat): a turn is named by its number in the session (the
  harness's id for it, which outlives our threads; `Turn::ordinal`), a
  message by its number among its turn's messages (what you said, the
  answers; tool calls aren't messages): `message_number`, one function
  both ends use. Checked first (D.1): our item and turn ids are new on
  every replay, but a replayed session makes one turn per message you
  sent, in order, as the live one did; a message added to a running turn
  (`Add to what it's doing`) is one turn live and two once replayed, so
  turns after it number differently after a reload. Not followed while
  it grows: a turn tagged while it runs is kept as it was then. A turn's
  tool calls are a line each, what each gave under it as detail lines
  (`DETAIL`), left out of reads unless `full`. Not checked live: there's
  no agent session in the sandbox but the user's own (covered by tests:
  the fake harness, now naming each session apart as harnesses do).
- Step 9 (code): owners can settle a subject as it's tagged
  (`Resolve::settle`): lines of a file get the snapshot of the files then
  (the last one again while nothing changed). When the code changes (the
  index's `Changed`), the tags are refreshed: lines moved by changes
  before them are kept where they are now, with a new snapshot; a change
  among them leaves them, named "(changed since tagged)"; their file gone,
  "(its file's gone)". A snapshot pruned since: the lines are taken as
  numbered. A file renamed isn't followed. The graph gives the card's
  component and draws no gutter (cards aren't rows); the outline and the
  code view do.

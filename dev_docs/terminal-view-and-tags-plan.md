# Plan: the terminal's view, its commands, and tags for everything

Written 2026-10-17 after the terminal plan, from what the user found
using it (flicker, a cursor left behind, output running off the bottom)
and what they want next (line actions, and tags across the whole system
that agents can be pointed at). Nothing here is built. Each step is
tested, checked live and committed on its own; **★** marks what the user
decides.


## What's wrong now, and why (measured)

- **Flicker.** Every keystroke draws one frame with the terminal empty.
  "Back to the live screen" set egui's scroll offset past the end of the
  content; `show_rows` picks the visible rows from that offset before
  egui clamps it, so it draws none for a frame. A known egui failure
  (md-viewer #140). Terminals don't scroll in pixels: they keep a whole
  number of lines back from the live screen (Alacritty's
  `display_offset`), and so does egui_term, the egui widget on the same
  backend: no `ScrollArea` for the grid.
- **A highlight left behind.** The not-typing cursor is a line number
  that nothing ends: typing doesn't, `clear` doesn't, and it's saved with
  the buffer's place.
- **Output runs off the bottom.** The view follows the output only while
  that cursor is unset, and it never gets unset.


## Rules (each checked by a test or a lint)

1. **The terminal's view is a whole number of lines back from the live
   screen.** Every frame draws exactly a screen's rows from what the
   buffer holds; no pixel scroll state, no frame without rows.
2. **A line has one number for its life**: lines ever pushed into the
   scrollback, from the server. Following output, fetching, the cursor,
   selections, commands and tags all use it.
3. **The not-typing cursor exists only while not typing**, on a line that
   exists.
4. **A tag points at a subject, never at a place on screen.** A subject
   is a durable reference to one thing in the system (a component, lines
   of code, a chat message, a terminal command…), kept with what it said
   when tagged.
5. **Tagging is one action of the shell, for every buffer.** A buffer
   only says what subject an item it draws is; the host draws tags on
   what's drawn. No buffer has tagging code of its own.
6. **Tags know no subject's owner.** The tag store asks each owner (code,
   chat, terminals…) through one interface what a subject says now;
   owners register at start-up.


## Part A: the terminal's view

A.1 **Line numbers (server).** `nah-terminal` numbers lines by how many
    were ever pushed into the scrollback: a screen update says the number
    of its first row (`top`), lines are asked for and sent by number.
    alacritty counts nothing past its scrollback's cap (checked:
    `Grid::scroll_up` only grows the history), so the crate counts the
    lines that scroll off the top itself. **Spike first**: the cleanest
    place to count (a thin wrapper over the emulator's handler that sees
    each line feed that scrolls the whole screen), with a test past the
    cap.
    **Spike results (step 1, 2026-10-17).** A wrapper over the
    emulator's handler is the wrong place: about 70 methods to forward
    (a later version's new one would be silently dropped), and lines go
    into the scrollback without a line feed too (wrapping at the line's
    end, scroll-up sequences, clearing the screen, a resize). Measuring is
    exact instead: the emulator gets 64k lines of room past ours and
    `nah-terminal` cuts its scrollback back to what's kept (in batches of
    4096), counting what goes; a line's number is lines gone + the
    scrollback's size + its row. Two things would hide lines from the
    count, so the crate controls them: a clear of the scrollback (the
    output is cut just before its last byte, to count what it takes:
    found with vte's own parser, `scan.rs`, which also replaces our
    hand-written OSC 133 reader), and synchronized updates, which the
    emulator held inside itself (Claude Code and other TUIs draw with
    them) and now `feed.rs` holds instead, feeding one whole when it ends.
    That also fixed a frozen screen: nothing ended a held update whose
    program never did (now 150 ms, on a small thread per terminal). The
    alternate screen leaves the numbering alone; if the emulator were ever
    flooded past its room (a burst of scroll sequences), every line is
    numbered past all numbers given, never reusing one. Cost: 400,000
    coloured lines (24 MB) feed in 320 ms against the emulator's own 200
    (the second parse is 27 ms; the rest is the emulator re-allocating
    rows the cut frees), about 75 MB/s. Also found: a reader still
    connected when its shell was started again never got the new screen
    whole (fixed: the socket sends it whole on a new start). For tags: a
    new start numbers from 0 again, so a terminal-lines subject needs
    which start it was (`Open::started`).

A.2 **The view (cockpit), its own module** (`terminal/view.rs`, no
    drawing in it): `lines_back`, `follow` (0 and new output keeps it 0;
    above 0 new output adds what it pushed, so what's read stays put),
    the wheel's pixels kept until they make a line, the scrollbar's
    geometry. Moves by keys go through the shared moving rules
    (`viewport.rs`), turned into whole lines, so `j`/`k`, half pages and
    `gg`/`G` work as in every buffer.
A.3 **The lines (cockpit), its own module** (`terminal/lines.rs`): the
    screen, and scrollback by line number, fetched a screenful ahead of
    what shows; an old line stays until it's replaced, never drawn
    blank. Dropped when the server says the scrollback was cleared.
A.4 **Drawing**: the grid painted from A.2 and A.3, a row per line, no
    `ScrollArea`; a thin scrollbar drawn from `lines_back`. Typing (and
    any input) returns to the live screen.
A.5 Tests: no frame without rows (typing, fast Enters, resizing,
    scrolling), the view stays on what's read while output arrives, past
    the cap too; and the flicker measured live as before (rapid
    screenshots: no blank frame).

    **Built (step 3).** `terminal/lines.rs` and `terminal/view.rs` as
    planned; the view is held by the number of its top line (rather than
    "lines back"), so output arriving below needs no adjusting. Two
    additions so no line is ever drawn blank: the server sends the lines
    that scroll off the screen (the screen's height at most) before the
    update that moves it, and the cockpit keeps rows that scrolled off as
    they showed until the server's come (still fetched: they may have
    changed in the same frame). Line answers are scrollback only (the
    screen's rows change; they come with the screen). Going far back,
    what showed stays up to half a second until the lines come. A
    scrollbar (drag the thumb, click the track) and a finger scroll the
    view; the wheel's pixels make whole lines. Measured live: 103
    screenshots in 4 s while typing and pressing Enter fast: no frame
    dips (before: one dark frame per key); scrolled back while a loop
    printed, the lines read stayed put. Found live and fixed (each a
    test first): scrolled back while typing, the browser scrolled the
    whole page to the program's cursor (egui's hidden text field went
    with it, far below): the cursor's place for the browser now stays on
    the grid. A frame with 10,000 lines of scrollback costs what one with
    a screenful does (about 2 ms in the test build).

**Stopping point 1 (2026-10-17): what the reviewers found, and what was done.**
Performance and correctness reviewers (fresh, given only the plan, the
system's description and the diff). All fixed unless said, each with a
test first:
- A new width re-wraps the scrollback (alacritty reflows), so numbers
  moved and, wider, were given again. Decided: a new width lays the
  lines out anew, numbered past every number given (none given twice);
  readers keep their place by its distance from the live screen and drop
  selections and search hits. Only taller or shorter keeps every number.
  (For tags on terminal lines this means a new width ends those lines:
  step 8 resolves them by their text.)
- A reset (`ESC c`) gave the new screen the old screen's numbers; the
  alternate screen entered mid-read left the count stale. Both are cut
  out of the output like clears.
- An update held past its wait didn't write the program's answers
  (a program asking where the cursor is hung) and a program ending
  inside one never showed its last output.
- A reader of an earlier start of the shell kept that start's lines
  (numbers start again): updates say which start they are (`shell`).
- The server now resends a row that scrolled off only if it changed
  after the reader was sent it (both apply updates the one way,
  `ReaderScreen` in the contracts); busy output was sent twice.
- Searching rebuilt every line every frame (the search's items are now
  the screen and the server's hits); the server cut its scrollback in a
  way that freed and remade a row per line (now it reuses them: feeding
  costs 1.12 times the emulator alone, was 1.6); reading the terminal
  took its lock on the server's async threads (now off them); a taller
  second panel froze while output was fast (following the live screen,
  nothing is held back); line asks are one at a time, a short answer
  isn't asked again until the next update, at most 1000 lines.
- Not changed: every output byte is parsed twice (ours and the
  emulator's; measured within the 1.12); a terminal has a second
  thread (for held updates). The not-typing cursor findings are step 4.

## Part B: the not-typing cursor

B.1 Starting to type drops it (the view follows again). Leaving typing
    puts it on the shell's cursor line.
B.2 Its line gone (`clear`, scrolled past the cap): it moves to the
    nearest line there is, or goes. Not saved with the place.
B.3 Its actions on a line: copy (there), tag (Part D).

**Built (step 4).** As planned; "the nearest line" is the first line
there is when its line went (`clear`), the last when the screen shrank.
Live: leaving typing puts it on the prompt's line; after `clear` it's on
the new prompt, nothing left behind.

## Part C: commands

C.1 The start-up hooks also mark where the prompt ends and the command
    starts (OSC 133 `A`, `B`; `C` and `D` are there): the crate keeps each
    command: its id, its text, its lines (prompt, command, output, by
    line number), its exit code and how long it ran.
C.2 Sent on the terminal's socket (and kept while its lines are), read by
    agents (`terminal_read` names them).
C.3 Not typing: the cursor moves by command (`[` `]`, ★ keys), and on
    one: copy its output, run it again (typed in, as the user would),
    tag it.
C.4 Tests against bash (the hooks), live.

**Built (step 5).** As planned, with these choices:
- Moving by command is the motion every buffer already has for "next
  thing at this level" (`nav.next_sibling` / `nav.prev_sibling`, keys
  `] ]` and `[ [`: next definition in the code, next change in Changes),
  rather than new keys `[` / `]`. ★ Say if single keys are wanted.
- A command is kept while its prompt's line is; updates carry the
  commands that changed (all of them on a whole screen).
- A new width moves the commands with their lines: the crate maps each
  place through the re-wrap (lines as the program wrote them, counted up
  from the cursor's), exact. If a program has the whole screen at the
  time (vim), the lines can't be read and the commands are dropped.
- Output that doesn't end its line (`printf x`) ends with the line, so
  the next prompt shares its last line.
- Copying a command's output whose lines aren't fetched yet asks for
  them and copies once they come. Run again types the text and Enter.
- Agents: `terminal_read` lists the last ten commands (number, text,
  exit code, time) and reads one's whole output by its number.
- Random-input tests (`proptest`, test-only): the output scan (however
  it's cut into reads, the same is seen), the key encoding (a paste can't
  end its bracket; every key is bytes), the terminal's links (what's
  found is within the line).

**Stopping point 2 (2026-10-17): what the reviewers found, and what was done.**
Correctness and consistency reviewers, on steps 4 and 5. Fixed, each
with a test first:
- The command line was read off the screen, so "run it again" could run
  something else (zsh's right prompt, `>` continuation lines, a wide
  character in the prompt, a space at the wrap). Now the shell says it
  (VS Code's `OSC 633 ; E`): zsh from `preexec`, bash from its history's
  newest entry when it took one; a command bash's history doesn't keep
  has no text, and isn't offered to run again. Nothing is read off the
  screen for it.
- The bash hook broke a `PROMPT_COMMAND` ending in `;` (a syntax error
  every prompt: nothing ran). bash 5.1's list form is used (the user's
  entries untouched); older bash gets one line with the trailing `;`
  taken off.
- "A long command finished" was lost when a command ran without a
  marked prompt, scrolled its prompt out, or was cleared under: a
  command now starts where it's run if no prompt was marked, and one
  that's running stays (from the oldest line there is).
- A prompt drawn again (Ctrl-L, Ctrl-C, completions) made a phantom
  command in the cockpit: it's the same command, moved.
- Run again only at an empty prompt (not into a running program, not
  over what's typed); copy and run again say why when they can't.
- Copying a command's output and an agent reading it now get the
  server's text (lines that wrapped joined, wide characters once:
  alacritty's own), all of it (an agent: the last 2000 lines); a pending
  copy goes with its terminal or its shell's start.
- `[[` in a program with the whole screen could underflow; agents' words
  for a command's end are the notifications' ("exit code 1, after 3 s").
- Known, not changed: output that doesn't end its line shares its last
  line with the next prompt; a command's number counts from 1 again in
  a new start of the shell.

## Part D: tags

**What a tag is** (settled): an id (the database's, stable when it's
renamed), a name the user gives (`flaky-build`, `auth-bug`), a colour and
a note, on any number of subjects; a subject can have several tags. Only
the name is asked for: the note starts empty, and the colour is the next
of the theme's tag colours (a theme palette, `tag_0`…`tag_7`, kept as its
index so the theme decides how it looks, as with a terminal's colours).

**Subjects** (`harness-contracts`, the shared vocabulary; one variant per
kind of thing, added as each owner supports it):

| Subject | Durable by | Now (resolved) | Kept when tagged |
|---|---|---|---|
| `Component { id }` | its id | its card | its name |
| `CodeLines { path, first, last, snapshot }` | a snapshot of the files when tagged | where those lines are now (the changes crate maps a snapshot's lines to the files: `on_files`) | the lines' text |
| `ChatMessage { thread, item }` | item id | the message | its text (clipped) |
| `Turn { thread, turn }` | turn id | the turn's messages | its first words |
| `TerminalLines { terminal, first, last }` | line numbers (A.1) | the lines while the scrollback has them | the lines' text |
| `TerminalCommand { terminal, command }` | command id (C.1) | command, output, code | the command and the output's end |
| later: a notification, a plan's section, a change, a review comment, a file | their ids | | |

A subject also turns into a `Target` (go to it with the go-to path).

**Where it lives (server).** A crate `nah-tags`: tags and what they're
on, kept in the state database (a migration), told on the event stream
(`TagUpdated`, `TagRemoved`); `tag`, `untag`, `rename`, `delete`, `list`.
It asks owners what a subject says now through one trait (`Resolve`:
"what is this, now, in text and where") that the code, chat and terminals
parts implement and register at start-up (rule 6).

**Agents.** MCP tools: `tags` (names, counts), `tag_read` (each subject:
what it is, where, what it says now and when tagged). In a prompt,
`#name` hands the agent that tag's subjects (expanded on the server when
the prompt is sent, so every harness gets it the same way). Agents read tags
with `view.read`, and tag, untag and edit with a new capability in
`nah-access`, `tags.edit` (the lead's, sub-slots' and workers' by
default; not an unknown agent's), as every tool declares what it needs.
Tags are a project server's (settled): its own subjects, its own database.

**Cockpit.**
- The desk keeps the tags (a service like the agents and terminals,
  read through a reader).
- A buffer says which subject an item it draws is
  (`Navigable::subject(key)`), the item under the cursor included. One
  shell action tags it: pick a tag or type a new one (the palette's
  picker). ★ Its key.
- The hosts draw tag chips on what's drawn: each spot a buffer reports
  (as hints use them) whose subject is tagged gets the tag's chip there.
  No buffer draws tags itself (rule 5).
- **Finding tags**: the palette's fuzzy picker lists them (name, note,
  how many things), wherever a tag is picked: tagging, going to one,
  managing them.
- **Going to what's tagged** (a go-to, like the others): pick a tag, then
  (when it's on several things) which one; it lands on that thing in the
  buffer that shows it (a chat message in the chat, lines in the code, a
  command in its terminal), through the go-to path (the subject turns
  into a `Target`), with the jump list. From a thing that's tagged,
  next / previous goes through the rest of its tag.
- **Managing tags: a Tags buffer** (a buffer kind like the inbox: every
  tag, with its colour, note and subjects; rename, recolour, edit the
  note, remove one subject, delete the tag; Enter on a subject goes to it
  as the go-to does). Editing a tag is there; going to what it's on is
  the go-to.

**Built (step 6).** As planned: `harness-contracts/tags.rs` (`Tag`,
`Tagged`, `Subject` with the terminal's two kinds, `SubjectNow`,
`TagRead`), `TagUpdated` / `TagRemoved` (kept by the hub for clients
that come later), `nah-tags` (`Tags`, `TagStore`, `Resolve`), the state
database's layout 8, `tags.edit` in the lead's, sub-slots' and workers'
roles, routes (`/api/tags`, `/api/tags/{tag}`, `…/edit`, `…/untag`;
reading `view.read`, changing `tags.edit`) and agent tools (`tags`,
`tag_read`; `tag`, `untag`, `tag_edit` with `tags.edit`). Choices:
- A tag's name is unique on its server and has no spaces (letters,
  digits, `- _ . /`, at most 40), so `#name` finds it in a prompt.
- Tagging asks the subject's owner what it says now: nothing to say (no
  owner, or it's gone) is refused rather than tagged blind.
- A tag left on nothing stays (a name the user made); delete removes it.
- The terminals' resolver (planned for step 8) was built here so tagging
  is tested end to end: a terminal's lines say their text, a command its
  line, how it ended and its output's last 20 lines; another start of
  the shell, or lines gone, say nothing (the tag keeps what they said).
- Agents name a terminal's subject by the terminal and a command's
  number or `lines: "120-124"`; `terminal_read` with `numbered` shows
  each row's number.

**Stopping point 3 (2026-10-17): what the reviewers found, and what was done.**
Responsibility, permissions and correctness reviewers, on step 6. Fixed:
- The agents' tools built terminal subjects themselves (the tags tools
  knew an owner): owners now name their subjects and read names back
  (`Resolve::named`: `terminal t1 command 3`, `terminal t1 lines
  120-124`, another start of the shell `terminal t1@<start> …`); the
  tools take a subject by its name, `tag_read` gives each one's name, so
  an agent can take a tag off what it's on in any start of the shell.
  Owners register themselves as they're made (start-up names none).
- A new width made every line tag "gone": the terminal keeps its last
  four re-wraps and follows a line's number through them (`moved`); the
  tag is kept where it is now.
- While a program has the whole screen, lines read its rows: lines say
  nothing then, and `terminal_read numbered` numbers no rows.
- Taking a tag off what it isn't on said it did: now it says it isn't.
- Sizes: what a subject said is kept up to 4000 characters, a tag is on
  500 things at most, at most 200 lines at once; tagging what a tag is
  already on changes and tells nothing.
- One kept tag this build can't read (a newer kind of subject) stopped
  the server starting: it's left as kept and not given.
- Names: letters, digits, `-`, `_` (a name goes in a link as it is, `#name`
  ends at punctuation), never shaped like an id; a tag is found by id
  first, and tagging by an existing tag's id adds to it.
- Who: each subject keeps who tagged it (`you`, or the agent's slot) and
  a note who wrote it; `tag_read` says so. Without the owner here (no
  shells run), `tag_read` doesn't give what a subject said.
- Kept as planned, said here: workers and sub-slots may change tags
  (`tags.edit`); a note is text any of them can write, now signed.

**Built (steps 7 and 8).** In the cockpit:
- The desk's tags (`Shared::tags`, a `Held<Tags>` buffers read through a
  reader) from the event stream; changes go through `Ask::Tags`.
- `Navigable::subject(key)` (what an item is, as a tag's subject) and
  `Navigable::drawn_at(subject)` (where it's drawn now): a buffer only
  answers these. The host draws the chips: for each tagged thing a
  buffer says it draws, a chip in the tag's colour at the row's right
  end. (Planned: chips on the spots hints use; a terminal's lines aren't
  hint spots, so the buffer is asked where a subject is instead.)
- One shell action tags what the cursor's on (`M`): the palette's picker
  lists the tags whose names have what's typed, and a new tag by that
  name. Taking a tag off picks from those on it. A terminal's subject: on
  a command's prompt line, the command; else the line (the selection's
  lines when the cursor's in it).
- Going to what's tagged (`g T`, since `g t` is the tree's go-to): every
  tagged thing in one fuzzy list (`tag · what`), landing through the
  go-to path (`Target::Subject`); the terminal shows it once its lines
  come, cursor on the line, not typing. `] t` / `[ t` step through the
  rest of what the tag on what the cursor's on is on.
- The Tags buffer (`b T`): every tag with its colour, note (and who wrote
  it) and things (what each is, what it said, what it says now when
  that's different or gone, who tagged it); rename, recolour (eight
  colours by name), note, take off a thing, delete (picked twice);
  Enter goes to a thing. Tag colours are the theme's `tag_0`…`tag_7`.
- Step 8's parts: the terminals' resolver was built in step 6; the
  terminal's `subject` and `drawn_at` here.
- Found on the way: a finger dragged on the code's text didn't move it
  (egui gives a press to what was drawn the frame before, and code lines
  are selectable): once a finger's been on the code, its text isn't
  selectable, so a drag moves it. (The test that showed it had passed by
  luck: its finger landed between lines until the panel header grew a
  row for the Tags tab.)

**Built (step 9).** `Tags::expand`: a prompt naming tags (`#name` at a
word's start; what follows a name, a full stop say, isn't part of it)
gets, after it, each tag's note and what it's on as it is now (a thing
gone: as it was when tagged), up to about 16 KB. The user's prompts to
a slot and the lead's messages to sub-slots (`slot_send`) are expanded
on the server, so every harness gets the same; a `#word` no tag has is
left as it is. The chat shows what the agent got.

**Stopping point 4 (2026-10-17): what the reviewers found, and what was done.**
Every reviewer, on steps 7–9. Fixed:
- Tags on lines went stale after a new width: the server moved them only
  when read, so chips vanished and `g T`, `] t` and untagging missed the
  lines. The terminals now tell the tags when lines move (a new width, a
  shell started again or closed) and the tags keep them where they are,
  told on the event stream.
- The cockpit named subjects and routed them to a kind of buffer itself
  (`in_words`, `shown_in`, `Target::Subject`): every new owner would have
  changed the core. Now the owner's name is kept with each thing
  (`Tagged::what`, kept up to date), a subject turns into a `Target`
  (terminal lines, a terminal's command), and a go-to lands in the kind
  of buffer that can show it (`Ask::Show` without a kind).
- A tag on several lines couldn't be stepped through or taken off from
  its lines: a subject says whether it has another in it
  (`Subject::contains`).
- `g T` was one flat list of every tag's things, by position: now the
  tag (its name, how many things, its note), then which thing, by the
  thing itself. The Tags buffer's rows are keyed by the thing too: a
  change meanwhile can't put the cursor on another. `] t` follows the
  tag stepped last.
- The tag picker matched part of a name, case and all, and offered a new
  tag last (Enter took the first match): it finds as search does (names,
  then notes), a new tag first unless one has the name (the server finds
  names in any capitals too), and only names a tag can have.
- A late reply could overwrite a newer event (a deleted tag came back):
  events are the one source; a reply says what came of the change.
- Going to a thing in a closed terminal reconnected every frame: it says
  the terminal is closed. In a shell gone with the server: said gone
  (it waited for lines that never came). Landing faked focus to keep
  from typing: a `landing` flag. A go-to waiting in one terminal ended
  when another was shown.
- Found live: a go-to onto the line the view ends on set no cursor (the
  view moved nothing); `ctrl+o` couldn't return to a line in the same
  terminal (its place is only the terminal: kept so by B.2, see stopping
  point 5);
  lines selected with the mouse weren't what `M` tagged (the cursor
  stayed where it was: a click puts it on a line, a drag where it ends,
  and a selection starts where it was pressed, not a line further on);
  bash's `ignoredups` (Debian's default) lost the command line of a
  command run twice in a row (the hook now lets the line into the
  history while it's read and takes it out after, as bash would have);
  the activity said "on 1 things".
- Who untags, renames, recolours, notes and deletes wasn't kept: each
  change is kept in the activity record with who made it. The user is
  `user` there and in a tag (an agent read "tagged by you" as itself);
  the cockpit says "you", agents read "the user".
- The code view's own touch flag never reset (one tap stopped the mouse
  selecting code): the host says whether the pointer is a finger, for
  every view (its own commit). A first touch that drags on code text
  still selects: in `TODO.md`.
- Smaller: the shell's searches go to their action; colour names come
  from the theme's colours; deleting asks yes or no; a note can be
  cleared; what a tag's things say now is read once the cursor rests on
  it, and again after a resync; at most 200 lines are tagged from the
  cockpit and what lines say is bounded; nothing is tagged while a
  program has the whole screen.
- Tests: the bodies sent are checked, events come through the stream,
  replies are answered (`NetClient::answer`), chips are checked against
  their rows, a go-to into another terminal and back with `ctrl+o`, the
  Tags buffer's actions.

Kept, said here:
- ★ Chips are placed where a buffer says a thing is drawn
  (`Navigable::drawn_at`), not on hint spots (planned): a terminal's
  lines aren't spots (every line labelled for hints would be noise).
  A chip sits at the row's right end, over a long line's last letters.
- ★ Permissions (older than tags, wider with them): an agent connecting
  with no key gets the `external` role, which may read; `tag_read` gives
  it what tagged terminal lines said, kept after the scrollback's gone
  (a tagged `export TOKEN=…` stays readable). An agent driving the
  cockpit (`view.drive`) tags as the user. A worker can't message other
  agents, but can write a tag's note they read.
- Each change sends the whole tag twice (its reply and its event); the
  server holds its tags while writing one to the database. Small now.

**Stopping point 5 (2026-10-17): every reviewer on the whole plan, and a
day in the life.** Fixed:
- Security: what a program prints passed for the shell's command marks,
  so "Run the command again" could type a line chosen by a file shown
  with `cat`. Each shell start has its own mark now (handed in by
  environment, dropped there at once); a mark counts only with it. zsh
  no longer says lines kept out of its history for their leading space.
  An agent's tag note reached the user's prompts unattributed: it says
  whose it is; no slot may be named for the user.
- B.2 kept: the cursor isn't saved with the place (stopping point 4 had
  put it there for `ctrl+o`; it came back after a reload).
- Terminal: a command's output read from a program's whole screen; run
  again into text after the cursor; a second held update that stayed
  held; blank rows while a re-wrap's lines came (the old lines stand in
  now); a late answer stored under new numbers; a restart racing a
  reader; a size said every frame of a drag (now once it holds still)
  and a thread per move for tags (now refreshed where lines moved, before
  they can move again); `NG` went to the Nth kept line, not line N.
- Tags: `#name` was expanded on two paths only (now where every prompt
  goes: reports, queue briefs too), and only in exact capitals; names
  can't end in `-` or `_` (`#name` stops there); one 200-line limit,
  checked by the server (the cockpit cut silently); a read wrote; Enter
  in the picker could act on the results of what was typed before.
- Found live: a long press on a terminal line did nothing (no hint
  spot): it puts the cursor there; bash's `ignoredups` lost a repeated
  command's line (stopping point 4).
- Tests: rule 5 linted, rule 6 a dependency test; names of an earlier
  start; re-sent rows proven; no silent skip without bash or zsh; the
  shell tests run with an empty home.
- Day in the life, measured: 300,000 lines of output with 139
  screenshots in 5 s, no blank frame; back past the kept scrollback;
  a tag whose line left the scrollback reads gone, what it said kept;
  reload with the picker open; two panels on one terminal, chips in
  both; a server restart (shells end, tags stay, going to one says it's
  gone); width changes with tagged lines (followed).

★ For the user (kept as built, or left open):
- Keys: `M` tag, `g T` go to a tag (`g t` is the tree's), `] t`/`[ t`
  step, `]]`/`[[` move by command; copying a command's output and
  running it again have no keys of their own (the menu's numbers shift).
- Chips are placed where a buffer says a thing is drawn (`drawn_at`),
  not on hint spots (a terminal's lines aren't spots).
- `ctrl+o` after a go-to inside one terminal returns to the terminal,
  not the line (B.2). A line kept only in the jump list would do it.
- Permissions: an agent with no key reads tags (`view.read`), and what
  tagged lines said outlives `clear` and closing the terminal; an agent
  driving the cockpit tags, edits and runs commands again as the user;
  a worker's note is a channel to the agents who read it (now signed);
  any role that may send to a slot hands it what `#name` names.
- Each change sends the whole tag (reply and event); a new tag's colour
  is the least used (the plan said the next); only the terminal's lines
  and commands are subjects so far (chat messages, components, code
  lines are the later steps).
- Small: the Tags buffer offers its own "take the tag off this" beside
  the shell's; a note is edited by typing it again; when a thing was
  tagged isn't shown.

**Built in this plan:** the core (contracts, `nah-tags`, store, events,
routes, MCP tools, the desk's service, the shell action, host-drawn
chips) with terminal lines and commands as its first subjects. Then each
owner adds its subjects with its resolver and `subject(key)` (chat
messages, components, code lines) as its own small step, with no change
to the core.


## Process

Built as `dev_docs/process/build-and-validate.md` says: each step tested
and checked live as a user, then committed. Stopping points (linters, the
whole suite, the live pass, the reviewers in
`dev_docs/process/reviewers.toml`, fixes, findings recorded here):

1. After the terminal's view (steps 1–3): performance and correctness
   reviewers; the flicker measured (rapid screenshots, no blank frame).
2. After the cursor and commands (4–5): correctness, consistency.
3. After the tags core (6): responsibility, permissions, correctness.
4. After tags in the cockpit and the terminal's subjects (7–8): every
   reviewer.
5. At the end (9–10): every reviewer on the whole change, and a full
   "day in the life" live run.

New tests stay few and aimed at real risk: random-input tests
(`proptest`, test-only) for the small parsers (key encoding, command
marks, links), and one speed budget for drawing a terminal with 10,000
lines of scrollback, with generous headroom.

## Steps

1. Spike: counting lines past the scrollback's cap (A.1); results here.
2. Line numbers on the server and the socket (A.1).
3. The terminal's view, lines and drawing (A.2–A.5), measured live.
4. The not-typing cursor (B).
5. Commands (C).
6. Tags core (D): contracts, `nah-tags`, store, events, routes, tools.
7. Tags in the cockpit: service, `subject(key)`, the tag action and the
   picker, chips, the go-to, the Tags buffer.
8. Terminal lines and commands as subjects (resolver, `subject(key)`).
9. `#name` in prompts.
10. README; then (each its own later step) chat messages, components,
    code lines.


## Open questions (★)

- Marks become tags (settled): a mark is a tag with a one-letter name,
  on one thing (setting it again moves it). Done once every buffer says
  what its items are (marks work in every buffer today), and the marks
  code goes then; until then marks stay as they are.
- The tag key, and moving by command (`[` / `]` suggested).


## Checklist

- [x] 1 spike: counting lines past the cap
- [x] 2 line numbers
- [x] 3 the view, lines, drawing (no blank frame, measured)
- [x] 4 the not-typing cursor
- [x] 5 commands
- [x] 6 tags core
- [x] 7 tags in the cockpit (action, picker, chips, go-to, Tags buffer)
- [x] 8 terminal subjects
- [x] 9 `#name` in prompts
- [x] 10 README

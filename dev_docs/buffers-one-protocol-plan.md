# Plan: buffers talk to the shell through one protocol, and the outline works

Written 2026-10-16 from a full review of the code (every place the shell
reaches into a buffer; the outline and the server's tree walk). Nothing in
it is built yet. Each step is tested, checked in the running cockpit and
committed on its own. The checklist at the end is the order; tick it as it
goes.

Words: a **panel** shows a **buffer** (chat, code, graph, tree, spec,
queue, inbox, changes). The **shell** is `app/` and `app.rs`. An **agent
slot** is an agent the server runs.


## What's wrong today

### The outline (tree buffer)

1. **Rows deeper than two levels never open.** The outline asks the server
   for the whole tree with the rows you opened (`opened`, oldest first). The
   server walks them newest first (`arch-graph/src/tree/walk.rs:216`); a row
   whose parent is opened later in the same walk isn't there yet
   (`walk.rs:217`), so it's skipped without a word. The outline already has
   it in `opened`, so pressing it again sends nothing (`tree.rs:79-84`); it's
   stuck until the source changes.
2. **Rows that don't fit are closed again silently.** Over the 400 cards one
   answer holds, the server drops the row and lists it in `dropped`; the
   outline forgets it (`tree.rs:107`) and says nothing, the arrow stays, and
   every press asks again and is dropped again.
3. **Two kinds of "folded" that disagree.** Folded by you (`closed`) or never
   loaded (`more > 0`, no children). The arrow is the same; Enter unfolds only
   the first kind (on the second it sends the row to the graph), `l` loads
   the second, Backspace's label says "Fold" but goes up, `h` marks it folded
   and only goes up on the second press, "Unfold everything" opens only the
   first kind.
4. **It ignores what the server cut** (only 60 starting points, branches cut
   for time, slow answers) and applies answers in arrival order (a late old
   one can overwrite a newer one).
5. **It's built as a code outline.** Rows are `ComponentId`s, Enter's label
   is "Select X in the graph", it builds the graph's Contains request itself,
   and Enter asks the shell (through `tree.select`) to move the graph.
6. The graph's full reload has the same newest-first skip (rarely hit: it
   normally opens one branch with `expand`).

### Buffers and the shell

The buffers don't call each other, but the shell wires them together kind
by kind, reaching into their fields:

- **Requests through fields the shell polls** (`app/views.rs` `link_views`):
  `graph.requests.open_code`, `spec.requests.{reread, insert,
  review_changed}`, `code.{open, select, reread}`, `tree.select`,
  `queue.requests.open_thread`, and the inbox's `take_asks`. Three are never
  set (`graph.requests.open_spec`, `spec.requests.select`,
  `spec.requests.open_code`). Each is handled its own way: most skip the
  go-to rules (can it show this, which panel, the jump list, focus).
- **Follow-along copied between particular buffers**: the graph's source is
  everyone's source; specs follow the graph's selection; code and spec
  follow the changes' comparison; the graph gets change badges, review
  marks and stale files; the queue gets the chat's harness and the graph's
  plan; specs get review comments, the diff, types and the user's name.
- **Direct control**: the shell calls buffers' own methods (`graph.reveal`,
  `code.load_file`/`show_span`, `spec.show`, `chat.show_thread`,
  `chat.answer`, `inbox.show`/`sync`, writes `chat.ui.slot`, sets
  `spec.pinned`/`floating`, `*.invalidate`, `*.tick`, and eight different
  `render` signatures).
- **Reads of private state**: `graph.nav.source` (nine places),
  `graph.component`, `graph.selection`, `spec.pinned`, `spec.detail`,
  `code.defs`/`current_file`, `chat.store` (the bell, the AGENT light, the
  inbox all read notifications out of the chat), `inbox.item`,
  `changes.comparison`.
- **The go-to path itself has side doors**: a Code landing calls
  `code.load_file`/`show_span` directly (the code view has no
  `show_target`), and a Spec landing moves the graph by calling
  `graph.reveal`.

So the same thing ("show this over there") behaves three ways, and a new
buffer (a tree of plan steps, of text) would need new shell wiring.


## How it will work

1. **A buffer only says what it's on, and asks.** Every request a buffer
   makes is one `Ask`, collected the same way from every buffer and handled
   once by the shell:
   - `Show { target, in_ }`: show this target in that kind of buffer. This
     *is* the go-to path: can it show it, which panel, focus, the jump list.
     Opening code at a span, selecting in the graph, opening an agent's
     session and the inbox's "open its chat" all become this.
   - `Reread(file)`: read a file into the index again.
   - `Type`: give me the keyboard (a typing box opened).
   - `Refresh(what)`: shared data changed (the review after a write).
   - the inbox's own: next/previous item, put away, answer a request.
   Request fields, the inbox's private ask type and the dead fields are
   deleted.
2. **Every buffer lands the same way.** The code view gets `show_target`
   (a file at a line or span, a message's link, a change), so the go-to path
   never calls `load_file`/`show_span` itself.
3. **Follow-along is one shared value.** The desk keeps one `Shared` value:
   the source in use, the changes' comparison and what it changed, review
   marks/comments/diff, component types, the user's name, the index's stale
   files, the agent slot's harness, the component being followed. Every
   buffer (shown or parked) reads it through one trait method; buffers change
   it only through asks. The per-kind copying in `link_views`, `lifecycle`
   and `set_up` is deleted.
4. **The shell drives buffers only through their traits.** Events, refresh,
   ticking, drawing (one `render` signature), the quick look's card mode and
   saving/restoring go through trait methods. Global state that lives inside
   one buffer today (notifications in the chat's store) moves to the desk.
5. **The compiler enforces it.** A protocol crate (the traits and shared
   types), a buffers crate (each buffer, fields private), the shell crate
   (sees only the protocol). The shell can't name a buffer's field because
   it doesn't exist outside the buffers crate.
6. **The tree buffer is about trees, not code.** It shows rows from a tree
   source; the code outline is one source. Enter goes in (unfold, then the
   first child), Backspace goes out to the parent, `z` keys fold, and
   showing a row elsewhere is a go-to (`g m`), like every buffer.


## Steps

### 0. The outline's unfolding (first, on its own)

0.1 Server, `arch-graph/src/tree/walk.rs` `walk_known`: apply `opened` until
    nothing more opens: each pass goes through the entries not applied yet
    (newest first, so the newest wins the budget), applies those whose card
    is drawn, and passes again while any applied. Only a branch that
    doesn't fit goes into `dropped`; a card never drawn (under a closed
    card, gone) stays as asked, so reopening its holder opens it again.
0.2 Server test (`arch-graph/src/tree/tests.rs`): opened = [A, B] with B a
    child of A shows B's children; a card that never appears comes back in
    `dropped`.
0.3 The graph's full reload is the same walk on the server: the 0.2 test
    covers it (no client change).
0.4 Tree buffer, one notion of folded: `folded(id)` = in `closed`, or
    children not loaded and `more > 0`. The arrow, `has_children`, Enter,
    Backspace, `h`, `l`, `z a`/`z o`/`z c`, the fold labels, "Unfold
    everything" and search all use it.
0.5 Tree buffer, unfolding a row not loaded always asks again: `load`
    re-sends even when it's already in `opened` (no stuck rows).
0.6 Tree buffer reads the answer fully: `dropped` rows say why in the note
    ("Couldn't open X: it goes over what one view holds"), as the graph
    does; a cut answer says what was left out on its top line; an answer
    older than the last request is ignored (a request number).
0.7 Tree buffer, in and out: Enter goes in (a folded row unfolds; an open
    row goes to its first child); Backspace goes out to the parent; `tree.select`
    and the "Select X in the graph" label are deleted. Showing a row in the
    graph is `g m`.
0.8 Tests (`tree.rs` tests): loading a row deeper than the first answer,
    Enter/Backspace/`h`/`l` on folded, open and not-loaded rows, a dropped
    row's note, a stale answer ignored.
0.9 Live: open rows three and four levels deep in this repository's
    outline; Enter/Backspace; `g m`. README: the outline's keys. Commit.

### 1. One ask, handled once

Done together with step 2 (an ask to show code needs the code view's own
landing). As built: `Ask { Show, Follow, Reread, Type, ReviewChanged,
Waiting(Waiting) }` in `nav.rs`; `Target::Span` and `Target::Agent { slot,
thread }` (one target for an agent's chat, by slot or by session); the
shell's one place is `app/asks.rs`; a landing in a panel closes the quick
look; the lint `buffers_reach_past_themselves_only_by_asking`.

1.1 `nav.rs`: `pub enum Ask { Show { target: Target, in_: BufferKind },
    Reread(String), Type, Refresh(Refresh), Inbox(InboxAsk) }` (InboxAsk
    moves here as the inbox's variants: Step, Dismiss, Answer); `Navigable`
    gets `fn take_asks(&mut self) -> Vec<Ask>` (default: none).
1.2 `Target` gains what `Show` needs and doesn't exist yet: a code span
    (`Target::Span { file, start, end }`) and an agent session
    (`Target::Session { thread }`).
1.3 Shell, one place (`app/asks.rs`, replacing the request parts of
    `link_views`): every frame and after every invocation, take the asks of
    every buffer in use and handle each: `Show` through `show_target_in`
    (the go-to path), `Reread` through `net.arch_refresh`, `Type` focus plus
    Insert mode, `Refresh(Review)` the review, the inbox's through what
    `attention.rs` does now.
1.4 Buffers move to asks, one at a time, deleting each field and its
    `link_views` branch in the same change:
    - graph `requests.open_code` → `Show { Span, Code }`; delete
      `GraphRequests` (and the never-set `open_spec`).
    - spec `requests.reread` → `Reread`; `insert` → `Type`;
      `review_changed` → `Refresh(Review)`; delete `SpecRequests` (and the
      never-set `select`, `open_code`).
    - code `open` (ctrl-click a name) → `Show { Component, Code }`; `reread`
      → `Reread`; `select` (click a name) → `Ask::Follow` (2.3).
    - queue `requests.open_thread` → `Show { Session, Chat }`; delete
      `QueueRequests`.
    - inbox `take_asks` → the trait method; OpenChat → `Show { Session,
      Chat }` (no more writing `chat.ui.slot`).
1.5 The chat gets `can_show`/`show_target` for `Target::Session` (the slot
    running that thread; why not when none does). Delete
    `ChatStreamState::show_thread` as a shell entry point.
1.6 Tests: each former request now behaves as a go-to (lands, focuses,
    records a jump, says why when it can't); a lint (`lint.rs`) fails if
    `app/` names a buffer's field.
1.7 Live: graph find hit → code; spec Re-read; spec comment box takes the
    keyboard; queue `o` → the agent's session; inbox Enter → its chat; code
    ctrl-click → definition. Commit.

### 2. Every landing the same

2.1 Code view `show_target`: `Target::Span`, `Line`, `Change`, a message's
    link. Its file is fetched when next drawn (no network in a landing or
    a restore). A `Component` is found by the go-to path (`land`: where
    its lines are, from what the desk knows) and lands as its `Span`.
2.2 `going.rs`: delete the Code side path (`load_file` at going.rs:49-53),
    `show_code`, `open_component_code`'s direct calls; every landing is
    `show_target_in`. The spec landing no longer calls `graph.reveal`: the
    graph follows through `Shared` (2.3).
2.3 Following: `Ask::Follow(id)` from the graph's selection (when it
    changes), a click on a mark in the code's margin, a card landed on or
    gone back to (not pinned). Every buffer in use (each kind's used last)
    is told (`Navigable::follow`): the graph moves to it, a card not pinned
    shows it, a pinned card remembers it and shows it once unpinned. A
    parked buffer keeps its own place (each panel's graph is its own).
2.4 `restore_place` (jumps, marks) loses its Code and Spec special cases:
    every buffer's `restore` does its own.
2.5 Tests and live: every go-to from every buffer; jumps back and forward;
    the quick look's card. Commit.

### 3. Shared state in one place

In three commits. 3a (done): `shared.rs` (`Shared` on the desk: source,
review, types, index, user, settings; each part a `Kept` that counts its
changes); `Navigable::share` called for every buffer, shown or parked,
each frame; the graph says its source (`Ask::Source`); the copying in
`link_review`, `link_queue`, `lifecycle` (types, settings, out of date) and
`set_up` is gone. 3b (done): the comparison is the desk's (`Comparing` in
`changes/comparing.rs`: the comparison, its list, the components it
touches, the timeline, each counting its reads); the changes view, the
code view, the cards, the graph and the chat read it; the changes view
asks to compare (`Ask::Compare`) and to read again (`Ask::Refresh`). 3c
(done): the agents are the desk's (`agents.rs`: the store applied from
the server's events, sessions opening, errors, sign-in, tool servers,
saved sessions); buffers hold a reader (`AgentsView`), only the shell
changes them; the chat and the inbox ask them to act (`Ask::Agents {
slot, act }`), each with its own answer drafts; the bell, the tray, the
AGENT light, the queue's harness and the talk box read them; a chat on
screen says its slot was seen itself; asks raised while drawing are done
in the same frame.

3.1 `Shared` (desk): source in use, comparison + read count, changed
    components + read count, review marks/comments/diff, component types,
    user name, index stale files, the agent slot's harness instance,
    plus the config pieces buffers read (`graph`, `code`,
    `changes` UI settings).
3.2 `Navigable` (or a `Follows` trait): `fn follow(&mut self, shared:
    &Shared)`; the shell calls it for every buffer, shown and parked, once
    per frame (each buffer keeps the read counts it needs to skip work).
3.3 Delete the copying: `link_views` follow parts, `link_review`,
    `link_queue`'s copies, `lifecycle.rs:26-48` and `:172-177`,
    `layout.rs` `set_up`'s seeding.
3.4 The source becomes the desk's: the graph's source picker asks
    `Ask::SetSource(source)`; the nine `graph.nav.source` reads read
    `Shared`.
3.5 Notifications move from the chat's store to the desk (they're global):
    the bell, the tray, the inbox and the AGENT light read the desk, not
    `chat.store`.
3.6 Tests and live: badges, review marks, stale files, the queue's plan,
    specs following, the bell across servers. Commit.

### 4. The shell drives buffers only through traits

Done as: `buffer.rs`, one `Buffer` trait (on `Navigable`, `ActionProvider`,
`Hosted`): `render` (one signature, with a `Drawing`: the inspector,
focus, the leader key), `tick` (before drawing), `on_event`, `invalidate`,
`keep`/`restore_kept`, `knows` (name, code place, what it says beside a
card), `as_card`, `located`, `describe`; `Hosted::alert` for the top
bar. The desk holds every buffer in one map by `BufferId` (the struct
`Buffer` renamed), made by `buffer::new(kind)`: no per-kind fields, no
parking or swapping, a chat is a buffer like the others. The quick look
shows what the other buffers say about its card. The saved layout keeps
what each buffer keeps (its version moved to 4: an older one starts
fresh). A lint keeps buffer types out of the shell.

4.1 One `render(&mut self, ui, cx)` for every buffer (the inspector and the
    pointer state go in `cx`); delete `render_view`'s per-kind match.
4.2 Events: `fn on_event(&mut self, event: &Event)` on every buffer (arch
    events and client events); delete the per-kind dispatch in
    `lifecycle.rs`; `invalidate`/`tick` through the trait.
4.3 The quick look's card mode (`spec.pinned`/`floating`) through a host
    method (`Hosted` or `Navigable`: "you're shown as a card over a
    panel"); the graph's card context drawn by the spec from `Shared`.
4.4 Saving and restoring: every buffer saves and restores its own state
    (`fn keep(&self) -> Value`, `fn restore_kept`), replacing the shell's
    reads of `pinned`, `input_buffer`, `comparison`, `layout`.
4.5 `describe` (a buffer in words, for lists) through the trait instead of
    reading `chat.store`, `changes.comparison`, graph places.
4.6 Tests (the shell's tests drive through keys, actions and the screen
    read), live, commit.

### 5. The compiler enforces it

Done without separate crates: the shell holds every buffer as a
`Box<dyn Buffer>` (step 4), so it can't reach a field or method the trait
doesn't offer; buffers never hold each other (they get `&Shared`). What
was left for crates to close is closed here: places are each buffer's own
(`Place` is opaque: a buffer reads its own back; `place_in_words` says it
for a mark), targets carry no buffer's types (`Target::Spec` is gone; a
card's item is the component it's about), `BufferKind` lives in `kind.rs`,
the diff drawing two buffers share in `diff.rs`, the message summary in
the agents, and lints fail the build's tests if the shell names a buffer
type or downcasts, or if a buffer names the shell or another buffer. A
physical crate split remains possible; it would also mean rewriting the
shell's tests that look inside buffers (about 300 assertions).

5.1 New crate `cockpit-protocol`: `Navigable`, `ActionProvider`, `Hosted`,
    `Ask`, `Shared`, `Target`, `Place`, `GoTo`, `Move`/`Motion`/`Fold`,
    `Item`/`Spot`, `RunCx`, the viewport rules, theme access.
5.2 New crate `cockpit-buffers`: each buffer kind, fields private; only
    constructors and the protocol's traits are public. Buffers depend on
    the protocol only, never on each other.
5.3 `cockpit-ui` keeps the shell and the hosts; it holds buffers as
    trait objects; buffer tests move with their buffers.
5.4 Build (native and browser), whole suite, clippy, live check, commit.

### 6. The tree buffer about trees

Done: `tree/` is `TreeBuffer` over a `TreeSource` (`tree/source.rs`: rows
with key, label, kind, parent and parts still to load; asking with rows
opened; answers by request number; what a row stands for elsewhere), the
code's outline one source (`tree/outline.rs`); a test drives a made-up
source of headings, and a lint keeps code-outline terms out of the buffer.

6.1 A tree source trait (rows: key, label, kind, children or "more to
    load", how to load more); the code outline is one source, built from
    the graph's Contains answer, in the buffers crate.
6.2 The tree buffer uses only the source: no `ComponentId`, no
    `TreeRequest`, no graph words; its target comes from the source (a
    component for the code outline).
6.3 Tests with a second, made-up source (plain text headings) to prove it
    knows nothing about code. Live, README, commit.

### 7. Wrap up

7.1 Whole suite, clippy, live recheck of everything above; update this
    plan's status, the README and the notes.


## Checklist

- [x] 0.1 server: opened applied until nothing more opens; never-opened → dropped
- [x] 0.2 server test: nested opened rows
- [x] 0.3 graph full reload with nested opened rows (same walk; 0.2 covers it)
- [x] 0.4 tree: one notion of folded everywhere
- [x] 0.5 tree: a row not loaded always asks again
- [x] 0.6 tree: dropped rows say why; cuts shown; stale answers ignored
- [x] 0.7 tree: Enter in, Backspace out; tree.select deleted
- [x] 0.8 tree tests
- [x] 0.9 live check, README, commit
- [x] 1.1 Ask and take_asks
- [x] 1.2 Target::Span, Target::Agent
- [x] 1.3 shell handles asks in one place
- [x] 1.4 graph, spec, code, queue, inbox moved to asks; their fields deleted
- [x] 1.5 chat shows a session target
- [x] 1.6 tests and the asks lint (fields private: step 5)
- [x] 1.7 live check, commit
- [x] 2.1 code view show_target
- [x] 2.2 go-to path without side doors
- [x] 2.3 following: buffers in use are told each move; pinned cards catch up
- [x] 2.4 restore_place without special cases
- [x] 2.5 tests, live, commit
- [x] 3.1–3.5 Shared state; source, comparison and agents on the desk
- [x] 3.6 tests, live, commit
- [x] 4.1–4.5 render, events, card mode, keeping, describe through traits
- [x] 4.6 tests, live, commit
- [x] 5.1–5.4 enforced by trait objects, opaque places and lints (crate split optional)
- [x] 6.1–6.3 tree buffer on a tree source
- [x] 7.1 wrap up (701 tests pass, clippy -D warnings clean, live rechecked)

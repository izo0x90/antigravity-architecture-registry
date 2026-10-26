# End review: correctness

Scope: `git diff 3e1a805^..HEAD` (18 commits). Paths are relative to `meta-harness/`.
Read-only review. Nothing was built or run.
Ranked most important first.

## 1. The chat can panic: the row layout cache is keyed by a freed pointer

`crates/cockpit-ui/src/chat/render.rs:600-602` (`lay_out_rows`) reuses `RowsAt` when
`(Rc::as_ptr(shape), heights_changed)` matches the last frame's key. The pointer is
the address of an `Rc<Shape>` that may already be freed. A new shape can be allocated
at the same address (all `RcBox<Shape>` blocks are the same size, and the wasm
allocator reuses the last freed block). Then stale `tops`/`heads` are used with a
shape that has a different number of rows.

What goes wrong:
1. Frame N draws shape A (address X). `rows_at.of = (X, h)`. The heights did not change.
2. Frame N+1: a new item arrives. `handle_keys` gathers the bound actions. The chat's
   `actions()` calls `shape()`, which builds B (A is still held while B is allocated,
   then A is freed).
3. The key the user pressed folds or unfolds a run (`folds.changes()` moves).
   `render_stream` calls `shape()`, which builds C while B is alive. C lands at X.
4. `lay_out_rows` sees `(X, h)`, so it keeps A's `tops`.
5. If C has more rows than A and the cursor is none, the ranges run to
   `shape.rows.len()` (`render.rs:507-513`), so `tops[at]` at `render.rs:533` is out
   of bounds. If C has fewer rows and the cursor is set, `&shape.rows[at]` at
   `render.rs:537` is out of bounds. The live-line code (`tops[start]`) can also
   index past the end. A panic in wasm stops the cockpit until a reload.

Even without a panic, a hit with the same row count lays rows out at A's positions
for the frame. Key the cache by something that cannot be reused, like a counter
bumped with each new shape.

## 2. Server stamps are compared against the browser's clock

`crates/cockpit-ui/src/agents.rs:217-218` turns each event's `atMs` into local time
with `now_ms() - at_ms`, using the browser's wall clock. Nothing corrects for skew
between the server's clock and the browser's. The same goes for
`crates/cockpit-ui/src/chat/subagents.rs:26-29` (`ran_s`). Through home, a remote
project server's own clock is kept (`src/home/links.rs:184`, `publish_at`), so a
third clock comes in.

What goes wrong:
- The browser is 2 seconds ahead of the server. Every update looks 2 seconds old.
  "Receiving" needs an update newer than `receiving_s` (1.5 s), so the live line
  never says the agent is receiving while it streams.
- A phone or a remote server is 70 seconds off (browser ahead). The first update
  already counts as more than `quiet_after_s` (60 s) old, so the chat says the agent
  has gone quiet while it is streaming.
- The browser is behind the server. `saturating_sub` gives 0, so a sub-agent chip
  says "0 s" for as long as the skew.

The server already stamps `Replayed` and `Resync` with its own now. That gives an
offset per connection that could be applied.

## 3. With the quick look open, a tap on the key bar is eaten

`crates/cockpit-ui/src/overlay.rs:118-160` (`outside_tap`) treats any press outside
the top overlay's drawn rects as outside. When the overlay is the quick look
(`Outside::Closes`), it removes the press and its release from the raw input. The
key bar is a `TopBottomPanel` (`crates/cockpit-ui/src/app/draw/bars.rs:70`) and is
never registered as part of the look. While the quick look is open, the key bar shows
the look's keys (it holds the keys).

What goes wrong: open a sub-agent pop-up or the inbox in the quick look on a phone.
Tap a key bar chip for one of the look's actions. The look closes and the chip's
action never runs, because egui never sees the click. The old code closed the look
but let the click through. On touch screens this leaves no way to use the key bar
while a quick look is up.

## 4. In bar mode, a request still shows no answers while you type

`crates/cockpit-ui/src/app/draw/host.rs:152` and `:165-180`. A request's answers
count as "mine" only when `control.under` equals `here_key`. `here_key` is the
chat's `here()` (the pending item, but only when the cursor is none) or else the
cursor row. In bar mode the other waiting groups are dropped at line 165.

What goes wrong: set item buttons to the bar. Put the cursor on an older message.
Press `i` and type (the cursor stays). The agent asks "Run invoke_subagent?". Its
card shows, but the bar has no Allow or Deny, because `here_key` is the cursor row
and not the card. The `y`/`d` keys go into the text box. You have to press Escape
first. This is the bug that plan item 13 fixed, still there in bar mode.

## 5. Any failed call marks the agent "can't run" everywhere

`crates/cockpit-ui/src/agents.rs:314-316`. `state()` returns `CantRun(err)` whenever
`errors` holds anything for the slot. `errors` is filled by every failed act: a
prompt that failed to send (`:551`), an answer that failed (`:540`), a refused or
failed stop of a sub-agent (`:546`).

What goes wrong: tap "Stop it" on a sub-agent and the call fails (a network blip,
or the thread just closed). The lead's AGENT light turns red with "can't run: …".
The inbox lists its notifications as "lead (can't run: …)". A discussion says
"waiting on lead: can't run: …". Meanwhile the agent is running fine. Plan item 12
defines "can't run and why" as a state of the agent, not as the last failed request.

## 6. The search status reuses another buffer's matches

`crates/cockpit-ui/src/app/capture.rs:186-190` caches what a search found by
`BufferKind`, and trusts the cache when `items_changed()` is the same number. The new
counters are per buffer and all start at 0 (`code.rs` `lines_changed`,
`terminal/mod.rs`, `tree/mod.rs`, `inbox.rs`, `queue.rs`). Two buffers of one kind
can have equal counts. `desk.view(kind)` is whichever one is in use.

What goes wrong: two code panels each load one file, so both counters are 1. Search
`/fn` in panel A, which finds lines 3, 10 and 20. Focus panel B with the cursor on
line 10. The key bar says `/fn 2/3`, from A's matches, whatever B holds. Before this
change these views returned `None` and were searched again. (`n`/`N` search again, so
only the status line is wrong.)

## 7. Older servers can't talk to a newer home

`crates/harness-contracts/src/events.rs:46`. `Stamped.at_ms` has no
`#[serde(default)]`. `src/home/links.rs:184` fails the link on an event it can't
read.

What goes wrong: home is upgraded first, and a project server on another machine
still runs the previous build. Every event lacks `atMs`, so home's link to that
server fails with "an event home can't read (missing field `atMs`)" and retries.
The cockpit shows nothing from that server until it is upgraded too.

## 8. The hub keeps one sub-agent per id across threads

`crates/harness-core/src/hub.rs:99` keys `SubagentUpdated` by
`Key::Subagent(subagent.id)` alone. Sub-agent ids are not unique across threads.
Claude Code's is the child session id, which a replay of the same session brings
back. Antigravity's is `{call id}/{n}` from the replayed start call.

What goes wrong: the same saved session is open in two slots at once (resumed in
each). Both threads publish the same sub-agent id. The later one replaces the
earlier one's cache entry. After a reload, the first thread's sub-agent item has no
`Seen` (no state, no chip, and the pop-up says "That sub-agent isn't in its chat
now."). When that thread closes, `retain` keeps the entry, because the entry now
names the other thread. Before this change the adapter never emitted
`SubagentUpdated`, so this key was never hit.

## 9. "Talk" over an open sub-agent pop-up keeps the pop-up

`crates/cockpit-ui/src/app/look.rs:35-43`. The sub-agent pop-up and talking to the
lead use the same quick-look chat buffer (`BufferId(Chat, PANE)`). `talk()` calls
`show_target(lead)` only when the look holds another buffer.

What goes wrong: open a sub-agent pop-up, then press the talk key. The buffer is
already the chat, so `alone` stays set. The look still shows the sub-agent, now
with `InputMode::Insert` but no typing box (render skips it when alone). Whatever
you type goes nowhere. If the sub-agent belongs to a sub-slot, the look also stays
on that slot instead of the lead.

## 10. Resuming a session flashes its old sub-agents as just ended

The Claude Code connector replays each sub-agent as `running`, then sends its end
(`subagent-replay.js` `announce` and then `finish`). In the cockpit, a resume happens
while the store is live, so `crates/cockpit-ui/src/store.rs:134` sets
`ended_seen = now` for each one.

What goes wrong: resume a session that used 5 sub-agents. For a moment the slot is
`Working { subagents: 5 }` (light, discussions, inbox). Then the strip above the box
shows "5 sub-agents ended" for `subagent_ended_s` (6 s), for work that ended days ago.

## 11. Absolute paths resolve against the session's folder, not the project's

`crates/cockpit-ui/src/chat/nav.rs:18` (`folder_of`) uses
`thread.policy.cwd`. The plan says the server tells the cockpit its project folder.
Tool-call locations now go through `in_folder` too.

What goes wrong: a slot runs with its cwd in a subfolder (`meta-harness/`) of the
project. The agent writes `/…/repo/meta-harness/src/a.rs:12`. The chat strips the
cwd and opens `src/a.rs`, which is a different file or none. This only differs when
cwd is not the project root.

## Also seen (uncommitted fixes in the working tree)

`crates/harness-acp/src/pieces.rs` `piece_at` adds `pos + len` with `len` read from a
label's digits. A label like `&len=18446744073709551615` overflows. That panics in
debug builds while the session state lock is held. `Piece::read_label` also scans to
the next whitespace from every `nah://` it finds. The working tree already changes
both (`checked_add`, `LABEL_MOST`). This is noted only because HEAD still has them.

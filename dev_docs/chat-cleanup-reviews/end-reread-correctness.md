# End re-read: correctness

Scope: `git diff 182025f..516ef76` (e43de96, 8bc4bca, 516ef76). Paths are relative
to `meta-harness/`. Line numbers are at 516ef76. The working tree has uncommitted
edits to some of these files. This review is of the commits. Read-only: nothing
was built or run. Ranked most important first.

## 1. A remote server's times are still read against home's clock

`crates/cockpit-ui/src/agents.rs:243-256` takes the least gap seen as one server's
lead. It resets on `Resync` and starts again from the `Resync`'s own gap. But a
remote server's desk gets stamps from two clocks:

- `Resync` and `Replayed` are stamped by home (`src/home/socket.rs:90`,
  `crates/harness-core/src/hub.rs:170` from `hub.reset()` in
  `src/home/links.rs:174,188`). An event with no time is stamped by home too
  (`links.rs:201`).
- Every other event keeps the remote server's own stamp (`links.rs:202`,
  `publish_at`).

What goes wrong (remote server's clock 90 s behind home, browser in sync with home):
1. The desk gets `Resync` stamped by home. Its gap is about 0, so the lead is 0.
2. Each event from the remote has a gap of 90 s plus its real age. The least gap
   stays 0, so every event is put 90 s in the past.
3. While the agent streams, the chat says "No word for 1 min" and never
   "receiving". Sub-agent chips use `server_now_ms()` (home's time) against the
   remote's `started_ms`, so a sub-agent that just started reads "1 min 30 s".

This is the "third clock" case from the first end review (correctness 2). It
stays until a fresher stamp from the same clock beats home's, which never happens
when the remote is behind. (The uncommitted `links.rs` edit works on this.)

## 2. One failed open sticks as "can't run" and removes the way back

`crates/cockpit-ui/src/agents.rs:366` returns `CantRun` whenever `cant_open` holds
the slot. `cant_open` is cleared only in `open()` (`:531`). Nothing else clears it:
not a thread starting, not `Act::PutErrorAway` (`:504`, which clears only
`errors`), not a `Resync`, not the slot closing.

What goes wrong:
- The lead's open fails once (a timeout, or "sign in first"). The user signs in and
  starts the session from a phone, or the server starts it (a discussion's wake).
  In this tab, the AGENT light stays red with "couldn't start a session: …" while
  the agent works. Every discussion says "waiting on lead: couldn't start a
  session: …". The inbox says the same.
- The user dismisses the error in the chat. The error goes, but the state stays
  `CantRun`. Before this change, dismissing it cleared the state.
- `way_out()` (`:81`) answers only for `NoSession`. So after one failed open, the
  chat's "Resume its last session" banner action (`chat/actions.rs:101`) and the
  discussion's wake button (`discussion.rs:457`) are gone. Typing in the chat
  calls `open(None)`, which starts a new session instead of resuming the last.
- A sub-slot closed after a failed open reads `CantRun`, not `Gone`, because
  `cant_open` is checked before `store.closed` (`:370`).

## 3. A `Resync` handled late makes everything queued look new

`agents.rs:252` sets the lead from the `Resync` event's gap when the event is
handled, not when it came. While the tab is hidden, frames stop, and events wait
in the desk's queue.

What goes wrong:
1. The tab is hidden. The project server restarts. Home's link reconnects and
   `hub.reset()` sends a `Resync` stamped T0. The replay and new live events follow.
2. The user comes back at T0 + 5 min. The `Resync` is handled then. Its gap is
   5 min, so the lead is set to 5 min.
3. Every event in the queue is put 5 minutes later than it happened. The last ones
   get an age of 0. The agent's last update before the restart reads as "just
   now": the chat says it's receiving, and "No word for …" counts from the return,
   not from the real silence. A sub-agent that ended while hidden counts as just
   ended and shows in the strip. This is what the stamps were meant to prevent
   (`chat/tests.rs` `an_update_is_as_of_when_it_happened`).

## 4. The key bar is "inside" every overlay, including modal ones

`crates/cockpit-ui/src/overlay.rs:122-123` treats the key bar's rect as inside
whatever overlay is on top. It's registered for every overlay
(`app/draw/bars.rs:87`). The fix was meant for the quick look, whose keys the bar
shows. For the leader menu, `mode_keys` returns `None` (`bars.rs:104`), so the bar
shows the panel's chips, not the menu's.

What goes wrong: open the leader menu (modal: what's under it takes no keys).
Tap a key bar chip. Before, the tap closed the menu and was dropped. Now the menu
stays open, and the panel's action runs under it. The same applies to Feedback and
Issues (both modal). With hint labels up (`ClosesAndPasses`), a chip tap used to
close the labels and then run. Now the labels stay up, still capturing keys, while
the chip's action runs.

## 5. A message with more than 256 "nah://" anywhere isn't read back

`crates/harness-acp/src/pieces.rs:218` counts every `nah://` in the whole replayed
text. That includes the user's own words and all pasted content. The comment says
"no message is sent with more links than this". Nothing on the send side caps a
message's links, and pasted content isn't links at all.

What goes wrong: attach a file or a tag whose content mentions `nah://` often (a
log, a probe recording, a review of this code), or link more than 256 things.
Replay the session. `read` returns `None`, so the message shows with all of its
pasted content inline and no chips, as messages from before the change do.

## 6. The replay's "told once" path never runs in production

`crates/harness-acp/src/state.rs:200-206` and `state/subagents.rs:211,237` hold
sub-agent events back while `replaying`, then emit each sub-agent once in
`replayed()`. But `SessionManager` calls `session.events()` only after
`open_session` returns (`crates/harness-core/src/sessions.rs:46-48`). Every event
emitted during the open, including the batch emitted in `replayed()`, goes to a
broadcast with no receivers. Clients get sub-agents from the snapshot instead
(`state.rs:126`, `replay()`), which already holds each one once, as it ended. The
Claude connector sends each sub-agent's end before it answers the load (it awaits
the replay), so the snapshot was already final before this change.
`a_sub_agent_antigravity_replays_is_there_ended` subscribes before the replay,
which production never does. So the test passes on a path that doesn't run.
Either the original finding (correctness 10) had another cause that is still
there, or this code is dead.

## 7. The search status can stay stale indefinitely

`crates/cockpit-ui/src/app/capture.rs:196` keeps the old matches when they are
less than 500 ms old, even though the items changed. Nothing asks for a frame
after the 500 ms.

What goes wrong: search `/error` in a chat while the agent streams. Its last
message (with "error") comes 200 ms after the last search, and the turn ends.
Nothing else repaints. The key bar keeps saying `/error 2/3` (there are 4). On a
phone, where no pointer moves to cause a frame, it stays that way until the next
tap.

## 8. `last_seen_ended` can move backwards

`crates/cockpit-ui/src/store.rs:146` sets `last_seen_ended` to the time of the
latest end handled, not the latest end time. `render_subagents` returns 0 early
when that time is older than `subagent_ended_s`.

What goes wrong: sub-agent A ends at t=100 (handled live). Sub-agent B's end is
handled next, but its stamp puts it at t=96 (items 1 and 3 can make stamps go out
of order). At t=103, A should still show (until t=106). The early exit sees
`103 - 96 >= 6` and hides the strip. Taking the max fixes it.

## 9. "Stopping" is still kept by sub-agent id alone

8bc4bca keys the hub's sub-agent entry by thread and id, because ids repeat
across threads (correctness 8). The cockpit's `Agents::stopping`
(`agents.rs:213`) is still a set of bare `SubagentId`s, and `:268` removes an id
on any thread's update.

What goes wrong: the same saved session is resumed in two slots. Stop sub-agent X
in slot A. Slot B's copy of X sends an update with an ended status. A's chip stops
saying "Stopping…" before A's X has stopped. Stopping X in A also marks B's X as
stopping.

## 10. Malformed escapes in a label are now accepted

`crates/harness-contracts/src/links.rs:204-205` uses `urlencoding::decode`. It
keeps a bad `%` sequence (`%G1`, a trailing `%`) as literal text instead of
failing. The hand-written decoder it replaced returned `None` for those. A label
an agent writes with a bad escape now reads back as a piece with a literal
subject, instead of being rejected.

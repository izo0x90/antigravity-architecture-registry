# Re-review of 225bf33 (sign-in flow), correctness

Three problems, most severe first. The rest holds up. Every way of sending (Enter, Ctrl+Enter, the box's button, `chat.send`, `chat.inject`, `Space a c`, the palette) now goes through `send_typed`. The order within a frame (`take_events`, `handle_keys`, then render with `tend_sign_in_box` before the typing box) doesn't let Enter reach the agent once the box is the sign-in's. A late draft load, or a slot change on the same harness, no longer puts a draft into the sign-in's box. A paste being checked keeps the text.

## 1. When the sign-in ends, the box gets back the draft from when the desk was opened, not what the user was writing

`crates/cockpit-ui/src/chat.rs:264-267` (handing the box back sets `unsent_for = None`), `crates/cockpit-ui/src/chat/nav.rs:52-65` (`take_unsent`), `crates/cockpit-ui/src/drafts.rs:50` (`Drafts::saved`)

When the box is handed back, the commit takes the saved draft again: `unsent_for = None`, then `take_unsent` reads `self.unsent.get().saved(&slot)`. But `Drafts::saved` returns `loaded`, which is the map read from the server once when the desk opens (`Reply::Loaded`). `changed`/`tick` update `pending` and `sent` only, never `loaded`, and `load` doesn't read again once `loaded` is `Some`. So the draft that `tend_sign_in_box` reports just before it empties the box (line 257) reaches the server, but the box never sees it again. The new app test misses this because its draft is the one read at load and is never edited.

Sequence:
1. The desk opens. The lead's saved draft is `""`, or nothing.
2. The user types "refactor the parser" into the lead's box. `report_unsent` saves it (`sent[lead]`, server). `loaded[lead]` is still nothing.
3. A sign-in starts (`Space a l`, or another device). `tend` reports the same text again (no change) and empties the box. The note says "what you were writing is kept and comes back after".
4. The sign-in ends. `tend` empties the box and sets `unsent_for = None`. `take_unsent` gets `saved(lead) = Some(None)` and restores nothing, and `unsent_said = ""`. The box is empty.
5. The user types anything. `report_unsent` saves it over "refactor the parser" on the server, and the draft is lost for good.

Variant: the draft read at load was "fix the tests". The user sent it (the box was cleared and `""` was saved), then typed something new. After a sign-in, the box holds "fix the tests" again, a message already sent, ready to go out twice with one Enter.

This contradicts the Kept point "What was written is saved first and comes back after". A slot change during a sign-in to a slot on another harness hands back the same way and restores that slot's load-time draft, with the same problem.

## 2. If the drafts haven't been read, what's in the box is erased, yet the note says it's kept

`crates/cockpit-ui/src/chat.rs:256-261`

`tend_sign_in_box` saves the box through `report_unsent()`, which does nothing unless `unsent_for == slot`, i.e. the drafts were read and taken for this slot. If they weren't, `self.ui.input = Default::default()` throws the text away, and the note still says it is kept and comes back. The old `set_aside` kept it locally either way. This commit removed it.

Sequence:
1. The desk opens and the drafts read fails ("Couldn't read the unsent chat text: …"). `asked` goes back to false, but `load` runs again only on a reconnect, so `loaded` stays `None` and `unsent_for` never gets set.
2. The user writes a long prompt in the lead's box.
3. A sign-in starts for the lead's harness. The box is emptied, nothing is reported, and the note says "kept and comes back after".
4. The sign-in ends. `take_unsent` finds `saved() == None` and restores nothing. The prompt is gone.

The same thing happens without any failure if a sign-in snapshot arrives in the first moments after a reload, while the user has already typed but the drafts reply hasn't arrived.

## 3. (Low) The kept group id can be signalled after it's free, and could be reused

`crates/harness-acp/src/launch.rs:139-150`, with `crates/harness-acp/src/login.rs:107-110` and `login.rs:83-96`

`Group` now keeps the leader's pid and runs `kill_process_group(pid, KILL)` on drop, even after `wait()` has reaped the leader. While any member of the group is alive, the id can't be reused, so the common case is safe. If every member has exited, the id is free once the leader is reaped. On `finish` and on the `exited` path of `start`, the drop comes only after `read_out()`. That is up to 2 s per stream (4 s), when a process outside the group, such as a daemonised helper that `setsid`'d, still holds the pipes. If pids wrap in that window and a new process takes that pid as its group leader, its whole group is SIGKILLed. In this process, agents (`Group::spawn`) and terminal shells start their own groups. This is unlikely, but nothing rules it out. Signalling the group as soon as `wait()` returns, before `read_out`, shrinks the window. Signalling while the leader is still unreaped (a zombie keeps the id reserved) would close it.

Agents are not affected: their `Group` child is never waited on, so its pid stays reserved until drop.

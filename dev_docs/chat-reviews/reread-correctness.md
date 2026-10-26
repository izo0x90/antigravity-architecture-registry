Re-read of the chat fixes (50a125f..HEAD): correctness.

Ranked most serious first. Paths are relative to meta-harness/crates/cockpit-ui/src unless they say otherwise.

## 1. A stuck agent still never reads as quiet when you open the cockpit

`chat/live.rs:38`, `chat/live.rs:77`; `store.rs:302` (`Replayed`).

The findings say this is fixed: "opening the cockpit on a stuck agent reads as quiet, not as a burst." The burst is gone. The quiet state never comes.

The replay is not news, so `arrivals` stays empty. `since_last` is None. `activity()` maps None to `Liveness::Waiting`. `live_words` gives that state no time and `Next::OnChange`.

Sequence: the agent has been silent for 10 minutes. You open the cockpit or reload.

What you see: "Reading render.rs…" in grey, a flat trace and a breathing dot, for as long as the agent stays silent. "No word for 1 min" never shows. The plan asks for this state to be shown honestly. Nothing in the tests covers a thread with no arrivals after `Replayed`.

## 2. The turn the agent takes your added message into is drawn after the queued turn

`store.rs:213` (a new turn goes at the end of `turns`), `store.rs:234` (the stream is ordered by that place, not by `ordinal`). Server side: `crates/harness-acp/src/state.rs:189-218` (`steered`).

`put_item` now moves S into its new turn. But the new turn T comes after every turn the cockpit already holds, including a queued one.

Sequence (end review 2, unchanged):
1. Turn 1 runs. You queue prompt Q (turn 2, queued).
2. You add message S. The agent takes it.
3. The server makes turn T with ordinal 2 and renumbers Q to 3. It sends T, then Q, then S.

What you see: turn 1's items, then "Turn 3 QUEUED" with Q, then "Turn 2" with S, the agent's work and the live line. Turn 3 sits above Turn 2. A reload shows the same thing, because the hub replays by first-seen order and T was first seen after Q.

The new test `an_item_moved_to_another_turn_is_shown_there` has no queued turn, so it passes.

## 3. Words with a slash become file links in the agent's messages

`places.rs:74` (`path.contains('/') || …`), `places.rs:78`, `places.rs:22` (`breaks`); used by `conversation::places` with `Names::Placed`.

The old chat reader needed a name with an extension. It also skipped paths that start with `/` and split words at `*`. The shared reader takes any word with a `/` and letters in it. It turns an absolute path into a project path because the folder passed is `""`. And `*` is not a break.

Sequences:
- "Use read/write locks, see src/lock.rs." The links are `read/write`, then `src/lock.rs`. Enter opens the first one: "Open read/write", a file that doesn't exist.
- "Check /etc/hosts." This gives a link to `etc/hosts` in the project.
- "and/or", "client/server", "github.com/x/y" all become links.
- "Edit **src/a.rs**." The word is `**src/a.rs**`. It fails the path-character check, so no link is found.

## 4. After a reload the cursor lands on another item, and the chat isn't scrolled to it

`chat/render.rs:254` (`tend` pins `cursor_on` to whatever is at the index); `chat/nav.rs` `restore` (it scrolls only if the row exists yet).

The saved layout (one small HTTP answer) usually arrives before the event replay (thousands of socket messages over many frames). `restore` sets `cursor = Some(n)` with no thread, or with part of one. The first frame that has n+1 items pins the cursor to the item at n. The replay then inserts items before it, and the cursor follows the wrong item. `Replayed` exists, but `tend` doesn't wait for it.

Sequence: while turn 1 runs you queue a prompt. Later you put the cursor on message 40 (in the queued turn's span) and reload. Turn 1's later items were first seen after the queued message. They are replayed after it and placed before it.

What you see: the cursor sits on a different message from the one you left it on. Also, if the thread wasn't there at restore, `scrolling.show` is never called. The cursor isn't followed (`stick_to_bottom(false)`), so the chat opens at its top with the cursor's row out of sight.

## 5. Large changes say "Comparing…" forever and are re-sent every 30 s

`comparer.rs:14` (`call_bytes: 2 MiB` of raw text); `asked.rs:76`; `src/routes/changes.rs` (`TEXTS_MOST`; axum's default JSON body limit, also 2 MiB).

The client caps a call at 2 MiB of raw text. The JSON body is larger (escaped newlines and quotes, the keys). axum refuses any body over 2 MiB with 413 before the handler runs. The whole call fails. After `RETRY_S`, the same pairs are asked again in the same grouping and fail again. While a call is failed, `get` says `asked: true`, so the row shows "Comparing…", not a failure.

Sequences:
- One edit whose old and new texts total about 2 MiB, alone in its call.
- 32 full-file edits in one turn adding up to about 2 MiB.

What you see: "Comparing…" for as long as the chat is open. A 2 MiB POST is sent every 30 s while the row is drawn.

The same loop exists for a code block over 256 KB. The server refuses it, it stays plain, and it is re-sent every 30 s.

## 6. The tag places cache still survives a resync

`chat/things.rs:27`: the key is `(thread id, activity)`.

End review 13 is fixed for the shape (`shape_of` carries the epoch) and for the heights. It is not fixed for `tag_places`. After a resync the new view counts `activity` from 0 again.

Sequence: the cache is built at activity 500. A resync comes. The new view reaches 500 again, and the agent goes quiet at that count.

What you see: the stale vector comes back. It was built from the old stream, which held fewer items. Tags on the newest messages aren't drawn where they are. `show_target` for a message tagged there says "Turn N isn't in the chat".

## 7. Quiet folds errors and proposed plans into a blank line

`chat/rows.rs:177` (quiet: anything that isn't a message or a request joins); `chat/rows.rs:182` (one item is enough); `chat/rows.rs:391` (`summary` counts only calls and reasoning).

Sequence: quiet. The agent writes its message, then an `Error` item ("the harness crashed…"), a `Notice`, a `TodoList` or a `ProposedPlan` comes before its next message.

What you see: one folded row reading just "▸", with no words. The error or plan is hidden under it. `summary` finds nothing to count, so the line is empty.

## 8. Quiet: opening a run spills a sub-agent's work with no way to fold it

`chat/rows.rs:349` (an open run's rows get `nested: 0`).

In quiet, a Task call and all its sub-agent's items join one run. Opened, every item in it becomes a row. The Task's row gets `nested: 0`, so `here()` offers no "its sub-agent's work" fold, and Enter doesn't fold it.

Sequence: quiet, a Task whose sub-agent made 150 items. `z o` on the folded line.

What you see: all 150 items, a step in, and only the whole run can be folded again.

## 9. Answering a request that waited over a minute shows the agent gone quiet

`chat/live.rs:33` (`asking` is only while `waits`).

Once you answer, `waits` is false at once (`answering`). `since_last` is still the time the request came, because the echo of your answer isn't news.

Sequence: the agent asks to run a command. You answer after 3 minutes.

What you see: amber "No word for 3 min · the server still reaches it", with the trace dimmed, from your click until the agent's next update.

## 10. "Kept for each chat" is kept for each view

`chat/nav.rs:153-160` (`keep` / `restore_kept`); `chat/actions.rs` (`ChatCommand::Verbosity`, `PickSlot`, `StepSlot`).

`Picked` belongs to the buffer, not to the slot it shows.

Sequence: set the lead's chat to quiet. `J` to the coder slot: it is quiet too. Set it to everything. `K` back to the lead: everything. Two panels on the same slot can also show it two ways.

## 11. The key bar's search position goes stale as items come in

`app/capture.rs:185` (`desk.found` holds stream indexes from when the search ran).

Sequence: `/foo` finds items 4, 9 and 12. You're on 9 ("2/3"). A sub-agent item is put in before it. The cursor follows its item to 10.

What you see: the "/foo 2/3" status goes away. Or, if 10 was another hit's old index, it shows the wrong place. `n` recomputes and fixes it.

## 12. A tagged turn's drawn area leaves out its folded runs

`chat/things.rs:93` (`s.key.parse::<usize>()`).

A folded run's spot key is now `"3:run"`. It doesn't parse as a number, so `tag_drawn_at` skips it.

Sequence: a tag on a turn that is drawn as your message plus one folded run.

What you see: the tag's mark covers only your message.

## 13. What's kept is dropped oldest first, not least recently used

`asked.rs:122` (`order` is pushed only when a key is first kept; `get` never refreshes it).

The findings say "the longest unused go first". A block or change that is on screen the whole time, asked first, is the first one dropped once 1024 (colours) or 512 (changes) others have been asked.

What you see: a code block that stays in sight goes plain, is asked again, and is coloured again.

## 14. Every plain message of yours now waits on the state database in the event path

`crates/harness-core/src/sessions.rs` (`typed_wanted`, the `spawn_blocking(...).await` in the forward loop); `crates/nah-state/src/lib.rs:81` (one connection behind one mutex); `crates/harness-acp/src/session.rs:178` (`filter_map(|e| async move { e.ok() })`).

Before, only a message carrying `HANDED_ON` was looked up. Now every user `ItemUpdated` without `typed` is: every plain message, every replayed one, and each chunk of a replayed one. The session's forwarder stops on each lookup until the shared connection is free. Every `keep_typed` also runs `DELETE … NOT IN (SELECT … LIMIT 10000)` under that same lock.

The adapter's channel holds 1024 events. A lagged receiver's events are dropped silently.

Sequence: a long history loads (`session/load`) while other state writes hold the connection.

What you see: if the backlog passes 1024, items and turn updates are missing for good. The hub's cache never had them, so a reload doesn't bring them back.

## 15. The code view copies its whole file each time the actions are gathered

`code.rs` (`copy_here`: `self.content.clone()`); `app/commands.rs:112` (`panel_actions` calls `copy_here` to decide whether to offer `nav.copy`).

The offer check builds a full `Copyable`. Sequence: a 5 MB file open in the code view. Each time the panel's actions are gathered (the key bar's list), a 5 MB string is cloned and thrown away. The chat does the same with the whole message under the cursor.

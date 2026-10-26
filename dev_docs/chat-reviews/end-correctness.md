End review of the chat plan (d54286c..50a125f): correctness.

Ranked most serious first. Paths are relative to meta-harness/.

## 1. Your own message, and requests, get the last sub-agent's call as their parent

`crates/harness-acp/src/state.rs:323` (`parent_id: self.parent.clone()` in `push_item_in`), reached from `begin_turn` (162), `inject` (174) and `open_request` (622). `crates/cockpit-ui/src/chat/rows.rs:350` (`nesting`).

`self.parent` is set by `apply()` from the last ACP update and is never reset. Every item pushed afterwards gets it as its parent, including items that are not part of that update.

Sequence:
1. Claude Code runs a Task. Its sub-agent sends tool calls with `parentToolUseId`, so `self.parent` is the Task's item.
2. While the sub-agent works, you press Add (steer) or send a queued prompt.
3. `inject` / `begin_turn` pushes your UserMessage with `parent_id` set to the Task.
4. Cockpit: `place()` puts it after the Task's descendants (same turn). `nesting()` does not check turns, so a queued turn's message also lands at depth 1 if the Task is still open on the stack. That stretches the Task's `end` over your message.

What you see: your message disappears into the Task's folded "N steps by its sub-agent" row. For a queued prompt, no row starts in the queued turn, so its "Turn N QUEUED" line is not drawn either. After a reload it stays hidden, because the item keeps its parent.

Same cause, different case: a permission request is nested under whichever call sent the last update. With two Tasks running in parallel, sub-agent A's request can land under Task B. A main-agent request after a sub-agent update lands under the Task.

## 2. A message the agent takes mid-turn stays where it was typed, under the old turn

`crates/cockpit-ui/src/store.rs:117-123` (`put_item`); server side `crates/harness-acp/src/state.rs:192, 208` (`steered`).

`put_item` replaces an existing item in place. It never re-places the item when its `turn_id` changes. `steered()` moves the added message into a new turn T (inserted after the running turn, with later turns renumbered) and re-emits it with `turn_id = T`.

Sequence:
1. The agent runs turn 1. You queue prompt Q (turn 2, queued).
2. You Add message S. It is placed in turn 1's span.
3. The agent takes S. The server creates turn T (now ordinal 2, Q becomes 3) and re-emits S with `turn_id = T`.
4. The cockpit appends T to `turns` after Q, but S stays in turn 1's span.

What you see: `render_stream` (`chat/render.rs:348`) draws a new turn line whenever `turn_id` changes. You get turn 1's items, then a "Turn 2" line over S in the middle of turn 1, then "Turn 1" again, then "Turn 3 QUEUED", then a second "Turn 2" over the agent's work. The live line goes on the first, misplaced "Turn 2" line, above older items. `items_of(T)` leaves out S, so tags and message numbers for S follow turn 1. A reload puts S in a different place, because the hub replays S with its new turn.

## 3. The cursor is a place in the stream, and items are now inserted mid-stream

`crates/cockpit-ui/src/chat.rs:92` (`cursor: Option<usize>`); `crates/cockpit-ui/src/store.rs:158-170` (`place`).

Before this plan, items were only appended. Now `place()` inserts a sub-agent's items after their call, and a running turn's items before any queued turn. Every item after the insertion point moves down one place. The cursor and `pick` keep the old number.

Sequence: Claude Code starts Tasks A and B in parallel. You put the cursor on Task B's row. Sub-agent A sends a tool call. It is inserted after A, before B. The cursor now points at A's new item, which sits inside A's folded row.

What you see: the highlight jumps to Task A. `z a`, Enter and `c` then act on A, not B. Each new item from A moves it again. The same happens with the cursor on a queued turn's message: every new item of the running turn takes the cursor's place.

## 4. In a narrow panel the live line's words stop updating

`crates/cockpit-ui/src/chat/live.rs:84` and `:107`.

When the turn line has no room for the indicator's spot (`room < SPOT.x`), `render_live` returns no spot. `show_live` then returns at `let Some(spot) = spot else { return }` before it asks for the next repaint. That repaint is also what keeps the words current.

Sequence: a panel about 260 px wide, a running turn that changed files ("Turn 3 · 4 files changed" leaves under 64 px). The agent goes silent.

What you see: "Reading render.rs… 12 s since anything" stays frozen. It never turns into the amber "No word for 1 min" until something else causes a repaint, and the next update shows it as receiving again. In a narrow panel you never see "quiet too long".

## 5. An update's arrival time is when a frame applies it, not when it came

`crates/cockpit-ui/src/agents.rs:154, 402`; `crates/cockpit-ui/src/store.rs:106, 221-224`.

`now` is the frame's clock. Events wait in `net.incoming` until the next frame. Only a Resync after an earlier state is told apart from news. On the first connect, `before` holds no threads, so the whole replay counts as news.

Sequence A: the agent has been stuck for 5 minutes. You open the cockpit. The replay comes in.
Sequence B: the tab sits hidden for 10 minutes (no frames run). The agent was busy 10 to 6 minutes ago and silent since. You come back.

What you see in both: blue "Thinking…", a burst of blips, then "3 s since anything". "No word for 1 min" shows only a minute after you looked. The truth is 5 or 6 minutes. The plan asks for this state to be shown honestly.

## 6. Waiting on you for a minute reads as the agent gone quiet

`crates/cockpit-ui/src/chat/live.rs:26, 68`.

Liveness counts only the time since the last update. A pending approval stops all updates. After `quiet_after_s` the line becomes `Liveness::Quiet`. That state's words drop `doing` ("Asking you").

Sequence: the agent asks to run a command. You don't answer for 61 seconds.

What you see: amber "No word for 1 min · the server still reaches it", with the trace dimmed, as if the agent were stuck. In fact it is waiting on you.

## 7. A call's change is diffed from what the harness sends, with wrong numbers

`crates/cockpit-ui/src/chat/render.rs:474-482`; `crates/cockpit-ui/src/diff.rs:66`.

The chat runs its own line diff (`similar`) over the call's `old_text`/`new_text`. The plan says the Changes view's diff, and no second diff. For Claude Code's Edit, those texts are the replaced snippet, not the file. A missing `old_text` becomes "".

Sequence 1: the agent edits 3 lines at line 400 of a file.
What you see: "@@ −1,3 +1,3 @@" and line numbers 1 to 3. The Changes view shows 400 to 402 for the same change.

Sequence 2: a call sends no old text for a file that already exists (Claude Code's Write does this).
What you see: "+305 −0", every line marked added, though most lines did not change.

## 8. A switch between whose update it is splits a streamed message into pieces

`crates/harness-acp/src/state.rs:373-376`.

`apply()` closes every stream when the parent of the update differs from the last one. Any update without `_meta` (usage, plan, mode, or the Task's own progress) counts as the main agent's.

Sequence: two sub-agents run in parallel and both stream text or reasoning. Their chunks interleave: A, B, A, B.

What you see: each change of sender closes the other's stream. Each sub-agent's message turns into many short messages, each with its own header. The same happens to the main agent's streaming message when a sub-agent update, or a usage update during a sub-agent's stream, comes in between its chunks.

## 9. The verbosity you pick for a chat is not kept

`crates/cockpit-ui/src/chat/actions.rs` (`ChatCommand::Verbosity` sets only `self.ui.folds`); `crates/cockpit-ui/src/chat/nav.rs:172`.

The pick lives only in the view's folds. It is not in the chat's place, the config, or the slot. `share()` also writes the config's `shows` over it whenever the settings change. That happens once at start-up, when the user config arrives.

Sequence: you set a chat to "everything" (Space , 3) and reload the page.
What you see: the chat is back to "normal". What you opened or folded by hand is gone.

## 10. A search hit on a folded run's first call stays folded

`crates/cockpit-ui/src/chat/render.rs:442` (`if row.first == index … break`); `crates/cockpit-ui/src/chat/nav.rs:343, 358`.

`/` searches every item, including calls inside folded runs. `put_cursor` opens a run to show a hit unless the hit is the run's first item.

Sequence: `/parse.rs`, where the first of three folded Read calls read parse.rs.
What you see: the cursor lands on "Read 3 files" with nothing showing parse.rs. `n` to a hit on the run's second call then opens the run.

## 11. A code block being written flickers between plain and coloured

`crates/cockpit-ui/src/highlighter.rs` (`colours`, `tick`).

Colours are kept by block text. A streaming block has new text with each chunk. That text is unknown, so it is drawn plain and a new request goes to the server. When the reply arrives, the block is coloured until the next chunk. More than 1024 kept blocks clear everything, and every block on screen goes plain and is asked for again. A request that fails (the server briefly down) is kept as "no colours" and never asked again.

What you see: while the agent writes a long code block, it flashes between coloured and plain. A block drawn during a reconnect stays plain for the rest of the session.

## 12. Code blocks and tables lose or swap their sideways scroll as you scroll the chat

`crates/cockpit-ui/src/conversation.rs:131, 156` (`id_salt(ui.next_auto_id())`); `crates/cockpit-ui/src/chat/render.rs:364`.

A box's id comes from how many widgets came before it. A row out of sight now takes one allocation, not its widgets. As rows above move in and out of the drawn window, every later box gets another id.

Sequence: scroll a wide code block sideways, then scroll the chat up or down by a screen.
What you see: the block jumps back to its left edge, or shows another box's offset.

## 13. After a reconnect, caches can return what was worked out before it

`crates/cockpit-ui/src/chat.rs:299` (the shape kept by `(thread, activity, folds)`); `crates/cockpit-ui/src/chat/render.rs:474-482` (diffs kept by `changed`); `crates/cockpit-ui/src/chat/render.rs` `tend_folds`.

A Resync rebuilds the ThreadView with `activity` from 0, but the chat keeps its caches. When the new count reaches the number a shape was kept at, that old shape comes back for the new stream. If the agent goes quiet at that count, it stays. The rows then miss the items that came since, or point at the wrong ones, until the next update. A diff kept at a `changed` number that comes around again is shown for the item's new text. Also, if a frame runs between the Resync and the slot's replay, `tend_folds` sees no thread and wipes every fold you set by hand.

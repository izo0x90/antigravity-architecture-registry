End review of the chat plan (d54286c..50a125f): faithfulness.

Paths are relative to meta-harness/. Ranked most serious first.

1. A second diff was built, against the plan's rule.
   - Plan: "Changes: the Changes view's diff, shown folded; no second diff."
   - Code: crates/cockpit-ui/src/diff.rs:66 (`hunks`) compares the two texts in the browser with a new crate, `similar` (crates/cockpit-ui/Cargo.toml:31). The chat calls it at crates/cockpit-ui/src/chat/render.rs:474-482.
   - Gap: the Changes view's diff comes from git on the server (nah-changes, git2). The chat now has a second diff engine. Only the drawing (`draw_hunk`) is shared. Its line numbers count from 1 in whatever text the harness sent with the call. If that text is a snippet, the numbers are not the file's.

2. The chat keeps its own list of what waits on you, not the inbox's.
   - Plan: "Pending requests: the request card and the inbox's list of what waits, not a new kind of card."
   - Code: crates/cockpit-ui/src/chat.rs:276 (`pending`) builds the list from the thread's requests, filtered by a new client-side `answering` set (crates/cockpit-ui/src/agents.rs:133, `waits` at :164). The inbox still lists notifications (crates/cockpit-ui/src/attention.rs:12) and never reads `waits`.
   - Gap: there are now two lists of what waits. They disagree: while an answer is on its way, the chat drops the request but the inbox still shows it.

3. The "per chat" verbosity is reset by unrelated settings, and it is not kept.
   - Plan: "A verbosity setting per chat"; "a chat setting like its others (the config actions, `m #`)".
   - Code: crates/cockpit-ui/src/chat/nav.rs:169-175. When any graph, code, Changes or chat UI setting changes, every chat's verbosity is set back to `[chat] shows`. That also clears every fold set by hand, through `set_verbosity` and then `afresh`. `ChatPlace` (crates/cockpit-ui/src/chat.rs:219) keeps no verbosity, so a reload loses it too.
   - Gap: a chat's choice lasts only until any setting changes or the page reloads. It is also a settings-row action (`Space , 3`, chat.verbosity), not one of the `config.*` actions reached by `m #`.

4. The width test passes without drawing several of the long lines it claims to test.
   - Plan: "A test draws long lines in a narrow panel and fails if anything is outside it"; "tested once for all items."
   - Code: crates/cockpit-ui/src/app/tests/chat_view.rs:95, :98, :99. These put the long text in reasoning, in a tool call's output and in a command's output. By default (normal) all three are folded (crates/cockpit-ui/src/chat/rows.rs:159, :182), so the long text is never laid out. The test passes on the "after it" line alone. The test says it covers "every kind of item", but it has no proposed plan, to-do list, sub-agent item, or opened file change with hunks.
   - Gap: the width rule is not tested for any folded content once it is opened.

5. A run of calls is not "everything between two messages".
   - Plan: "A run of calls between two messages is one line"; quiet shows "messages and what needs you".
   - Code: crates/cockpit-ui/src/chat/rows.rs:216. Only calls join a run (in quiet, reasoning joins too), and only calls with no sub-agent work.
   - Gap: in normal, any reasoning between calls splits the run into several lines (Claude Code often thinks between calls). In quiet, to-do lists, notices, plan updates and a Task call with sub-agent work still show as their own rows between two messages.

6. The heartbeat does not blip once for each update.
   - Plan: "blips once for each update that comes in, so its rhythm is the real one."
   - Code: crates/cockpit-ui/src/store.rs:106 groups updates into tenths of a second. crates/cockpit-ui/src/indicator.rs:123-136 then draws any updates closer than a blip's width (6 px, about 1.2 s on a 64 px, 10 s trace) as one blip. The count stored with each tenth is never used.
   - Gap: a steady stream shows evenly spaced blips about 1.2 s apart, whatever the real rhythm is.

7. "Several at once" was not tested with the mock harness, or in the inbox.
   - Plan: "several at once answerable (a mock test with two at once)"; "each seen and answerable, in the chat and the inbox."
   - Code: crates/cockpit-ui/src/app/tests/chat_view.rs:152 feeds hand-made events into the cockpit's store. The mock agent (crates/harness-acp/src/testing.rs:244-279) has no prompt that asks twice at once. No test covers two waiting requests in the inbox (crates/cockpit-ui/src/inbox.rs:363).
   - Gap: neither the server taking two waiting requests nor the inbox side was tested.

8. Copy does not use the one copy mechanism the plan named.
   - Plan: "Code blocks: ... the copy mechanism decided in the selection TODO (one)."
   - Code: crates/cockpit-ui/src/chat/actions.rs:488 adds another direct `ctx.copy_text`, like the code view's and the terminal's. TODO.md:24-27 still says the one mechanism is "to decide".
   - Gap: one more separate copy path was added, and the plan's dependency was not raised.

9. Chips in a sent message do not look as they were typed.
   - Plan: "Your messages as typed, links as chips."
   - Code: crates/cockpit-ui/src/conversation.rs:76 draws every chip in a sent message in the accent colour. Its comment says "a tag's own colour is the shell's to say", but nothing re-colours them. In the typing box the shell does re-colour chips (crates/cockpit-ui/src/app/linking.rs, `tags.get().link_colour`).
   - Gap: a tag chip has its tag's colour while you type and loses it once sent.

10. Built without being asked for.
    - A copy of the running turn's line pinned at the top of the stream when its own line scrolls away: crates/cockpit-ui/src/chat/live.rs:96-130 (`pin_live`). The plan says the indicator draws in "its spot on the turn's line".
    - Rows out of sight are not drawn; they keep the height they had last frame: crates/cockpit-ui/src/chat/render.rs:364. The plan has no step for this.

11. The key's comment says something other than what the key does.
    - Code: crates/cockpit-ui/config/default.toml:251 says `c` copies "the code block you're on (picked with f, else the message's first)". `copy_here` (crates/cockpit-ui/src/chat.rs, `copy_here`) copies the whole message when no block is picked. The action's own words and README.md say the same as the code, not the comment.

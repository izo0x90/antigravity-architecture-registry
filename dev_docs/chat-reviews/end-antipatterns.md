End review of the chat plan (d54286c..50a125f): anti-patterns and bolt-ons.

Paths are relative to meta-harness/. Ranked most serious first.

1. What you typed is recorded by a hash of the sent text, through a side hook, and two different checks decide it.
   - crates/harness-core/src/slots.rs:595-600 keeps the typed message only when `text != typed.text`.
   - crates/harness-core/src/sessions.rs:41-47 looks it up only when the text contains `HANDED_ON`.
   - crates/nah-state/src/slots.rs:107-161 and migrations.rs:65 store it in a new table keyed by the SHA-256 of the whole sent text.
   - src/modules.rs:331-333 and sessions.rs:36,57 wire it in late through a `OnceLock` callback. That callback rewrites every `ItemUpdated` event on its way to the hub.
   - Effect: `Things::attach` (crates/nah-things/src/attach.rs:56-58) returns the text unchanged when nothing is handed on. A message whose links hand nothing on (pointer links, or a reader who can't reach them) is never kept. It shows with no chips.
   - Dead parts: the `at_ms` column is written but never read, and nothing prunes the table.
   - Instead: for live turns, put `typed` on the `UserMessage` item when the turn is begun (`SlotManager::send` hands the typed `Message` to the session with the prompt). Keep a text-keyed record only for replayed history. Keep and look up under one test ("it has links").

2. The handed-on block is a magic string inside the message text, and three crates parse meaning from it.
   - crates/harness-contracts/src/links.rs:56 defines `HANDED_ON`. attach.rs:61 writes it.
   - sessions.rs:44 and crates/cockpit-ui/src/chat/rows.rs:129 test for it with `contains`. crates/cockpit-ui/src/conversation.rs:412 splits on it.
   - A user who types that text breaks all of them.
   - Instead: carry the parts as fields, for example `UserMessage { text, typed, handed_on: Option<String> }`, filled from `Expanded`. Nothing should search the text.

3. A second diff engine. The plan says "the Changes view's diff, shown folded; no second diff."
   - crates/cockpit-ui/src/diff.rs:66-94 computes hunks in the browser with a new dependency, `similar` (crates/cockpit-ui/Cargo.toml:29). crates/cockpit-ui/src/chat/render.rs:474-485 uses it.
   - The server already builds the same `Hunk`s with git2 (crates/nah-changes/src/compare.rs:162-191).
   - Instead: produce a change's hunks in that one place (git2's `Patch::from_buffers` takes an old and a new text). Send them with the item or from an endpoint. Drop `similar`.

4. A second markdown parser beside the one already used.
   - crates/cockpit-ui/src/conversation.rs:176-300 (`parts`, `opening`, `closes`, `tables_apart`) finds fences and tables by hand. It then passes each fragment to `CommonMarkViewer` on its own.
   - Cutting the text apart breaks anything that spans the cuts: a fence inside a list item or a quote, or a reference link defined in another part.
   - A table is any run of `|` lines with `--` somewhere in them (line 297).
   - pulldown-cmark is already in the build (Cargo.lock), through egui_commonmark.
   - Instead: one parse with pulldown-cmark's offset iterator to find the code-block and table ranges.

5. The place keys carry their kind in a letter prefix, and they are built in one file and parsed in another.
   - crates/cockpit-ui/src/chat/render.rs:497, 505 and 521 write `"{index}:{n}"`, `"{index}:c{n}"` and `"{index}:l{n}"`.
   - crates/cockpit-ui/src/chat.rs:206-214 (`Pick::of`) reads them back with `strip_prefix('c')` and `strip_prefix('l')`.
   - Nothing ties the two sides together.
   - Instead: `Pick` owns both directions (`Pick::key(index)` and `Pick::of`), so the encoding lives in one place.

6. Another copy action of its own. The plan says to use "the copy mechanism decided in the selection TODO (one)."
   - crates/cockpit-ui/src/chat/actions.rs:278 and 485-490 add `chat.copy`, bound to `c` (config/default.toml:252).
   - Other copies already exist in code.rs:954, terminal/mod.rs:293 and 801, and chat/actions.rs:459. Each has its own note wording.
   - TODO.md:24-27 names exactly this spread as the thing to replace.
   - Instead: one copy action, offered by the host, that asks the buffer what can be copied where you are (its text and its name).

7. The same chip gets two colour rules.
   - crates/cockpit-ui/src/conversation.rs:76 colours every chip in a sent message with `accent`.
   - The typing box's chips get their tag's colour from `Tags::link_colour` (crates/cockpit-ui/src/tags.rs:112), added in this change and used by app/linking.rs:228.
   - The comment at conversation.rs:73-74 says the tag's colour is "the shell's to say". The shell never says it for messages.
   - Instead: `Messages` reads the tags (as it already reads the highlighter) and calls `link_colour`.

8. There are now two measures of the agent's activity, with different rules.
   - `ThreadView::activity` (crates/cockpit-ui/src/store.rs:31) counts every event, echoes and replays included. It drives the AGENT light (agents.rs:265, app/draw/top_bar.rs:211).
   - The new `arrivals` (store.rs:43, 106-113) counts only real news from the agent. It drives the live line.
   - Instead: one measure. The light reads `arrivals` too.

9. Replays after a resync are detected by a time window instead of a signal.
   - crates/cockpit-ui/src/store.rs:17 and 254-260: for 10 s after a `Resync`, anything equal to what was held before counts as replayed.
   - To compare, it keeps a full clone of every thread (`before`).
   - Instead: the server marks where the replayed state ends, or the client treats the batch that follows the `Resync` as the replay. No timer.

10. The indicator setting is a string, and the verbosity's names are kept in two places.
    - crates/cockpit-ui/src/config.rs:133 and 145-151 store `indicator` as a `String`, checked against `indicator::ALL`.
    - chat/nav.rs:173 and the `Indicator` command look the name up again.
    - `Verbosity` gets its names from serde (`rename_all = "lowercase"`, chat/rows.rs:58) and also from its own `name`/`from_name` (rows.rs:69-79). The two must agree by hand.
    - Instead: deserialize the indicator once into a typed choice. Take Verbosity's names from one source.

11. Data carried and never read, and a comment that doesn't match what the code does.
    - `Activity::updates` is `(f64, u32)` (crates/cockpit-ui/src/indicator.rs:32). The count travels from the store through the live line to the indicator. The heartbeat ignores it (indicator.rs:131).
    - indicator.rs:102 says it "blips once for each update as it came". It actually blips once per tenth-second bucket and drops any blip closer than 6 px to the last one (lines 131-135). The plan asked for one blip per update.
    - Instead: draw by the count or drop it, and make the comment say what is drawn.

12. A doc comment in the wrong place.
    - crates/arch-service/src/service.rs:403-412: the new `highlight` sits between `code_analysis`'s doc comment and `code_analysis` itself.
    - `highlight` now has both functions' docs, and `code_analysis` has none.
    - Instead: move `highlight` and its own comment below `code_analysis`.

13. Magic numbers outside the theme and config.
    - crates/cockpit-ui/src/chat/live.rs:12: `SPOT` is 64×12.
    - chat/render.rs:91 and 140: `WAITING_SHARE` is 0.4, with `.max(80.0)`.
    - render.rs:350: `Separator::spacing(8.0)`, a second copy.
    - indicator.rs:123 and 137: the blip shape (6, 2, 4, 0.6) and a 3 s breath.
    - live.rs:70 hard-codes 60 s per minute in "No word for N min", although `quiet_after_s` can be set. Set it under 60 and the line reads "No word for 0 min".
    - store.rs:13: `ARRIVALS_KEPT = 600` is "a minute's", so it assumes the default `quiet_after_s = 60`.
    - Instead: put sizes in the theme's `look`, and derive the arrivals bound from `quiet_after_s`.

14. An empty string stands for "nothing", and the "one table" has words outside it.
    - crates/cockpit-ui/src/chat/words.rs:31, 33 and 39 use `thing: ""` to mean the verb has no object. rows.rs:420-422 matches on `""`.
    - words.rs:1-3 says each kind's words live "in one table". chat/live.rs:34-39 adds "Working", "Starting", "Writing" and "Asking you" outside it.
    - Instead: make it `Option<&'static str>`, and put those words in the table.

15. Duplicated helper.
    - chat/rows.rs:429 and chat/render.rs:397 each capitalize a first letter by hand, by byte slicing.
    - `spec::capitalized` (crates/cockpit-ui/src/spec.rs:873) already does this.
    - Instead: use `capitalized`.

16. Matching a fence to a language is uneven.
    - crates/code-analyzer/src/pack.rs:938-941 builds a `names` check (the name or an extension) but applies only the extension check to dialects.
    - `Pack::for_fence` is public and case-sensitive. `PackSet::for_fence` (line 1012) lowercases first.
    - Instead: apply `names` to dialects, and normalize case in one place.

17. Two parsers for `path:line`.
    - crates/cockpit-ui/src/conversation.rs:237-275 (`places`, `is_path`) reads `path:line` out of a message's words.
    - crates/cockpit-ui/src/app/linking.rs:129-138 (`lines_of`) already parses `path:line` and `path:a-b`.
    - Instead: one parser of the notation, used by both.

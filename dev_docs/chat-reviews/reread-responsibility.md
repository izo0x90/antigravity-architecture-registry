Re-read of the chat plan's end-review fixes (50a125f..HEAD): responsibility.

Paths are relative to `meta-harness/`. Line numbers are as of HEAD (c2b5abc). Most serious first.

## 1. The chat's row-cache rule now lives in the wire contract and the entity store

- `crates/harness-contracts/src/items.rs:74-86` adds `ItemBody::has_detail`, "whether it has more than its line, folded under it". That is the chat's fold rule (`chat/rows.rs:89-91` `folds` now only calls it). The contract crate that the server, the adapters and every harness share now holds a display rule of one cockpit view.
- `crates/cockpit-ui/src/store.rs:155-217` and `:262-265` (`shape`, `epoch`, `shape_facts`, the global `EPOCHS` at `:17`) make the store bump a counter whenever "what the stream's shape depends on" changes: kind, status, parent, turn, `has_detail`. That list is a copy of what `chat/rows.rs` reads today (`is_call`, `folds`, `runs`, `item_open`). The chat keys its `Shape` cache on it (`chat.rs:326`).
- Nothing ties the two together. If `rows.rs` starts reading anything else (a call's `ToolKind`, its title, a request's state), the store doesn't bump `shape`, and the chat keeps showing stale rows with no error.
- The cache's invalidation belongs with the chat. For example, the chat can hash what it reads, or the store can expose a plain change counter per item that the chat combines itself.
- Also, the new function went in between `said`'s doc comment and `said` (`items.rs:71-77`). `has_detail`'s doc now opens with "What a message says…", and `said` (`:88`) has no doc.

## 2. "One path parser" made every absolute path in a message a bogus project path

- `crates/cockpit-ui/src/conversation.rs:390`: `places` calls `crate::places::links(line, "", Names::Placed)`, with an empty project folder.
- `crates/cockpit-ui/src/places.rs:75`: `path.strip_prefix("")` always succeeds, and `.strip_prefix('/')` then cuts the leading slash. So `/Users/izo/code/x/meta-harness/src/arch.rs:128` becomes the link `Users/izo/code/x/meta-harness/src/arch.rs` at line 128. Enter opens a file that isn't there.
- Before the fix, the chat dropped absolute paths. The test that said so (`/abs/a.rs` in the "Not" list) was taken out of `the_files_a_message_names_are_read_from_its_words`.
- Agents (Claude Code) usually write absolute paths. The chat has no project folder to pass, so the shared reader was wired in without an input it needs.
- `*` is gone from the chat's word breaks. `places.rs:23` `breaks` doesn't have it, and the `plausible` check refuses it. So a bolded `**src/a.rs**`, which agents write often, is no longer a link.

## 3. The session layer still calls up into the slot store, now for every message of yours

- `crates/harness-core/src/sessions.rs:42-52` (`typed_wanted`) now asks for a lookup on every `UserMessage` update whose `typed` is `None`. Before, it asked only for ones holding the marker. That covers:
  - every lead report, queue brief and discussion prompt;
  - each chunk of a replayed message: `harness-acp/src/state.rs:465-474` emits `ItemUpdated` per chunk, so a message replayed in N chunks is looked up N times, with prefixes that never match.
- Each lookup is a `spawn_blocking` SQLite read plus a SHA-256. It is awaited inline in the forwarding loop (`sessions.rs:94-97`), so all of that session's later events wait behind it.
- The upward call itself (`SessionManager` reaching `SlotStore` through the late-bound closure, `src/modules.rs:333`, `sessions.rs:68`) was the end review's responsibility 1 point. It isn't listed as fixed or as left.

## 4. The indicator's interface is cut to one indicator's constant, and the store depends on it

- `TRACE_S` is the heartbeat's own span (`crates/cockpit-ui/src/indicator.rs:142`, under `pub struct Heartbeat`). It now limits what goes through the interface:
  - `chat/live.rs:43` hands every indicator only the updates of the last `TRACE_S` seconds;
  - `store.rs:13` keeps `ARRIVALS_KEPT = TRACE_S * 10` arrivals per thread.
- The entity store's record now depends on a drawing implementation's constant. An indicator wanting a longer window (the planned lemmings) can't get one without changing the store and the chat.
- The plan's interface was "the times of recent updates" as plain data. The window should be the interface's, not the heartbeat's.

## 5. Responsibility 12 is half fixed: the store still decides what counts as the agent's news

- `Replayed` replaced the store's 10 s replay guess.
- The other half of the finding was that the store holds the live line's rules for what counts as activity. Those rules are still in the store:
  - a queued turn isn't news (`store.rs:353-354`);
  - an answer's echo isn't news (`store.rs:371-372`).
- The store also now keeps two counters per thread, `activity` and `news` (`store.rs` `ThreadView`). The AGENT light reads one and the live line reads the arrivals.

## 6. The server's size limits are copied into the cockpit, and a call over them is retried forever

- Each limit is written twice, with no shared source:
  - `crates/cockpit-ui/src/comparer.rs:14` (`call_bytes: 2 MiB`, "the server's bound") copies `src/routes/changes.rs:126` (`TEXTS_MOST`);
  - `crates/cockpit-ui/src/highlighter.rs:20` (256 KiB) copies `src/arch.rs:148` (`HIGHLIGHT_MOST`).
- `TEXTS_MOST` is the same 2 MiB as axum's default body limit, and nothing raises that limit (no `DefaultBodyLimit`). Once the texts are JSON-encoded (quotes, escapes, both sides), a call near the bound is refused by axum with 413 before the route's own check runs.
- `asked.rs:107` (`call.is_empty() || …`) sends one question bigger than the bound alone. The server refuses it, `Reply::answered(None)` marks it failed, and `RETRY_S` asks again every 30 s for as long as it's drawn.
- Example: one large file an agent rewrote is re-posted to the server every 30 s and never compared.

## 7. The one copy action builds the copied text just to label a button

- `crates/cockpit-ui/src/app/commands.rs:114` calls `copy_here()` every time the panel's actions are gathered, only to get `copied.what` for "Copy {what}".
- `copy_here` must return the text too:
  - `code.rs:1050` clones the whole open file;
  - the terminal builds its selection text;
  - the chat runs `codes_here` (a markdown parse) or `said()`.
- The interface ties "is there something to copy, and what is it" to producing it. It should say `what` cheaply and build the text only when `nav.copy` runs (`commands.rs:439`).

## 8. Config choices are named two ways, and config and the indicator name each other

- The findings say the choices are now "named one way (`config::Named`)". `PanelsUi.item_buttons` (`config.rs`, `crate::hosted::ItemButtons`, `hosted.rs:15`) is still named by `serde(rename_all = "lowercase")`, with no words or detail for the screen. A config choice now has two mechanisms.
- `config.rs:134` names `crate::indicator::Kind`, and `indicator.rs:77` implements `crate::config::Named`. Config depends on the indicator module and the indicator depends on config.
  - `Verbosity` moved into config to end the same kind of dependency (responsibility 11), but `Kind` didn't.

## 9. The key bar's search count reads a second copy of the matches that goes stale

- `crates/cockpit-ui/src/app/desk.rs:60` adds `found` beside `searches`. It is filled only on Enter and on `n`/`N` (`app/capture.rs:85`, `:160`), and the key bar reads it (`capture.rs:185-187`).
- The chat's search keys are stream places, and they shift when items are put in before them: the case `tend` now handles for the cursor (`chat/render.rs:233-258`).
- After a sub-agent's item lands, "/q 2/5" names the wrong match, or none, until the next `n`.

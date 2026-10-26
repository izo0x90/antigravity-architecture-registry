# End review: responsibility and coupling

Scope: `git diff 3e1a805^..HEAD` in `meta-harness/` (18 commits). Paths are relative to `meta-harness/`. Ranked, most important first.

## 1. The overlay stack is not the one mechanism. Each kind still has its own closing code.

- `crates/cockpit-ui/src/app/overlays.rs:11-43`: whether an overlay is open, and how it closes, is a `match` over every kind in the shell. It reads and writes other parts' state directly: `self.feedback.is_inspecting`, `is_modal_open` and `is_issues_window_open` (public fields, `feedback.rs:87-100`), `pending_keys`, `pending_count`, `capture`, `input_mode`, `palette`. A new kind needs a new enum variant (`overlay.rs:13`), two new match arms here, and its own Escape code. The plan said a new kind is "a new entry in the stack, not new closing code."
- `overlay.rs:56-62`: five of the eight kinds set `own_back: true`, so the stack skips them on Escape. Their Escape is still the old per-mode code: pending keys and the quick look at `app/keys.rs:172-180`, the menus at `keys.rs:137-142`, the hints in `on_capture`, and the palette's own code. So the stack's "Escape closes the top one" covers only the three developer windows.
- `overlays.rs:66`: a setting's list of choices (`InputMode::Choosing`) is not in the stack. It relies on egui's own outside-click closing. Toasts are not entries either (the plan named them). The floating item buttons count as part of the quick look only when the host is the quick look (`app/draw/host.rs:224-228`). In a panel they are not an overlay at all.
- `overlays.rs:54-59`: `keys_reach` makes the quick look hold the keys from panels. Its own `Rules` say it is not modal (`overlay.rs:58`). The same fact is decided in two places, and they disagree.

## 2. The chat buffer doubles as a sub-agent viewer through a mode flag.

`ChatView::alone` (`crates/cockpit-ui/src/chat.rs:148`) turns the chat into a second view. Every layer branches on it:
- `chat.rs:338-341, 349, 365-372`: the shape, the cache key and `pending_in` change meaning.
- `chat/render.rs:182-262`: the head, the plan, the live line, the strip and the typing box are each switched on or off.
- `chat/actions.rs:345-353`: after building the chat's actions, it strips settings controls by a hard-coded list of action ids (`ITS`). The buffer patches its own output by string match.
- `chat/nav.rs:281, 292-310`: `show_target` has to reset `alone` in three branches.
- `chat/render.rs:947`: the title switches too.

One buffer does two jobs. Any new chat feature has to remember the flag.

## 3. The shell names which buffer shows a sub-agent.

- `crates/cockpit-ui/src/app/going.rs:160-166`: `land` hard-codes `Target::Subagent => BufferKind::Chat`, shown over the panels. The shell already routes targets by asking buffers (`can_show`, `show_where_shown` at `going.rs:182-186`). The chat already says it can show `Target::Subagent` (`chat/nav.rs:265`). The new case extends the old Discussion special case instead of putting "opens over the panels" on the target or the buffer.
- `chat/actions.rs:409`: "Where it started" names `BufferKind::Chat` itself in `Ask::Show`. Opening a sub-agent (`chat/subagents.rs:192`) leaves the kind to the shell. The two paths for the same feature split the decision differently.

## 4. Two stamps and two clocks for "when it happened".

- The hub now stamps every event (`crates/harness-core/src/hub.rs:146-155`, `Stamped.at_ms`). The adapter stamps sub-agents separately with its own private clock: `crates/harness-acp/src/state/subagents.rs:40-43` (`now_ms`), used at `:211-212` and `:229` for `started_ms` and `ended_ms`. That `now_ms` duplicates `harness_core::now_ms`, which the same change added (`harness-core/src/lib.rs:23`) and which harness-acp can already use.
- The cockpit compares server wall-clock times with its own browser clock. `crates/cockpit-ui/src/agents.rs:215-218` turns `at_ms` into local time by `attention::now_ms() - at_ms`. `chat/subagents.rs:26-29` measures a running sub-agent as browser now minus the server's `started_ms`. Home relays other servers' events with their own stamps (`src/home/links.rs`, `publish_at`). Any clock skew between a server and the browser shifts every age, every "no word for N min", and every sub-agent's time.

## 5. The cockpit store still decides whose an event is.

`crates/cockpit-ui/src/store.rs:515-528`: a turn seen for the first time is "yours", and any `UserMessage` item is "not news". The store infers who caused an event from its shape. Plan item 4 asked whether the server should mark events by who caused them. The UI still decides a fact only the server knows. (If this was left on purpose, it is not visible in the code.)

## 6. The agents service now formats discussions, and the plan tree keeps a copy of that.

- `crates/cockpit-ui/src/agents.rs:348-360`: `Agents::whose(&Discussion)` moved here from `conversation.rs`. The agents service now knows discussion moves, discussion names and their wording. The one shared answer was to be the agent's state; the discussion's wording could combine `state()` with the discussion where the discussion is shown.
- `crates/cockpit-ui/src/tree/plan.rs:31, 152-166`: the plan tree builds a map of every discussion's `whose` words on each `share` and keeps it, to tell if it changed. It is a side copy of derived state, rebuilt as strings every time.

## 7. A message's pieces are assembled by zipping two parallel lists across crates.

- `src/linking.rs:14` returns `(Vec<Link>, Attached)` per thing. `crates/nah-things/src/attach.rs:61` returns a `Vec<String>` of texts, one per `Attached`, by position. `src/modules.rs:319-331` unzips the first, calls the second, and zips the texts back with the links into `Piece`s.
- Nothing ties a text to its links but the index. If `attach` ever skips or merges an entry, every later piece gets the wrong label. The composition root holds domain logic. The plan said the expansion in `nah-things` returns the pieces.

## 8. The host's control layout leaks into the buffer.

`crates/cockpit-ui/src/buffer.rs:34` adds `room_under_waiting` to the shared `Drawing` struct. The host computes it from its floating-button size (`app/draw/host.rs:68-72`). The chat copies it into its own state (`chat/nav.rs:146`, `chat.rs:151`) and pads under each waiting card (`chat/render.rs:249`). Only the chat uses it. The buffer now has to know how hosts place controls, and the generic interface carries one buffer's need.

## 9. The chat reads "sub-agents at work" by two paths.

`crates/cockpit-ui/src/chat/live.rs:38` asks `agents().state(slot)` for `Working { subagents }`. `chat/live.rs:79` asks the thread view (`subagents_working()`). They read the same count. `AgentState` returns `WaitingOnYou` before `Working`, so the two can disagree while a request is pending.

## 10. "What shows an agent at work" has two sources.

The plan put it in the UI config (`[chat] indicator`). The change also keeps a per-desk override in saved UI state: `crates/cockpit-ui/src/saved.rs:62`, `app/keeping.rs:35, 71`, `app.rs:338`, patched over the config every frame in `app/lifecycle.rs:28-33`. Once picked in the action list, the config value is ignored for good. There is no way to go back to "as the config says."

## 11. Smaller ones

- `crates/nah-packs/src/lib.rs:18-28`: `sha256_hex` is public only for the crate's own tests (`tests/packs.rs:40`, `tests/steps.rs:16`). The packs crate now exports a general `hex` helper. `crates/code-analyzer/src/extract.rs:210` keeps its own SHA-256. So the "one helper" lives in an unrelated crate and the second one remains.
- `Resync` and `Replayed` are stamped in three places: `harness-core/src/hub.rs:162`, `src/routes/events.rs:35` and `src/home/socket.rs:89`. The last two each build their own `now` closure.
- `crates/cockpit-ui/src/app.rs:295`: a stale doc comment, "A sub-agent over the panels (its slot's).", now sits on the `alerts` field. The field it described is gone.

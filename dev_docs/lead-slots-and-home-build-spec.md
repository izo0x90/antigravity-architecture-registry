# Build spec: lead and sub-slots, permissions, state, notifications, home

This is the step-by-step plan I follow to build what's described in
`lead-slots-and-home-plan.html`. Every step says what changes, where, what
must not change, and how it's proven. If a step turns out to be wrong
against the code, I stop and ask before improvising.

All paths are relative to `meta-harness/` unless they start with `dev_docs/` or `/`.
Baseline: commit `77182a9` (everything before this plan is committed).

---

## 0. Rules of engagement

1. **Two kinds of step only.**
   - **MOVE:** structure only. Code moves between files and modules, and visibility widens only as far as needed (`pub(super)`, `pub(crate)`). No logic, names, ids, keys, routes, tool names, wire formats or UI change. Proof: the full test suite, clippy `-D warnings`, the WASM build, the surface snapshots (A2) unchanged, and a diff that reads as relocation.
   - **FEATURE:** new behaviour, with its tests in the same step.
   - A step is never both. If a MOVE needs a behaviour change to compile, that change becomes its own FEATURE step first.
2. **No arbitrary changes.**
   - These stay as they are unless a step names the change and why it's needed: action ids, key bindings, route paths, MCP tool names and descriptions, event shapes, config file formats, UI layout and wording, error texts.
   - Anything else I notice goes in "Noticed, not changed" at the end of this file.
3. **No placeholders.** No `todo!()`, no stubs, no "for now" branches, no commented-out code, no flags hiding half-built work. A step ends complete or not at all.
4. **One path per job (clean cuts).**
   - When a step replaces a mechanism, the old one is deleted in the same step, with its tests moved to the new one.
   - No compatibility shims, no duplicate routes, no second way to open a session, check a permission, store state or send an event.
5. **Checkpoints.**
   - Each step ends green:
     - `c.sh test --workspace --no-fail-fast`, with only the known timing-sensitive `arch-service` tests (gates, read_gate, perf) allowed to fail, and only when they're within a few percent of their limit on a loaded machine and the change didn't touch what they measure;
     - `c.sh clippy --workspace --all-targets -- -D warnings`;
     - the WASM build;
     - the surface snapshots, unchanged unless the step names the change.
   - Each phase ends with a live run in the user's Chrome through `window.nah`.
   - Commits only with the user's approval; each step is one proposed commit. The README is updated in the step that changes what it describes.
6. **Reuse before writing.**
   - Ranking: `arch_search::rank` / `rank_text` / `nav::rank_matches`.
   - Events, cache and replay: `EventHub`.
   - Pickers: the action list (`Param::Choice` / `Param::Search`) and the palette.
   - Floating views: the quick look. Request cards: `request_card.rs`. State chips: `status.rs`. LEDs: `ActivityLed`.
   - SQLite: the `rusqlite` version, pragmas and migration style of `arch-store`.
   - Config: `#[serde(default, deny_unknown_fields)]`, as in `harness-acp/src/settings.rs`. User places: `nah_packs::Places`.
   - Tickets: the one-time tickets of `remote.rs`.
   - UI tests: `window.nah` / `from_outside`. Agent tests: `MockAgent` and `FakeDriver`.
7. **Performance is part of correctness.**
   - No per-frame allocation-heavy walks, no polling where an event exists, no unbounded queues, no buffering where streaming works.
   - Hot checks are O(1): permission checks, tool lookup, notification derivation per event.
8. **Consistency rules (§S) apply to every step that stores or relays state.**

---

## §S. State and consistency: the rules every phase follows

1. **Each fact has one owner.**
   - **The state database owns intent:** which slots exist, their settings and saved-session ids, notifications and their state, held reports, queue runs, the activity log, the cockpit's saved views.
   - **The live harness owns what's happening now:** whether a session runs, turn status, pending approvals.
   - Derived states (running, idle, waiting, failed) are computed from live state and **never stored**.
2. **Write order: database, then memory, then events.**
   - One transaction per operation.
   - If the write fails, the operation fails with the error and memory and events are untouched.
   - Nothing is "saved later".
   - When an operation has an outside side effect (opening an agent session):
     1. do the side effect first;
     2. record it in one transaction;
     3. if the record fails, undo the side effect (close the session) and return the error.
3. **Restart cleanup happens in one function, `reconcile_on_start`, run once before the server takes requests:**
   - open Waiting notifications from the previous run are closed as Expired;
   - queue runs that were in progress are marked Interrupted;
   - worker slots are removed;
   - every lead and sub-slot session is "not running" (that's derived, so nothing is written).
   - It logs one line with the counts.
4. **When reality disagrees, say so loudly, with a fix; never patch it silently.**
   - A slot whose harness is gone stays, shown as Unavailable with the reason and how to fix it.
   - A saved session the harness can't resume opens fresh, with a SessionCrashed update saying exactly that.
5. **States are enums with checked transitions.** Each entity has a `transition(&self, to) -> Result<Self, StateError>`. An impossible change (a Done notification reopening, a closed slot being sent to) is an error with the entity id and both states. It's returned and logged at `warn`, never ignored and never "fixed up".
6. **Repeats are harmless.** Rows are keyed by stable ids (request, turn, notification, slot), and writes are upserts on those keys. A replayed or duplicated event changes nothing the second time.
7. **Falling behind is answered with a full resync, never by skipping.**
   - `EventHub` uses a bounded broadcast. A subscriber that lags gets `Lagged`, and the answer is that subscriber's full resync: send the replay again (cockpit), or rebuild from the replay (notification hub, home's links).
   - Today `stream_events` silently `continue`s on `Lagged` (`server.rs`), dropping events. This is fixed in C5.
8. **One server per project.** At start the server takes an OS lock on `.nah/state/lock` (std `File::try_lock`, Rust ≥ 1.89; this toolchain is 1.90). A second server refuses with "another server is running for this project (pid N)", the pid read from the lock file. The lock dies with the process, so it can't go stale.
9. **Explicit schema versions.**
   - The state database uses `PRAGMA user_version` and ordered migrations in code.
   - A database newer than the code refuses to start ("made by a newer NAH; update"). A damaged one refuses with its path and the fix: move it aside to start empty.
   - Unlike the analysis cache, the state database is never deleted automatically.
10. **Clients hold nothing that matters.** The cockpit's stores are caches that a resync rebuilds. Truth is on the server.

---

## Phase A: Baseline and characterization

### A1. Done
The baseline is committed (`626df30`, `77182a9`, `76e5907`).

### A2. Pin today's surface (FEATURE: tests only)
- **`tests/surface_snapshot.rs`:**
  - **(a) MCP `tools/list`:** names in order, and a hash of each description and schema, against a pinned list.
  - **(b) HTTP routes:** for each pinned `(method, path)` from the survey's route table (path parameters filled with sample values), a request with a method the route doesn't register answers 405. A path that doesn't exist answers 404, so 405 proves the path exists. The registered method answers neither 404 nor 405.
  - **(c) cockpit:** a golden of `(action id, key hint)` from `bound_actions()` for `full_app()` with each view focused.
- **`request_card.rs`:** a test that clicking Yes sends the response, pinning today's click path before E3 changes it.
- **`harness-acp/tests/acp_adapter.rs`:** a direct `[permission]` test (request → respond → the agent sees `allow-once`). Slot and notification tests build on it.
- Pinned lists change **only** in steps that intend it, and the step says so.

---

## Phase B: Pay down structure (MOVE only)

### B1. `cockpit-ui/src/app.rs` (4782 lines) → `app/`
| New file | Takes (from app.rs, line ranges from the survey) |
|---|---|
| `app/mod.rs` | imports, `BufferKind`, `ShellCommand`, `IndexOp`, `Hinting`/`Searching`/`Capture`, consts, the struct, `new`, `impl eframe::App`, `frame`, `draw_frame` |
| `app/lifecycle.rs` | `update_lifecycle`, `apply_user_config`, `fetch_index_status`, `mark_out_of_date`, `run_index`, `show_code` |
| `app/layout.rs` | columns, panes and parking (540-765), `focused_kind`, `slot_of`, `ensure_visible`, `focus_kind` |
| `app/keeping.rs` | saved state (767-839) |
| `app/look.rs` | quick look (841-907), `render_quick_look`, `render_talk` |
| `app/views.rs` | view dispatch and linking (992-1265) |
| `app/commands.rs` | actions and shell commands (1267-1680) |
| `app/capture.rs` | hints, search, marks (1682-1846), `render_hints`, `hint_chip` |
| `app/going.rs` | go-tos and details (1848-2106) |
| `app/keys.rs` | `text_input_focused`, `release_keyboard`, `leader_key`, `handle_keys` |
| `app/draw/top_bar.rs` | `ActivityLed`, `render_top_bar`, `render_index_light`, `index_state`, `server_line` |
| `app/draw/bars.rs` | key bar, prefix popup, key sheet, `keys_of`, `describe_jump` |
| `app/draw/which_key.rs` | `group_label`, `leader_title`, `render_which_key_hud` |
| `app/draw/slots.rs` | `tab_keys`, `render_slot`, `render_palette` |
| `app/outside.rs` | `serve_calls`, `actions_listed`, `run_named`, `remote_status` |
| `app/tests/*.rs` | tests split by topic; helpers in `app/tests/support.rs` |

- Methods used across files become `pub(super)`. The tests' private uses (survey §0) are covered by `pub(super)`, because `tests` is a child of `app`.
- `theme.rs` `glyph_tests::SOURCES` lists the new files, with the array length updated. The colour/size lint now also scans the moved code (it's clean per the survey).
- **Verified cleanups,** allowed in this MOVE because they're dead or misplaced:
  - remove `reveal_inbox`, which is drained but never filled;
  - move the stray doc comment at 2867 to `render_hints`.
- `app/tests/split.rs` asserts no file in `app/` exceeds 900 lines, modelled on `the_graph_is_split_as_agreed`.

### B2. `cockpit-ui/src/chat.rs` (1508 lines) → `chat/`
`mod.rs` (types, struct, `process_event`, getters), `session.rs` (open, send, steer, tick, drain, saved sessions, tools), `render.rs`, `nav.rs` (`Navigable`), `actions.rs` (`ChatCommand`, `ActionProvider`), `tests.rs`.

### B3. `src/server.rs` → `src/routes/`
- New route files: `harness.rs`, `ui.rs`, `files.rs`, `feedback.rs`, `tool_servers.rs`, `events.rs`.
- `arch.rs`, `graph.rs` and `queue.rs` move there unchanged.
- `server.rs` keeps `AppState`, the error types, `create_router`, `bridge_architecture_events`, `queue_runner`, `static_dir`.

### B4. MCP service → tool sets
- **`trait ToolSet`:**
  - `fn tools(&self, trees: &TreeConfig) -> Vec<McpTool>`;
  - `async fn call(&self, name: &str, args: Value) -> Result<String, ServiceError>`.
- **Sets in `src/mcp/sets/`:** `explore`, `plans`, `build`, `review`, `cockpit`. Each holds its tools' schemas (from `tools.rs`) and handlers (from `service.rs`).
- **Shared helpers** go to `src/mcp/text.rs`.
- **`McpService::new`** builds a `HashMap<&'static str, usize>` from tool name to set, once, and **errors at construction on a duplicate name**. Dispatch is a map lookup, not a scan.
- The list order reproduces today's exactly, which snapshot (a) proves.
- `tools.rs` and the `match` are deleted.

### B5. `src/main.rs` → `src/startup.rs`
`listen`, `open_project`, `harnesses`, `tool_servers`, `queue`, `serve`. Env vars, order and log lines are unchanged.

### B6. Shared SQLite setup (MOVE)
`arch-store`'s connection set-up (WAL, `synchronous`, cache size, busy handling: `sqlite.rs:140-180`) becomes `pub fn configure(conn, &Settings)` in `arch-store::sqlite`, so the state database uses exactly the same set-up. Nothing else moves.

---

## Phase C: Permissions (FEATURE)

### C1. New crate `crates/nah-access`
Dependencies: serde and toml. It has no notion of slots: identity stays with the caller, and access only knows roles.

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Capability { ViewRead, ViewDrive, PlanEdit, CodeCheck, SlotsObserve, SlotsManage,
                      SlotsMessage, SlotsReport, UserNotify, RequestsAnswer, PlansApprove, SessionsControl }
impl Capability { pub const ALL: [Self; 12]; pub fn name(self) -> &'static str; pub fn from_name(s: &str) -> Option<Self>; }
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)] pub struct CapabilitySet(u16);
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Principal { User, Home, Agent { role: Arc<str> }, Anonymous }
pub struct Access { roles: HashMap<Arc<str>, CapabilitySet> }
impl Access {
    pub fn defaults() -> Self;                                          // config/access.toml (include_str!)
    pub fn with_overrides(self, text: &str) -> Result<Self, String>;    // .nah/access.toml
    pub fn allows(&self, who: &Principal, needs: Capability) -> bool;   // User/Home: all; Anonymous: none
    pub fn role(&self, name: &str) -> Option<CapabilitySet>;
}
```

- **The user isn't configurable.** User and Home have every capability, because home relays the user. Anonymous has none.
- **Agent roles live in config:**
  - `[roles.<name>] can = [...]`, with `deny_unknown_fields`;
  - unknown capabilities are an error listing the known ones;
  - an override replaces the whole role;
  - `lead`, `sub`, `worker` and `external` must exist.

| Role | Capabilities |
|---|---|
| `lead` | view.read, view.drive, plan.edit, code.check, slots.observe, slots.manage, slots.message, user.notify |
| `sub` | view.read, slots.report, user.notify |
| `worker` | view.read, plan.edit, code.check, user.notify |
| `external` | view.read |

- **Scope stated plainly.** These capabilities govern **NAH's tools**. A slot's own harness tools (its file edits, its shell) are governed by that slot's runtime mode and the approvals you answer, as today. A sub-slot can't edit plans or drive the IDE, but its harness can still propose file changes for you to approve.
- **README:** `.nah/access.toml` goes in the configuration table, with the scope note.
- **Tests:** defaults; overrides; error texts; User, Home and Anonymous; each role's exact set.

### C2. Each tool declares its capability
- `ToolSet::tools` returns `Vec<ToolDef { tool, needs: Capability }>`.
- **Mapping:**

| Capability | Tools |
|---|---|
| view.read | search, inspect, view, list_types, plan_list, contract, review_comments, ingest, cockpit_read, cockpit_actions |
| view.drive | navigate, cockpit_run, cockpit_click, cockpit_type, cockpit_keys |
| plan.edit | plan_create, plan_import, plan_edit, answer_comment, reply_comment |
| code.check | plan_validate (it runs logic-step commands), verify, run_tools, review |

- The filtered `tools/list` is cached per role. Roles are fixed for the process, so the cache is never invalidated.
- **`tools/call`** checks the capability and refuses with a tool error: `this session's role (sub) can't use navigate (it needs view.drive)`. Refusals are written to the activity log (D0).
- **`agent()`** → `Actor::Agent { session: <slot name, or "external"> }`.

### C3. Who's calling on MCP
- **`McpCaller { principal: Principal, slot: Option<SlotId>, label: String }`** is resolved from `?key=` on `GET /api/mcp/sse`, and on the POST endpoints for HTTP-mode agents (`McpMessagesQuery.key`). Resolution goes through a `CallerResolver` trait (the slot manager implements it).
- **`SseSessionManager`** maps `session_id → (sender, McpCaller)`. `handle_request` and the sets receive `&McpCaller`.
- **Key validity:**
  - A key is valid while the session it was made for is open.
  - Unknown, missing or closed-session key → `Agent { role: "external" }`.
- **Secret hygiene:**
  - **Broadcast copy.** `McpServer::redacted()` (contracts) removes the URL query and replaces header values with `…`. `harness-acp/src/adapter.rs:316` stores the redacted copy in `Thread`. Test: no key in `ThreadUpdated`.
  - **Request logs.** `TraceLayer` spans use a `make_span_with` that records the path only, never the query, so keys and tickets never reach the logs. Test: a request with `?key=` logs no key.
- C3 lands with D2, which issues the keys. Until then every harness session would be `external`, so they're tested and shipped together.

### C4. Each route declares its capability
- **`Needs<C>` replaces `CockpitUser`.** Marker types come from a macro over `Capability::ALL`. The principal is taken from the request:
  - the cockpit cookie → User;
  - `Authorization: Bearer <server secret>` → Home (F3);
  - a valid socket ticket → User (sockets only, §C5);
  - otherwise Anonymous.
- **Routes:**
  - Mutating routes take `Needs<SessionsControl | PlansApprove | RequestsAnswer>` as fits. These are user-only, because agents use MCP.
  - Read routes take `Needs<ViewRead>`.
  - Open: `/health`, `/cockpit`, static files, `/api/mcp/*` (key), and socket upgrades (ticket).
  - The 403 text stays `only the user, in the cockpit, can do this`.
- **Tests:**
  - a source-scan test: every handler registered with post, put or delete under `src/routes/` takes `Needs<`;
  - the surface snapshot asserts 403 for every non-open route without credentials;
  - e2e tests move from `http` to `http_cockpit`. That's test-only and listed in the commit.
- **This closes the deferred "cockpit login on every route" item.**

### C5. Sockets: tickets, and resync instead of skipping
- **Why tickets.** The user's sandbox proxy drops cookies on socket upgrades (found with `/ws/cockpit`), so every socket uses a one-time ticket from a cookie-checked request.
- **Shared tickets.** The tickets in `remote.rs` move to a shared `cockpit::Tickets`, and `POST /api/cockpit/ticket` serves them. `/ws/events`, `/ws/cockpit` and later `/ws/home` require `?ticket=`. Home's links use the Bearer header instead.
- **Cockpit side.** `NetClient::connect_ws` fetches a ticket before every connect and reconnect, reusing `Remote::keep_socket`'s pattern. That pattern moves into `NetClient`, and `remote.rs` uses it.
- **`stream_events` on `Lagged`** sends `{ "resync": true }`, then the full replay, instead of skipping. The cockpit's `Store` clears itself on `resync`.
- **Tests:**
  - no ticket gives 403; a ticket works once;
  - a forced lag (a tiny broadcast capacity in the test) produces a resync, and the client ends with the right state.

---

## Phase D: State, then slots (FEATURE)

### D0. New crate `crates/nah-state`: the state database
- **Files:**
  - `.nah/state/state.sqlite`;
  - `.nah/state/lock` (§S8);
  - `state` is added to `NAH_IGNORED`.
- **Opening:**
  - `StateDb::open(dir) -> Result<StateDb, StateError>` takes the lock, then opens the database with `arch_store::sqlite::configure` (B6).
  - Migrations are ordered `(version, sql)` pairs. Each one runs in a transaction that sets `user_version` when it commits.
  - A newer database or a damaged one is refused, with messages per §S9.
- **Access:**
  - One `Mutex<rusqlite::Connection>`, used from async code only through `spawn_blocking`. Operations are single short transactions, so one connection is enough; it's simple to reason about and avoids writer contention.
  - Nothing outside `nah-state` touches SQL.
- **Ports and adapters.** Each feature defines its own store trait in its own crate, and `nah-state` implements them:
  - `harness_core::slots::SlotStore`;
  - `nah_notify::NotificationStore`;
  - `QueueRunStore` in the server crate's queue module;
  - `ActivityLog`;
  - `UiStateStore`;
  - `FeedbackStore`.
  - Feature crates test against an in-memory fake of their own trait. `nah-state` tests the real SQL.
- **Tables** (in migration 1):
  - `slots(id PK, role, name, instance, policy_json, model_json, status, created_at)`;
  - `slot_sessions(key PK, slot FK, native_id, title, current BOOL, opened_at)`;
  - `held_reports(id PK, from_slot, text, at)`;
  - `notifications(id PK, server, level, kind, slot, thread, request, title, detail, at, state, closed_at)`;
  - `queue_runs(plan, component, attempt, kind, state, thread, outcome_json, started_at, ended_at, PK(plan, component, attempt))`;
  - `activity(seq INTEGER PK, at, actor, action, subject, detail)`;
  - `ui_state(id PK = 1, json)`;
  - `feedback(id PK, json, at)`.
- **Moves into it** (clean cuts in this step):
  - **`UiStateFile`** (`.nah/ui/state.json`) → `UiStateStore`. The `/api/ui/state` routes are unchanged. The old file is no longer read; the cockpit opens fresh once, which D5's saved-state version bump causes anyway. `ui` leaves `NAH_IGNORED`.
  - **Dev feedback** (`./dev_feedback.json` in whatever folder the server started from) → `FeedbackStore`. The `/api/dev/feedback` routes are unchanged.
- **Tests:**
  - migrations from empty;
  - a newer version is refused;
  - a damaged file is refused with its path;
  - the second-server lock is refused with the pid;
  - every store trait round-trips;
  - a failure injected inside a transaction leaves no partial rows.

### D1. Contracts
```rust
id_type!(SlotId); id_type!(SessionKey);
pub struct Slot {
    pub id: SlotId, pub role: String, pub name: String,
    pub instance_id: InstanceId,                 // the slot's harness
    pub policy: RuntimePolicy, pub model: Option<ModelSelection>,
    pub sessions: Vec<SlotSession>, pub current: Option<SessionKey>,
    pub started_by: Starter,                     // User | Lead | Queue
    pub status: SlotStatus,                      // Ready | Unavailable { reason, fix }
}
pub struct SlotSession { pub key: SessionKey, pub native_id: Option<NativeId>, pub title: Option<String>,
                         pub thread: Option<ThreadId> /* live; None = not running */ }
```
- Add `ClientEvent::SlotUpdated { slot }` and `ClientEvent::SlotClosed { slot_id }`.
- `EventHub` caches `Key::Slot(id)`; `SlotClosed` evicts it.
- Wire-format tests.

### D2. `harness-core/src/slots.rs`: the slot manager
```rust
pub struct SlotManager {
    sessions: Arc<SessionManager>, hub: Arc<EventHub>, store: Arc<dyn SlotStore>,
    inner: Mutex<Slots>,          // one lock: slots, keys (key → (slot, session)), lead id; never held across .await
    ours: OurServer,              // per-session MCP entry: "{base}/api/mcp/sse?key={key}"
    project_servers: Arc<dyn Fn() -> Result<Vec<McpServer>, String> + Send + Sync>,
}
pub enum Delivery { Queue, AddToRunning }
impl SlotManager {
    pub async fn load(...) -> Result<Self>;                         // from the store; creates the lead slot on first run
    pub fn lead(&self) -> SlotId;
    pub fn list(&self) -> Vec<Slot>;
    pub async fn create(&self, role: &str, name: Option<String>, instance: InstanceId, settings: SlotSettings, by: Starter) -> Result<SlotId>;
    pub async fn set_harness(&self, slot: &SlotId, instance: InstanceId) -> Result<()>;   // only with no session running
    pub async fn open_session(&self, slot: &SlotId, resume: Option<SessionKey>) -> Result<ThreadId>;
    pub async fn switch(&self, slot: &SlotId, session: &SessionKey) -> Result<()>;
    pub async fn close_session(&self, slot: &SlotId, session: &SessionKey) -> Result<()>;
    pub async fn close(&self, slot: &SlotId) -> Result<()>;                               // never the lead
    pub async fn send(&self, to: &SlotId, text: String, delivery: Delivery, from: Origin, wait: Option<Duration>) -> Result<Sent>;
    pub async fn report(&self, from: &SlotId, text: String) -> Result<Reported>;          // Delivered | Held
    pub fn read(&self, slot: &SlotId, last: usize) -> Result<SlotReport>;
}
impl CallerResolver for SlotManager { ... }
```

- **Sessions in a slot work exactly as the chat's do today:**
  - several can be open, one is current;
  - new, resume, switch and close;
  - swapping the lead's session never touches other slots.
- **One harness per slot.** It's chosen when the slot is created. The lead's harness can be changed only while it has no session running, which is today's rule for "Choose harness". Otherwise the error says "close its sessions first".
- **Opening a session, step by step** (§S2):
  1. Under the lock, check the slot exists and is Ready, and reserve its key.
  2. Release the lock. Open the harness session (new, or resume from `native_id`). Its MCP list is `[ours with this key] + project_servers()`, the same for every slot, so queue jobs now get the project's tool servers too.
  3. In one transaction, record the session and make it current.
  4. Under the lock, apply it to memory.
  5. Publish `SlotUpdated`.

  If step 3 fails, the new session is closed and the error returned. If the slot was closed while step 2 ran, the new session is closed and the result is `NotFound`.
- **Resume falls back loudly.** If the harness can't resume (it doesn't support it, or the session is gone), a fresh session opens, and a SessionCrashed update says so with the harness's reason.
- **Slot status.** On load, a slot whose instance isn't configured or isn't Ready is `Unavailable { reason, fix }`, taken from the instance snapshot. It flips back to Ready when the instance does: the manager subscribes to `InstanceUpdated`. Calls on an Unavailable slot fail with that reason.
- **`send` (lead → sub, or user → slot):**
  - It subscribes to the session's events **before** sending, so the end of a fast turn can't be missed.
  - The prompt starts with `From the lead (<name>):` and a blank line when the lead sends.
  - With `wait`, it returns the turn's final assistant text, from a pure function over `TurnItem`s with its own tests, or `StillWorking { turn }` at the deadline.
  - A slot that isn't running is resumed first (on-first-use, below).
- **`report` (sub → lead):**
  - It's **queued** into the lead's current session, starting `From <sub name> (sub-slot):`. It is **not added to a running turn**: with Claude, that cuts off the lead's current work. The lead reads it when its turn ends.
  - Every report is also a Message update for the user.
  - With no lead session running, the report is **held** (`held_reports`). The next time a lead session opens, the held reports go in as one queued prompt, and the cockpit's note says how many were delivered. The sub-slot is told the report was held.
- **On first use after a restart** (opening its chat, a message from the user, `send` from the lead), a slot resumes its current session from `native_id`, with a new key. Agents aren't started for slots nobody uses.
- **`read`** builds the report from the hub's cached items for that slot's current thread. It's O(cached entries), acceptable for an occasional tool call, and documented as such.
- **Activity log:** create, close, open, switch, send, report, and refused tool calls (C2).
- **The work queue.** `SessionRunner` asks for a worker slot per job (`role "worker"`, `name "queue: <component>"`, `Starter::Queue`), runs the brief, and closes the slot. Job attempts and outcomes go to `queue_runs`. This deletes `SessionRunner`'s own `sessions.open`.
- **Tests,** with `FakeDriver` and `MockAgent` through a new `tests/common` helper that registers an ACP instance over `MockAgent`:
  - lifecycle: the lead is untouched by sub changes;
  - keys: one per session, and invalid after it closes;
  - the session's MCP URL carries the key, and the broadcast `Thread` doesn't;
  - `send` with wait;
  - `send` to a slot that isn't running resumes it;
  - a report while the lead runs is queued, and while it's down is held, then delivered;
  - restart restores slots without starting agents;
  - a resume failure opens fresh with an update;
  - a missing harness makes the slot Unavailable, and it recovers;
  - a store failure during open closes the session and returns the error;
  - a close racing an open;
  - the lead can't be closed;
  - queue jobs appear as worker slots and are gone after the job.

### D3. Routes (`Needs<SessionsControl>`)
- `GET /api/slots`
- `POST /api/slots` with body `{ role, name?, instanceId, policy, model? }`
- `DELETE /api/slots/{slot}`
- `POST /api/slots/{slot}/harness` with body `{ instanceId }`
- `POST /api/slots/{slot}/sessions` with body `{ resume?: SessionKey }`, returning `{ threadId }`
- `POST /api/slots/{slot}/current` with body `{ session }`
- `DELETE /api/slots/{slot}/sessions/{session}`

`POST /api/threads` is deleted. The thread routes (turns, inject, steer, interrupt, requests) stay, and `DELETE /api/threads/{t}` is replaced by the slot route above.

### D4. MCP set `slots`
| Tool | Needs | Arguments |
|---|---|---|
| `slot_list` | slots.observe | — |
| `slot_read` | slots.observe | `slot`, `last?` |
| `slot_start` | slots.manage | `harness`, `name?`, `model?`, `mode?` |
| `slot_close` | slots.manage | `slot` |
| `slot_send` | slots.message | `slot`, `text`, `add_to_running?`, `wait_secs?` |
| `tell_lead` | slots.report | `text` |

`tell_user` (user.notify) is added in E2. Results end with next steps, like the other tools.

### D5. Cockpit: a chat per slot
1. **(MOVE) Split chat state.**
   - **`Sessions`**, one per project: store, slots, harness choice for new slots, opening and error, saved sessions, tools, login, drafts, mailbox, defaults.
   - **`ChatView`**, one per chat view: its slot, input, cursor, scroll, link pick, markdown cache, spots, drawing, and view.
   - `ActionProvider::actions(&self)` takes no context, and every provider is a field the app hands out by reference. So `Sessions` is shared as `Rc<RefCell<Sessions>>`, with every access through two short-scoped helpers that never hold a borrow across a call. A test runs every chat action with two chat views open, to show no double borrow happens.
   - With one view on the lead, every existing test passes unchanged.
2. **(FEATURE) Chat views per slot.**
   - Chat stops being one-only. `Parked` gains `chats`, and every one-only spot from the survey is updated in this step.
   - egui ids are salted with the pane id.
   - **Actions:**
     - `slot.pick` (Choice, with a `status.rs` chip and ranking);
     - `slot.new`;
     - `slot.close`;
     - `slot.harness` for the lead, only while no session runs;
     - `buffer.new.chat`;
     - the existing `session.*` actions act on the view's slot.
   - **Keys:** `w = "Slots"`: `w w` pick, `w n` new, `w c` close.
   - **Requests:** `y`/`Y`/`A`/`d` act on the focused chat view's slot.
   - **Saved state:** `Place::Chat` gains `slot`. The thread and draft move into the view's place, and `saved::VERSION` goes to 3.
   - **Queue:** the "open the job's session" link shows the worker slot in a chat view.
   - **Top bar:** the AGENT LED shows the lead.
   - **Talk** goes to the lead.
   - **Held reports** delivered on lead open show in the note.
   - **Tests:** through `from_outside`: two views on two slots; request keys; saved state per view; the snapshot gains the new action ids.

---

## Phase E: Notifications (FEATURE)

### E1. Contracts
```rust
pub struct Notification { pub id: NotificationId, pub server: Option<String>, pub level: Level /* Waiting | Update */,
    pub kind: NotificationKind, pub slot: Option<SlotId>, pub thread: Option<ThreadId>, pub request: Option<RequestId>,
    pub title: String, pub detail: Option<String>, pub at_ms: u64, pub state: NotificationState /* Open | Done | Expired | Dismissed */ }
```
- **Kinds:**
  - Approval, Question and Link are Waiting.
  - SignIn is Waiting.
  - SessionCrashed, TurnFailed, TurnFinished, Message and ServerLost are Updates.
- **Ids are stable per subject:** `req:<request>`, `turn:<turn>`, `crash:<thread>`, `signin:<instance>`, `msg:<uuid>`, `lost:<server>`.
- **Transitions,** checked (§S5):
  - Open → Done, Expired or Dismissed;
  - nothing leaves Done, Expired or Dismissed;
  - SignIn and ServerLost can recur, as a new id with a counter suffix.
- Add `ClientEvent::NotificationUpdated { notification }`. The hub caches open ones only.
- **Order,** in one shared `fn order`:
  1. Waiting;
  2. Updates, SessionCrashed and TurnFailed first;
  3. then oldest first.

### E2. New crate `crates/nah-notify`
- **`rules.rs`** is pure: `derive(event, world, open) -> Vec<Change>`.

| Event | Notification | Ends |
|---|---|---|
| Request Pending | Waiting: Approval (Permission, Command, FileRead, FileChange), Question (UserInput), Link (McpElicitation) | Done when the request is no longer Pending |
| A harness a slot uses is signed out or needs sign-in | Waiting: SignIn | Done when signed in |
| A slot's thread errors, or closes when nobody asked | Update: SessionCrashed | Seen or dismissed |
| Turn Failed | Update: TurnFailed | Seen or dismissed |
| Turn Completed (any slot, anyone's) | Update: TurnFinished. For the lead with nothing queued, the title says the lead is idle. | Seen or dismissed |
| A report or `tell_user` | Update: Message | Seen or dismissed |

- **No separate "lead idle" kind.** It would duplicate the lead's TurnFinished.
- **`NotifyHub`:**
  - Holds the open set in memory, loaded from `NotificationStore` at start.
  - It's fed by an `EventHub` subscription. On `Lagged` it rebuilds from the hub's replay and the store (§S7): pending requests in the replay must have Open notifications, and Open Waiting items with no pending request become Done.
  - Writes follow §S2.
  - It publishes `NotificationUpdated` and hands each change to its adapters.
- **Restart:** in `reconcile_on_start`, Waiting notifications from the previous run → Expired.
- **`Adapter`:** `pub trait Adapter: Send + Sync { fn deliver(&self, n: &Notification); }`. It's non-blocking: an adapter keeps its own bounded queue, and on overflow drops the oldest and logs the count.
  - The cockpit doesn't need an adapter, because it reads the event stream.
  - The trait is the extension point for Slack, email and a phone app. A test adapter proves delivery.
- **Answers keep the existing respond route,** so there's one answer path. The rule closes the notification when the request resolves.
- **Routes** (`Needs<SessionsControl>`):
  - `GET /api/notifications?state=open|recent`
  - `POST /api/notifications/seen` with body `{ slot }`
  - `POST /api/notifications/{id}/dismiss`
- **MCP:** `tell_user` (user.notify) goes in the `slots` set.
- **Tests:**
  - the rules table;
  - with `MockAgent`: permission → Waiting → answered → Done → the agent continues;
  - question → Waiting → answered;
  - a lead-started sub turn → TurnFinished;
  - a report → Message, and it reached the lead;
  - a dropped connection → SessionCrashed;
  - a lag rebuild leaves the set correct;
  - a restart expires Waiting items;
  - a late client gets the open items in the replay;
  - an impossible transition errors.

### E3. Cockpit: attention
- **`attention.rs`** (shell level): notifications by id, with running counts.
- **Top bar,** after the INDEX light: a **waiting** counter (warn colour, hidden at zero) and an **updates** counter (muted). Clicking one opens its picker. Both are registered as inspectable.
- **Pickers:**
  - `attention.waiting` and `attention.updates` (Choice, ranked);
  - each item's detail: `slot · kind · age`;
  - picking opens the item in the quick look;
  - an item resolved meanwhile opens as "already handled".
- **The quick look gains a second kind of content:** `Look::Card { .. } | Look::Attention { id }`. The card look is unchanged.
  - The attention look shows the slot and harness, the request card or the message, and the turn's last assistant message.
  - Its actions win key conflicts while it's open:
    - `attention.next` (`n`) and `attention.prev` (`N`);
    - `request.*` (`y`/`Y`/`A`/`d`) for the shown request;
    - `attention.open_slot`;
    - `attention.dismiss`.
  - Esc closes it.
- **`attention.next_waiting`** (`SPC n`) opens the oldest waiting item, or says nothing is waiting.
- **Request-card clicks** go through the chat mailbox like the keyboard path, so failures show in the note. This fixes `request_card.rs:118-120`.
- **Seen:** a drawn chat view on slot X sends `seen { slot: X }` when there are unseen updates for X, debounced.
- **Browser notifications:** the `attention.browser` toggle (saved) uses the web Notification API for new Waiting items while `document.hidden`. A click focuses the tab and opens the item. It needs the `web-sys` features for Notification and visibility.
- **Tests** (`from_outside`): counters; picker; next and previous inside the look; answering in the look; seen.

---

## Phase F: Server roles

### F1. (MOVE) Module states
- **States:** `ProjectState`, `HarnessState`, `CockpitState`, `NotifyState`, `AccessState`, each extracted with `FromRef`.
- **A `Module` trait** has `fn routes(&self) -> Router` (already given its state) and `fn start(&self)`. `startup.rs` composes the modules. Behaviour is identical.

### F2. (FEATURE) Flags
- **`serve` options:** `--no-cockpit`, `--no-project`, `--listen <addr:port>`, `--listen-public`. `HOST` and `PORT` still work.
- **The default bind becomes `127.0.0.1`** (decided).
- **Without `--listen-public`, a bind is allowed only on:**
  - loopback;
  - private IPv4 (10/8, 172.16/12, 192.168/16);
  - **100.64/10 (Tailscale's range)**;
  - IPv6 ULA (fc00::/7) and link-local.
- **Everything else is refused, including `0.0.0.0` and `::`** (they bind every interface). The error names the address and the flag.
- **A no-project server** runs only the cockpit, home and access modules. Its state database lives in `{data_home}/nah/home/`.
- **`meta-harness token`** prints this server's secret, for adding it to a home.
- **Tests:**
  - the bind rules as a table;
  - a no-project start has no arch, LSP or watch;
  - `--no-cockpit`.

### F3. (FEATURE) Home's credential
- `Authorization: Bearer <server secret>` → Principal::Home, through `Needs<C>`.
- The secret is never accepted in a URL, except the existing launch link.

---

## Phase G: Home (FEATURE)

### G1. Dependencies
`hyper-util` (client, http1, tokio; already in the tree) for streaming pass-through. `tokio-tungstenite` becomes a normal dependency for links and socket pass-through.

### G2. Server list, in home's state database
- **Table:** `servers(name PK, url, secret, added_at)`. The database file is owner-only.
- **Names** use the `PlanId` slug rule. `here` is reserved.
- **Routes:** `GET /api/home/servers` (without secrets), `POST`, `DELETE /api/home/servers/{name}`.
- **CLI:** `meta-harness servers list|add|remove`.

### G3. Links
- **One task per server.** It connects to `/ws/events` with the Bearer secret and keeps its own `EventHub` per server, reusing the cache and replay.
- **On every (re)connect,** the link clears that server's hub, applies the replay, and tells clients `{ "resync": server }`, so nothing stale survives (§S7).
- **Backoff:** 0.5 s doubling to 10 s.
- **State:** Connecting, Connected or Lost. Lost raises a `ServerLost` notification (home's own); reconnecting closes it.
- **Disconnect** stops the task and keeps the entry.

### G4. `/ws/home` (ticket)
- For every server, including `here` when home has a project, it sends that hub's replay and then live events, as `{ server, event }`.
- Each server's broadcast is subscribed separately. `Lagged` on one triggers `{ resync: server }` and its replay; the others continue (§S7).
- Notifications from workers arrive as their own events. Home sets `server` and hands them to its adapters, so the rules never run twice.

### G5. Pass-through `/s/{server}/{*rest}` (`Needs<ViewRead>` by cookie or ticket)
- Streams requests and responses without buffering.
- Adds the Bearer secret, and removes `Cookie` and hop-by-hop headers.
- `/s/{server}/ws/*` bridges sockets both ways, including the per-server cockpit-control socket (H2).
- The worker then checks the real capability against Home.
- **Tests** (two servers and a home in one process):
  - a request and a socket go through;
  - the cookie isn't forwarded;
  - an unknown server gives 404;
  - a large body streams;
  - the worker dropping mid-stream errors cleanly.

### G6. Disconnect, close, shut down
| Action | How |
|---|---|
| Disconnect / connect | `POST /api/home/servers/{name}/disconnect` and `/connect` |
| Close a session or slot | The existing routes, through the pass-through |
| Shut down a server | `POST /api/server/shutdown` on every server (`Needs<SessionsControl>`), triggering the existing graceful shutdown. The cockpit confirms through a Choice. |

Tests: disconnect leaves the worker running; shutdown stops it, home shows Lost, and the entry stays until removed.

---

## Phase H: Many servers in the cockpit

### H1. (MOVE) `Desk`
- The per-project fields (the survey's **P**) move into `Desk` (`app/desk.rs`), and the app holds one desk.
- Methods that only touch desk state move to `impl Desk`.
- No visible change; `tests/headless.rs` is unchanged.

### H2. (FEATURE) Desks per server
- **The app holds `desks` and `current`.**
  - Every desk processes its events and replies each frame (cheap upserts).
  - Only the current desk takes input, draws, and fetches view data.
  - A background desk fetches when it's shown.
- **`NetClient` gets a path prefix:** `""` for `here`, `/s/<name>` for the others.
- **One `/ws/home` socket** feeds every desk. `resync` resets that desk's store.
- **Saved state:**
  - each desk saves through its own server's `/api/ui/state`;
  - shell state (current server, attention settings) is saved on home.
- **Cockpit control per server:**
  - each desk keeps its own control socket through `/s/<name>/ws/cockpit`;
  - a call from server B's agents acts on desk B;
  - a `read` of a desk that isn't on screen returns its columns and views, and says the screen shows another server;
  - `navigate` from B moves desk B's graph without switching.
- **Switcher:**
  - a top-bar chip with a dot when something is waiting there;
  - `server.switch` (ranked Choice, with waiting counts);
  - `server.add` (Text: `name url secret`);
  - `server.connect` / `server.disconnect`;
  - `server.remove`;
  - `server.shutdown` (confirmation Choice);
  - keys `S = "Servers"`.
- **Attention across servers:**
  - counters total every desk;
  - pickers show `server · slot · kind`;
  - `next` switches desk before opening the look.
- **Tests:** two desks fed events (switching keeps state, counters total, next crosses desks); a live run with home plus one worker.

---

## Order and checkpoints

| # | Steps | Result |
|---|---|---|
| 1 | A2 | Today's surface pinned |
| 2 | B1–B6 | Same behaviour, smaller files, shared SQLite set-up |
| 3 | C1, C2, C4, C5 | Tools declare capabilities, routes closed, sockets ticketed and resynced |
| 4 | D0, D1, D2 + C3 | State database; slots and per-session identity on the server |
| 5 | D3–D5 | Slots in the cockpit and for the lead |
| 6 | E1–E3 | Notifications and answering in the quick look |
| 7 | F1–F3 | Server roles and flags |
| 8 | G1–G6 | Home |
| 9 | H1–H2 | Many servers in the cockpit |

## Decided (2026-10-14)
1. **Notifications for everything,** including turns the lead started.
2. **Sub-slots report to the lead and to the user.** Reports are queued, not added to the lead's running turn, so they never cut off its work.
3. **Slots come back after a restart** and resume on first use.
4. **Durable state lives in a state database:** per project, plus home's own. Plans and config stay as files. The analysis cache stays separate and throwaway.
5. **Keys:** `SPC w`, `SPC n` (with `n`/`N` in the look), `SPC S`. All configurable.

## Known limits (stated, not worked around)
- **Agents running as the same OS user can read local files,** including the cockpit token and home's database. Keeping them out is the harness sandbox's job.
- **Config files in the repo** (`.nah/access.toml`) can be edited by an agent's harness, with the user's approval of the edit. Changes take effect at the next start and show in the diff.
- **A cockpit in a background tab draws rarely,** so `window.nah` / `cockpit_*` calls answer slowly there.
- **`slot_read` is O(cached entries)** per call.

## Noticed, not changed
- `arch-service/tests/perf.rs` (`this_repo`) sits at its limit (343–366 ms against 360) on this sandbox. It measures the biggest Rust file, `cockpit-ui/src/app.rs`, which B1 splits.
- The sandbox disk is 20 GB, and builds on the host mount fail to mmap. Builds use `~/.cache/meta-harness-sandbox-target` with symbols stripped.

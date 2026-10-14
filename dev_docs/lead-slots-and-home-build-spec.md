# Build spec: lead and sub-slots, permissions, notifications, home

This is the step-by-step plan I follow to build what's described in
`lead-slots-and-home-plan.html`. Every step says what changes, where, what
must not change, and how it's proven. If a step turns out to be wrong
against the code, I stop and ask before improvising.

All paths are relative to `meta-harness/` unless they start with `dev_docs/` or `/`.

---

## 0. Rules of engagement

1. **Two kinds of step only.**
   - **MOVE:** structure only. Code moves between files and modules, and visibility widens only as far as needed (`pub(super)`, `pub(crate)`). No logic, names, ids, keys, routes, tool names, wire formats or UI change. Proof: the full test suite, clippy `-D warnings`, the WASM build, and a diff that reads as relocation.
   - **FEATURE:** new behaviour, with its tests written in the same step.
   - A step is never both. If a MOVE needs a behaviour change to compile, that change becomes its own FEATURE step first.
2. **No arbitrary changes.**
   - These stay as they are unless a step names the change and why it's needed: action ids, key bindings, route paths, MCP tool names and descriptions, event shapes, config file formats, UI layout and wording, error texts.
   - When I touch code that looks improvable but isn't in the step, I leave it and add a line to "Noticed, not changed" at the end of this file.
3. **No placeholders.** No `todo!()`, no stubs, no "for now" branches, no commented-out code, no feature flags hiding half-built work. A step ends complete or not at all.
4. **One path per job (clean cuts).**
   - When a step replaces a mechanism, the old one is deleted in the same step, with its tests moved to the new one.
   - No compatibility shims, no duplicate routes, no second way to open a session, check a permission or send an event.
5. **Checkpoints.**
   - Each step ends green:
     - `c.sh test --workspace --no-fail-fast`, with only the known timing-sensitive `arch-service` gates/read_gate tests allowed to flake, and only after a passing re-run on their own;
     - `c.sh clippy --workspace --all-targets -- -D warnings`;
     - the WASM build.
   - Each phase ends with a live run in the user's Chrome through `window.nah`.
   - Commits only with the user's approval. Each step is one proposed commit.
6. **Reuse before writing.**
   - Ranking: `arch_search::rank` / `rank_text` / `nav::rank_matches`.
   - Events, cache and replay: `EventHub`.
   - Pickers: the action list (`Param::Choice` / `Param::Search`) and the palette.
   - Floating views: the quick look. Request cards: `request_card.rs`. State chips: `status.rs`. LEDs: `ActivityLed`.
   - File writes: `write_atomic` (made `pub`). Config loading: `#[serde(default, deny_unknown_fields)]`, as in `harness-acp/src/settings.rs`. User places: `nah_packs::Places`.
   - UI tests: `window.nah` / `from_outside`. Agent tests: `MockAgent` and `FakeDriver`.
7. **Performance is part of correctness.**
   - No per-frame allocation-heavy walks, no polling where an event exists, no unbounded queues, no buffering where streaming works.
   - Hot checks are O(1): permissions and notification derivation.

---

## Phase A: Baseline and characterization

### A1. Commit the current work (needs the user's approval)
- Everything since `eebe4b7` is uncommitted: the UI work, the Claude Code harness, cockpit remote control.
- I ask to commit it as the baseline before any refactor, so every later diff is reviewable on its own.

### A2. Pin today's behaviour where coverage is thin (FEATURE: tests only)
Add tests that freeze what the refactors must not change:
- **`tests/surface_snapshot.rs`:**
  - (a) the MCP `tools/list`: names, order, and a hash of each schema and description;
  - (b) the HTTP route table: method and path, listed by probing every known route with `OPTIONS`/an invalid method and asserting not 404, against a pinned list taken from the survey (§ route table);
  - (c) `cockpit-ui`: a golden of `(action id, key hint)` for `full_app()` in each focused view, built from the existing `bound_actions`.
- **The request-card click path test** (`request_card.rs`): clicking Yes sends the response. This pins the behaviour before Phase E reroutes it.
- **`MockAgent` coverage gaps** used later: `[permission]` through `AcpAdapter` → `RuntimeRequest` → `respond` → the agent sees `allow-once`. The contract test covers this per pack; add one direct adapter test so slot and notification tests can rely on it.

The pinned lists are updated **only** by steps that intend the change, and the step says so.

---

## Phase B: Pay down structure (MOVE only)

Ordered so each split is along a seam a later phase needs.

### B1. Split `crates/cockpit-ui/src/app.rs` (4782 lines) into `app/`
Target layout. Line ranges are from the survey; the content moves verbatim.

| New file | Takes (from app.rs) |
|---|---|
| `app/mod.rs` | imports, `BufferKind`, `ShellCommand`, `IndexOp`, `Hinting`/`Searching`/`Capture`, consts, the `CockpitApp` struct, `new`, `impl eframe::App`, `frame`, `draw_frame` |
| `app/lifecycle.rs` | `update_lifecycle`, `apply_user_config`, `fetch_index_status`, `mark_out_of_date`, `run_index`, `show_code` |
| `app/layout.rs` | the columns, panes and parking block (540-765): `used_pane` … `pinned`, plus `focused_kind`, `slot_of`, `ensure_visible`, `focus_kind` |
| `app/keeping.rs` | saved state (767-839) |
| `app/look.rs` | quick look (841-907) and `render_quick_look`, `render_talk` |
| `app/views.rs` | view dispatch and linking (992-1265): `provider*`, `view*`, `run_on`, jumps, `situation`, `reaches`, `link_*` |
| `app/commands.rs` | actions and shell commands (1267-1680) |
| `app/capture.rs` | hints, search, marks (1682-1846), `render_hints`, `hint_chip` |
| `app/going.rs` | go-tos and details (1848-2106) |
| `app/keys.rs` | `text_input_focused`, `release_keyboard`, `leader_key`, `handle_keys` |
| `app/draw/top_bar.rs` | `ActivityLed`, `render_top_bar`, `render_index_light`, `index_state`, `server_line` |
| `app/draw/bars.rs` | `bar_keys`, `render_key_bar`, `render_prefix_popup`, `render_key_sheet`, `keys_of`, `describe_jump` |
| `app/draw/which_key.rs` | `group_label`, `leader_title`, `render_which_key_hud` |
| `app/draw/slots.rs` | `tab_keys`, `render_slot`, `render_palette` |
| `app/outside.rs` | `serve_calls`, `actions_listed`, `run_named`, `remote_status` |
| `app/tests/{support,layout,keys,goto,look,outside,...}.rs` | the tests, split by topic, with the shared helpers in `support.rs` |

Rules for this step:
- Methods called across files become `pub(super)`. Fields keep their current visibility; children of `app` see private fields.
- **Theme lints.** Update `theme.rs` `glyph_tests::SOURCES` to list the new files, and bump the array length. The colour/size lint now scans the moved code too (the survey found no violations in 205-3180).
- **Two cleanups**, each verified dead or misplaced:
  - remove `reveal_inbox`, which is drained but never filled;
  - move the stray doc comment at 2867 to `render_hints`.
- **Add `app/tests/split.rs`**, modelled on `graph/tests.rs::the_graph_is_split_as_agreed`. It asserts no file in `app/` exceeds 900 lines, so the split can't silently regrow.

### B2. Split `crates/cockpit-ui/src/chat.rs` (1508 lines) into `chat/`

| New file | Takes |
|---|---|
| `chat/mod.rs` | `SessionStatus`, `Reply`, `Mailbox`, `post`, the struct and Default, the helpers (127-168), `process_event`, the getters |
| `chat/session.rs` | `open_thread`, `send_prompt`, `add_to_turn`, `steer`, `say`, `who`, `last_reply`, `tick`, `drain_replies`, `load_saved`, `reconnect`, `save_tools`, `saved_choices` |
| `chat/render.rs` | `render` … `render_input`, `render_steps`, `block_*`, `mcp_label` |
| `chat/nav.rs` | `links_of`, `item_text`, `impl Navigable` |
| `chat/actions.rs` | `ChatCommand`, `wire_name`, `impl ActionProvider` |
| `chat/tests.rs` | the tests |

### B3. Split `src/server.rs` routes by concern
- `src/routes/{harness.rs, ui.rs, files.rs, feedback.rs, tool_servers.rs, events.rs}`, each exposing `fn routes() -> Router<AppState>`.
- `server.rs` keeps `AppState`, the error types, `create_router` (merging the route modules and the layers), `bridge_architecture_events`, `queue_runner`, `static_dir`.
- `arch.rs`, `graph.rs` and `queue.rs` move under `src/routes/` unchanged.
- Paths, handlers and layers are identical, which the surface snapshot proves.

### B4. Split the MCP service into tool sets (structure only)
- Add a `ToolSet` trait with `fn tools(&self, trees: &TreeConfig) -> Vec<McpTool>` and `async fn call(&self, name, args) -> Option<Result<String, ServiceError>>`, where `None` means "not mine".
- **Move each tool's schema** (from `tools.rs`) **and its handler** (from `service.rs`) together into `src/mcp/sets/`:
  - `explore.rs`: ingest, list_types, search, inspect, view, navigate;
  - `plans.rs`: plan_list, plan_create, plan_import, plan_edit, plan_validate, contract;
  - `build.rs`: verify, run_tools, review;
  - `review.rs`: review_comments, answer_comment, reply_comment;
  - `cockpit.rs`: the six `cockpit_*` tools and `drive`.
- **Shared helpers** go to `src/mcp/text.rs`: `parse`, `pretty`, `marked`, `next_steps`, `walkthrough_steps`, `comments_text`, `findings_text`, `plan_id`, `agent`.
- **`McpService`** keeps `handle_request` and iterates the sets in the order that reproduces today's list exactly, which the snapshot test proves.
- `tools.rs` and the long `match` in `call` are deleted.

### B5. Split `src/main.rs` start-up into `src/startup.rs`
- Functions: `listen`, `open_project` (arch, ui_state, ingest/watch), `harnesses`, `tool_servers`, `queue`, `serve`.
- The env vars, order and logs are unchanged.

---

## Phase C: Permissions (FEATURE)

### C1. New crate `crates/nah-access`
Dependencies: serde and toml only, so it's WASM-safe.

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Capability { ViewRead, ViewDrive, PlanEdit, CodeCheck, SlotsObserve, SlotsManage,
                      SlotsMessage, SlotsReport, UserNotify, RequestsAnswer, PlansApprove, SessionsControl }
impl Capability {
    pub const ALL: [Capability; 12];
    pub fn name(self) -> &'static str;              // "view.read" …
    pub fn from_name(s: &str) -> Option<Self>;
}
#[derive(Clone, Copy, Default)] pub struct CapabilitySet(u16);   // contains/insert are O(1)
pub struct Role { pub name: String, pub can: CapabilitySet }
pub enum Principal { User, Home, Slot { slot: SlotId, role: String }, External }
pub struct Access { roles: HashMap<String, Role> }
impl Access {
    pub fn defaults() -> Self;                                 // config/access.toml, include_str!
    pub fn with_overrides(self, text: &str) -> Result<Self, String>;  // .nah/access.toml
    pub fn allows(&self, who: &Principal, needs: Capability) -> bool;
    pub fn role_of<'a>(&'a self, who: &Principal) -> &'a Role;
}
```

- **The user role is not configurable.** `Principal::User` and `Principal::Home` always have every capability, because home relays what the user does.
- **Agent roles live in config.**
  - `[roles.<name>] can = [...]`, parsed with `deny_unknown_fields`.
  - An unknown capability errors with the known names, e.g. `"access: role sub: unknown capability 'view.reed' (known: view.read, …)"`.
  - A role in `.nah/access.toml` replaces that role entirely, the "replace or extend" pattern.
  - The required roles are `lead`, `sub`, `worker` and `external`; a missing one errors.
- **Defaults**, as agreed:

| Role | Capabilities |
|---|---|
| `lead` | view.read, view.drive, plan.edit, code.check, slots.observe, slots.manage, slots.message, user.notify |
| `sub` | view.read, slots.report, user.notify |
| `worker` | view.read, plan.edit, code.check, user.notify |
| `external` | view.read |

- **`SlotId`** lives in `harness-contracts` (`id_type!`), and nah-access depends on contracts for it. Contracts stay WASM-safe.
- **README:** add `.nah/access.toml` to the configuration table.
- **Tests:** defaults parse; overrides replace roles; unknown capabilities and roles are rejected with the message above; User and Home allow everything; each default role allows exactly its list.

### C2. Every MCP tool declares its capability
- `ToolSet::tools` returns `Vec<ToolDef { tool: McpTool, needs: Capability }>`. The compiler forces every tool to name one.
- **Mapping:**

| Capability | Tools |
|---|---|
| view.read | search, inspect, view, list_types, plan_list, contract, review_comments, ingest, cockpit_read, cockpit_actions |
| view.drive | navigate, cockpit_run, cockpit_click, cockpit_type, cockpit_keys |
| plan.edit | plan_create, plan_import, plan_edit, answer_comment, reply_comment |
| code.check | plan_validate (it runs logic-step commands), verify, run_tools, review |

- **`McpService`** builds `index: HashMap<&'static str, (set, Capability)>` once. It caches the filtered `tools/list` per role name, computed lazily and never invalidated, because roles are fixed for the process.
- **`tools/call`** checks before dispatch. The refusal is a tool error: `"this session's role (sub) can't use navigate (it needs view.drive)"`.
- **`agent()`** becomes `Actor::Agent { session: <slot name or "external"> }`, so comments and answers say which agent wrote them. This is the identity the code already had a slot for.

### C3. Know who's calling on MCP
- **`GET /api/mcp/sse`** takes `?key=<slot key>`. `SseSessionManager` stores `session_id → (sender, Caller)`.
- **`POST /api/mcp/messages`** looks up the caller by `session_id`. `McpMessagesQuery` gains `key: Option<String>` for HTTP-mode agents, which POST to `/api/mcp/sse?key=…` because ACP downgrades SSE to HTTP and keeps the URL.
- **Key → principal** goes through a `CallerResolver` trait object: the slot manager in Phase D, and in Phase C a resolver where every key is unknown, so every caller is `External`.
  - Unknown or missing key → `External`.
  - A key that existed but has been rotated → `External`, which is read-only.
- **Secret hygiene (fixes a leak found in the survey).**
  - `harness-acp/src/adapter.rs:316` copies the full `mcp_servers` into `Thread`, which is broadcast to every client. Add `McpServer::redacted()` in contracts: it drops the URL query and replaces header values with `"…"`. Use it there.
  - Test: the `ThreadUpdated` event has no key in it.
- **Until Phase D gives slots keys,** the server's own session MCP entry has no key, so harness sessions are `External`. They'd temporarily lose plan editing and building.
  - So C3 lands together with D2 (slot keys), as one checkpoint. The plan keeps them as separate commits but tests and ships them together.

### C4. Every HTTP route declares its capability
- **`Needs<C>`** replaces `CockpitUser`:
  - `pub struct Needs<C: Cap>(PhantomData<C>)`, with marker types made by a macro from `Capability::ALL`.
  - It extracts the request's principal: the cockpit cookie → `User`; `Authorization: Bearer <server secret>` → `Home` (Phase F); else `Anonymous`.
  - It rejects with 403 `{"error": "only the user, in the cockpit, can do this"}`, today's text kept for user-only capabilities.
- **Routes:**
  - Every mutating route takes `Needs<SessionsControl>`, `Needs<PlansApprove>` or `Needs<RequestsAnswer>`, as fits. These are user-only, because agents use MCP and not REST.
  - Read routes take `Needs<ViewRead>`. The cockpit always has its cookie.
  - Left open: `/health`, `/cockpit`, the static files, `/api/mcp/*` (agents; identified by key), `/ws/cockpit` (ticket).
- **`/ws/events` requires the cookie.** It carries everything the server knows.
- **This closes the deferred "cockpit login on every route" item.**
- **Tests:**
  - A source-scan test (the `clean_cut.rs` style) asserts every handler registered with post, put or delete in `src/routes/*` takes a `Needs<`.
  - The surface snapshot is extended so every non-open route returns 403 without the cookie.
  - Existing e2e tests switch from `http(...)` to `http_cockpit(...)`. That's a test-only change, listed in the commit.

---

## Phase D: Slots (FEATURE)

### D1. Contracts
- `SlotId`, and `Slot { id, role: String, name: String, instance_id: InstanceId, current: Option<ThreadId>, threads: Vec<ThreadId>, started_by: Starter }` with `Starter { User, Lead, Queue }`.
- `ClientEvent::SlotUpdated { slot }` and `ClientEvent::SlotClosed { slot_id }`.
- `EventHub` caches `Key::Slot(id)`. `SlotClosed` evicts it.
- A wire-format test for the new events.

### D2. `harness-core/src/slots.rs`: the slot manager
`SlotManager` wraps `SessionManager`, which stays a plain thread store.

```rust
pub struct SlotManager {
    sessions: Arc<SessionManager>, hub: Arc<EventHub>,
    slots: RwLock<BTreeMap<SlotId, SlotEntry>>,      // SlotEntry { slot, keys: HashMap<ThreadId, String>, kept: Vec<String> /* reports waiting for a lead session */ }
    keys: RwLock<HashMap<String, SlotId>>,
    ours: OurServer,                                  // builds the per-slot MCP entry: base url + ?key=
    project_servers: Arc<dyn Fn() -> Result<Vec<McpServer>, String> + Send + Sync>,
    store: Arc<dyn SlotStore>,                        // persistence, implemented by the server
}
impl SlotManager {
    pub fn lead(&self) -> SlotId;                                       // exists from start-up
    pub async fn create(&self, role: &str, name: Option<String>, instance: InstanceId, by: Starter) -> Result<SlotId>;
    pub async fn open_session(&self, slot: &SlotId, open: SessionOpen) -> Result<ThreadId>;   // new or resume
    pub fn switch(&self, slot: &SlotId, thread: &ThreadId) -> Result<()>;
    pub async fn close_session(&self, slot: &SlotId, thread: &ThreadId) -> Result<()>;
    pub async fn close(&self, slot: &SlotId) -> Result<()>;            // not the lead
    pub async fn send(&self, slot: &SlotId, text: String, how: Send, by: &Principal) -> Result<TurnId>;   // lead → sub
    pub async fn report(&self, from: &SlotId, text: String) -> Result<Delivered>;                       // sub → lead (Delivered | Kept)
    pub async fn wait_reply(&self, slot: &SlotId, turn: &TurnId, within: Duration) -> Result<Reply>;
    pub fn read(&self, slot: &SlotId, last: usize) -> Result<SlotReport>;  // from the hub's cached items
    pub fn caller(&self, key: &str) -> Option<Principal>;              // the CallerResolver for C3
}
```

- **Sessions in a slot work as today.** The slot holds `threads` (all still open) and `current`. New, resume, switch and close behave exactly like the chat's session actions do now; they just live in a slot. Swapping the lead's session never touches other slots.
- **Keys.** Each `open_session` makes a new random key. The old thread's key stays valid while that thread is open and is dropped when it closes.
  - The session's MCP list is `[ours with ?key=] + project_servers()`, the same for every slot.
  - Queue sessions now get the project's tool servers too. Today they don't, which the survey found as an inconsistency. This is the intended consequence of one path.
- **Messages from the lead** arrive as a prompt in the sub-slot's session that starts with `From the lead (<lead name>):` and a blank line, so both the agent and the user can see where it came from.
- **Reports from a sub-slot** go the other way: into the lead's current session, starting `From <sub name> (sub-slot):`.
  - They're added to the lead's running turn when its harness takes that, otherwise queued.
  - Every report is also an update for the user (a Message notification), so the user sees what sub-slots tell the lead.
  - With no lead session open, the report is kept on the lead slot and delivered as the first message when a lead session opens. The sub-slot is told it was kept, and the user still gets the update.
- **`wait_reply`** subscribes to the session's events and waits for the turn to end, with no polling. It returns the turn's final assistant text, built by a pure function over `TurnItem`s that is tested on its own.
- **Persistence and restarts.**
  - **What's saved:** `.nah/state/slots.json`, written with `write_atomic` on every slot change, with `state` added to `NAH_IGNORED`. Per slot: id, role, name, harness, policy and model, the `native_id` of each open session and which one is current, and kept reports.
  - **What can't be saved:** running agent processes, which end with the server. Transcripts aren't saved either; each harness keeps its own history.
  - **On start:**
    - Every lead and sub-slot comes back with its name, role and harness, and its sessions marked "not running".
    - Worker slots aren't restored: the queue starts its jobs afresh, as it does today.
  - **On first use, each slot resumes its session:** when its chat is opened, when the user sends it a message, or when the lead sends to it. The resume uses `native_id`, so the conversation's history returns. It gets a new key, as every new session does.
    - Agents aren't respawned at start-up for slots nobody touches.
  - **When a harness can't resume** (it doesn't support it, or the saved session is gone), the slot starts a fresh session instead, and the user gets a SessionCrashed update saying so.
- **The work queue.** `SessionRunner` asks the slot manager for a worker slot per job (`role = "worker"`, `name = "queue: <component>"`, `Starter::Queue`), runs the brief, then closes the slot. This deletes `SessionRunner`'s own `sessions.open`.
- **Tests** (`FakeDriver` plus `MockAgent` through a new `tests/common` helper that registers an ACP instance over `MockAgent`):
  - create, open, switch and close keep the lead untouched;
  - keys rotate per session;
  - a sub's session MCP URL carries its key, and the broadcast `Thread` doesn't;
  - `send` with wait returns the reply;
  - the lead can't be closed;
  - slots restore from file;
  - queue jobs appear as worker slots and go away.

### D3. Routes (user-only; `Needs<SessionsControl>`)
- `GET /api/slots`
- `POST /api/slots` with body `{ role: "sub", name?, instanceId, policy, model? }`
- `DELETE /api/slots/{slot}`
- `POST /api/slots/{slot}/sessions` with body `{ policy, model?, resume? }`, returning `{ threadId }`
- `POST /api/slots/{slot}/current` with body `{ threadId }`
- `DELETE /api/slots/{slot}/sessions/{thread}`

`POST /api/threads` is deleted: opening a session always goes through a slot, the lead by default. The thread routes (turns, inject, steer, interrupt, requests, delete) stay as they are.

### D4. MCP set `src/mcp/sets/slots.rs`

| Tool | Needs | Arguments | What it does |
|---|---|---|---|
| `slot_list` | slots.observe | — | Each sub-slot's name, harness, model, state (running / idle / waiting on the user / failed) and last activity |
| `slot_read` | slots.observe | `slot`, `last?` | That slot's recent messages, from the hub cache |
| `slot_start` | slots.manage | `harness`, `name?`, `model?`, `mode?` | Creates a sub-slot and opens its session |
| `slot_close` | slots.manage | `slot` | Closes a sub-slot |
| `slot_send` | slots.message | `slot`, `text`, `add_to_running?`, `wait_secs?` | Sends a message; with a wait, returns the reply or "still working" |
| `tell_lead` | slots.report | `text` | A sub-slot reports to the lead (and the user sees it as an update) |

- Every tool's text ends with next steps, like the existing tools.
- Sub-slots see only `tell_lead` and `tell_user` here, because the role filters the list.

### D5. Cockpit: chat per slot
1. **(MOVE-like refactor, no visible change.)** Split `ChatStreamState` into:
   - **`Sessions`**, one per project. It holds `store`, `active_instance` (the harness chosen for new slots), `opening`, `error`, `saved_sessions`, `after_saved`, `pending_palette`, `tools`, `tools_asked`, `login`, `auth_paste`, `drafts`, `mailbox`, `runtime_mode`, `model`, and the slot list.
   - **`ChatView`**, one per chat view. It holds `slot`, `input_buffer`, `queued_prompt`, `markdown_cache`, `requested_file_jump`, `cursor`, `link_pick`, `scroll`, `spots`, `drawing`, `view`.
   - `Sessions` is shared through `Rc<RefCell<Sessions>>`. That's safe in the single-threaded WASM app with one rule: no borrow is held across a call that can re-enter. It's the only interior mutability added, and it's tested.
   - With one chat view showing the lead, every existing test passes unchanged.
2. **(FEATURE)** Chat views per slot:
   - `BufferKind::Chat` stops being one-only, and `Parked` gains `chats`.
   - Every one-only assumption the survey listed is updated in this step: `columns.rs:81-83,198-199`, `swap_in`, `use_pane`, `back_to_used`, `delete_view`, `reset`, `place_of`, the `saved_ui.used` filter, `shell_actions` (adds `buffer.new.chat`), `provider*`, `render_slot`, and `render_talk` (talks to the lead).
   - The fixed egui ids at `chat.rs:397` and `766` get salted with the pane id.
   - **Actions:**
     - `slot.pick` (Choice) shows a slot in this chat view. Choices show the state through a `status.rs` chip and are ranked by `arch_search::rank`.
     - `slot.new` is a new sub-slot: pick the harness (Choice), then it opens.
     - `slot.close`.
     - The existing `session.*` actions act on this view's slot.
   - **Keys:** group `w = "Slots"`, with `w w` pick, `w n` new, `w c` close.
   - **Requests:** `y`/`Y`/`A`/`d` and `SPC r …` act on the focused chat view's slot, as they act on the one chat today.
   - **Saved state:** `Place::Chat { message }` gains `slot`. `SavedUi.thread` and `draft` move into each chat view's place, and `saved::VERSION` goes to 3 (older state opens fresh, as the version rule already does).
   - **Queue:** the "open job's session" link shows the worker slot in a chat view, instead of overwriting `active_thread`.
   - **Top bar:** the AGENT LED shows the lead slot.
   - **Tests:**
     - app tests through `from_outside`: two chat views on two slots;
     - request keys act on the focused one;
     - saved state round-trips per view;
     - the surface snapshot gains the new action ids.

---

## Phase E: Notifications (FEATURE)

### E1. Contracts
```rust
pub struct Notification {
    pub id: NotificationId,          // stable per subject: "req:<request>", "turn:<turn>", "crash:<thread>", "idle:<slot>", "msg:<uuid>", "signin:<instance>", "lost:<server>"
    pub server: Option<String>,      // set by home
    pub level: Level,                // Waiting | Update
    pub kind: NotificationKind,      // Approval, Question, Link, SignIn, SessionCrashed, TurnFailed, TurnFinished, Message, LeadIdle, ServerLost
    pub slot: Option<SlotId>, pub thread: Option<ThreadId>, pub request: Option<RequestId>,
    pub title: String, pub detail: Option<String>,
    pub at_ms: u64,
    pub state: NotificationState,    // Open | Done | Dismissed
}
```
- Add `ClientEvent::NotificationUpdated { notification }`.
- The hub caches open notifications by id. A non-Open update is published once, then evicted.
- Ordering: Waiting before Update. Among updates, SessionCrashed and TurnFailed come first. Within a group, oldest first. This is one `fn order(&Notification) -> impl Ord`, shared by server and cockpit.

### E2. New crate `crates/nah-notify`
Dependencies: contracts, access, tokio (server side only).

- **`rules.rs`** is a pure function: `fn derive(event: &ClientEvent, world: &dyn World, open: &OpenSet) -> Vec<Notification>`. `World` answers "slot of thread", "is the lead" and "close was asked for". The rules:

| Event | Becomes | Closed by |
|---|---|---|
| Request Pending | Waiting: Approval (Permission, Command, FileRead, FileChange), Question (UserInput), Link (McpElicitation) | the request not Pending any more |
| Instance auth SignedOut or NeedsAuth, for a harness a slot uses | Waiting: SignIn | SignedIn |
| ThreadStatus Error, or ThreadClosed not asked for, for a slot's thread | Update: SessionCrashed | seen or dismissed |
| Turn Failed | Update: TurnFailed | seen or dismissed |
| Turn Completed, in any slot, whoever started it | Update: TurnFinished | seen or dismissed |
| A sub-slot's report to the lead, or `tell_user` | Update: Message | seen or dismissed |
| The lead slot's thread goes Running → Idle with nothing queued | Update: LeadIdle | it runs again |

- **`hub.rs`:** `NotifyHub` subscribes to `EventHub`, runs the rules, keeps the open set, publishes `NotificationUpdated` back through `EventHub`, and hands every change to its adapters. It also offers `seen(slot)`, `dismiss(id)` and `message(slot, text)`, the last one for `tell_user`.
- **`adapter.rs`:** `pub trait Adapter: Send + Sync { fn deliver(&self, n: &Notification); }`. It's non-blocking; an adapter queues internally.
  - Adapter #1 is the event stream itself. The cockpit receives `NotificationUpdated` on `/ws/events`, so no extra code path is needed for it.
  - The trait is the extension point for Slack, email and a phone app. A test adapter proves delivery.
- **Answers keep using the existing respond route.** `POST /api/threads/{t}/requests/{r}` is unchanged. The rule closes the notification when the request resolves, so there's exactly one answer path.
- **Routes** (`Needs<SessionsControl>`):
  - `GET /api/notifications`
  - `POST /api/notifications/seen` with body `{ slot }`
  - `POST /api/notifications/{id}/dismiss`
- **The MCP `tell_user` tool** (needs user.notify) goes in `src/mcp/sets/slots.rs`.
- **Tests:**
  - the rules as a table;
  - end to end with `MockAgent`: `[permission]` → Waiting → answered → Done → the agent continues;
  - `[question]` → Waiting → answered;
  - a turn the lead started in a sub-slot makes a TurnFinished like any other;
  - a sub-slot's report reaches the lead's session and makes a Message update;
  - a crash (drop the mock connection) → SessionCrashed;
  - a late client gets the open notifications in the replay.

### E3. Cockpit: attention
- **`src/attention.rs`** (shell level) keeps notifications by id from the events, plus running counts, so nothing is rescanned per frame.
- **Top bar:** two counters right after the INDEX light, drawn with `ActivityLed::show`-style chips:
  - **waiting**, in the warn colour, hidden when zero;
  - **updates**, muted.
  - Clicking one opens its picker. Each is registered with `feedback.register_inspectable`.
- **Pickers** are shell actions with `Param::Choice`, like `buffer.list`, ranked by the palette's `arch_search::rank`:
  - `attention.waiting` and `attention.updates`;
  - each choice: `label = title`, `detail = "slot · kind · age"`;
  - picking one opens it in the quick look;
  - an item resolved while the picker is open opens as "already handled", never as an error.
- **The quick look gains a second kind of content.** `QuickLook` becomes `{ content: Look::Card { .. } | Look::Attention { id }, .. }`. The existing card look is unchanged.
  - The attention look shows: the slot name and harness; the request card (`request_card.rs`) or the message; and the last assistant message of that turn, for context.
  - While it's open it's the focused provider, so its actions win key conflicts:
    - `attention.next` (`n`) and `attention.prev` (`N`), oldest first, matching vim's next and previous;
    - `request.*` (`y`/`Y`/`A`/`d`) for the shown request;
    - `attention.open_slot` shows the slot's chat in a column;
    - `attention.dismiss`.
  - Esc closes it, as for the card look.
- **Starting a walk:** `attention.next_waiting` (`SPC n`) opens the oldest waiting item. When nothing is waiting, it says so in the key-bar note.
- **The request card's click path** sends through the chat mailbox like the keyboard path, so failures show in the note. This fixes the ignored-error bug the survey found (`request_card.rs:118-120`); it's needed because the quick look reuses the card.
- **Seen:** when a chat view showing slot X is drawn, the cockpit sends `seen { slot: X }` once per change, debounced.
- **Browser notifications:** `attention.browser` (Toggle, saved) uses the web Notification API for new Waiting items while `document.hidden`. It needs `web-sys` features `Notification`, `NotificationOptions`, `NotificationPermission` and `Document` visibility. Clicking one focuses the tab and opens the item.
- **Tests (app, through `from_outside`):**
  - counters follow events;
  - the picker lists, ranks and opens;
  - next and previous walk in order inside the look;
  - answering in the look sends the response and closes the item;
  - updates clear on seen.

---

## Phase F: Server roles (MOVE, then FEATURE)

### F1. (MOVE) Split `AppState` into module states
- **States:** `ProjectState` (arch, ui_state, queue), `HarnessState` (registry, sessions, slots, tool_servers), `CockpitState` (auth, remote), `NotifyState`, `AccessState`.
- **Each route module** uses only its own state through `FromRef`.
- **The server becomes a list of modules.** A `Module` trait has `fn routes(&self) -> Router` (already given its state) and `fn start(&self)`. `startup.rs` assembles the list.
- Behaviour is identical, which the surface snapshot proves.

### F2. (FEATURE) Flags
- clap `serve` options:
  - `--no-cockpit` (no static files);
  - `--no-project` (no project, harness, queue or watch modules; the MCP sets that need a project are absent);
  - `--listen <addr:port>`;
  - `--listen-public`.
- The `HOST`/`PORT` env vars keep working as defaults, so `serve.sh` and the Makefile work unchanged.
- **The default bind goes from `0.0.0.0` to `127.0.0.1`** (agreed: safe by default). A non-loopback, non-private address without `--listen-public` is refused with a message saying how to allow it.
- **A no-project server** keeps its UI state and settings in `{config_home}/nah/home/`.
- **Tests:**
  - a no-project server starts without arch, LSP or watch, and its start time and memory are logged by a test;
  - a public bind is refused;
  - `--no-cockpit` serves no `/`.

### F3. (FEATURE) Home's credential
- `Authorization: Bearer <this server's cockpit token>` → `Principal::Home`. It goes through the same `Needs<C>` extractor.
- The token is never accepted in a URL, except the existing `/cockpit?nah=` launch link.

---

## Phase G: Home (FEATURE)

### G1. Dependencies
Add `hyper-util` (client, http1, tokio; it's already in the tree through axum) for streaming pass-through, and promote `tokio-tungstenite` (already a dev-dependency) to a normal dependency for socket links and socket pass-through. No reqwest.

### G2. `src/home/servers.rs`: the server list
- File: `{config_home}/nah/servers.toml`, as `[[server]] name, url, secret`.
  - Parsed with `deny_unknown_fields`.
  - Names use the `PlanId` slug rule.
  - The file is written owner-only, like the cockpit token.
- `here` is reserved for the local server.
- **Routes** (`Needs<SessionsControl>`):
  - `GET /api/home/servers`, without secrets;
  - `POST /api/home/servers`;
  - `DELETE /api/home/servers/{name}`.

### G3. `src/home/link.rs`: one link per server
- A task per server connects to `<url>/ws/events` with the Bearer secret.
- **Each server gets its own `EventHub` on home.** The existing cache and replay are reused, keyed by server name. On reconnect, that hub's cache is cleared before the replay is applied, so nothing stale survives.
- **Backoff:** 0.5 s doubling to 10 s, the cockpit's existing values. Link state is Connecting, Connected or Lost.
- **Lost** publishes a `ServerLost` notification into home's notifications; reconnecting closes it.
- **Disconnect** stops the task and keeps the entry. **Connect** starts it.

### G4. `src/home/stream.rs`: one stream to the cockpit
- **`GET /ws/home`** (cookie required) sends, for every server including `here`, its hub's replay, then live events, as frames `{ "server": name, "event": ClientEvent }`.
- **Each client has a bounded queue of 4096.** A client that falls behind gets `{ "resync": name }`, then that server's replay. Memory never grows unbounded.
- **Notifications from other servers** arrive as their `NotificationUpdated` events. Home sets `server` on them and hands them to its own adapters, so the rules never run twice.

### G5. `src/home/pass.rs`: pass-through
- **`/s/{server}/{*rest}`, any method:**
  - streams the request to `<url>/<rest>` and streams the response back;
  - adds the Bearer secret;
  - removes `Cookie` and hop-by-hop headers;
  - doesn't buffer bodies.
- **`/s/{server}/ws/{*rest}`** bridges the socket with tokio-tungstenite in both directions.
- **Permissions:** the route itself needs `Needs<ViewRead>` from the cookie. The worker then checks the real capability against Home.
- **Tests,** with two servers in one test process (`spawn_app` twice plus a home):
  - a request and a socket go through;
  - the cookie isn't forwarded;
  - an unknown server gives 404;
  - a large body streams.

### G6. The three actions
| Action | How |
|---|---|
| Disconnect | `POST /api/home/servers/{name}/disconnect` (and `/connect`) |
| Close a session or slot | The existing routes, through the pass-through |
| Shut down a server | New `POST /api/server/shutdown` on every server (`Needs<SessionsControl>`), which triggers the graceful shutdown already in `serve`. The cockpit asks for confirmation through a Choice ("Shut down <name>" / "Keep it running"). |

Tests: disconnect leaves the worker running and serving; shutdown stops it; home shows Lost, then the entry stays until removed.

---

## Phase H: Many servers in the cockpit

### H1. (MOVE) Introduce `Desk`
- Every per-project field the survey marked **P** moves into `struct Desk` (`app/desk.rs`): net, sessions and chat views, code, graph, spec, tree, review, queue, `review_seen`, columns, parked, active, last_used, fresh, landing, types, quick look, keeper, jumps, marks, searches, missed, details, pending_goto, choosing, at_inbox, index, index_inbox, types_inbox, anchor_checked.
- `CockpitApp { desk: Desk, ..shell }`. Methods that only touch desk state move to `impl Desk`; mixed ones stay on the app and take `&mut self.desk`.
- Still one desk, so nothing changes visibly. Every test passes, and `tests/headless.rs` is unchanged.

### H2. (FEATURE) Desks per server
- **`CockpitApp`** holds `desks: Vec<Desk>` and `current: usize`. Only the current desk is updated by input and drawn. Every desk applies its incoming events each frame, which is a cheap upsert.
- **`NetClient`** gains a path prefix: `""` for `here`, `/s/<name>` for the others. Every endpoint function uses it (`base_url` + prefix).
- **The socket.** With home available, one `/ws/home` socket per app replaces each desk's `/ws/events`. Frames are routed to `desks[server].incoming`, and `resync` resets that desk's store. Each desk keeps its own reconnect handling only for its HTTP calls.
- **Saved state:** each desk loads and saves its own server's `/api/ui/state` through its prefix. Shell state (current server, attention settings) is saved on home in `{config_home}/nah/home/`.
- **Switcher:**
  - a top-bar chip with the server name, a ▾, and a dot when anything is waiting there;
  - actions:
    - `server.switch` (Choice, ranked, with the waiting count in each detail);
    - `server.add` (Text: `name url secret`);
    - `server.disconnect` / `server.connect`;
    - `server.remove`;
    - `server.shutdown` (confirmation Choice);
  - keys: group `S = "Servers"`.
- **Attention across servers:** counters total every desk. Pickers show `server · slot · kind`. `attention.next` switches desk before opening the look.
- **Tests:**
  - app tests with two desks fed fake events: switching keeps each desk's columns and views; the counters total; next crosses desks;
  - a live run in the user's Chrome with home plus one worker.

---

## Order and checkpoints

| # | Steps | Shippable result |
|---|---|---|
| 1 | A1, A2 | Baseline committed; today's surface pinned |
| 2 | B1–B5 | Same behaviour, smaller files |
| 3 | C1, C2, C3 + D1, D2, C4 | Per-agent tools, closed routes, slots on the server |
| 4 | D3–D5 | Slots usable from the cockpit and by the lead |
| 5 | E1–E3 | Notifications and answering in the quick look |
| 6 | F1–F3 | Server roles and flags |
| 7 | G1–G6 | Home |
| 8 | H1–H2 | Many servers in the cockpit |

At the end of each row: the full checks, an update of README (configuration table, agent tools table, code map, cockpit keys), and a live run in Chrome through `window.nah`.

## Decided (2026-10-14)
1. **Notifications for everything,** including turns the lead started in sub-slots.
2. **Sub-slots report to the lead and to the user** (`tell_lead`, `tell_user`).
3. **Slots come back after a restart** and resume their sessions on first use (D2).
4. **Keys:** `SPC w` slots, `SPC n` next waiting, `n`/`N` inside the attention look, `SPC S` servers. All configurable, like every binding.

## Known limits (stated, not worked around)
- Agents running as the same OS user can read local files, including the cockpit token file. Keeping agents out of them is the harness sandbox's job, as today.
- A cockpit in a background tab draws rarely, so `window.nah` / `cockpit_*` calls answer slowly there.

## Noticed, not changed
- *(filled in during the work)*

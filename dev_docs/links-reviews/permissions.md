## Review: links change (0dfa7ff..HEAD), permissions angle

I found one real problem and one test gap.

1. **Linked content goes to the agent without checking it may read it. The plan says the server checks this. Severity: medium.**
   - **Where:**
     - `src/linking.rs:8-25` and `crates/nah-things/src/attach.rs:695-740` resolve and render every link without any reach check.
     - The doc comments state this outright: `slots.rs` `Expand`: "what's linked is handed whole"; `attach.rs`: "everything linked is handed on".
     - The plan's "Each side its own job" says the server "checks the reader may read it". The code does no such check.
   - **Sequence 1 (discussions):**
     1. The user starts a discussion asking sub-slot `helper` about lead's chat.
     2. `routes/discussions.rs` `start` refuses it through `may_read` ("helper can't read that").
     3. The user instead starts a discussion on anything, or replies in one, with a link to `chat lead` or a turn of it.
     4. `discussing.rs:79-81` passes `said.links` to `slots.tell`, and `helper` gets lead's chat pasted in. It comes in full when Auto and small or when Whole, or as its first 10 lines when a pointer.
   - **Sequence 2 (prompts):** the same happens through `POST /api/slots/{sub}/prompt` with a `Linked::Thing { ChatSession/ChatTurn of lead }`.
   - **Dead end:** when the link goes as a pointer, the text tells the agent "`thing_read` (...) reads the rest". `thing_read` (`src/mcp/sets/things.rs:944`) then refuses with `NOT_YOURS`. Tag links do the same: they say "use `tag_read`", and `tag_read` also checks reach (`mcp/sets/tags.rs:142,162`).
   - **Net effect:** the discussion route's `may_read` guard can be bypassed by the same user through a link. A pointer to something the agent can't read still leaks its first lines and gives an instruction that fails.
   - **To settle:** ask the user whether "checks the reader may read it" means the receiving slot, as `Reach::of_slot`, which `modules.rs` used before this change. Or does the user's own link override that, as plan default 1 implies for agents only? If the check is meant, `linking::attached` needs the target slot's `Reach`: name what it can't read, don't hand it over. The code had this before (`(not handed on: it's a chat this slot can't read)`).

2. **No test shows `thing_read` refuses another slot's chat. Severity: low.**
   - `thing_read` is a new general read path over every owner (terminals, chats, code, plans, discussions) under `view.read`, which sub and worker have.
   - The only test (`tests/terminals_tests.rs:500`) has the lead reading a terminal command.
   - The reach check is in the code (`things.rs:944`), but nothing pins it down. Confirming it needs a test where a sub calls `thing_read {"thing":"chat lead"}` and gets an error.

## Areas checked with nothing real found

- **New route `/api/links/check`:** takes `Needs<can::SessionsControl>`, the same as the prompt route. It returns only notes (thing names), not content.
- **Changed routes:**
  - `put_draft` keeps `SessionsControl` and `get_drafts` keeps `ViewRead`.
  - Discussion `start`/`reply` keep `Discuss`; `settle`/`reopen`/`change` keep `PlansApprove`.
  - HTTP principals are only User, Home and Anonymous (`cockpit.rs:125`), so agents can't reach these routes.
- **Agent tools:**
  - `thing_read` declares `ViewRead` and is in the role lists. External has no capabilities, so it gets nothing.
  - `slot_send` sends `Origin::Lead` as plain text. Reports, queue briefs and agents' discussion messages pass `&str` and carry no links.
  - No agent tool wraps the user's link path.
- **Files:** `thing_read` reads them through `project_file` (`src/code_tags.rs:52`), which stays inside the root, skips ignored and hidden files and doesn't follow symlinks. So `.nah/state`, `.env` and anything outside the project can't be read.
- **Other servers:** links resolve only against the server they're sent to (same `NetClient` prefix), so there's no cross-server reach.

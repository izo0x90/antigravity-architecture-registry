# To do

## Things to test by hand (from the chat cleanup, 2026-10-26)

- Touch, on another device (a phone): tap a sub-agent chip to jump to its line, double-tap to open it; open the Space menu and tap outside it (it closes and nothing under it runs). Needs a sitting with the cockpit on that device
- Antigravity's Google sign-in on its new connector (1.4.3, since 2026-10-25): sign out (Space a), sign in from the card; it ends saying signed in
- A session that won't start (a harness that can't run, or the network off): it says "couldn't start a session"; put the error away and "Resume its last session" shows again
- A discussion waking the lead: with the lead's session closed, a discussion waiting on it says "no session running"; `w` starts or resumes the lead
- Links survive a resume: send the lead a message with a `#tag` and an `@file`, resume its session; the message comes back with its chips and the content folded under them

## Next (2026-10-26)

- Next plan: `dev_docs/rules-and-subagents-plan.md`, written, waiting for your go and its four questions
    - The system's rules enforced by types and lints instead of remembered; one kit of pieces every view is built from (keys, folds, copy, search from the shell); what the key bar offers decided by written rules, not hand-picked numbers
    - One conversation view for the chat, a sub-agent and (maybe) discussions; one Sub-agents list instead of the chips; a sub-agent's view keyboard-first
    - Your notes: long text hidden below the typing box; clearing the box (`x` exists, rarely shown); slow session resume (Claude Code itself takes ~0.8 s: the rest is ours, to measure)
- About 40 open "Decide" items across `dev_docs/*-findings.md` (chat 10, discussions 14, menus and lists 5, chat cleanup 5, tags 3, fonts 2, logins 2): to sort so only the real calls reach you
- The branch: merge `refactor/harness-abstraction` into `main`, or move the meta harness to a repository of its own (to discuss)

## The rest

- Agents can reach harness settings and logins (left as is, 2026-10-25; to address some day)
    - An agent that writes the project's files can edit `.nah/harnesses.toml` (how each harness launches, where its login lives); one with a shell can read any harness's saved login, since it runs as the server's user
    - The same exposure as running a harness natively with access outside the project folder. Options when it's taken up: move per-machine settings out of the project; run agents as a separate, restricted user

- An agent's messages and a piece's label (left as is, 2026-10-26; from the chat cleanup's end review, its permissions report, items 3 and 4; in git history, commit 22ac268)
    - An agent's message to another agent arrives as a prompt. If it ends in a well-formed piece in the receiver's format, a replay shows it as chips with "content" laid out like what the server attached. Options: read pieces back only from messages the user sent, or check them against something an agent can't write
    - A piece's label carries the thing's raw subject (a terminal command's every line, another slot's session id) even when the reader can't read the thing. When it can't, the label could carry an opaque id

- Review the cockpit's overall look: visual aesthetic and theming (raised 2026-10-25)
    - Fonts: the default stays egui's own for now (Hack for code, Ubuntu Light for text, built in). Seven more are installable as font packs (`meta-harness pack install <name>`; `[fonts] mono` in `~/.config/nah/ui.toml`); comparison images in `dev_docs/fonts/`. JetBrains Mono is the likely default; built into the binary it's about 270 KB. Iosevka Term is 11 MB a face in the build we fetch (a slimmer one exists)
    - The screenshots show little difference between fonts on a sparse screen: compare them on a busy one (code, a long chat, the graph)
    - Look at the whole: colours, spacing, sizes, contrast, how panels, menus and cards read together; light theme too

- Agent asked to drive the graph view doesn't (seen 2026-10-24, Antigravity lead)
    - Asked to show something in the graph view, it read the graph as text through the explore tools (`view`, `navigate`: those names sound like moving the screen, and don't) before trying `cockpit_read`, which then waited on an approval nobody could see (its card was cut off: fixed by the chat plan)
    - Look at: the tool names and descriptions agents get (explore vs cockpit), whether "Let agents drive" was on, what the role's brief says about driving the cockpit

- Processes the server started can outlive it (found 2026-10-24)
    - 72 `claude auth login` processes had piled up over ten days, orphaned: a login command is a wrapper (`claude-agent-acp --cli auth login`) starting the real one, and only the wrapper was stopped. Fixed for a running server (the login runs in its own process group, stopped whole: dd6c5dd's follow-up)
    - Still open: when the server itself is killed (or exits without dropping its harnesses), what it started (agents, logins) is left running. Options: the server stops its harnesses' groups on the way out; on start, it stops groups a previous run left (recorded in its state); Linux's parent-death signal for the direct child

- The top bar's own buttons (left out of the menus-and-lists plan's step 3; to decide)
    - The panels' top lines show their menus; the top bar still has its own row of buttons, each with its keys: re-read the code, what's waiting, add and remove a panel, where the buttons of what you're on show, the inspector, issues, the action list, plus Menus
    - Decide: keep them, or fold them into menus (Panels, Index, Developer) the way a panel's top line was folded; the top bar's right side takes its room first, so its keys are said wherever there's room

- Selecting, copying and pasting: one way everywhere (to investigate; raised 2026-10-24)
    - Mouse selection is broken: it starts only a few letters in, often not at all, and drags one way only
    - Wanted: one selection mechanism for the whole cockpit, Vim-style keys and the mouse, with cut, copy and paste
    - Copying is done in a few lines each by the code view and the terminal (twice), and the sign-in's Copy link will do the same: one copy mechanism to decide with the rest

- Harness logins and homes (discussed 2026-10-23; to plan before building)
    - Decided: a harness's login belongs to the project server running it (credentials stay there; the UI server only relays the login steps to the browser)
    - Decided: first, make the browser logins solid for every harness (Claude Code: paste a code; Antigravity: browser callback, its failed last page pasted back and replayed on the server; Antigravity also needs a way to say signed in/out, it shows "unknown")
    - Decided: where each harness keeps its home (login, settings, session history) is a setting per project server: native (its own default folders, `~/.claude`, `~/.gemini`) or kept (a folder we choose, default `.nah/harness/<harness>/`, git-ignored, server user only; the path configurable). Default: native
    - Each harness pack declares how its home moves; the server applies it and knows no harness by name: Claude Code `CLAUDE_CONFIG_DIR`; Antigravity `GEMINI_HOME` plus `AGY_ACP_FORCE_FILE_STORAGE=1` (on macOS it otherwise uses the shared Keychain). A harness that can't move is native only, said in the cockpit
    - The cockpit shows, per harness: native or kept, the folder, signed in or not
    - Later: token logins as a third kind (a long-lived token or API key pasted in the cockpit); the sandbox's network must reach each provider's token endpoints (logins refresh from there); ties in with "Machines and project servers" below

- Search: one picker-source shape (links work, 2026-10-23)
    - The `#` picker (tags, ranked in the browser) and the `@` picker (files and symbols, asked of the server) build their results two ways; one interface each kind supplies matches through, ranked by the one matcher (frizbee) wherever the candidates live
    - Prefix characters for every linkable kind (symbols, terminal commands, chat turns, plan items, discussions), each a quick filter in one picker; needs a search source per kind. Part of the search rework below

- Buffers: revisit the whole model (found 2026-10-21, user's discussions run)
    - Twelve buffer kinds now (chat, code, graph, tree, spec, queue, inbox, changes, terminal, tags, plan, discussion). Too many; reconsider what a buffer kind is and whether some are views of one another (the plan buffer is a tree source; an item's card is the spec buffer; the discussion buffer shares the chat's message and typing parts)
    - Every buffer kind re-implements the same per-buffer hooks (place/restore, cursor motion, spots, enter/leave, actions, title line): about a third of each one. One scaffolding, not twelve copies
    - How a buffer is chosen can't stay a row of `Space b x` buttons across the top; the row is already two lines with "11 hidden". Needs a different picker

- Explicit review subagents implementers spawn, separate ones per category (NEED THIS ASAP bro)
    - Done by hand for now: each plan's reviewers, per `dev_docs/process/build-and-validate.md` and `reviewers.toml`; not yet something the meta harness runs itself
    - How faithfully is the design implemented (Critical one, even sub agents invent on non common use cases)
    - Anti patterns, also possibly using AST grep for global patterns?
    - Logic correctness
    - Custom linting one based on AST grep or similar tooling with custom rules built for design

- Missing location field on mcp
```

The MCP tools  add_component  and  update_component  do not expose a  location  parameter in their schemas, making 
it impossible to write or modify the  location  field in the registry using the first-class MCP tools.
```

- When oh when do we trigger auto-compaction does opencode expose this or do we need to wait to v2

- Arch Planning stage
    - Implement logic blocks as independent testible scripts
    - Have a path for visual design, do wireframes in .html artifacts for approval

- Implementation stage
    - Build skeleton of components and allow for a user check gate on just those changes and APIs before 
      fully implementing functionality 

- Cockpit code viewer: performance on giant files (tens of thousands of lines)
    - Colouring recomputes every line start of the whole file for each drawn line, every frame (`code.rs` `coloured` calls `line_starts`): compute once when the file loads, keep it
    - Visible lines (after folds) recomputed every frame: cache, redo on fold or load
    - The whole file, its names and references come in one answer (`/api/code/analysis`): page it, or send the text first and the rest after
    - Add a test with a ~30,000-line file that keeps a frame fast

- Admin screen: one place to see, prune and wipe everything the system keeps
    - Git snapshots (`refs/nah/snapshots/...`) and their timeline windows: how many, how big; prune by age or ended sessions; wipe
    - The code index and search index: size; rebuild or clear
    - The state database (notifications, saved sessions, review snapshots, the cockpit's saved layout...) and caches (layouts, diffs, language servers' data)
    - For each: what it is, how big, when last pruned; prune by a rule, or wipe
    - Open questions: do pruning rules also run on their own on a schedule? Does wiping ask to confirm?

- Search: re-evaluate, centralize, add filtering
    - Test the fuzzy finder's behaviour against the cases in mind (what it matches, ranks, misses)
    - One search for everything (symbols, files, lines, the panels' own items) instead of each surface its own
    - Filtering: by kind, by path, by component or plan...
    - While at it, generalize and clean up the architecture around the picker (the palette's choice list) as it relates to search and everything else it does: one picker that knows nothing about tags, slots or plans, fed by actions; triggers, marks and filters as data it is handed (user, 2026-10-21; the tags review's #41 does the first pass)
    - Prefix characters as quick filters in the general search: `#name` limits to tags, `@name` to a slot or person, and so on; typed at the start of the box, so the picker that `#` opens in a chat today becomes one case of this (user, 2026-10-21)

- Reconsider how sessions are held: today a session is loaded whole (ACP's session/load replays all of it; there's no paging) and every message is kept in the server's memory and in every cockpit connected
    - Option: keep the replay on disk in our own format, only the latest page in memory; older pages read from disk as the chat scrolls up
    - Option: read each harness's own session files (fragile: one reader per harness, breaks when their format changes)
    - Measure first: the server's and a browser's memory with a long session

- Touch on small screens: taps land on the wrong things
    - Seen in the browser's mobile emulation: a tap hits a different control than the one under the finger
    - Likely the positions worked out when the screen is resized or scaled are wrong
    - Check tap positions against what's drawn at phone sizes, and after resizing
    - A first touch that drags on the code view's text selects it rather than moving the code: egui gives a press to what was drawn there the frame before, and the view only learns the pointer is a finger from that touch (after any earlier touch it moves it: `CockpitApp::fingers`)
- Cockpit: make the buffer boundary a compiler rule, not a lint (step 5 of the buffers-one-protocol plan, skipped; the plan is in git history, commit 22ac268)
    - Split `cockpit-ui` into a protocol crate (traits, `Shared`, `Target`/`Place`, the shared services), a buffers crate (each buffer, fields private: only constructors and the traits public) and the shell
    - Buffers' fields are still `pub`; the shell can't reach them today only because it holds `Box<dyn Buffer>`, and the lints in `lint.rs` only match text
    - Shared types: private fields behind accessors where they guard a rule (`Shared`, `Known`, `RowData`, `Drawing` were added with public fields); ast-grep's `rust-no-public-struct-fields` flags them
    - Rewrite the shell tests that read buffer internals (~300 assertions) to check through keys, actions and the screen read
- Machines and project servers: link, start and reach them easily (discussed 2026-10-17, revisit)
    - More than one project per machine; a machine is a VM or a box on the LAN; project servers run in a VM/sandbox or directly on it
    - A manager per machine as a command run over SSH (`ssh box nah up <project>` → address and token as JSON), not a daemon: nothing listens or advertises; the system's `ssh` (your keys, config, agent, known hosts) rather than an embedded library
    - Once up, home links the project server directly as today: no extra hop; notifications, events and calls still go through home
    - Tokens per project server stay (enough on a LAN); beyond the LAN, home tunnels over SSH
    - Ties in with the Docker SBX sandbox scripts (Python now; keep or rewrite in Rust)
    - Open: its own package or part of the system; how home finds machines; the manager's language
    - Words (pinned): server = where a project runs; desk = the cockpit's workspace for one server; home = the server that serves the cockpit (docs and code only); drop "here" and name home's own desk by its project; on screen, switch desks by project name, "server" only for link/unlink/shut down
    - Who may connect and do what, rethought with this (discussed 2026-10-19; more to it than these notes)
        - Today: slots' keys only tell which slot is calling; anything reaching a server's agent tools without a key was "external" and could read (terminals, tags, sessions). Locked down for now (2026-10-19): keyless gets nothing unless `.nah/access.toml` grants it
        - An agent driving the cockpit acts with the user's full power (its tag changes recorded as the user's; it can run commands again, type into terminals) whatever its role allows
        - What one agent writes others read (a tag's note, `#name` in a message): channels between agents that their roles don't otherwise give
        - An agent's own tools run as the server's user: they read `.nah/state` (the tags' logs, terminal output tagged) whatever its role says
        - What the cockpit drives is growing (sessions, slots, terminals, servers placed elsewhere over SSH): who can reach each, and how that's proven beyond a LAN

- Reinvented code: find where a crate would do better
    - Go through the cockpit (code view and its highlighting, the graph's drawing and layout, scrolling, the palette and fuzzy matching, the request forms, the markdown) and the server, and list what's hand-built that a maintained crate does
    - For each: the crate, what it replaces, what it costs (size, wasm support, control we'd lose), and whether to switch

- Talking to the agent (`t`): decide what it shows (since 2026-10-17 it's the lead's whole chat in the quick look, typing)
    - The point: a quick message from anywhere ("go to X", "show me Y") without seeing the chat; the reply often doesn't matter
    - Options: just the field (send and it closes), the field and the last reply, or the whole chat (now); maybe the reply only when asked
    - Keep it one mechanism: a buffer in the quick look (a chat drawn small, or the chat's own "field only" look), not a box of its own again
    - The quick look may need settings per use (where it sits, how big: a one-line field at the bottom vs a card beside the panel)

- Palette: same behaviour, clean up how it reaches buffers
    - It stays its own component and input mode (a picker over everything: commands, searches, values); not a buffer
    - The shell polls every buffer each frame for `take_palette_request()`: make it an ask (open the palette on this action)
    - The palette calls `view.search(...)` on a buffer directly (`take_search`): go through the buffer's actions/asks like everything else
    - Check what else it reads or changes inside buffers or the shell, and whether its field takes the keyboard on its own

- Tags for navigating (later; the tags core is built: the terminal-view-and-tags plan is in git history, commit 22ac268)
    - Some tags are for moving around, kept for a long time, not references: jumping to one shouldn't always need the fuzzy picker
    - Something you can see and reach in a key or two, so you don't have to remember them (in the style of ThePrimeagen's Harpoon)
    - Decide how a navigation tag differs from a reference tag (a kind, a pin, an order)

- Keeping state beyond this machine (later; discussed 2026-10-20, put aside to keep the discussions plan's scope)
    - Today everything under `.nah/state/` (the database, discussions, tagged output) is ignored by git and stays on this machine; plans stay committed
    - Idea: export and import state through adapters, chosen per kind of state: a repository as one adapter (plain files that make sense without the database), a tracker such as Linear as another
    - The database file itself can't be committed (binary, written while the server runs, can't be merged, holds this machine's own data)
- Project servers sharing with each other (later)
    - Tags (and other things) are a project server's: one server can't assume it reaches another's files
    - A way for one project server to ask another for something and sync it: a mailbox, backed by what the setup allows (a shared mounted volume, or a small cloud bucket with the servers we already run)
    - Ties in with "Machines and project servers" above

- The user's decision profile, kept and used by the meta harness (raised 2026-10-25)
    - Today it's a hand-written file (`dev_docs/process/decision-heuristics.md`) the agent is told to read; nothing makes sure it's read, kept current, or applied
    - Make it programmatic: record each decision the user makes (the "Decide" items and their answers, corrections in chat) as structured entries; distill them into a profile the harness keeps per user/project
    - Hand it to every session the harness drives (its prompt or a tool to ask), especially for unattended plan runs; have reviewers check open decisions against it; flag decisions made against it for the end summary
    - Keep it current: new answers update it; a decision the user overturns updates it

- Tests: cut them back to what earns its keep (getting urgent)
    - About 25,000 lines of tests against about 66,000 of code (rough count, 2026-10-17), and every agent build adds more
    - Many likely test themselves (tautological), check wording instead of behaviour, or pin internals that the shell tests reach into (~300 assertions)
    - Go through them: keep tests of behaviour a user or an agent relies on, merge overlapping ones, delete the rest; say what each kept test protects
    - Then a rule for new work: a test must name the behaviour it protects; reviewers (the test-quality agent) check it
    - Evidence (chat cleanup's end review, 2026-10-26): tests that pass without the fix they claim to protect, tests that only test themselves. When the same agent writes the code and its test, the test tends to confirm what was built: have someone other than the builder write them, and enforce rules with types and lints instead of new tests
    - The sandbox's 20 GB disk: `make sandbox-clean` frees it (2026-10-26)

- Rework the work queue and validation (later; needs a design discussion)
    - The queue was designed for earlier, smaller models; what it does per component may not fit now
    - Validation component by component with a sub-agent each: narrow agents miss the forest for the trees, and token use gets extreme
    - Maybe one reviewing agent (or none beyond the driving agent) per change set, at a plan's stopping points, instead of per component; when to fork a session and what to hand it
    - Validation as part of the system: reviewers that run on a set of changes at a plan's checkpoints; a plan's steps and checkpoints as something a harness drives
    - Reuse what this build used by hand: the reviewer briefs (in `.nah/reviewers.toml`'s shape) and the build-and-validate process (`dev_docs/process/build-and-validate.md`)

- Replace `ttf-parser` in `arch-graph` (its text measuring, `tree/measure.rs`): unmaintained (RUSTSEC-2026-0192), accepted in `meta-harness/deny.toml` until then (2026-10-26)

- A flaky test: `arch-service` `gates.rs` (a content-identical touch) failed once under the whole suite's parallel load (2026-10-24, its trigger setup at line 69: likely the database locked by another test) and passed three runs out of three alone
- Another: `nah-terminal`'s library tests failed once under the whole suite while the cockpit's tests built and ran beside them (2026-10-24, which test wasn't kept: the suite prints only the last lines), then passed six runs out of six alone. Rerun the suite with the full output kept to name it
    - Find what it shares with the tests beside it (a database file, a folder) and give it its own

# Finished

- Slots say "no session running" wherever they're named: the chat, discussions ("waiting on lead: no session running", with Start / Resume), the inbox, the plan tree (chat cleanup, 2026-10-26). Sessions don't reopen on their own: you resume them (decided 2026-10-25)

- Chat items drifting left, off the panel: a markdown table widened the column (chat plan, step 1). Each item is laid out in the column's width, what can't wrap scrolls sideways inside it, and every panel clips to itself

- Your messages in the chat history: shown as typed, links as chips, what was handed on folded under them; read back from the harness's own replay (chat cleanup, Part 1, 2026-10-26)

- Graph: opening some cards in the calls view showed nothing new (fixed: the server recounts what each card has left to open; `l` lands on the first card)

- Issue with dicts not being hashable
```
Bug Report: TypeError: unhashable type: 'dict' in Architecture Registry
- Symptom: Calling get_next_actionable_components or visualize_architecture(format="review_markdown") fails with TypeError: unhashable type: 'dict'.
- Root Cause: In system_architecture.json, component definitions store modification_tasks as a list of dictionaries (e.g., [{"task": "...", "completed": false}]). The tool's topological sorting and graph-traversal logic attempts to insert these component dictionaries directly into a Python set or use them as dict keys. Since Python dict and list types are mutable, they are unhashable, triggering the runtime exception.
- Impacted Components: Any component containing structured modification_tasks or complex properties.
- Immediate Workaround: Convert all modification_tasks in the registry file to a flat list of primitive strings (which are immutable and hashable).
```

```
When you call add_usage_node with parent_node_id, the tool's implementation physicalizes this relationship by pushing the entire child node dictionary inside the parent's "dependencies" array in the JSON file. It does not use flat ID-based references for usage relationships.
However, other query commands in the same tool (like get_next_actionable_components) are buggy: they are not written to correctly traverse or flatten this "dependencies" list. When they traverse it, they try to add those nested dictionary nodes directly into a Python set, triggering the unhashable type: 'dict' crash.
Summary of the Situation
1. No Manual Editing: Nobody edited the registry incorrectly; the nested JSON structure was generated entirely by the add_usage_node tool.
2. The Tool's Conflict: The tool's write command (add_usage_node) structures relationships via nested array dictionaries, but the tool's read commands (get_next_actionable_components and review_markdown visualizer) crash when trying to traverse those nested dictionary nodes.
The Clear Path Forward
By registering each of our call sites as separate parallel roots (without passing a parent_node_id), the tool writes them as flat, independent, depth-0 entries. This completely avoids the buggy recursive parsing branch, unbreaks the tool immediately, and keeps our architecture 100% clean and documented. 
I am in Plan Mode (Read-Only). Please let me know if you would like to proceed with flattening these trees!
```

- Further biasing needed to make sure orch. agent never approves arch without explicit user request to do so
agent must pause once arch defined and validates
```
I was trying to move quickly to progress the workflow, but in doing so, I bypassed the strict instructions to present 
the contract details to you and obtain your explicit sign- off first.

```

- Need to have a way to prevent the planning and impl. agents from needed to go over existing
components that are not going to be changing, skip existing components with no change to implemented


- Right now model can wonder off the workflow defined in the skill, use skill to only bootstrap 
make tools provide continues guidance


- The requirement to capture further detail as specs seems unclear to model sometimes, stronger prompting
and some sort of programmatic gates needed


- On occasion model doesn't by itself get that it needs to capture the existing code "touch points" where new
functionality will integrate into existing

- Arch review stage
    - Review planned arch. in visual navigator that is driven by chat ui



## Possible enhancements (not decided either way)

- Edit a chat message in Vim, through the terminal (raised 2026-10-25)
    - "Edit in Vim": the draft saved to a file on the server, your own `vim` (`$EDITOR`) opened on it in the pop-up terminal, the text put back in the chat box when you quit. Every Vim command and your config, no editor of our own. Links (`#tag`, `@file`) are plain text while in Vim, made links again when it comes back
    - Smaller, for fixing a dictated word: a few Vim moves in the chat box itself (by word, start/end, delete or change a word); partial, so it would always feel incomplete

- A QR code beside a sign-in's link, so a phone or iPad opens it by scanning, with nothing to copy (raised 2026-10-24)

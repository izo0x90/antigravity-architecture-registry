## Consistency review, 772aa54..0e24374

I read the code only; nothing was built or run.

**1. A question an agent asks the user can never be settled from the cockpit. Severity: high.**
- The engine allows the user to settle an open discussion that asks them (`crates/nah-discussions/src/lib.rs:467-468`, `(_, Open) if d.asked == USER => Ok(())`).
- The discussion buffer only offers Settle when the discussion is Answered (`crates/cockpit-ui/src/discussion.rs:424-429`, `when(if answered …)`).
- How to trigger it:
  1. An agent calls `discuss` with `ask: "user"`.
  2. The inbox says "lead asks you on …". The user opens it and finds Settle greyed out.
  3. The user replies. Now it's the agent's move (`whose_move`, `harness-contracts/src/discussions.rs:141`).
  4. The agent can't `discussion_answer`, because only the one asked answers (`lib.rs` answer check). It can only reply, which makes it the user's move again.
- This repeats forever. On a plan component it also blocks approval (`src/startup.rs` `approvals_ask`). No cockpit test covers a discussion asked of the user.

**2. Keys can't open the discussions on the thing at the cursor. Severity: medium-high for keyboard use.**
- In code, terminal, chat and graph views, the only way in is clicking the gutter mark (`crates/cockpit-ui/src/app/notes.rs:330-345`). That click calls `land()` directly instead of running an action.
- `discuss.open` only exists after a click on a mark with several discussions (`app/starting.rs:39`). `keys.txt` has no key for opening one.
- Tags already do this through actions: `g T`, `] t` / `[ t`. Here the agreed rule that "every control runs the same path as its key" is broken.
- To confirm: in the code view, put the cursor on a line that has a discussion. Nothing in the palette or key bar opens it.
- Fix: offer `discuss.open` whenever `NotesHere.discussions` is not empty, bind it in `default.toml`, and have the mark click run it.

**3. Search hand-writes a matcher that `arch-search` already has. Severity: medium-low.**
- `nah_things::logs::matches` (`crates/nah-things/src/logs.rs:59`) does "every word, case heeded when capitals". `arch_search::rank_text` (`crates/arch-search/src/lib.rs:82`) already does this, including the word operators, and the cockpit search uses it (`cockpit-ui/src/nav.rs:678`).
- Both `logs.rs:356` and `nah-things/src/lib.rs:510` use the hand-written one, so `tag_find` and `Discussions::find` read a query like `!flaky` or a quoted phrase differently from every other search.
- Also, the FFF grep runs with an effectively unlimited match limit (`usize::MAX >> 1`, `logs.rs:353`) over the whole logs folder. It filters to the asked-for files only afterwards, so every hit goes into memory.

**4. The palette's Send is a plain `ui.button`, not an action. Severity: low.**
- `palette.rs:285`. The agreed pattern is that send buttons are normal actions drawn by `actions::control`, as the chat (`chat/actions.rs:264`) and the discussion buffer (`discussion.rs:419`, `.on_typing(Button, "Send")`) do.
- As a result it isn't in the `?` sheet, doesn't go through `control()`, and its button shows no key.

**5. `discussion.cancel` ("Stop") copies what the back key already does. Severity: low.**
- `discussion.rs:420`. In Insert mode the keymap's back key already leaves the box (`app/keys.rs`, Insert branch).
- The action has no binding, so its control shows no key. The chat has no such action, so the two typing boxes behave differently.

**6. A new hand-written time parser where maintained crates exist. Severity: low.**
- `arch_graph::when::read` (`crates/arch-graph/src/when.rs:36`) splits the seconds on `+`/`-` and drops the offset: `12:00:00+02:00` reads as 12:00 UTC. It also accepts month 13 or day 45.
- `time`, `jiff` and `chrono` are already in `Cargo.lock` as indirect dependencies. To confirm whether one can be made a direct dependency at no extra cost: `cargo tree -i time`.
- It's only used to bring old plan comments over (`src/plan_comments.rs:109-114`), and those times were written in `Z`, so today's impact is small.

**7. Copies instead of shared code. Severity: low (upkeep).**
- `src/routes/discussions.rs:36-56` (`Refused`, `blocking`) is a line-for-line copy of `src/routes/tags.rs:31-52`.
- "user" is defined twice: `nah_discussions::USER` and `routes/tags.rs:61` `USER`.
- A discussion's kind is parsed three ways: through serde in `app/starting.rs:79`, by hand in `src/mcp/sets/discussions.rs:203-207`, and through serde in the route.
- The cockpit acts on discussions with string action names (`net.discussion_act(id, "settle", json!({}))`, `discussion.rs:136,447,450`). Tags use a typed enum (`TagAct`, `tags.rs:19`).

**Checked, nothing found:**
- Key bindings and how keys are said: `C`, `Space k r/c/n/x/d/o` and the per-view `discussion.*` keys are all in `default.toml`, and nothing names a key outside the keymap.
- How the API reports errors (same shape as tags).
- `.nah/terminals.toml` (`running_log_mb` follows the existing settings pattern).
- The event stream and Resync handling (`discussions.rs` matches `tags.rs` and `terminals`; the hub replays by key).
- Asks and what buffers share: `Shared::discussions` is the one source, and the review and plan tree are built from it.
- The gutter's sizes come from the theme.

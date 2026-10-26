# End review: consistency

Scope: `git diff 3e1a805^..HEAD` (18 commits). Paths are relative to
`meta-harness/`. Ranked, most important first.

## 1. An agent's state doubles its own words when it can't run

- `crates/cockpit-ui/src/agents.rs:47` writes `CantRun(why)` as
  `"can't run: {why}"`.
- `crates/cockpit-ui/src/agents.rs:364` (`harness_problem`) already
  builds `why` as `"{display_name} {status_words}"`, and
  `status_words` (agents.rs:198) says `"can't run: {reason}"`.
- Result on screen: "can't run: Antigravity can't run: …". It shows in
  the top bar's hint (`app/draw/top_bar.rs:208`, "Lead: can't run:
  Antigravity can't run: …"), the chat's banner
  (`chat/render.rs:375`, "Antigravity: can't run: Antigravity can't run:
  ….", a second period too), the live line (`chat/live.rs:66`), the inbox
  line (`attention.rs:61`) and a discussion's status (`agents.rs:354`).
- `CantRun` also takes the `errors` map's full sentences
  ("No harness instance is available yet."), which come out as "can't
  run: No harness instance is available yet." (capital mid-line, period
  inside a phrase).
- Pattern to follow: one place writes the words. Keep `CantRun` holding
  only the reason (`reason`, `message`, the error), and let `words()`
  add "can't run:" once, as `status_words` does for an instance.

## 2. `AgentState` bypasses the status vocabulary

- `crates/cockpit-ui/src/agents.rs:44-66` hard-codes each state's word
  and `Tone`. `app/draw/top_bar.rs:204-207` then overrides the tone's
  colour for `NoSession`/`Gone` with `colors().faint`.
- The same change put sub-agent states in `[status.subagent_*]`
  (`config/default.toml:641-669`) and reads them through
  `status::look` / `status::chip` (`chat/subagents.rs`, `chat/render.rs`).
- Pattern to follow: `status.rs` header: "every state the cockpit shows
  has one icon, one word and one theme colour, from the `[status]`
  section". Add `[status.agent_*]` entries and a `status::agent(&AgentState)`
  name function, as `status::subagent` and `status::job` do. The top bar
  then reads the colour from there, not a second match.

## 3. The overlay stack hand-builds what egui 0.33 already has

- `crates/cockpit-ui/src/overlay.rs` (whole file) and
  `app/overlays.rs` filter `RawInput` to swallow taps outside, keep a
  stack order, and block keys under a "modal" one.
- egui 0.33.3 (the version in `Cargo.lock`) ships `egui::Modal`: a custom
  `Area` is allowed (`Modal::area`), the backdrop colour can be
  transparent, the backdrop takes the outside tap so it never reaches
  what's under it, `Memory::set_modal_layer` / `top_modal_layer` keep the
  most recently shown one on top and block input to layers under it, and
  `ModalResponse::should_close` covers backdrop tap and Escape. Non-modal
  ones that let the tap through map to `egui::Popup` with
  `PopupCloseBehavior::CloseOnClickOutside`.
- The code already relies on egui's own for one case:
  `app/overlays.rs:66` skips the stack while a setting's choices are open
  ("it closes itself on a tap outside it (egui's own)"). So two
  close-on-outside mechanisms now coexist.
- At minimum, the menus, the action list, the issues window and the
  feedback window (modal, `Outside::Closes`) should be checked against
  `egui::Modal` before keeping the raw-input filter.

## 4. Pack placeholders: a third `{name}` parser

- `crates/harness-acp/src/pieces.rs:69-110` (`Template::parse`,
  `Template::ours`) parses `{uri}` / `{content}` by hand.
- Existing owners of the same `{name}` grammar:
  `crates/nah-packs/src/template.rs` (`names`, `check`, `expand`, with
  stray-brace errors at load time) and, in the same crate,
  `crates/harness-acp/src/settings.rs:388` (`Places::fill`).
- Also inside `pieces.rs`: `Template::ours()` (line 102) splits `OURS`
  by hand instead of running it through `Template::parse(OURS, true)`,
  so our own format skips the checks every pack format gets.
- Pattern to follow: one placeholder parser. Either expose
  `nah_packs::template::parse` (its parts list) to harness-acp's
  settings, or move the grammar into `harness-contracts` and use it in
  all three places.

## 5. A piece's label hand-rolls percent-encoding and query parsing

- `crates/harness-contracts/src/links.rs:146-213` (`read_label`,
  `escaped`, `unescaped`) builds a `nah://…?at=…&len=…` URL by hand:
  its own `%XX` escaper, its own unescaper, its own `&`/`=` split.
- The workspace already depends on maintained crates for this:
  `urlencoding` (cockpit-ui, used by `net.rs` for the sub-agent id) and
  `url` 2.5 (harness-acp; brings `percent-encoding` and
  `form_urlencoded`).
- Pattern to follow: `percent_encoding::utf8_percent_encode` with an
  `AsciiSet` for writing, and `form_urlencoded::parse` (or
  `url::Url::parse(..).query_pairs()`) on the label slice once its end is
  found. Keep only the "find the end by `len=` digits" step custom.

## 6. A new private `now_ms` beside the one this change made public

- `crates/harness-core/src/lib.rs:24` adds `pub fn now_ms()` in this
  change.
- `crates/harness-acp/src/state/subagents.rs:41` adds a private copy in
  the same change; harness-acp already depends on harness-core.
  (`harness-acp/src/auth.rs:248` is an older copy of the same.)
- Pattern to follow: `harness_core::now_ms()`.

## 7. Touch sizes and widths are hand constants, not theme sizes

- `crates/cockpit-ui/src/actions.rs:801` `ENTRY_FINGER = 40.0`.
- `app/draw/bars.rs:113` and `:118` use `32.0` for the key bar's tap
  size (the plan says the menus should match the key bar; they now
  differ: 40 vs 32).
- `app/draw/which_key.rs:89` `ENTRY_W = 200.0`;
  `app/draw/bars.rs:170` `(width - 64.0).min(420.0)`;
  `app/draw/bars.rs:9` `CHIP_SHARE = 1/3`.
- `app/draw/host.rs:68-71` computes `room_under_waiting` as
  `interact_size.y + space_s * 3.0`, a copy of the floating controls'
  layout arithmetic.
- Pattern to follow: sizes live in `[look]` (`config/default.toml:385`,
  the `look!` macro in `theme.rs`), as `quick_look_width` and
  `gutter_*` do. One `touch_target` token would serve the key bar and
  the entries alike.

## 8. Two signals for "touch" and a new enum for a bool that exists

- The menus decide finger height from `self.fingers`
  (`app/draw/which_key.rs`, `app/draw/bars.rs:162`); the key bar decides
  from `narrow` (window width, `[keymap.bar] narrow_below`). Same
  purpose, two signals.
- `actions.rs:795` adds `enum Fingers { Yes, No }`, converted from the
  `bool` at each call site. `buffer::Drawing::fingers`,
  `CockpitApp::fingers` and `code.rs` all carry it as `bool`.
- Pattern to follow: pass the existing `fingers: bool`, and use it for
  the key bar too.

## 9. Two ways to say "starting / resuming a session"

- Chat: "New session" with `icon::LIGHTNING`; "Resume the last" with
  `icon::ARROW_COUNTER_CLOCKWISE` (`chat/actions.rs:88`, `:91`).
- Discussion: "Start its session" / "Resume its session", both with
  `icon::PLAY` (`discussion.rs:467`).
- The discussion's label names the slot by id (`format!("Resume
  {slot}'s session")`), while the status beside it names it by
  `d.name_of(&slot)` (`agents.rs:353`).
- Pattern to follow: one caption and icon per act. Use the chat's
  ("Resume the last", arrow-counter-clockwise; "New session",
  lightning) and the slot's display name.

## 10. Two phrasings of the same sub-agent count

- `chat/subagents.rs:21` `working_words`: "1 sub-agent working" (strip,
  live line, panel top line hover).
- `agents.rs:45-46` `AgentState::words`: "working · 1 sub-agent" (top
  bar, discussions, pickers).
- Pattern to follow: one helper; `AgentState::words` can call
  `working_words`, or the reverse.

## 11. `items_changed` built three ways

- The change adds `nav::Changes` (a counter bumped where items are taken
  in) and uses it in inbox, queue, graph, tree, code and terminal.
- `discussion.rs:349-354` hashes `(version, id)` with `DefaultHasher`
  instead.
- `chat/nav.rs:440` mixes `epoch * 2^40 ^ activity` by hand.
- Pattern to follow: `nav::Changes`, bumped when the shown discussion or
  its version changes, and in the chat when epoch or activity moves.

## 12. Doc comments left on the wrong function

- `discussion.rs:347` "What's said, to search." now sits on
  `items_changed`; `items` lost its doc.
- `tag_list.rs:431` "Every tag and every thing it's on…" likewise.
- `chat/nav.rs:437-439`: two stacked docs on `items_changed`.
- `config.rs:126` `ChatUi` still says the indicator is used "till it's
  picked there"; it isn't picked per chat any more (Part 2d).

## 13. The SHA-256 hex helper hand-builds what `sha2` formats

- `crates/nah-packs/src/lib.rs:20-28` adds `hex()` (a `{b:02x}` loop).
- `sha2` 0.10's digest output implements `LowerHex`:
  `format!("{:x}", Sha256::digest(bytes))` is its documented idiom. That
  replaces `sha256_hex` and the download checksum in `engine.rs:422`.
  Only the 8-byte cache key (`engine.rs:573`) needs a slice formatted,
  which doesn't justify a public `hex()` in the crate's API.
- `crates/code-analyzer/src/extract.rs:209-212` still has its own loop,
  so plan item 9 ("one shared helper") leaves two.

## 14. Key and id placement

- `look.at_work` (`config/default.toml:159`) starts a new `look.`
  namespace with one member, bound under the `l` = "Panels" menu. The
  only other cockpit-wide look setting, `panel.item_buttons`, uses the
  `panel.` id. Either name it `panel.at_work`, or add a "Look" group
  label for it.
- `chat.stop_subagent` is bound under the Session menu (`s x`) beside
  `session.*` ids; its sibling `chat.subagent_start` uses a `g` key
  (`g w`), where `g` keys are otherwise `goto.*` or `tag.goto`. Fine
  alone, but the `chat.` prefix under `s` breaks the "Session menu holds
  `session.*`" pattern.
- `discussion.wake` has no key, while every other discussion action has
  one (`discussion.reply` r, `.settle` a, `.reopen` x, `.change` e).
- `session.resume_last` has no key; `session.resume` is `s r`.

## 15. Test helpers duplicated

- `app/tests/subagents.rs:13` `fn session` copies
  `chat/tests.rs:39` `fn session` (make that one `pub(crate)`, as
  `chat::tests::item` already is).
- `app/tests/overlays.rs:9` `fn tap` is `app/tests.rs:89` `fn click`
  with a time argument; add the time to `click` (or a `click_at`) rather
  than a second helper.
- The `Subagent { .. }` literal is written out in full at `agents.rs:673`,
  `chat/tests.rs:524` and `app/tests/subagents.rs:19`. One fixture
  builder would follow `chat::tests::lead` / `item`.

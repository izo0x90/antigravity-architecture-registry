# Plan: what agents ask, the keys, and slots in the chat

Four changes, one rule each. Order: R (requests) first, then K (keys), then
L (lint), then S (chat slots). Each step is green on its own and committed.

## Findings (from the code)

- **Requests are classified by transport, not by what's asked.**
  `harness-acp` turns an ACP `session/request_permission` into a request
  whose kind comes only from the tool call's kind (`state.rs::request_kind`):
  execute → Command, edit/delete/move → FileChange, read → FileRead,
  anything else → **Permission**. Forms (`elicitation/form`) become
  UserInput; links (`elicitation/url`) McpElicitation.
- Antigravity asks plain questions through `request_permission`, with its own
  answer labels, every option of kind `allow_once`. So a question shows as
  "PERMISSION REQUIRED", with green "accept" buttons, and every answer
  claims `y`.
- **Questions can be answered without the user.** `profile.auto_approve`
  picks the first `allow_once` option of any permission request when the
  slot's mode is Full access or Auto. A question asked that way is answered
  "yes" without being shown. That breaks "questions always come to the user".
- The card (`request_card.rs`) has one heading per transport kind and draws
  every option as a green/red button. The inbox draws the card, then draws
  each answer again as its own row (for its cursor): the duplicates you see.
- **Keys read outside the keymap**: the palette (arrows, Ctrl+P/N, Enter),
  every typing box (Enter sends, Shift+Enter newline; Ctrl+Enter adds to the
  running turn in the chat; Enter in the code view's path box), the talk
  box's Escape (not the configured back key), Cmd/Ctrl+click to go to a
  definition. Nothing checks for this.
- The chat has no key for its slots; the slot picker orders slots by id.

## R. One classification of what's asked, one card

R1. **Contracts** (`harness-contracts/src/requests.rs`). Replace
`RequestKind` with what's asked, not how it came:

```rust
pub enum RequestKind {
    /// Allow a tool call or not: what it would do.
    Approval { action: ToolAction },          // Command | FileRead | FileChange | Tool
    /// A question: its answers are `options` (one picks), or `questions` (a form).
    Question,
    /// A page to open (then Done).
    Link,
}
```

`ApprovalDecision` gains `Answer`: an option that is one of a question's
answers (not allow/deny). Clean cut: every use updated in the same commit.

R2. **Each harness pack says how its questions arrive** (data, not code).
Antigravity's ACP server (Google's, closed; 1.1.1 and 1.3.0 alike) sends
its questions as `request_permission` on a made-up tool call whose id starts
`interaction_`, every answer `allow_once` (`_ask_user_handler`). Its pack:

```toml
[provides.settings.questions]
permission_tool_prefix = "interaction_"   # those permission requests are questions
free_answer = false                       # only its listed answers can go back
```

Claude Code declares nothing: its questions come as forms (elicitation).
Parsed into `HarnessSettings` like the pack's other settings.

R3. **One classifier in `harness-acp`** (`classify.rs`, pure, built from the
pack's settings, tested): form → Question (free answer and Skip as the form
allows); URL → Link; a permission request on a declared question tool call →
Question (its options are `Answer`s; one named deny/block stays `Decline`),
no free answer; a permission request with an allow/deny set (at least one
reject, no two of one kind) → Approval, its action from the tool kind;
anything else → Question, logged once per harness (`<harness> asked through
a permission request without a deny: shown as a question`). `request_kind`
and the scattered kind choices in `adapter.rs` go.

R3b. **Only approvals are auto-approved.** `auto_approve` runs after
classification, only on `Approval`; a question always waits for the user.

R3c. **Answers the transport can carry.** A question that came as a
permission request goes back only as one of its options (the adapter
refuses anything else); the card offers no free text or Skip for it (only
forms have those). Not answering is stopping the turn, which sends ACP's
`session/cancel`. No new fields were needed.

R3d. **Antigravity's pack moves to 1.3.0** (Google renamed its downloads:
`agy-acp-server-<version>-<platform>.zip`); `supports` pins the rule to the
versions it was checked on; its contract test replays a recorded 1.3.0
question and approval.

R4. **Notifications follow** (`nah-notify/rules.rs`): Approval → Approval,
Question → Question, Link → Link; nothing else maps kinds.

R5. **One card** (split `request_card.rs` into `request_card/{mod.rs,
answers.rs, draw.rs}`):
- `answers.rs`: `Drafts` and `answers(request)`: the actions and their keys
  (y/Y/A/d only for a kind that's the only one of its kind; `Answer`s are
  always numbered: `r 1`, `r 2`…).
- `draw.rs`: draws by kind. Approval: "Wants to run a command / change a
  file / read a file / use a tool", the command or file, Allow (ok colour)
  and Deny (error colour). Question: the question, its answers as choices
  (neutral, numbered), its form fields when it has a form, a free answer
  when allowed. Link: the page, Open, Done. Each answer shows its key
  (from the bound keys, passed in, never written in the card).
- The card returns its rows (each answer's rect) and the one clicked. The
  inbox's cursor moves over the card's own rows; its duplicate answer rows
  go. The chat draws the same card.

## K. Every key from the keymap

K1. **Keymap sections** (`keymap.rs`, `config/default.toml`), parsed as
`KeySeq`s like every binding:

```toml
[keymap.list]      # every list: the palette, its pickers
next = ["ctrl+j", "ctrl+n", "ArrowDown"]
prev = ["ctrl+k", "ctrl+p", "ArrowUp"]
pick = ["Enter"]

[keymap.input]     # every typing box
send = ["Enter"]            # Shift+Enter stays a new line (the text box's own)
add = ["ctrl+Enter"]        # the chat: into the running turn

[keymap.pointer]
go_to = "ctrl"              # held while clicking a name: go to it
```

K2. **One way to read them**: `KeymapConfig::list()`, `::input()`,
`::pointer()` with `pressed(&InputState)`; set with the rest of the config
the way the theme's look is (`keymap::set_input`), so views read them without
threading the config through. The palette, chat, talk box, spec comment box,
code path box and code clicks use them; the talk box uses `back`.

K3. Hints in those places name the configured keys (“Enter sends” comes from
the binding's label).

## L. Nothing reads keys behind the keymap's back

A test (beside the colour lint in `theme.rs`) scans the cockpit's sources:
`key_pressed(egui::Key::`, `key_released(egui::Key::`, `modifiers.ctrl`,
`modifiers.command`, `modifiers.shift` appear only in `keymap.rs` (and
`remote.rs`'s key-name parser, which turns an agent's “ctrl+enter” into
input, listed by name). A new hard-coded key fails the build.

## S. J/K through the slots in a chat

S1. **One slot order** (`chat/slots.rs`, moved out of `actions.rs`): the
lead, then sub-slots, then queue workers, each by name. The picker and J/K
use it.

S2. **`chat.next_slot` / `chat.prev_slot`** (J / K, the chat's own, like the
inbox's): the view shows the next slot in that order; stops at either end
and says so; only this chat view moves (other chat views keep theirs).

## Tests

- classify: allow/deny set → Approval per tool kind; a declared
  `interaction_` permission → Question with no free answer; an undeclared
  permission without a deny → Question (logged); forms → Question; URLs →
  Link.
- Full access: a question isn't answered for the user; an approval is.
- Not answering an Antigravity question stops the turn (`session/cancel`
  sent), never a bare “cancelled”.
- The card: an approval's Allow/Deny; a question's numbered answers, no `y`;
  the inbox cursor on the card's own rows, no second list.
- Keys: Ctrl+J/K move in the palette; a config that rebinds `list.next`
  works; the chat's send key comes from `input.send`.
- The lint.
- J/K in a chat: next/previous slot in order, ends said, other views kept.
- Live: an Antigravity question shows as a question (not permission), its
  answers numbered; a Claude command approval shows Allow/Deny.

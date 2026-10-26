# Review of ee6d6d3..HEAD (sign-in flow), tests

Paths are under `meta-harness/crates/`. Most severe first.

## 1. The "a paste is never saved as a draft" assertion can't fail

`cockpit-ui/src/chat/tests.rs:444-447`. `report_unsent` only reports when
`unsent_for == Some(slot)`, and only `take_unsent` sets that, after the
server's drafts have been read. The test never calls `take_unsent` and never
loads any drafts, so `unsent_for` stays `None`. `report_unsent` then pushes
nothing whether or not the new `set_aside` guard (`chat/nav.rs:71-74`) is
there. Delete the guard and the test still passes. So nothing protects the
plan's rule that a pasted code is never saved as the slot's draft. To fix it,
load a saved draft for the slot (so `take_unsent` claims it) and run the
chat's `render` order, not the helpers one by one.

## 2. The test calls `tend_sign_in_box` directly, so the real frame order goes untested, and that order loses drafts

`cockpit-ui/src/chat/tests.rs:442,450` calls the private `tend_sign_in_box`
by hand. `report_unsent` was made `pub(super)` (`chat/nav.rs:70`) only so the
test can reach it. In the real frame, `render` runs
`tend_sign_in_box -> take_unsent -> draw -> report_unsent` (`chat/nav.rs:84-87`),
and no test runs that sequence during a sign-in. Two cases go wrong with it:

- **A sign-in is already waiting when the cockpit opens** (or the slot is
  shown for the first time).
  1. Frame 1 sets aside an empty box.
  2. When the drafts arrive, `take_unsent` sees an empty box and restores the
     saved agent draft into the sign-in's box.
  3. Enter now sends that draft to the server as the sign-in answer
     (`Act::FinishSignIn`).
  4. When the sign-in ends, `tend_sign_in_box` takes the draft out and
     restores the empty set-aside.
  5. `report_unsent` then reports an empty draft, and the server erases the
     saved one.
- **Switching slots while a draft is set aside.** The first branch of
  `tend_sign_in_box` (`chat.rs:256-260`) clears the box and drops the
  set-aside. The new slot's `take_unsent` then restores its saved draft into
  a box that may again be a sign-in's (same harness). The same mix-up
  follows.

No test covers either case.

## 3. The new login test fails on macOS and uses a fixed 200 ms wait

`harness-acp/src/login.rs:166-179`, `what_a_login_command_started_goes_with_it`:

- It has no `#[cfg(target_os = "linux")]`. Its `alive` check reads
  `/proc/{pid}`, which doesn't exist on macOS. There `alive` is always false,
  so `assert!(alive(pid))` fails at once. The sibling test
  `harness-acp/tests/process_tree.rs:13` is gated to Linux for exactly this
  reason.
- After the drop it sleeps a fixed 200 ms, then asserts the process is dead.
  On a loaded machine the kill can take longer than that, and the test fails
  for no real reason. `process_tree.rs` polls with a 10 s limit; this test
  should do the same.
- It also copies the `alive` helper from `process_tree.rs` (a duplicate).

## 4. "Each key runs its action" is not tested; the test restates the TOML instead

`cockpit-ui/src/chat/tests.rs:413-417` reads `keymap.bind[id].leader` and
compares it with the same strings as `config/default.toml`. That is the config
file copied into a test. It proves no key press reaches any action, and it
has to be edited every time a key is rebound. The plan's step-1 test is "each
key runs its action". A harness that does exactly that already exists:
`every_key_shown_runs_the_action_it_shows` in
`cockpit-ui/src/app/tests/bindings.rs:419` presses the keys and checks
`app.ran`. That test is not extended to a chat with a sign-in waiting, and no
test runs these actions at all:

- `auth.page` (open the page),
- `auth.cancel`,
- `auth.end.dismiss`.

A broken binding, or an action that is hidden when its key is pressed, would
pass.

## 5. "It still waits: paste again" checks a label, not that a second paste goes through

`cockpit-ui/src/chat/tests.rs:340` (and the same check at :348). The old
assertion checked that Finish was offered again once the first paste was
refused. The new one checks that `chat.send`'s label is "Finish signing in".
That label is the same whether a paste is being checked or was refused
(`sign_in_box` ignores `pasted.finishing`). So it can't tell "blocked while
checked" from "open again after the refusal". Only the `!p.finishing` line
just above says anything, and that line is about the agents store, not the
chat's Send. Run `chat.send` with text after the refusal and assert that a
`FinishSignIn` Ask goes out.

## 6. The plan's "after Cancel, Send messages the agent" test isn't there

The plan (step 2) asks for exactly this test. `tests.rs:428-434` instead ends
the sign-in with a server snapshot (`signed_out_harness(None)`).
`auth.cancel` only asks the server (`Act::StopSigningIn`, `agents.rs:309`),
and the box stays the sign-in's until the snapshot arrives. Nothing tests
running Cancel, or what Send does in the time between Cancel and the
snapshot.

## 7. Other behaviour the plan relies on that no test protects

- **Steering and slash commands during a sign-in.** "Typed text never
  reaches the agent then" also covers `chat.inject` (adding to a running
  turn) and the `/command` matches under the box. Both are switched off by
  `to_agent` (`chat/actions.rs:218-230`). No test types into the box during
  a running, steerable turn while a sign-in waits, so dropping `to_agent`
  would let the pasted code go into the agent's turn.
- **When the box goes back to the chat.** Nothing checks that the box is
  the chat's again once the sign-in has run out (`ran_out`), or for a
  sign-in with no link or no `expects` (`sign_in_box`'s filter,
  `chat.rs:248`).
- **Text typed while a paste is being checked.** The test sends "again"
  then (`tests.rs:333-335`) and only asserts that nothing was asked.
  The code throws "again" away in that case (`input.take()`, then the
  `(Some(_), _) => {}` arm). No test says whether that's intended, so
  either behaviour passes.
- **Restoring the full draft.** The set-aside test checks only
  `input.text`. Nothing checks that a set-aside draft's attachments or
  other fields come back, though `restore` takes the whole `Message`.

## 8. Wording and duplicate checks

- `tests.rs:412` asserts the status note's exact words, "Copied the sign-in
  link.". The behaviour is that a note is said; the exact sentence is
  wording and breaks on any rephrasing.
- The `chat.send` label ("Finish signing in" / "Send the prompt") is
  asserted five times across the two sign-in tests (`tests.rs:326, 340, 348,
  420, 430`). Each time the routing is right next to it, or is what's
  actually meant. The Ask checks at :423-425 and :432-433 are the real
  tests; the label checks repeat them as wording.

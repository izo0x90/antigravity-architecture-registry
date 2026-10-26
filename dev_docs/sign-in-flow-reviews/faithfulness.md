# Review of ee6d6d3..HEAD (sign-in flow), faithfulness

Paths are relative to `meta-harness/`. Six problems, most severe first.

## 1. "Finish signing in" no longer has a button on the chat's top line

Plan (Decided): "Every sign-in step has a fixed key under Account (`Space a`) and a button on the chat's top line: … Paste back: `Space a c`, "Finish signing in" (as now)".

Code: `crates/cockpit-ui/src/chat/actions.rs:254-255`. The header button for `auth.complete` (the old `takes` → `.header(Button, "Finish signing in", …)`) was deleted. Now `auth.complete` is pushed with no control at all. Its place was taken by the typing box's Send, relabelled "Finish signing in" (`actions.rs:274-277`). That label is `on_typing`, so it shows only while the box is being typed in. Out of typing mode, the top line has no "Finish signing in" button.

The plan asks for both the button and the box. The code ships only the box and does not say so: the commit message for 24660e6 does not mention the removal, and the plan was not changed. `README.md:292-293` still says "every device shows it starting, then its link, the time left and Finish signing in".

## 2. Send while a paste is being checked throws away what was typed

Plan (step 2): "Send finishes the sign-in (one id, as Finish does)".

Code: `crates/cockpit-ui/src/chat/actions.rs:458-464`. `self.ui.input.take()` runs first. When the paste is still being checked (`pasted.finishing`) or the text is only whitespace, the arm `(Some(_), _) => {}` drops the taken text. Nothing is sent and the status line says nothing.

During that time the box still offers its Send, labelled "Finish signing in" (`actions.rs:274-275`). The old top-line button was hidden while a paste was checked ("not again while it's checked"). Neither the plan nor the commit message for dd6c5dd ("A second paste isn't sent while one is checked") says the text is lost. Example: the user pastes the corrected address while the first paste is still being checked and presses Enter. The box empties and the paste is gone.

## 3. Step 2's Cancel test is not the test the plan names

Plan (step 2): "Tests: … after Cancel, Send messages the agent."

Code: `crates/cockpit-ui/src/chat/tests.rs:428-434` (in `while_a_sign_in_waits_the_typing_box_is_its_and_its_link_is_copied_by_a_key`). The test ends the sign-in by applying a snapshot with no sign-in (`signed_out_harness(None)`, line 429). It never runs `auth.cancel`. The path the plan names, Cancel (`Act::StopSigningIn`, `crates/cockpit-ui/src/agents.rs:309-312`) followed by the box coming back, is not tested. That includes the time after Cancel and before the server's snapshot arrives, when the box is still the sign-in's.

## 4. Step 1's "each key runs its action" only checks the keymap table

Plan (step 1): "Tests: each key runs its action; copy puts the link on the clipboard and says it."

Code: `crates/cockpit-ui/src/chat/tests.rs:413-417` only checks that `keymap.bind` maps each id to its leader path. No key is pressed. `auth.copy` is run once by action id through `run_action` (line 408), which bypasses the keymap. `auth.page`, `auth.cancel` and `auth.end.dismiss` are never run by their keys or by id in the new tests. The copy half of the test is done; the key half is not.

## 5. The key sheet snapshot does not include the new keys

Plan (step 3): "README (keys), key sheet snapshot, suite, lints".

Code: `crates/cockpit-ui/tests/snapshots/keys.txt` has no `auth.*` entries at all. The app built by `crates/cockpit-ui/src/app/tests/keys_snapshot.rs` (`full_app()`) has no harness that offers a sign-in, so the Account actions never appear. The snapshot did not change in this range, and `Space a y/p/x/d` are pinned nowhere except the table check in item 4. The step was done in name only, and nothing says so.

## 6. Built but not in the plan: login commands run in their own process group

Plan: nothing about stopping login processes. Its steps are keys and buttons, the typing box, and README/tests/live.

Code: commit cc6b530. `crates/harness-acp/src/launch.rs:121-145` adds a new `Group` (`kill_on_drop` plus `process_group(0)`, with the group stopped on drop). `ProcessTree` now holds a `Group` instead of a `Child`. `crates/harness-acp/src/login.rs:25-26, 56` moves `LoginRun` onto the same `Group`. There is a new test at `login.rs:166`.

The commit says this was found live (orphaned `claude` logins after Cancel). It may be needed, but it changes how every agent process is launched and stopped. That is outside this plan, and neither the plan nor a findings file records it.

## Checked and matching the plan

- Keys `a y`, `a p`, `a x` and `a d` are in `config/default.toml`; `a l`, `a c` and `a o` are unchanged.
- The "Copy link", "Open the sign-in page", "Cancel sign-in" and "Dismiss" buttons are there.
- Copy link copies to the clipboard and puts a note on the status line, as the terminal's Copy does.
- The box's hint names what to paste and gives no key.
- The steering action and slash-command matches are turned off while the box is the sign-in's.
- The link is shown as a truncated label instead of an edit box.
- The README key text is updated.

Not counted as gaps: the live check and `sign-in-flow-findings.md`, which come after these reviews.

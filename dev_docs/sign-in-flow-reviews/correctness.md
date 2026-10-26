# Review of ee6d6d3..HEAD (sign-in flow), correctness

Six problems, most severe first.

## 1. Enter in the typing box sends the pasted code to the agent, not to the sign-in

`crates/cockpit-ui/src/chat/render.rs:360-367` (`render_input`)

The sign-in branch exists only in the `chat.send` action (`chat/actions.rs:457-467`). In insert mode the host lets every key through to the text field (`app/keys.rs:72-77`). `TypingBox::draw` catches Enter and Ctrl+Enter itself (`conversation.rs:344`), and `render_input` then calls `self.send_prompt(message)` or `self.add_to_turn(message)` directly. That code never checks `sign_in_box()`. The `.typed(Sent::Send)` on `chat.send` only gives its key hint "Enter" (`actions.rs:486, 498`); it does not send Enter through the action. So the key bar shows "Enter: Finish signing in", but Enter goes to the agent.

Sequence:
1. Start the sign-in with `Space a l`. The banner shows the link, and the box hint says "Paste … here and send it: it goes to the sign-in".
2. Press `i`, paste `http://127.0.0.1:4000/?code=4/abc…`, then press Enter.
3. `render_input` takes the box and sends `Act::Prompt { message: "http://…?code=…" }`. The OAuth code is sent to the agent as a prompt and stays in the session's history. The sign-in gets nothing and keeps waiting. Ctrl+Enter does the same through `add_to_turn`, even though `chat.inject` is hidden.

Only the mouse or touch "Finish signing in" button under the box, and the palette, reach the sign-in. The tests run the `chat.send` action directly (`chat/tests.rs` `run_action(.., "chat.send", ..)`), so they never cover the Enter path.

## 2. A saved draft is put into the sign-in's box, then sent as the answer or erased on the server

`crates/cockpit-ui/src/chat/nav.rs:84-85` (`tend_sign_in_box` runs before `take_unsent`), `crates/cockpit-ui/src/chat.rs:253-270`

`tend_sign_in_box` sets aside what is in the box and leaves the box empty. `take_unsent` then runs in the same render and restores the slot's saved draft into any empty box. It does not know that the box now belongs to the sign-in. `report_unsent` is muted while `set_aside` is `Some`. Nothing corrects this until the sign-in ends, and at that point the draft is erased on the server.

Sequence (switching slots; the lead and `sub-1` share harness `i`, which has a sign-in waiting):
1. On the lead, the box is the sign-in's: `set_aside = (lead, "D-lead")` and the box is empty.
2. Press `J` to go to `sub-1`. `tend`: the slot differs, so `set_aside = None` and the box is taken (it is empty). `sign_in_box()` is still `Some` (same harness), so `set_aside = (sub-1, "")`.
3. `take_unsent`: `unsent_for` is `lead`, not `sub-1`, and the box is empty, so `restore("D-sub")`. The sign-in's box now holds sub-1's draft and the paste hint is hidden.
4. One of these follows:
   - The user clicks "Finish signing in": `Act::FinishSignIn { answer: "D-sub" }`. For a command login, `LoginRun::finish` consumes the run and the command reads "D-sub" as its code. It fails, and the sign-in ends Failed.
   - The user pastes after the draft: the answer becomes "D-sub" plus the code, and it is refused.
   - The sign-in ends: `tend` (false, true) drops "D-sub" and restores `""`. `report_unsent` sees `""` against `unsent_said = "D-sub"` and sends `Ask::Draft { sub-1, "" }`. sub-1's saved draft is erased.

The same happens on a reload when the harness snapshot (sign-in waiting) arrives before the drafts are read. `tend` sets aside an empty box, then `take_unsent` restores the saved draft into the sign-in's box once `drafts.saved()` turns `Some`. Switching back to the lead in step 2 also restores "D-lead" into the sign-in's box.

## 3. What a login command started is left running once the command itself has exited

`crates/harness-acp/src/launch.rs:141-145` (`impl Drop for Group`), with `crates/harness-acp/src/login.rs:103-112` and `crates/harness-acp/src/auth.rs:311-315, 447`

`Group::drop` signals the process group through `self.child.id()`. tokio's `Child::id()` returns `None` once the child has been polled to completion. `LoginRun::finish` always waits the child to completion (`self.group.child.wait()`), and so does `LoginRun::start` when the command exits by itself. On those paths the drop finds no pid and kills nothing. `kill_on_drop` covers only the direct child, which is already gone. The new test `what_a_login_command_started_goes_with_it` drops the run while the wrapper still runs, so it does not cover this.

Sequence:
1. A login wrapper starts the real login, or a helper such as a local callback server or `xdg-open`, in the background, then prints the link.
2. The user pastes the code. `finish` writes it, and the wrapper exits 0 (`wait()` returns).
3. `run` is dropped at the end of the `finish_command` task. `Group::drop` gets `id() == None` and skips `kill_process_group`. The background child keeps running, holding the group alive (and possibly stdout: `read_out`'s comment "A stream something else keeps open" describes this case).

The same happens for `(Some(true) | Some(false), _)` in `begin_command`, where the command printed and exited by itself. To fix it, keep the pid (the group id) taken at spawn and signal the group with it on drop, whether or not the leader was reaped.

## 4. The box changes owner mid-typing: half the message is set aside and the rest is answered to the sign-in

`crates/cockpit-ui/src/chat.rs:261-262`, `crates/cockpit-ui/src/chat/actions.rs:460-463`

Who owns the box follows the server's snapshot, not anything the user does here. A sign-in started on another device, or in another tab of the same desk, takes the box between two keystrokes.

Sequence (two devices):
1. On device A, the user is typing "please also update the README" in insert mode.
2. After "please also upd", device B runs `Space a l`. The snapshot reaches A, and the next render sets aside "please also upd" and empties the box. The hint changes, but the user is looking at the keys.
3. "ate the README" lands in the sign-in's box. Pressing Enter sends it to the agent today (problem 1). Once problem 1 is fixed, it goes to the sign-in as its answer. For a command login, that consumes the `LoginRun` and the command fails on "ate the README", so the sign-in ends Failed and the link device B is following is dead.
4. When it ends, A gets back only "please also upd".

## 5. The set-aside draft comes back trimmed

`crates/cockpit-ui/src/chat.rs:262`

`set_aside` stores `self.ui.input.take()`, and `TypingBox::take` returns `message()`, which is `.trimmed()`. Everything else that keeps unsent text uses `draft()`, "as it is (untrimmed)".

Sequence: the user has typed "refactor the " (trailing space) when a sign-in starts. It ends and the box gets "refactor the" back with the caret at the end. The user types "parser" and gets "refactor theparser". Leading or trailing newlines in a multi-line draft are lost the same way, and `report_unsent` then saves the trimmed text over the saved draft.

## 6. Send while a paste is being checked silently throws away what was typed

`crates/cockpit-ui/src/chat/actions.rs:458, 464`

`let message = self.ui.input.take();` runs before the match. When `now.pasted.finishing` is true, or the text is blank, the arm `(Some(_), _) => {}` drops the message. Nothing is said and nothing is put back.

Sequence:
1. Paste the wrong address and click "Finish signing in". It shows "Checking what you pasted…".
2. Before the answer comes, paste the right address and click "Finish signing in" again. The box empties, nothing is sent, and no note appears.
3. The first paste is refused. The user has to copy the address from the other browser again.

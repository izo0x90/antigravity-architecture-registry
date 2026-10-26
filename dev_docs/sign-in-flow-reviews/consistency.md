# Review of ee6d6d3..HEAD (sign-in flow), consistency

Five problems, most severe first. Line numbers are at HEAD.

## 1. A second draft store beside the slot's saved draft; the two disagree when the slot changes

`crates/cockpit-ui/src/chat.rs:253-271` (`tend_sign_in_box`, `ui.set_aside`), `crates/cockpit-ui/src/chat/nav.rs:70-79` (`report_unsent` returning early), `nav.rs:84-87` (`render` order: tend, then `take_unsent`).

**Existing pattern.** The chat already keeps what was typed and not sent, per slot. `report_unsent` sends it as `Ask::Draft`, the server saves it, and `take_unsent` puts it back into an empty box when the slot is shown. A box that is taken out and later put back exactly as it was uses `TypingBox::put_back` (`conversation.rs:240`). What is kept unsent is `draft()`, which is not trimmed (`conversation.rs:229`).

**What the change does.** It adds a second store, `set_aside: Option<(SlotId, Message)>`, filled with `input.take()`. It also stops `report_unsent` while that store is set. One branch of `tend_sign_in_box` already depends on the saved draft ("its draft is as saved; it's that slot's to take back"). The other branch restores from its own copy. Because the two stores don't know about each other, they go wrong together:

- **Two slots on the same harness, a sign-in waiting, user on slot A.** The user steps to slot B (`J`). In the next `render`:
  1. `tend_sign_in_box` drops A's copy and sets aside B's box, which is empty.
  2. `take_unsent` runs next. It sees an empty box for a newly shown slot and restores B's saved draft into the box. That box now belongs to the sign-in.
  3. Enter (whose button now reads "Finish signing in") sends B's half-written message to the server as the sign-in answer.
  4. When the sign-in ends, `tend_sign_in_box` throws the box away and restores the empty copy it set aside. `report_unsent` then reports an empty draft for B, so B's saved draft is erased on the server.
- **A draft that ends in a space or newline comes back trimmed**, because it was set aside with `take()` (which goes through `message().trimmed()`) and not with `draft()`. `take()` also drops the caret, and `restore` recolours link chips with `accent` instead of each tag's own colour.

**Fix that uses what exists.** Drop `set_aside`:
- While the box is the sign-in's, empty it, don't report it (as now), and don't let `take_unsent` restore into it.
- When the sign-in ends, empty the box and clear `unsent_for`, so `take_unsent` brings back the slot's saved draft.

## 2. Send while a paste is being checked throws the typed text away and says nothing

`crates/cockpit-ui/src/chat/actions.rs:457-467` (`ChatCommand::Send`).

`let message = self.ui.input.take();` runs before the match. The `(Some(_), _)` arm (a paste being checked, or no instance) then does nothing. What was typed is gone and the status line is silent.

**Existing patterns.**
- **Don't offer a control that can't run.** Before this change the "Finish signing in" header button was hidden while a paste was checked: the `takes` test, removed at `actions.rs:254-255` in the diff. The `chat.send` control still says "Finish signing in" during that time.
- **Say why on the status line when an action can't run.** Examples: `TermCommand::Copy` says "Nothing to copy: select some text, or put the cursor on a line." (`terminal/mod.rs:807`), and `StepSlot` says "This is the last agent slot." (`chat/actions.rs:337`).

Either offer `chat.send` only while it can take the paste, or leave the box alone and set `cx.note`.

## 3. Two "Finish signing in" actions with different guards: one need, two paths

`crates/cockpit-ui/src/chat/actions.rs:253-255` (`auth.complete`) and `actions.rs:275` (`chat.send` relabelled), run at `actions.rs:429-433` (`CompleteLogin`) and `actions.rs:457-467` (`Send`).

While a sign-in waits, the palette and the `?` key sheet list two different actions with the same label, "Finish signing in": `chat.send` (Enter) and `auth.complete` (`Space a c`).
- `auth.complete` asks for the answer in the palette's own text prompt (`Param::Text`). It sends `Act::FinishSignIn` with no `finishing` check and no `sign_in_box` check.
- `chat.send` takes the answer from the typing box and does check both.

So `Space a c` sends a second paste while one is being checked, which Send now prevents. `auth.complete` is also still offered when no sign-in waits at all: before, only its header button was gated; now it has no gate anywhere.

This conflicts with the brief's "one mechanism per need". The plan's "`Space a c` too" can be met by giving the box's Send its own key (`auth.complete` runs `C::Send` with the same gate), not by keeping a second prompt with a weaker gate.

## 4. Finish signing in lost its top-line button; every other step has one

`crates/cockpit-ui/src/chat/actions.rs:253-255` and `actions.rs:275`.

**Existing pattern in this view.** Each sign-in step is an action with a `.header(Button, …)` control on the chat's top line. In this change Copy link, Open the sign-in page, Cancel sign-in and Dismiss all are; the plan lists "Finish signing in" with them.

**What the change does.** It removes the header button from `auth.complete`. The only control left is `chat.send`'s `.on_typing(...)`, which is drawn only while the box is typed in (`actions.rs:441-445`: "Shown under the box being typed in, while it's typed in"). In normal mode the top line therefore offers Open, Copy link and Cancel, but no way to finish.

## 5. "Copy link" is weighted differently from the other copy controls

`crates/cockpit-ui/src/chat/actions.rs:283`.

**Existing pattern.** Both existing copy actions are `.importance(MINOR)`:
- `code.copy` (`code.rs:967`), a header button
- `terminal.copy` (`terminal/mod.rs:750`)

**What the change does.** `auth.copy` takes the default importance, so it competes with Open the sign-in page (`PRIMARY`) for the key bar and the top line, while Cancel is `MINOR`. If the default is meant (copying is one of the two ways to the page here), the code should say so. If not, it should follow the copy pattern.

## Outside the angle, noticed in passing

`crates/harness-acp/src/launch.rs:140-146` (`impl Drop for Group`) finds the group from `self.child.id()`. Tokio returns `None` from `Child::id()` once the child has been waited to completion. `LoginRun::finish` (`login.rs:107`) and `LoginRun::start`'s exit branch (`login.rs:85-91`) both wait on `group.child` first. So for a login command that has exited, the drop never kills the group, and anything the wrapper left running survives. That is the case `what_a_login_command_started_goes_with_it` is meant to cover, but the test only drops a run that is still waiting. The pgid could be recorded at spawn time and used in `drop`.

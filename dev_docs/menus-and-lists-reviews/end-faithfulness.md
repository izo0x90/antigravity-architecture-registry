# End review: faithfulness (menus and lists)

Scope: the plan (`dev_docs/menus-and-lists-plan.md`) against commits eda7934, a7f349c, c464f49, 856f36c, 17d4757, 03c4a3e and the code as it is now. Paths are under `meta-harness/crates/cockpit-ui/` unless they say otherwise. Ranked from most to least serious.

## 1. What needs you now went on a new row under the top line, not on the banners (against Decided 1 and Step 3's design; the plan doesn't say so)

- Decided 1: "What needs you now stays with what it's about (a request's answers on its card, a sign-in's buttons on its banner), not in the menus."
- Step 3's design: "the banners are a spot of the chat's ("banners"); `here()` names it when nothing else is waiting and the cursor follows the latest; their actions are item controls (Copy link, Open the sign-in page, Finish, Cancel, Dismiss, Retry, Connect, Resume)."
- What was built instead: a new placement, `At::Notice` (`src/actions.rs:132-135`, `Action::notice` at `src/actions.rs:454-457`). The host draws it as a row of buttons under the panel's top line (`src/hosted.rs:175-177`).
- No "banners" spot exists (grep finds none), `here()` never names the banners, and the cursor doesn't follow them. The banners draw text only. `src/chat/render.rs:76-77` says so: "its ways out (Dismiss, Retry, signing in) are its actions, on its top line".
- The sign-in's Copy link, Open the sign-in page, Finish, Cancel and Dismiss, the notice's and the error's Dismiss, Retry, Connect and Resume are all `.notice(...)`: `src/chat/actions.rs:73, 79, 255, 281, 283, 285, 289, 292, 296, 298`.
- In the commit, 17d4757 mostly renamed `.header(Button, …)` to `.notice(…)`. These buttons were already on the panel's top line before; now they're on a line under it. So the user's decision (buttons on the banner) was never built.
- The plan's Step 3 section still describes the banners-as-a-spot design. Its Status says "Step 3: done (17d4757)" and never mentions the switch. Only the commit message and the README (`meta-harness/README.md:697-700`) describe the notice row.

## 2. The notice row holds view buttons the design sent into the views' menus, and Stop, which it sent to Session

- Step 3's design, under "Off the top line, into their groups":
  - "a view's top-line buttons (the view's menu)";
  - "Stop (Session `s`, until the chat's live line, chat plan step 2, gives it a place on the turn)".
- Instead, these old top-line buttons moved to the notice row. None of them is about something that needs you now:
  - Stop: `src/chat/actions.rs:115`;
  - the graph's "Show N levels" and Return: `src/graph/actions.rs:116, 119`;
  - the queue's Start and Stop: `src/queue.rs:233, 235`;
  - the spec's Re-read: `src/spec.rs:949`;
  - the inbox's Previous, Next, Open it, Open its chat, Open its terminal and Put away: `src/inbox.rs:356-375`;
  - the discussion's Settle and Reopen: `src/discussion.rs:429, 435`.
- The result is still a row of controls under the top line, on top of the settings row. The Why ("in narrow columns that's rows of controls, busy and hard to read") is only partly answered. The plan doesn't record this choice anywhere.

## 3. Groups are filtered by "importance" (against Decided 1; the README states the user's rule, not the code's)

- Decided 1: "A group shows when something in it can be used here. Nothing decides what's 'important'."
- Step 3's design narrowed this rule while the user was away: groups "about where you are … not everywhere's".
- In the code, `groups_of` first drops every group whose closest action is everywhere's (`src/app/draw/which_key.rs:22`, `is_primary` at `src/actions.rs:630-632`). Only after that does it check whether anything in the group applies (`which_key.rs:27`).
- So a group whose usable actions are all everywhere's (Index `I`, Developer `D`, Servers `S`, Tags `t`…) never gets a button, even when it has something usable here. That is a deciding rule the user ruled out.
- `meta-harness/README.md:692-695` says groups are "shown when something in it can be used there". That is the user's rule, not what the code does.

## 4. The top bar was not "Recorded under Decide"

- Step 3's design: "Not in this step: the top bar across the screen (its own buttons: index, what's waiting, panels, inspector). Recorded under Decide."
- `TODO.md` (including its uncommitted changes) has no entry for the top bar's buttons. No other doc outside this plan records it either.

## 5. Step 4's checks: one replaced by a cheaper stand-in, others not recorded

- Order step 4: "live in the user's tab (keyboard, mouse, a phone-width window)". This was replaced by an app test, `src/app/tests/menus.rs` (a 390×844 screen in a test context). The Status gives the reason, but the plan wasn't changed and the user wasn't asked.
  - The test checks the Chat panel only, and four groups ("Chat actions", "Session", "Buffer", "Panels").
  - It doesn't check the notice row, the settings row, or the which-key path at that width.
- Decided 4: "Measured in the user's tab before and after (rapid screenshots while typing)."
  - The only "after" in the findings is step 1's `:` while typing (`dev_docs/menus-and-lists-findings.md`, "Live").
  - `?` is now that same list and has no measurement.
  - The opening flaw measured before ("fades in over about four frames … its rows showing a frame before its title and field") was not measured again.
- The Status says "The live checks were in the user's tab". The findings record no live check of step 2 (`?`, Tab, faded rows) or step 3 (the groups, a tap, the path, the long press).
- Order step 4 also lists "key sheet and layout snapshots". No commit touches `tests/snapshots/` or `src/app/tests/layout.rs`. Nothing pins the new top line's layout.

## 6. `?`'s headings are not the four the plan names

- Decided 3: "grouped under the headings the sheet has now (on what you're on, this view, this panel, everywhere)".
- The code makes one section per layer *and* action group, titled "{layer} · {group}" (`src/sheet.rs:63-72`). That gives many more headings than four.
- The README (`meta-harness/README.md`, the `?` paragraph from 856f36c) still describes the four.

## 7. Built but not asked for

- **A panel not focused shows no keys.**
  - Its group buttons and its controls lose their keys (`src/app/draw/host.rs:36-37, 48`).
  - Step 3's design: "Each button is the group's name and its one key". Decided 1: a long press shows "what a button does and its key".
  - On an unfocused panel, a control's tip has no key, because the keys are stripped before `draw_control` builds the tip (`src/actions.rs:725-726`).
- **Every unfocused panel now gathers and binds keys every frame**, with a cache kept per kind of view (`src/app/commands.rs:166-167, 209-221`). This is a new per-frame cost that wasn't planned or measured.
- **`?` runs actions that don't apply.** A faded action with no parameter is a pickable row (`src/sheet.rs:157`). Decided 3 says only "faded, with why when known".

## 8. Long press and `?`'s gestures are incomplete

- Decided 1: "Long-press on touch shows what a button does and its key, as hover does with a mouse." Two places still have hover only:
  - the key bar's chips, which are the big tappable buttons when narrow (`src/app/draw/bars.rs:97`);
  - the which-key entry's label beside its key (`src/app/draw/which_key.rs:137-139`).
- The which-key entry's tip says what the entry does but not its keys (`which_key.rs:131`). Step 3's design: "what it does and its keys".
- Decided 3 has `?` showing the "mouse and touch gestures". Its pointer section (`src/app/draw/bars.rs:174`) lacks step 3's gestures (tapping a group, a long press on a button showing what it does). It still says only "long press: what can be done with it (the palette)".

## 9. Smaller departures

- **Two ways of writing the path.** Decided 1: "A path shows where you are ('Account ›')." The which-key menu says "Space › Account ›". The top bar's mode label still uses `leader_title`, "LEADER > ACCOUNT" (`src/app/draw/top_bar.rs:205`, `src/app/draw/which_key.rs:49-60`).
- **An estimate recorded as a measurement.** The checkpoint said the per-frame re-sort and hash "is recorded with a measurement, not reworked here". The findings give an estimate, "That is tens of microseconds" (`dev_docs/menus-and-lists-findings.md:102`). Nothing was timed.
- **The list reads more than the menus.** Decided 5: "browsing (menus) and searching (the list) read one set". The list adds every other view's own actions (`src/app/commands.rs:254-265`). The menus never show those. This predates the plan, and the plan doesn't record it as an exception.

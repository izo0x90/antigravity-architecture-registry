# Review of 0dea926 (logins, round 4), correctness

Nine problems, most severe first. Paths are relative to `meta-harness/`.

## 1. A failed sign-in is still shown, as "Sign in again", while the harness is signed in, and a reload brings it back

`crates/cockpit-ui/src/chat/render.rs:99` now draws the banner whenever `sign_in_end_to_say()` is `Some`. `auth.ended` in the snapshot is cleared only by a newer `Waiting` (`instance.rs:83`). Nothing else clears it: not a status change, not a refresh that reads signed in. Dismiss only adds the id to `ends_put_away` (`agents.rs:126`, `:315`), an in-memory set.

1. A sign-in fails (a wrong code, say). The snapshot holds `ended = {id, Failed}`.
2. The user signs in another way, for example `claude login` in a terminal. The next refresh sets `status = SignedIn`. `ended` stays.
3. On every device the chat shows "The sign-in to X failed: <reason>. Sign in again." next to a harness that is signed in.
4. The user taps Dismiss. On a reload, a new tab or another device, `ends_put_away` is empty again and the banner comes back. It stays for the life of the server process.

This is round 3's "stale reason" (correctness 4, second bullet), not fixed. It is also worse than before: it no longer waits for a later Sign out.

In the same state the header doesn't offer Sign in. `actions.rs:245` needs `needs_login() || now.is_some()`, and both are false when the harness is signed in or unknown. So the banner says "Sign in again" with no Sign in button beside it, and the action is only in the palette.

## 2. Two Sign in taps at the same moment: the live sign-in is hidden from every device

`auth.rs:406` takes the id (`next_id()`) before `slot.start` takes the lock (`auth.rs:410`). So ids are not handed out in lock order. The snapshot's new rule (`instance.rs:80`) passes over a `Waiting` whose id is at or below the last `ended` id.

1. Device X calls begin and gets id 1. Device Y calls begin and gets id 2.
2. Y's `start` runs first, and the snapshot shows waiting 2.
3. X's `start` replaces 2. It says `Ended{2, Stopped}`, and the snapshot clears `waiting` and sets `ended = 2`. Then it says `Waiting(1)`, which is passed over because `ended.id 2 >= 1`.
4. Sign-in 1 is now the one held in the slot. Its agent or login command runs and prints its link. `show` (`auth.rs:133`) says `Waiting(1, url)`, which is passed over again.
5. No device shows the link, a Cancel or a Finish. The cockpit takes what to show from the snapshot only (`agents.rs` `AuthStarted`: "What's to be done comes in the instance's snapshot too"). Device Y's reply is "stopped". The process holds its callback port until `within_s`, and indefinitely for a pack without one, until someone taps Sign in again.

The window is small, but the rule depends on "ids only grow" in lock order, and the code doesn't guarantee that order. The id needs to be taken inside `start`, under the lock.

## 3. Starting a login command still isn't held as the sign-in: Cancel or a new Sign in doesn't stop it

The commit says every step, starting included, "runs in a task held as the sign-in". For a login command it doesn't. While `LoginRun::start` runs (`auth.rs:233`, up to `wait_s` = 30 s), the slot holds `Running::Pending`, and `stop` does nothing for that (`auth.rs:95`). The `started` task is not in the slot.

1. Claude Code: the user taps Sign in, then Cancel within the first seconds. The route answers 204, and the snapshot shows the sign-in stopped.
2. The `claude` login process keeps running until it prints its link or `wait_s` passes. Only then does `current() != Some(id)` (`auth.rs:237`) drop it.
3. If the user taps Sign in again meanwhile, two `claude` logins run at once against the same home.
4. A login command that finishes by itself (`exited = Some(true)`) during that time still writes its credentials. That is after Cancel, which is described as "nothing else changes".

## 4. A sign-in event can deadlock a worker thread when its instance is dropped at the same moment

`Slot::said`, `start` and `show` call `on_change` while holding `slot.state`. In the `Waiting` and `Ended` branches, the closure at `instance.rs:74` upgrades `me` to an `Arc<AcpInstance>` and drops it on return (`:87`, `:98`), still under that lock.

1. A sign-in's timer fires (`auth.rs:416`, no request in flight), or the agent task ends. Either one calls `slot.end` → `said` → `on_change`, and `me.upgrade()` succeeds.
2. At the same time, `InstanceRegistry::reconfigure` (`harness-core/src/registry.rs:76`, after a harness is installed or fixed) replaces the instance and drops the registry's `Arc`.
3. The closure's `Arc` is now the last one. Dropping it drops `AcpInstance`, then its `auth: Arc<AcpAuthController>`, which is the last reference when no request holds it. That runs `AcpAuthController::drop` (`auth.rs:208`), which locks `slot.state`, the same mutex this thread already holds.
4. A `std::sync::Mutex` locked twice by one thread deadlocks, and that tokio worker is blocked for good.

The window is narrow. The general rule: nothing that can drop the controller may run under its lock. The fix is to keep the upgrade outside the lock, or to say events after dropping the lock while keeping their order.

## 5. A failed Claude Code paste is answered "the sign-in failed: the sign-in failed: …"

`LoginRun::finish` already words its error as `"the sign-in failed: {last words}"` (`login.rs:113`), or "the sign-in didn't finish" (`login.rs:108`). `finish_command` stores that as `Failed { reason }` (`auth.rs:377`). The reply is now `gone(id)`, which adds the prefix again (`auth.rs:168`). Before this commit the request returned `run.finish`'s error as it was. So the device that pasted now reads "the sign-in failed: the sign-in failed: <last words>". The snapshot's banner reads "The sign-in to Claude Code failed: the sign-in failed: <last words>. Sign in again.", and point 1 now shows that banner in more states.

## 6. How the agent's login ended is published before it's recorded, so replies can misname it

The agent task calls `ended_to.send_replace(...)` (`auth.rs:309`) before `slot.end(id, how)` (`auth.rs:313`). Anything woken by the watch can run in between, on another worker:

- **`finish_agent`.** `how_it_ended` returns `Some(Err)`, or `ended.borrow()` is `Some` (`auth.rs:349-356`), and it calls `gone(id)`. `state.last` doesn't name `id` yet, so the reply is "that sign-in isn't under way any more (a new one started)". It should be "the sign-in failed: <reason>".
- **`begin_agent`'s "no link" branch (`auth.rs:328-333`).** The link reader has already dropped `link_to`, because the agent's output closed. The branch wakes on the same `send_replace` and calls `slot.end(id, Failed{"it ended before it showed a link"})`. If that wins the lock, the agent's own reason is lost from the snapshot and from the reply, and the agent's later `slot.end` is a no-op.

Calling `slot.end` before `send_replace` fixes both.

## 7. A login command's link can still be answered as live after its sign-in was stopped

`begin_command` puts the run and shows the link under two separate locks (`auth.rs:256-258`), and it ignores what `show` returns. Say a Cancel, a new Sign in or the timer ends the sign-in between the two. `show` then returns `false`, but begin still answers `Ok(Some(waiting))` with the link. That is the case round 3's fix 4 says is gone ("a link shown for a sign-in already replaced isn't answered as live"). The agent path checks `show`'s result (`auth.rs:296`); the command path doesn't. A paste that lands between `put` and `show` is also taken for a link no device has been shown yet.

## 8. A second paste while the first is being checked is told "wait for its link"

When the sign-in is `Running::Finishing`, or `Pending` between `take` and `put`, a `complete` reaches the `other` arm (`auth.rs:440-443`). It answers "the sign-in isn't ready for that yet: wait for its link". The cockpit hides Finish only on the device that pasted (`pasted.finishing`, per device).

1. Device A pastes.
2. Device B, which still shows Finish signing in, pastes too.
3. B is told to wait for a link that has been on screen for minutes.

## 9. `.nah/.gitignore` lines that let the logins back in are still kept when they don't name `harness` first

`store.rs:153` drops a `!` line only when its first path segment is `cache`, `state` or `harness`. Git's last matching line wins, so `!*`, `!**/harness/` or `!**` after `harness/` let `.nah/harness/` back in, and they survive the filter. When `harness/` is already present, nothing is appended after them (`missing` is empty), so the re-include stays in effect and a `git add -A` commits the logins. Lines like `!harness/claude-code/` that the filter does drop can't re-include anything anyway, because git doesn't re-include inside an excluded directory. So the filter catches only the plain `!harness/` case.

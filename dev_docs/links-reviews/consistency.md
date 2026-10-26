Review of 0dfa7ff..HEAD (links), consistency angle. Findings are ranked, most severe first.

1. **High: `ctrl+i` never fires on a Mac.** `crates/cockpit-ui/src/conversation.rs:344` checks the key with `Chord::shortcut()` + `consume_shortcut`. Every other configured key uses `Chord::consume`/`pressed` (`keymap/mod.rs:146,184`, `ListKeys::cycled`), which accept ctrl or command. `shortcut()` is documented as only for "a text box's return key". It sets both `ctrl` and `command`, and egui 0.33's `cmd_ctrl_matches` (checked in the registry source) then needs both flags on the key event. On a Mac, Ctrl+I sends ctrl without command and Cmd+I sends command without ctrl, so neither matches. The user's host is a Mac, so the only way to switch Auto/Whole/Pointer is dead there. To confirm: in host Chrome, put the caret after a chip and press ctrl+i; the chip doesn't change. Fix: use `keys.input.link.consume(i)`.

2. **Medium: palette links go through a side field, not the invocation.**
   - `palette.rs:605` stores the sent links in `Palette::sent_links`. Only `discuss.write` takes them (`app/starting.rs:105`).
   - A tag note (`tagging.rs:50`) still offers `#`/`@`, draws chips, cycles them and shows budget warnings, then silently drops the links.
   - Those links stay in `sent_links`. The next `discuss.write` that doesn't come through the palette (`run_named` from `window.nah` or an agent, `app/outside.rs:85`) picks them up and attaches them to unrelated text. The server expands them anyway, because `src/linking.rs` ignores spans.

3. **Medium: link checks bypass the shared-service pattern, and errors are lost.** `Drafts` is the precedent: a `Held` service in `Shared`, reached by `Ask::Draft`, debounced by `config.net.save_after_ms`, retried on failure with a "Couldn't save…" note. `Checks` (`app/linking.rs:28-83`) instead:
   - re-implements the debounce with a hard-coded `CHECK_AFTER = 0.4`;
   - lives on `Desk`, with the shell writing a `pub warning` and colours into every buffer's box each frame (`each_buffer_by_id`). Buffers don't read `Reader<Tags>` themselves.
   - Concretely: an `Err` from `/api/links/check` is dropped and `asked` is never reset (`linking.rs:72`). After one failed check (server restarting), no warning appears until the text changes, so the user sends a Whole link believing it fits.
   - Entries keyed by `format!("{id:?}")` are never removed for closed buffers.

4. **Low-medium: the `@` picker re-does the graph's find instead of reusing it.** `FOUND_EACH = 12` is hard-coded, while the same search's limits are config (`[graph.find]`, used at `graph/mod.rs:396`). The hits-to-choices mapping duplicates `GraphState::find_choices` (`graph/actions.rs:250`), but drops its "N of M" and "stopped at its time limit" detail. The user isn't told when results were cut.

5. **Low-medium: names are hand-built instead of using `Subject::name`.** `harness-contracts/src/tags.rs:228` defines `Subject::name` as "the one name a thing has, wherever it's shown (the cockpit's picker…)". Instead:
   - `linking.rs:155` rebuilds the line words.
   - Chips say `@src/a.rs` and `@step`, while the agent's heading says `@file src/a.rs` and `@component rust:a::step` (`src/linking.rs:16`). The agent can't tie the text to what's attached.
   - `run_link` gets the label back by JSON-parsing every choice again, and falls back to `@something`.

6. **Low: the link key isn't kept from the browser.** `KeymapConfig::ctrl_keys` (`keymap/mod.rs:410`) covers `bind` and `list` only, not `keymap.input`. `ctrl+i` is protected only because `nav.forward` also binds it. If `link` is rebound to e.g. `ctrl+e`, Chrome's address bar takes the key.

7. **Low: `Message` is rebuilt by hand per route.**
   - The server rebuilds it from `text` + `links` in `Prompt`, `Starting`, `Saying` (+ a `From` impl).
   - `net.rs` adds `links` by hand three times with `is_empty` checks, though `Message` already has `skip_serializing_if`. `#[serde(flatten)] message: Message` would cover all of these.
   - `src/routes/slots.rs:182-183` has an orphaned, mangled doc-comment fragment ("one opened) first if it isn't running…") on `Prompted.notes`.

8. **Low: words and predicates.**
   - `discuss.write` still says "What it asks (# names a tag)" (`app/starting.rs:50`).
   - The `link.thing` description says "the link's key" instead of the key's label, unlike the `#` hint, which uses `back.label()`.
   - Two predicates mean "has a typing box": the gates use `takes_typed` (`tagging.rs:142`, `linking.rs:107`) while `put_link` uses `typing_box()`. They can drift apart; `typing_box` should replace `takes_typed`.

Nothing found in key-bar rendering, the keys snapshot, or the error wording of the picker ("Couldn't search the code: …").

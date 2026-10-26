End review of the chat plan (d54286c..50a125f): consistency.

Paths are relative to meta-harness/. Most serious first.

1. crates/cockpit-ui/src/diff.rs:66 (and Cargo.toml `similar`). The change adds a second diff engine. The Changes view's hunks come from git2 on the server (crates/nah-changes/src/compare.rs:160-195, `Patch`). The chat now builds hunks in the browser with the new `similar` crate. Hunk edges and line pairing can differ from what the Changes view shows for the same change. The plan says "the Changes view's diff, shown folded; no second diff." git2 already diffs two texts (`Patch::from_buffers`). The server could make these hunks the same way it makes the Changes view's.

2. crates/cockpit-ui/src/conversation.rs:176-300 (`parts`, `opening`, `closes`, `tables_apart`). This is a hand-written markdown splitter for fenced code blocks and tables. `pulldown-cmark` (0.13, already in the tree through `egui_commonmark`) parses both, with byte ranges (`Parser::into_offset_iter`, `Tag::CodeBlock(CodeBlockKind::Fenced(info))`, `Tag::Table`). The hand rules differ from CommonMark/GFM. A fence inside a list item or a block quote is not found. Any `|` line with "--" in its block counts as a table.

3. crates/cockpit-ui/src/conversation.rs:237-276 (`places`, `is_path`). These re-implement file and `file:line[:col]` detection, which already exists in crates/cockpit-ui/src/terminal/links.rs:28-91 (`links`, `link`, `has_extension`). The two sets of rules disagree. The terminal accepts a bare `Cargo.toml` and an absolute path in the project. The chat rejects both. They split words on different characters (`|` vs `*`). One need, two mechanisms.

4. crates/cockpit-ui/src/highlighter.rs:38, 49-66. `Highlighter::colours(&self)` pushes into a `RefCell` queue. Buffers call it through their `Reader<Highlighter>`, so a buffer changes a desk service, and starts network calls, without asking. crates/cockpit-ui/src/shared.rs:57-75 says a buffer gets a `Reader` and "changes it only by asking." Every other service a buffer acts on goes through `Ask` (`Ask::Agents`, `Ask::Terminals`, `Ask::Tags`, crates/cockpit-ui/src/nav.rs:101-107).

5. crates/cockpit-ui/src/chat/render.rs:340-366. The chat skips rows that are off screen with its own scheme. It walks every row each frame and calls `allocate_exact_size` with a cached height (`ChatView::heights`, chat.rs:104). The views that already lay out rows of uneven height use `ScrollArea::show_viewport` over row tops and draw only the rows in view (crates/cockpit-ui/src/tag_list.rs:195-210, crates/cockpit-ui/src/changes/mod.rs:687). The chat is a third way, and it is still O(rows) per frame.

6. crates/cockpit-ui/src/conversation.rs:76. Chips in a sent message are always `colors().accent`. The comment says the tag's colour is "the shell's to say." The shell recolours chips only in typing boxes (crates/cockpit-ui/src/app/linking.rs:227-228, using the new `Tags::link_colour`, crates/cockpit-ui/src/tags.rs:112). So a tag link shows in its tag's colour while you type it, then in the accent colour once sent. `Messages` already follows `Shared`. It could read the tags and call `link_colour`.

7. crates/cockpit-ui/src/chat/render.rs:600 and 63-71. A change's counts are one muted label, "+12 −3". The Changes view shows "+N" in `ok` and "−N" in `error`, as separate monospace labels (crates/cockpit-ui/src/changes/mod.rs:748-749, 811-812; its status line, 842-843). Also, the new `diff::counted` (crates/cockpit-ui/src/diff.rs:97) does the same job as the Changes view's own `count` closure (changes/mod.rs:746), which was left in place.

8. crates/cockpit-ui/src/chat/actions.rs:486-490 (and 459-460). This adds more ad hoc `ctx.copy_text` calls, each with its own note. The plan says "the copy mechanism decided in the selection TODO (one)". TODO.md:27 already lists the scattered copies as the thing to unify. The notes now read four ways. Here: "Copied the message (3 lines)." Terminal: "Copied." (terminal/mod.rs:802) and "Copied its output." (terminal/mod.rs:294). The code view says nothing (code.rs:954).

9. crates/arch-service/src/service.rs:403-413. `highlight` was put between `code_analysis`'s doc comment and its `fn`. `highlight` now carries both doc comments ("One file for the code viewer, in one answer…" then "A code block's colours…"), and `code_analysis` has none.

10. crates/cockpit-ui/config/default.toml:251. The comment says `c` copies "the code block you're on (picked with f, else the message's first)". The code copies the picked block, else the whole message, or your message, or the plan (crates/cockpit-ui/src/chat.rs `copy_here`). The README says so too. The config file's word is wrong.

11. crates/cockpit-ui/src/chat/actions.rs:282, 284. The choices' labels are the raw config names: "quiet", "normal", "everything", "heartbeat", "none". The existing pattern pairs each value with words for the screen (`DiffLayout::ALL`, crates/cockpit-ui/src/changes/mod.rs:42: "Auto", "One column", "Side by side"). The indicator choices also have no detail, while the verbosity ones do.

12. crates/cockpit-ui/src/chat/actions.rs:285. The label "Show the agent at work as…" ends in an ellipsis. No other action label in the palette does. The ellipsis is used for groups in the which-key popup (app/draw/which_key.rs:145) and for "in progress" words.

13. crates/cockpit-ui/src/chat/rows.rs:428-430 and crates/cockpit-ui/src/chat/render.rs:397. Two hand-written "capitalize the first letter" pieces, each a byte slice `get(..1)`. `spec::capitalized` (crates/cockpit-ui/src/spec.rs:873) already does this, by `char`.

14. crates/cockpit-ui/src/app/draw/bars.rs:7, 109. The key bar cuts a label at 40 `char`s by hand. The code base cuts text by the room it has, with egui's `truncate()` (this change uses `Label::truncate` in chat/live.rs:80 and render.rs) or `fit_text` (graph/draw.rs:812). egui 0.33's `Button` has `truncate()` too.

15. crates/cockpit-ui/src/chat/live.rs:45-57 (`reach`). This is a third set of words for a harness's `ProviderStatus`. `Agents::harness_problem` (crates/cockpit-ui/src/agents.rs:255-262) already words `Unavailable` and `Error` as "{name}: {reason}". The harness picker uses `{other:?}` (chat/actions.rs:104-107). The live line says "its harness can't run: …" and "its harness failed: …".

16. crates/cockpit-ui/src/config.rs:133, 145-151 and crates/cockpit-ui/src/chat/rows.rs:57-80. Two settings in one `[chat]` table are checked two ways. `shows` is a serde enum, with names kept twice (`rename_all = "lowercase"` and a hand `name()`/`from_name()`). `indicator` is a `String` with a custom deserializer. Its error ("no indicator named …") does not follow the config check's form, "status.x: unknown icon `y`" (crates/cockpit-ui/src/status.rs:72-80).

17. crates/nah-state/src/slots.rs:107-111 (`sent_key`). This re-implements the SHA-256 hex digest that `code_analyzer::extract::content_hash` (crates/code-analyzer/src/extract.rs:209) and the nah-packs tests already write the same way. It is now a third copy.

18. crates/cockpit-ui/src/chat/nav.rs:42-43. "Next message" and "Previous message" are always offered (`offer`), even with no message to go to. The tag list offers its sibling moves only when they can act (`offer_if`, crates/cockpit-ui/src/tag_list.rs:480-481), as `Move::Leave` does on the next lines of this same function.

19. crates/harness-acp/src/convert.rs:210. The commit leaves a blank line at the end of the file. rustfmt removes it, and no other file has one.

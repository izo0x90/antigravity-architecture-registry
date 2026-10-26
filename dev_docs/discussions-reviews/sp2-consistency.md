## Consistency review of 53cd6b5..c7f8a98 (discussions plan, steps 3–5)

Ranked from most to least severe.

1. **The same plan is fetched by two separate hand-built readers, and they disagree** (medium).
   - Where: `crates/cockpit-ui/src/tree/plan.rs` (`PlanTree::ask` → `net.arch_plan`) and `crates/cockpit-ui/src/spec.rs` (`ItemCard`, `Reply::Plan`, the tick that calls `net.arch_plan`).
   - The pattern they skip: anything several buffers show is kept once on the desk (`shared.rs`: "kept once on the desk"; `Kept`/`Held`).
   - Different refresh rules: the tree refetches only on `ArchEvent::Changed` that names its plan. The card refetches on any `Changed` (`on_event` → `invalidate` → `asked = false`).
   - The card's replies carry no request number. `TreeSource` uses numbered answers (`(u64, NetResult)`) so a stale one is dropped.
   - Trigger: an agent's `plan_edit` `add_item` followed quickly by `set_item` gives two `Changed` events, so the card has two GETs in flight. Whichever reply lands last wins, so the card can show the older state while the tree shows the new one. To confirm: check whether `NetClient` calls can complete out of order (browser fetch).

2. **The generic tree buffer now has plan-item logic in it** (medium).
   - Where: `crates/cockpit-ui/src/tree/mod.rs`, in `moves()` ("Open …'s card") and `enter()` (`Step::GoTo(Spec)`).
   - Both match `PlanPart::Item` directly. That contradicts the module's own rule: "What a row stands for elsewhere is its source's to say." The source should tell the tree what Enter does on a row.
   - Wording also leaks between sources: on an empty plan tree, Enter says "The outline is empty." (`enter`/`leave`, lines ~663/676).

3. **Setting an item's state has no key** (medium-low).
   - Where: `app/plan_items.rs` defines `plan.item_state`, but `config/default.toml` binds nothing to it, and it is absent from `keys.txt`.
   - Every tag edit got a key (`Space k r/c/n/x/d/o`). Setting an item's state is the main act of going through a review list (C.2), and it is reachable only through the `:` palette.
   - The choice list doesn't mark the current state. `graph/source.rs` `source_choices` does mark the current one.

4. **On-screen words are mixed** (low-medium).
   - The action "Ask about this item" (`spec.comment`) opens a composer titled "Comment on {section}" (`spec.rs:966,970`).
   - The graph details say "{n} open review points" (`graph/details.rs:95`).
   - The status chip says "open discussions" but its config key is still `[status.open_comments]`.
   - `spec.next_point`'s detail says "The next change asked for that isn't settled", though questions count too.
   - Comment, point, question and discussion are all used for the same thing.

5. **The chat-turn renumbering watcher doesn't follow the existing event-loop pattern** (low-medium).
   - Where: `src/chats.rs`, `when_moved`. It runs on a `std::thread` with `blocking_recv`.
   - The established pattern (`harness-core/src/slots.rs` `follow`, `nah-timeline` `follow`) is `tokio::spawn` with re-reading state on `RecvError::Lagged`.
   - Here `Lagged` falls into `_ => {}`. A renumbering missed while lagging is never told, so notes on chat turns keep stale turn numbers until some other move happens.
   - The `numbers` map holds every turn the server ever saw, so it grows without limit.

6. **The plan tree builds its own section titles** (low).
   - Where: `tree/plan.rs`, `titled()`. It derives titles from keys, while `spec::Section::from_key(..).title()` already exists.
   - Result: the plan tree row says "Logic" and the card says "Logic steps".

7. **A hand-written date parser in the comment move** (low).
   - Where: `src/plan_comments.rs`, `millis()`, using Howard Hinnant's algorithm. The writer, `now_rfc3339` in `arch-service/src/service.rs`, is also hand-written, and the two sit in different crates.
   - `jiff`, `chrono` and `time` are already in `Cargo.lock` (pulled in by other crates); `jiff::Timestamp` parses this format.
   - Behaviour: an offset like `+02:00` is read as UTC, fractions are dropped, and an unreadable date becomes 1970, which sorts that message first. This only bites on hand-edited plan files.

8. **Moved comments name agents differently from new discussions** (low).
   - Where: `plan_comments.rs`, `by()`. It writes `"agent {session}"` and its doc says the slot isn't known.
   - But the removed `mcp/text.rs` `agent()` signed comments with `caller.label`, which is the slot name. So one thread shows "agent lead" for moved messages and "lead" for new ones.

9. **Discussion search can miss real matches** (low, a correctness slip rather than a pattern one).
   - Where: `nah-things/src/logs.rs`, `search`. FFF caps each file at `per_file + 1` lines matching the longest word, and only then applies the every-word filter (`matches`).
   - Trigger: `find("flaky build", 1)` on a message whose first two lines containing "build" lack "flaky" misses the third line, which has both.

10. **Who-said-it wording is copied** (low). `spec.rs` `said_by` duplicates `tag_list.rs:244`; both hard-code `"user"` → "you". This is already planned as #45 in G, so it is noted only.

11. **Agents can't answer or clear discussions until step 6** (gap between steps).
    - Step 5 removed `review_comments`, `answer_comment` and `reply_comment`, and the "review: N open point(s)" line in `next_steps`. The new agent tools come in step 6.
    - Until then a change request blocks approval of its component, and no agent can answer it. The cockpit offers the user no "answer" either, though the `/answer` route exists.
    - The agent packs still name the old tools (F.4 is step 6).

**Nothing found in these areas:**
- **Keymap and buffers:** `buffer.plan` (`Space b p`) doesn't clash with any other key, and the keys snapshot is updated.
- **Access:** `discuss` is added to the roles, the role test and `Needs<can::Discuss>`, the same way `tags.edit` is.
- **Routes:** the error shape (`Refused` → `{error}`) and `blocking` are copied straight from `routes/tags.rs`. They are consistent but duplicated, and could be shared.
- **Events:** sending the whole discussion (`DiscussionUpdated`/`DiscussionRemoved`) mirrors tags.
- **State:** `nah-state/discussions.rs` mirrors `tags.rs`.
- **Search:** logs are searched with FFF, as decided.
- **Section names:** they now come from one list (`CARD_SECTIONS`), with a test.

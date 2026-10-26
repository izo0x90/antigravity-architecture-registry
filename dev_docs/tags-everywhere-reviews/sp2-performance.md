# Stopping point 2: performance reviewer

Performance review of b29fbfc..HEAD, ranked with the most severe first. I only read the code; I built nothing and ran nothing.

**1. Every PTY read of a tagged running command rewrites, stores and broadcasts the whole tag. The cost grows as O(n²).** The path is `src/terminals.rs` `happened`→`tell_followed` (≈222–252), then `nah-tags/src/lib.rs` `grew` (≈371–387), then `keep` (503).
- Each `Happening::Changed` fires once per PTY read (`nah-terminal/src/lib.rs` 412–474). Each one runs `grew`, which:
  - clones the whole `Tag`;
  - appends one more `Piece` to `kept.pieces`;
  - makes a SQLite transaction with `serde_json::to_string(tag)` of the whole tag (`nah-state/src/tags.rs` 29–40, default journal, so it fsyncs per commit);
  - publishes `TagUpdated` with the whole tag, pieces included, to every client.
- Trigger: tag a build that prints a line every ~50 ms for 20 minutes. That gives ~24,000 pieces at ~60 B each, so the last tag is ~1.4 MB. The total serialized to SQLite and to each client is ~Σk·60 B ≈ 17 GB. Even 3,000 pieces is ~270 MB.
- The cockpit deserializes each event, clones it (`cockpit-ui/src/tags.rs` 77–79) and bumps `version`. That makes `TagList::sync` rebuild all rows and keys (`tag_list.rs` 78–88) once per event.
- The pieces themselves are never used by the cockpit. The plan's A.3 ("events carry what changed, not the whole tag") was not done: `keep` still sends the whole tag.
- Severity: high. Tagging any chatty command makes the server, the database and the network slower the longer it runs.

**2. All of that work runs on the terminal's reading thread, while holding the global `followed` lock and the single state-DB connection mutex.** `tell_followed` holds `lock(&self.followed)` across `(f.more)(...)`. That reaches `grew`, which does several log syscalls (`create_dir_all`, `metadata`, `open`, two writes, `flush`), then SQLite commit plus fsync, then `hub.publish`.
- While that runs, the shell's output is not read. The whole program is throttled to the fsync rate, and the screen stops updating.
- Every other terminal's `happened` blocks on the same `followed` lock. Every other state write (notifications, terminals) waits on `StateDb.conn`.
- Severity: high for anyone who tags a running command in a busy terminal.

**3. Nothing bounds a followed command.** `Followed` lives until the command ends (`terminals.rs` 241–249). A tagged `npm run dev`, `tail -f` or watch-mode build therefore grows without limit:
- `kept.pieces` grows, and so do the tag's JSON in the DB and in every event (items 1 and 2);
- the log files grow without bound (8 MB rollover only splits them, and every file stays because it is pointed into).

There are also two follow-on leaks:
- **Untagging does not stop following.** Untag the command, or delete the tag, and the `Followed` stays registered. If the tag still exists, `grew` keeps cloning it and scanning `current()` over all its things on every read, forever.
- **A failed write leaks memory.** If `tag()` fails after `owner.follow` (`self.logs.write(..)?`, e.g. a full disk), the follower is already registered and its `early` buffer stays `Some`. From then on every output chunk is pushed into that `Vec` until the command ends (`lib.rs` ≈345–360).

Severity: high (memory and disk).

**4. A read builds and loads the whole text just to show 4,000 characters.**
- `Tags::now`/`said` (`lib.rs` 221–240) builds the full text for every thing on the tag, then counts its lines and cuts it to `SHOWN_MOST`:
  - from the source: `command_output(id, u64::MAX)` (`terminals.rs` ≈454) renders the whole command, up to `scrollback_lines`, which can now be raised without limit;
  - from the log: `Logs::read` (`logs.rs` 62–70) opens the file once per piece, so 24,000 pieces means 24,000 `open`+`seek` calls, and loads everything into memory.
- This runs on every `tag_read` without `thing`, on every Tags-view rest, and on every prompt that names the tag (`expand` calls `said` per thing before checking room).
- Rendering from the source holds the terminal's state lock while it builds MBs of text, which stalls that terminal's reader.
- `tag()` also builds the full `owner.now()` and throws it away when the command is being followed (`lib.rs` 319).
- Severity: medium. Seconds of stall and tens of MB per read for big commands or big tags.

**5. A lagging client keeps every tag clone.** `EventHub` is a `tokio::broadcast` of capacity 4096 (`hub.rs` 122–131). A subscriber that lags (a background mobile tab, a slow Home relay) keeps up to 4,096 unread `TagUpdated` values alive, each a full clone of the growing tag. With item 1 that is 4,096 × hundreds of KB, which is GB-scale.
- Unconfirmed: this assumes tokio frees a slot only after all receivers have read it, which is my reading of tokio. A heap snapshot while a slow client is attached would confirm it.
- Severity: medium (conditional).

**6. `refresh` and `name` on terminal lines clone every command, while holding the tags lock.** `Resolve::name` for `TerminalLines` (`terminals.rs` ≈432–436) calls `o.commands()`, which clones the full `Vec<TerminalCommand>` with their texts (thousands with 10k+ scrollback), then scans it in reverse.
- `Tags::refresh` calls this for every thing on every tag, under `lock(&self.tags)`, on every width change. Dragging a panel edge repeats it per resize.
- Example: 50 tagged line ranges × 2,000 commands is ~100k command clones per resize.
- `tag()` also calls `name` twice (`owner()` and then `owner.name`).
- Severity: low to medium.

**7. `grew` finds its thing by an O(n) owner scan per chunk.** It does `position(|t| self.current(&t.subject) == self.current(subject))` (`lib.rs` ≈377). That is two `current()` calls per thing, each walking every owner and calling `Terminals::moved` → `lines_now`, which locks the terminal state. With 500 things that is ~1,000 owner walks per PTY read, on the reading thread. Severity: low by itself; it adds to item 2.

**8. Cockpit, per frame (low).**
- `tag_actions` (`app/tagging.rs`) now calls `things_here` twice, and `tagged_here` re-runs `tags.on()` per thing. `on()` re-sorts `list()` and scans all tagged things each time. That is cheap at hundreds of things, but it runs once per gathered frame.
- The palette's haystacks now include each action's `detail` (`palette.rs` ≈212). Ranking rebuilds and scans 2–3× longer strings every frame while the palette is open.
- Nothing else new per frame. The gutter (`draw/host.rs` 97–99) is O(all tagged things) and predates this change.

**9. Log layout (low).** Every piece gets its own ~70-byte `== … · more · <date>` header (`logs.rs` 50–56). When a command prints one line per read, more than half the log is headers, and the "search with fff over the folder" use reads mostly noise.

Not performance, but the code paths above expose them:
- **A follower can stop logging silently.** If more lines arrive in one feed than `scrollback_lines` keeps (a 64 KB read of short lines when scrollback is set low, or a released synchronized-output hold), `lines_text(f.next, end)` returns `None`. `f.next` never advances, so that follower logs nothing more, with no error.
- **Two concurrent `tag()` calls on the same subject both get past the "already on it" check.** It runs before `drop(tags)` and is not repeated after the lock is taken again. That gives two `Tagged` entries and two followers writing everything twice.

Server async threads: there are no blocking calls on the runtime. Routes, MCP and `expand` all use `spawn_blocking`.

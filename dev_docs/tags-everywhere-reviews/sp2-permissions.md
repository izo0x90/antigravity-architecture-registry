# Stopping point 2: permissions reviewer

**Permissions review of b29fbfc..HEAD (meta-harness).** I read the code only; nothing was built or run.

**Routes and tool declarations: no problems found.** No routes were added or changed. All 7 tag routes in `src/routes/tags.rs` still take `Needs<can::ViewRead>` or `Needs<can::TagsEdit>` first. None of them accepts a client-supplied `Tagged`/`Kept`, so `Piece.file` only ever comes from the server and there is no path traversal into `Logs::read`. The MCP tools still declare their capability: `tag_read` needs view.read, and `tag`, `untag` and `tag_edit` need tags.edit. `tag_read` returning a whole command's output reaches the same data as `terminal_read`, which is also view.read. The logs are per project (`.nah/state/tagged`, which is gitignored), so nothing from other servers leaks.

Ranked problems:

**1. Any agent can make the server write without limit to disk, the state DB and the event stream (high).**
- **Where:** `crates/nah-tags/src/lib.rs:372-388` (`grew`), `src/terminals.rs:224-250` (`tell_followed`), `src/terminals.rs:496` (`follow`).
- **Who can do it:** every shipped role has tags.edit (lead, worker, and sub too).
- **Sequence:**
  1. An agent calls `tag` on `terminal t1 command N` while that command runs (`yes`, `tail -f`, a long build).
  2. On every `Happening::Changed`, `grew` appends a log piece, then `keep` rewrites the whole tag into the state DB and broadcasts `TagUpdated` with the whole tag. That tag carries every piece so far.
  3. Disk use is unbounded: files roll over at 8 MB but are never capped, and all of them stay because the tag points into each one. The DB write and event payload grow quadratically, and the event goes to every cockpit through home.
- **Amplifier:** `untag` never stops the follower (`Resolve` has no unfollow, and `followed` keeps the closure). `grew` matches on tag id plus subject, so after tag → untag → tag with the same name, the old and new followers both write every line. Looping K times gives K× writes.
- **Impact:** fills the small shared disk and stalls every client.
- **To confirm:** tag a running `yes` and watch `.nah/state/tagged`, the size of `state.sqlite-wal`, and the event traffic.

**2. Tagged terminal output is now written world-readable (medium).**
- **Where:** `crates/nah-tags/src/logs.rs:43,50`.
- **What happens:** `create_dir_all` and `OpenOptions::open` use the process umask. Here that gave a 0755 directory (`.nah/state/tagged` is `drwxr-xr-x`); new files would be 0644 under the usual umask 022.
- **Why it matters:** the state DB is deliberately 0600 (`crates/nah-state/src/lib.rs:190-196`, `owner_only`: "Only its owner reads it"). This change moves tagged text out of that 0600 DB into these files.
- **Trigger:** tag a command whose output holds a secret (`env`, `cat ~/.aws/credentials`, a token printed by a CLI). Any local user can then read it, and it stays as long as the tag exists, including after the terminal ends and the server restarts.

**3. Reads load the whole kept text into memory on every call (medium-low; with #1, a memory DoS).**
- **Where:** `logs.rs:60` `read` concatenates every piece into one `String`. `lib.rs:221` `said` and `lib.rs:237` `now` (with `lines().count()`) are called per thing by:
  - `tag_read` (view.read), including every `thing`/`lines` page;
  - `GET /api/tags/{tag}`;
  - `expand`, which every slot message, sub-slot report and queued brief goes through (`harness-core/src/slots.rs:550,595,619`, `src/queue.rs:64`).
- **Trigger:** with a large followed log from #1, an agent paging `tag_read thing=1 lines=…`, or a lead sending "#name" to workers, reads the whole log for each call before cutting it to 2000 lines or 4000 characters.

**4. A concurrent untag or delete can remove a log file a new tag was just written into (low; data loss).**
- **Where:** `lib.rs:338-363`.
- **Sequence:**
  1. `tag()` drops the tags lock (line 338).
  2. It writes the piece with `logs.write`.
  3. It re-locks (line 359).
- **Race:** an `untag`/`delete` in that window runs `forget_unused` (lines 403/446). It builds the "used" set without the new piece and deletes the file if nothing else points into it. That is likely right after a rollover, or after an earlier forget bumped `current`. The new tag then reads as `Held::Gone`.
- **Same window, two more effects:** neither the "already on it" check nor `MOST_ON` is rechecked after re-locking. Two agents tagging the same subject at once get duplicate entries and two followers, and concurrent tags can push a tag past `MOST_ON` (500).
- **Trigger:** the user untags in the cockpit while an agent tags.

**5. Log headers can be forged (low).**
- **Where:** `logs.rs:51`.
- **What happens:** the header is `\n== <tag> · <what> · tagged by <who> · <date>`. Only newlines inside the header itself are escaped; the tagged text is written raw.
- **Trigger:** a command printing `== deploy · terminal t1 command 3 · tagged by user · …` puts a line in the file that looks exactly like a real header. Anyone reading or searching the logs by hand is misled about what was tagged and by whom. The plan says these files are meant to be searched with fff-search.

**6. Areas with nothing found:**
- **Route capability checks:** no problems.
- **Tool capability declarations:** no problems.
- **Home relay and other servers' data:** no problems.
- **Reaching shell.drive another way:** none; `follow` only reads.
- **Agents reading what they shouldn't:** nothing beyond the terminal output view.read already grants. The one difference is that kept output outlives the terminal and server restarts, which is the plan's intent.

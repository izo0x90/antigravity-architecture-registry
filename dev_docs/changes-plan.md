# Plan: viewing changes (git)

Agreed 2026-10-15. Built 2026-10-16: steps 1–6 (commits bd25a7f to cfc8aad).

## What's there today

Nothing reads git (git2 is in the build only through the file search
library). What we can build on: the index knows which component sits at
which lines of which file; the graph can badge cards (it does for plan
review); the code view has a gutter; the server's event hub carries every
turn's start and end (nah-notify already subscribes to it and keeps its
records in the state database).

## The shared idea: a comparison

`Comparison { from, to }`, a wire type every part speaks:

- `from`: the last commit (`HEAD`), a commit, a branch, or a snapshot.
- `to`: the working files, the staged files, a commit, or a snapshot.

Each desk has one comparison in use (like the graph's source). Every view
that shows changes reads that one. Default: uncommitted against the last
commit, with **staged** (index vs HEAD), **unstaged** (files vs index) and
**new untracked** files kept apart, so what a commit would take is plain.

## Server: `nah-changes` (a crate that owns git, nothing else)

- **Changed files** for a comparison: path, status (staged / unstaged /
  untracked / renamed / deleted), +/- counts. No content; paged.
- **One file's changes**: its hunks (unified, with line numbers on both
  sides), asked for only when a file is opened. Cached by comparison and
  file content; dropped on the file-change events we already have.
- **Changes on components**: hunks matched to components' line ranges from
  the index; where the index is out of date for a file, the answer says so.
- **Timeline** (below).
- Routes: `GET /api/changes`, `GET /api/changes/file`, `GET
  /api/changes/components`, `GET /api/changes/timeline`. Agents get the same
  through an MCP tool (reviewers read changes this way).

### The timeline: when things changed, and who was working

We don't decide how harnesses work: no worktrees of ours, nothing about
commits (whether an agent commits is the harness's, its instructions' or
the user's call). We observe the project folder:

- Whenever any session's turn starts or ends, a snapshot of the project
  folder is written as a git tree, through a private copy of the index
  (as `git stash create` does): the user's index, branch, history and
  files are untouched; ignored files are left out. Kept under
  `refs/nah/snapshots/…` so gc keeps them; pruned by policy (age, ended
  sessions).
- Consecutive snapshots are **windows**: what changed in each, and which
  sessions (and the user, from the cockpit) were working during it. Kept in
  the state database.
- A turn's changes: its start snapshot to its end snapshot; when another
  session overlapped, the view says so instead of guessing.
- Harness edit reports (file-change items) only label changes ("by the
  edit tool"); snapshots are the record (bash edits are files too).
- Cost: git checks timestamps first and stores only content it hasn't
  seen; a snapshot of unchanged files costs a status walk.
- Not seen: edits a harness makes in a worktree of its own elsewhere,
  until they land in the project folder (then they show in that window).

## Cockpit: one comparison, three ways to see it

- **Changes view** (a new view kind): the diff run together, files then
  their hunks; the cursor walks files and hunks (`j`/`k`, `]]`/`[[`);
  Enter opens the spot in the code view; an action widens a hunk's
  context. Its top line: the comparison (a setting), counts, staged /
  unstaged. A timeline mode lists windows (who was working, what changed);
  picking one (or a range, "since I sent that prompt") makes it the
  comparison.
- **In the code view**: changed lines marked in the gutter; deleted lines
  shown in place, greyed (`z d` toggles them): what was up there, in
  the file's own context. Only the shown file's changes are asked for.
- **In the graph**: a badge on changed components (a mark kind beside the
  review ones); a filter to only changed; the quick look on a component
  shows its hunks; `g x` goes to its changes.
- **Chat**: a turn says how much it changed; a key opens that turn's
  window as the comparison.
- **Layout**: one column (interleaved) or side by side, a setting per view
  with "auto": side by side only when the column is wide enough.
- Everything through the one action list (placements, layers, keys) like
  every other view; staging and unstaging (a file, a hunk) can come later
  as actions: the user acting, not the meta-harness deciding.

## Search

- In the Changes view, `/` searches its items: file names and hunk text,
  deleted lines included (the search every view has); `n`/`N` step.
- Project search (`g /`, agents' `search`) already finds added and changed
  lines (they're in the files); "only changed files" is a filter on it from
  the comparison in use, not an index of its own.
- Deleted lines and old versions: only through the Changes view.
- Search as a whole is to be re-evaluated later (TODO.md).

## Defaults (agreed)

git2 (in the build already); each desk's comparison kept with the
cockpit's state; snapshots kept 14 days, and a closed session's dropped a
day after (in config until the admin screen); keys `Space b f` (Changes
view), `g x` (a component's changes), `z d` (deleted lines in place); the
comparison and change types in harness-contracts.

## Performance

Lists carry counts only; hunks load when a file is opened; nothing holds
whole diffs with context; caches keyed by comparison and content; big
files: hunks only. A test with a large repo and a ~30,000-line file keeps
the list and a file's hunks fast.

## Steps (each green and committed)

1. `nah-changes`: the comparison and change types (in harness-contracts),
   file list, one file's hunks, staged / unstaged / untracked; routes;
   tests on a scratch repo.
2. Changes view (`Space b f`): list and hunks, layouts, Enter into the
   code, `/` over its items (deleted lines included); the comparison kept
   with the cockpit's state.
3. Code view gutter and deleted lines in place (`z d`).
4. Components: hunks to components; graph badge and "only changed"
   filter; quick look; `g x`. Project search's "only changed files"
   filter (`g /`, agents' `search`).
5. Timeline: snapshots on turn starts and ends, windows with who was
   working, pruning; chat turn → its window; the timeline mode.
6. MCP tool for agents; staging actions (if wanted then).

# Packs and one analysis path

Status: agreed with the user on 2026-10-08. Slice `AP` in `spec/requirements.toml`.
It replaces M.16 (accurate calls) and M.17 (docs), which move here.

## What the user asked for

1. **Everything we install is a pack.** Harnesses and language tools share
   one manifest format, versioned, with the version range of the external
   tool each pack supports.
2. **One installer, ours, and it's a generic engine.** It knows only kinds
   of steps (download, verify, unpack, run a package manager, check a
   version). Every URL, version, checksum, command, path and platform name
   lives in pack files and config. Nothing tool-specific is in code. No
   per-tool bash scripts; only a tiny bootstrap that fetches our binary.
3. **One analysis path for every language.** Our reader (ast-grep, generic,
   driven by the language pack's syntax rules) always runs. If the
   language's SCIP indexer pack is installed, its output is an input to the
   same step. Our analyzer combines both, once, at analysis time, and writes
   one complete result to SQLite. Nothing is lost: everything we read today
   stays; SCIP sharpens links, types and field access.
4. **Downstream reads only SQLite.** Graph, views, search, the agent and the
   code viewer never enrich on read, re-read code, or load side files.
5. **Honest marks.** A link the language's own analyzer confirmed is
   *proven*; one only matched by name is *unconfirmed*; what can't be
   resolved is listed with the reason. Views show the difference.
6. **The code viewer uses the same data**: syntax colouring from the
   language pack's tree-sitter highlight queries, computed when the file is
   read; jump to definition, references and hover from the stored
   occurrences. Read-only now, built so it can become an editor.
7. **No hacks.** If something doesn't fit this design, stop and ask.

## What exists today (checked 2026-10-08)

- `scripts/install-antigravity-acp.sh` (+ a sandbox variant): hard-coded
  URLs for ACP 1.1.1, no checksum, no record of what's installed.
- `provider-antigravity/src/driver.rs::find_acp_server`: hard-coded install
  paths and an `AGY_EXECUTABLE` env var.
- `arch-index`: language packs (TOML, syntax only, unversioned) compiled in
  via `include_str!`, plus `.nah/packs/*.toml`; the reader (`extract.rs`,
  `expr.rs`), the name linker (`resolve.rs`), code rules, the walk.
- `arch-store`: SQLite, one row per file (`FileExtract` JSON + stamp).
- `arch-service`: reads changed files in parallel, links everything in
  memory (`resolve`), swaps a graph snapshot.
- Code viewer (`cockpit-ui/src/code.rs`): plain text, line numbers, a
  highlighted line range; jumps go through the graph only.

## Decisions

- **D1. Three stages, one pipeline, every language.**
  1. *Read* (ours, per file, in parallel, incremental by size and time):
     structure, call sites (with the exact range of the called name), data
     flow, placeholders, code-rule matches, highlight runs.
  2. *Index* (the language's indexer pack, per project, optional, in the
     background): its SCIP output is parsed and kept per file, tagged with
     the content hash of the file it describes.
  3. *Combine* (ours): joins 1 and 2 by position, links the rest by name,
     marks proven/unconfirmed, and writes the complete result (components,
     relations, per-file occurrences) to SQLite in one transaction.
  A language without an installed indexer skips stage 2; nothing else
  changes. A file changed since the indexer last ran uses no SCIP data
  (its links are unconfirmed until the indexer runs again).
- **D2. Where our analyzer lives.** The reader, the name linker and the
  combiner move out of `arch-index` into a crate of their own,
  `code-analyzer`, versioned on its own. The server calls it as a library;
  the binary also exposes it as `meta-harness analyze` (the same function,
  writing the same result) so it can be run and inspected on its own.
  External indexers are separate programs, so they are run as processes
  through their pack's run contract. (Our analyzer is the combining stage,
  not a peer of the indexers: the indexers are its inputs.)
- **D3. One stored format.** SQLite holds, per file, our read facts and the
  file's *occurrences* (range → symbol, role, with where it came from:
  ours or the named indexer + version); per project, the combined
  components and relations. SCIP types (`scip` crate) are used for parsing
  the indexers' output; what we store is our own schema, versioned
  (`STORE_VERSION`, analyzer version, pack versions), so stale data is
  rebuilt.
- **D4. Packs are data; the engine is generic.** Kinds: `harness`,
  `language`, `indexer`. Pack files ship with the distribution in `packs/`
  (also compiled into the binary as its default pack source); a project can
  add packs in `.nah/packs/`. Where packs come from is itself config
  (`packs/index.toml`).
- **D5. Platform names are config.** `packs/platforms.toml` maps the OS and
  architecture the binary reports to the names packs use.
- **D6. Installs are recorded.** A lock file lists every installed pack:
  name, pack version, tool version found, install path, checksums, when.
  `check` compares the lock against the packs' `supports` ranges.
- **D7. Harness drivers stay code** (they speak a protocol), but where the
  harness program is comes from its installed pack (or the instance's
  explicit config). No hard-coded paths or env-var names.
- **D8. Types stay text** (plans write types as text; one normalizer for
  both). SCIP adds exact type *identity* where the indexer gives it (the
  symbols inside a signature), used to confirm data flow.

## Pack format (`pack.toml`)

```toml
[pack]
name = "rust-analyzer"            # unique
kind = "indexer"                  # harness | language | indexer
version = "1.0.0"                 # this pack's version (semver)
description = "Rust's own analyzer: exact links and types, as SCIP."

[tool]                            # the external program the pack manages
name = "rust-analyzer"
supports = ">=1.85.0, <2.0.0"     # semver range of the tool, tested with
[tool.check]                      # how to find its version
command = "rust-analyzer"
args = ["--version"]
version = '^rust-analyzer (\d+\.\d+\.\d+)'   # first group is the version

[[install]]                       # tried in order; the first whose platform matches
platforms = ["linux-arm64", "linux-x86_64", "macos-arm64", "macos-x86_64"]
[[install.steps]]
run = { command = "rustup", args = ["component", "add", "rust-analyzer"] }

[run]                             # indexers only: how to make its SCIP output
command = "rust-analyzer"
args = ["scip", "{root}", "--output", "{out}"]
output = "scip"
timeout_s = 900
```

A download step:

```toml
[[install.steps]]
download = { url = "https://dl.google.com/agy-extensions/releases/{os}/agy-acp-server-agy_acp_server_{tool_version}-{os_alt}-{arch}.zip", sha256 = "…", unpack = "zip", into = "." }
```

- Placeholders: `{root}`, `{out}`, `{dest}` (the pack's install dir),
  `{os}`, `{arch}`, `{platform}`, `{tool_version}`, and any key the
  platform entry defines (`{os_alt}`). Unknown placeholders are an error
  when the pack is loaded, not at run time.
- Per-platform values (URL pieces, checksums) live in
  `[install.platform.<name>]` tables so each download has its own sha256.
- A harness pack adds `[provides] executable = "agy_acp_server.par"`
  (relative to `{dest}`).
- A language pack adds `[language]`: `syntax = "syntax.toml"` (today's
  pack TOML), `highlights = "highlights.scm"`, `indexer = "rust-analyzer"`
  (optional).
- Arbitrary scripts: not supported. If a tool truly needs one, stop and
  ask.

## Plan

### Phase 1. The pack engine and the installer (crate `nah-packs`)
1. Crate `nah-packs`: manifest types (above), strict parsing
   (`deny_unknown_fields`), validation (kinds, semver ranges, regexes,
   placeholders, per-platform completeness, a download always has sha256).
2. `packs/platforms.toml`: binary OS/arch → platform name, plus extra
   per-platform keys; `Platform::current()` reads it.
3. Pack sources: `packs/index.toml` lists sources (`embedded` — the
   distribution's `packs/` compiled in — and directories:
   `.nah/packs`, `~/.config/nah/packs`); later sources override earlier
   ones by pack name; duplicates within one source are an error.
4. The engine, steps only: `download` (ureq, to a temp file), `verify`
   (sha256), `unpack` (zip, tar.gz, none), `place` (into `{dest}`, mark
   executable), `run` (a command with args, never a shell). Every step is
   logged; any failure leaves the previous install in place.
5. `check`: run `[tool.check]`, take the version, compare with `supports`.
6. Lock file (`~/.local/share/nah/packs.lock`, TOML; location from config):
   written after a successful install; read by `check`, `which`, and the
   server.
7. Install root from config (`~/.local/share/nah/packs/<name>/<version>`).
8. CLI on the binary: `meta-harness pack list | check [name] | install
   <name> | update [name] | which <name>`. `list` shows kind, pack version,
   tool range, installed version and state (missing / ok / out of range).
9. Tests: parsing and validation errors; platform mapping; a full install
   from a `file://` URL with checksum; a wrong checksum refused and nothing
   left behind; a `run` step; `check` in and out of range; lock written and
   read; source override order.

### Phase 2. Harness pack (Antigravity ACP)
10. `packs/harness/antigravity-acp/pack.toml`: version 1.1.1 downloads per
    platform with their sha256 (fetched and recorded once), provides
    `agy_acp_server.par`, check command.
11. `provider-antigravity`: the executable comes from the instance config,
    else the installed `antigravity-acp` pack (via `nah-packs`); delete
    `find_acp_server`'s hard-coded paths and `AGY_EXECUTABLE`.
12. Delete `scripts/install-antigravity-acp*.sh`; add
    `scripts/bootstrap.sh` (only: build or fetch the binary, then
    `meta-harness pack install antigravity-acp`); `agy.yaml` uses it.
13. Tests: the driver finds the program through the lock; a missing pack
    says how to install it.

### Phase 3. Language and indexer packs
14. Move `arch-index/packs/{rust,python,typescript}.toml` to
    `packs/language/<lang>/syntax.toml` with a `pack.toml` each
    (`kind = "language"`, version, `indexer`); the Go test pack becomes a
    test language pack.
15. Highlight queries: `packs/language/<lang>/highlights.scm` from each
    grammar's upstream query (licence noted in the pack).
16. Indexer packs: `rust-analyzer` (rustup component), `scip-typescript`
    (npm, pinned version), `scip-python` (npm, pinned; marked in its
    description as the least maintained). Each with check and run.
17. Language packs load through `nah-packs` sources instead of
    `include_str!`; `.nah/packs` keeps working (now as pack folders).
18. Tests: the three language packs load from the embedded source; a
    project pack overrides a built-in; an indexer pack's run command
    expands placeholders.

### Phase 4. The analyzer crate and the one pipeline
19. New crate `code-analyzer`: move the reader (`extract`, `expr`, `walk`,
    syntax pack model, code rules) and the name linker (`resolve`) out of
    `arch-index`; delete `arch-index`.
20. Reader additions: each call records the exact range of the called name
    (for joining with SCIP); highlight runs per file (tree-sitter-highlight
    with the language pack's query, on the same parse).
21. `index` stage: run the indexer pack's command into a temp file, parse
    with the `scip` crate, keep per file: occurrences (range, symbol,
    roles), symbol info (kind, signature text, relationships), the content
    hash of the file as indexed (SCIP documents carry text or we hash the
    file at index time), the indexer pack and tool version.
22. `combine` stage:
    - SCIP definitions ↔ our definitions by file and name range; SCIP
      symbols → our component ids.
    - Each call site: a SCIP reference at its name range → *proven* target;
      else our name linker → *unconfirmed*; else listed as unresolved.
    - Implements/extends from SCIP relationships → proven.
    - Data flow: our flows; endpoints from the proven targets; types
      confirmed by SCIP type symbols where present.
    - Field reads/writes: SCIP read/write roles where present.
    - Occurrences per file: SCIP's when indexed and current, else ours
      (definitions and resolved call sites).
23. `arch-graph`: `Relation` gets `confidence` (`proven` | `unconfirmed`)
    and `confirmed_by` (indexer + version).
24. `arch-store` schema v2: `files` (read facts + stamp), `index_docs`
    (per file SCIP data + content hash + indexer), `occurrences` (per file,
    combined), `components`, `relations`; `meta` versions. Combine writes
    components/relations/occurrences in one transaction. Start-up loads
    the graph from `components`/`relations` (no linking).
25. Service: changed files → read → combine → store → swap snapshot.
    Indexer: per language with an installed indexer pack, in the
    background after changes settle (debounce and timeout from
    `limits.toml`), then combine again. Status per language (indexer, tool
    version, last run, files covered, proven/unconfirmed counts) in
    `/api/arch/index` and `/api/perf` (indexer stage timed).
26. `meta-harness analyze --root R --out F`: runs the same three stages and
    writes the same result as JSON.
27. Tests:
    - fixtures without an indexer: the same graph as today, every call
      unconfirmed;
    - a synthetic SCIP index (built with the `scip` types) over the Rust
      fixture: calls it covers become proven, a name-matched wrong guess is
      replaced, a call we couldn't resolve gets linked, a stale file uses
      none of it;
    - a real `rust-analyzer scip` run on the Rust fixture (rust-analyzer is
      installed here): proven calls match the fixture's expected calls;
    - start-up reads the stored graph without linking.

### Phase 5. Views show proven and unconfirmed
28. Graph style: unconfirmed lines drawn with their own look (theme token),
    legend entry, details card says "confirmed by rust-analyzer 1.90" or
    "matched by name".
29. Agent `view`/`inspect` text marks unconfirmed links.
30. Tests: style table covers both; the agent's text marks them.

### Phase 6. Code viewer on the same data
31. `GET /api/code/file?path=`: text, highlight runs, occurrences (range →
    card key / symbol, role), from SQLite only.
32. Cockpit code view: colours from highlight runs (capture name → theme
    token in config); `g d` / ctrl-click on an occurrence jumps to its
    definition; `g r` lists references; hover shows the signature.
33. Tests: the endpoint returns runs and occurrences from the store; jump
    and references in the cockpit.

### Phase 7. Docs (was M.17)
34. README: packs, the installer, the analysis pipeline, proven vs
    unconfirmed, the code viewer, `/api/perf`, the three views, actions,
    plan focus.
35. Explainer: the same; regenerated data current; names nothing deleted.
36. `spec/perf-baseline.md`: final numbers, including the indexer stage.

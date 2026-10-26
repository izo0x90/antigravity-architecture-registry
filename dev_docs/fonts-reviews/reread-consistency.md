# Re-read of d3f743c (fonts fixes), consistency

Line numbers are as of d3f743c (paths under `meta-harness/`). Most severe first.

Checked and real in the code: consistency 1 (lookup in `nah_packs::font_face`, state asked first), 3 (`repo_root()` at `src/main.rs:193`), 4 (`[fonts]` before the `[changes]` comment; `FontsUi` without `serde(default, deny_unknown_fields)`, `UiConfig.fonts` without `#[serde(default)]`), 5, 6 (manifest rules and face states tested in `crates/nah-packs/tests/packs.rs`; the old loop is gone), 7 (`NetClient::at`, `accept` passed in), 8 (`want` in the `theme_dirty` block, `start` from `web/mod.rs:57`, no other `set_fonts` outside tests), 9 (`family` and `bold` gone from all seven packs and from `FontFiles`). Consistency 2 is only partly fixed; see 1.

## 1. A server-side pack fault still answers as the client's fault: every `PackError::Manifest` is a 400

`src/routes/cockpit.rs:92` maps `E::Manifest { .. }` to `BAD_REQUEST`, with the comment "Asked of a pack that isn't a font". The not-a-font case is only one source of that variant. The same closure in the route (`cockpit.rs:109-113`) also runs `Catalog::load` and `state()`, and both return `Manifest` for faults on the server:

- `crates/nah-packs/src/catalog.rs:273`: any user or project `pack.toml` that doesn't parse or validate (`Manifest::parse` → `PackError::Manifest { pack: "<origin>/<path>", .. }`).
- `catalog.rs:203`: a language pack whose `server` isn't a server pack.
- `crates/nah-packs/src/engine.rs:84`: a `[tool] supports` range that doesn't parse, raised from inside `state()`, which `font_face` now calls.

Example: a project has `.nah/packs/my-lang/pack.toml` with a typo. Every `/api/ui/fonts/<any>` request answers 400 with "pack .../my-lang/pack.toml: ...", even for a font that is installed and fine. This is the problem consistency 2 named (a server configuration fault looks like a different kind of failure); it now shows as 400 where it used to be 404. The route can't separate the two cases because `font_face` (`engine.rs:467`) reuses `Manifest` for "it isn't a font pack". `server_launch` (`engine.rs:558`) reuses it the same way, but its callers don't turn the variant into an HTTP status.

## 2. A third State-to-message mapping, worded differently from the two already there

`engine.rs:470-476` turns `State` into error text inside the engine. The pattern before this commit was different: the engine's lookups (`harness_launch` at `engine.rs:455`, `server_launch` at `engine.rs:555`) return the launch, and each caller asks `state()` and words the state itself. There are already two such callers: `src/packs.rs:184-197` (`pack_launch`) with `describe` at `src/packs.rs:214-223`, and `crates/arch-service/src/servers.rs:75-78`. The new mapping says the same states in its own words:

- Outdated: `engine.rs:473` writes "1.0.0 installed, 1.1.0 available". `describe` (`src/packs.rs:220`), which `pack check` and `pack update` print, writes "pack 1.0.0 installed, 1.1.0 available".
- OutOfRange: `engine.rs:474` wraps it in `PackError::Outdated`, whose Display (`crates/nah-packs/src/lib.rs:32`) says "is installed at another version". OutOfRange means the tool's own checked version is outside `supports`, not that another pack version is installed. `servers.rs:77` words the same state as "isn't in the supported range". No font pack can reach this arm: none of the seven has a `[tool] check`, so `state()` takes the `None` branch (`engine.rs:92-95`), which returns only Ready or Missing.

The new variants `PackError::Outdated` and `PackError::Broken` are also used only by `font_face`. `pack_launch` still builds its own install/update fix strings (`src/packs.rs:186-187`) for the same states, so "pack X is outdated, run update" now has two spellings, one in each crate.

## 3. The new tests don't pin the fixes they were added for

- Repo root (consistency 3): `tests/serve_tests.rs:55` sets `current_dir` and `:59` sets `NAH_REPO_ROOT`, both to `home/place`. `a_font_packs_face_is_served_once_installed_and_not_sent_again_unchanged` (`serve_tests.rs:162`) would therefore still pass if `src/main.rs:193` went back to `std::env::current_dir()?`. The fix has no test that fails without it.
- Typed statuses (consistency 2): the spawned-server test checks only 404 and 400 (`serve_tests.rs:210`). The 409 (Outdated) and 500 (Config, Lock, Broken) arms at `cockpit.rs:94-95` have no test. The one 400 case tested is the not-a-font case, so the wrong 400 in item 1 is kept in place by the tests rather than caught.

## 4. A test assertion that checks its own setup

`crates/nah-packs/tests/packs.rs:295`: `assert!(!installed.dest.join("ttf/Fake-Bold.ttf").exists(), "only what it names is kept")`. The test's own manifest (`packs.rs:258`) already says `extract = ["ttf/Fake-Regular.ttf"]`. The bold file is absent because the zip step extracts only what it lists; no font rule or `[font] face` handling is involved. The assertion would pass with any `[font]` behaviour.

## 5. Plan text left at two faces

`dev_docs/fonts-plan.md:24` still says Iosevka Term keeps "only two files". Since this commit it keeps one (`packs/font/iosevka-term/pack.toml:16`).

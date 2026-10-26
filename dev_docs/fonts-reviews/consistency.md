# Review of 2d03dd5 (fonts), consistency

Line numbers are as of 2d03dd5. Most severe first.

## 1. The installed-face lookup skips the pack engine: stringly errors, a copied message, and no state check

`meta-harness/src/packs.rs:141` `font_face` returns `Result<PathBuf, String>`. Every other per-kind lookup of an installed pack lives in `nah-packs` (`engine.rs` `harness_launch`, `server_launch`), takes `(catalog, lock, name)` and returns `PackError`. This one is different in three ways:

- It writes its own copy of `PackError::NotInstalled`'s text (`"{name} isn't installed (meta-harness pack install {name})"`, line 146) when it could return the variant.
- "isn't a font" and "no {style} face" are plain strings, where `server_launch` uses `PackError::Manifest { reason: "it isn't a server pack" }`.
- It never asks `nah_packs::state`. `harness_launches` and `pack_launch` decide Ready, Missing, Broken or Outdated through `state()`; fonts just join the face path onto `lock.dest`. Example: a font pack goes from 1.0.0 to 1.1.0 with a new face path in `holds`, and the user hasn't run `pack update`. The route joins the new path onto the old 1.0.0 folder and answers 404 "...: No such file or directory". It should say the pack is outdated and point to `meta-harness pack update`.

## 2. The route's errors don't follow the typed-error pattern; every failure is a 404

`meta-harness/src/routes/cockpit.rs:84-91` builds `(StatusCode, Json)` with a local `said` closure. Every catalog, lock, manifest and file error becomes `NOT_FOUND` (lines 90-91). The other route modules each wrap their error type (`ApiError`, `StateFailed`, `ChangesFailed`, `Refused`) and pick the status by variant. Because of item 1 the `PackError` variants have already been turned into strings, so this route cannot do the same. Examples:
- A stray file in `~/.config/nah/packs` makes `Catalog::load` fail with `PackError::Config`.
- `packs.lock` can't be parsed (`PackError::Lock`).
- The face file can't be read (permission denied).

Each of these comes back as 404, and the cockpit reports "The font X can't be had". A server-side configuration fault looks the same as "not installed".

## 3. The no-project server finds packs from a different root than the `pack` command

`meta-harness/src/main.rs:193` sets `packs_at: std::env::current_dir()?`. `meta-harness pack install` (main.rs, the `Cmd::Pack` branch) uses `startup::repo_root()`, which follows `NAH_REPO_ROOT` and canonicalizes the path. Example: `NAH_REPO_ROOT=/work/proj`, with a font pack in `/work/proj/.nah/packs/my-font`. `meta-harness pack install my-font` succeeds. `meta-harness serve --no-project` started from `~` loads its catalog from `~`, can't see `my-font`, and answers 404 "no pack named 'my-font'". Two code paths compute the same root in different ways.

In project mode, `packs_at` (`modules.rs:334`) is `p.arch.root()`. `HarnessState.root` already holds that value under a different name, so the same value is now stored twice in the states.

## 4. Config: `[fonts]` sits inside the `[changes]` comment, and it is the only strict section

- `crates/cockpit-ui/config/default.toml:750-762`: the `[changes]` comment block ("The changes view (git): context lines ...", 750-753) is now directly followed by the fonts comment and `[fonts]` (754-760), and only then by `[changes]` (762). Read top to bottom, the changes text describes `[fonts]`, and `[changes]` has no comment of its own.
- `crates/cockpit-ui/src/config.rs:93`: `FontsUi` is `#[serde(default, deny_unknown_fields)]`, and `UiConfig.fonts` also has `#[serde(default)]` (line 57-58). The sibling section structs (`PanelsUi`, `ChangesUi`, `CodeUi`, `NetUi`) take their defaults from `default.toml` only and don't reject unknown keys. So the defaults now come from two places (the Rust `Default` and `default.toml`). A typo is also handled two ways: `[fonts] mnoo = "x"` makes the whole UI config fail ("ui config: unknown field"), while `[changes] contxt = 3` is silently ignored.

## 5. The plan and the README don't match what was built

- `dev_docs/fonts-plan.md:50` (step 1) still says the theme names the face (`[theme] font_mono`, `font_text`, "a looks token like the rest"). The code adds a new top-level `[fonts] mono/text` section instead. The plan's Status line marks step 1 done and doesn't mention the change.
- `meta-harness/README.md:177-181`: the pack-kind table (`harness`, `language`, `server`) has no `font` row. Fonts are described in a separate paragraph further down (line 226).

## 6. The new manifest rules are tested in the wrong place, and the test copies the check

- `crates/nah-packs/tests/packs.rs` tests each kind's section rules (`"harness packs need [provides]"`, `"server packs need [server]"`). The new rules (`font packs need [font]`, `[font]` on another kind, `holds doesn't keep the face`) have no test there.
- `meta-harness/tests/packs_tests.rs:352-355` instead loops over the shipped fonts and checks again that `holds` contains `regular`. `Manifest::validate` (manifest.rs:408) already enforces this during `Catalog::load`, which runs on the line above. So the loop can never fail: if a pack broke the rule, `Catalog::load(...).unwrap()` would panic before the loop ran.

## 7. The network client builds its own path prefix and asks for JSON

`crates/cockpit-ui/src/net.rs:580-582`: `font_face` writes `format!("{}/api/ui/...", self.prefix, ...)` itself and then calls `call_bytes`. That repeats the `{prefix}{path}` joining that `call` (line ~285) owns. The bytes path also still sends `Accept: application/json` (line 327) for a font download. A byte-returning version of `call` (with the prefix applied in one place, and an Accept header set by the caller) would keep a single path.

## 8. Fonts get their own config-change detection and their own startup font set

- `crates/cockpit-ui/src/app/lifecycle.rs:30`: the rest of the config (theme, graph style, status, look, keymap, backoff) is applied in one place, the `theme_dirty` block (line 33). `Fonts::tend` instead keeps its own copy of the last choice (`asked`, fonts.rs:32) and compares it every frame. That gives config two mechanisms for "the config changed".
- `crates/cockpit-ui/src/web/mod.rs:56` still calls `set_fonts(font_definitions(&Default::default()))` at creation. On the first frame `Fonts::tend` does the same thing again (`asked` is `None`, fonts.rs:35). Two places now own the context's fonts. The lifecycle one was removed; this one was left behind.

## 9. Manifest data that nothing reads

`crates/nah-packs/src/manifest.rs:345` `FontFiles.family` is required in every pack but read nowhere. `bold` (line 349) is required to be in `holds`, is installed and is served by the route (`face("bold")`), but the cockpit only ever asks for `"regular"` (fonts.rs:41). Every pack downloads and keeps a face that is never used.

//! Language packs: data that maps a language's syntax onto the generic
//! component types via ast-grep rules. Built-in packs ship in `packs/`; more
//! can be loaded from any directory.

use std::path::Path;
use std::str::FromStr;

use ast_grep_config::{DeserializeEnv, RuleCore, SerializableRuleCore};
use ast_grep_language::SupportLang;
use serde::Deserialize;

use crate::IndexError;

const BUILT_IN: &[(&str, &str)] = &[
    ("rust.toml", include_str!("../packs/rust.toml")),
    ("python.toml", include_str!("../packs/python.toml")),
    ("typescript.toml", include_str!("../packs/typescript.toml")),
];

#[derive(Clone, Debug, Deserialize)]
pub struct LanguageSpec {
    pub name: String,
    /// ast-grep language id (e.g. "rust", "python", "typescript").
    pub ast_grep: String,
    pub extensions: Vec<String>,
    /// Separator in fully qualified symbols (e.g. "::", ".").
    pub separator: String,
    /// Separators that split a callee expression into qualifier and name.
    #[serde(default)]
    pub call_separators: Vec<String>,
    /// Call separators that can follow a module or type path (`util::shout`,
    /// `helpers.clean`). Empty with `member_separators` empty: all of them.
    #[serde(default)]
    pub path_separators: Vec<String>,
    /// Call separators that can follow a value (`store.save`), whose type
    /// then decides the target. Empty with `path_separators` empty: all of them.
    #[serde(default)]
    pub member_separators: Vec<String>,
    /// Punctuation before a type or a value that only says how it's passed
    /// (`&`, `*`); stripped along with leading `type_noise` words.
    #[serde(default)]
    pub reference_markers: Vec<String>,
    /// Words before a constructor call (`new`).
    #[serde(default)]
    pub constructor_prefixes: Vec<String>,
    /// Marks an import of everything in a module (`*`).
    #[serde(default)]
    pub glob_import: Option<String>,
    /// A file whose directory is a package root (e.g. Cargo.toml). Without it,
    /// module paths are relative to the repository root.
    #[serde(default)]
    pub package_manifest: Option<String>,
    /// Where the manifest names the package: a key path for TOML or JSON
    /// manifests (`["package", "name"]`)...
    #[serde(default)]
    pub package_name_key: Vec<String>,
    /// ...or a regex whose first group is the name (line-based manifests).
    /// Without either (or when the manifest doesn't say), the directory name.
    #[serde(default)]
    pub package_name_regex: Option<String>,
    #[serde(default)]
    pub package_name_replace: Vec<(String, String)>,
    /// Directories stripped from module paths (e.g. "src").
    #[serde(default)]
    pub source_roots: Vec<String>,
    /// Files that stand for their directory's module (e.g. lib.rs, __init__.py).
    #[serde(default)]
    pub index_files: Vec<String>,
    /// Receiver names meaning "the enclosing type" (self, this).
    #[serde(default)]
    pub self_names: Vec<String>,
    /// Leading qualifier segments that are relative markers, not names.
    #[serde(default)]
    pub relative_prefixes: Vec<String>,
    /// Syntax kinds that wrap a type (e.g. `: string` annotations); the type
    /// is their first named child.
    #[serde(default)]
    pub type_wrappers: Vec<String>,
    /// Groups of type names that carry the same data (e.g. String and str).
    #[serde(default)]
    pub compatible_types: Vec<Vec<String>>,
    /// JSON Schema type -> the written types that carry it.
    #[serde(default)]
    pub schema_types: std::collections::BTreeMap<String, Vec<String>>,
    /// Words in written types that don't describe the data (e.g. mut, dyn).
    #[serde(default)]
    pub type_noise: Vec<String>,
    /// Wrappers that make a value optional (e.g. Option, Optional).
    #[serde(default)]
    pub nullable_wrappers: Vec<String>,
    /// Written forms of an array of something (`string[]`, `[]string`).
    #[serde(default)]
    pub array_suffixes: Vec<String>,
    #[serde(default)]
    pub array_prefixes: Vec<String>,
    /// Syntax kinds whose body may never run where it's written (closures,
    /// lambdas): calls inside are deferred.
    #[serde(default)]
    pub deferred_scopes: Vec<String>,
    /// Tokens on a definition that change how it's called or what it
    /// returns (async, unsafe, const), found among its children or inside
    /// children of a `qualifier_containers` kind.
    #[serde(default)]
    pub qualifier_tokens: Vec<String>,
    #[serde(default)]
    pub qualifier_containers: Vec<String>,
}

impl LanguageSpec {
    pub fn is_path_separator(&self, sep: &str) -> bool {
        (self.path_separators.is_empty() && self.member_separators.is_empty()) || self.path_separators.iter().any(|s| s == sep)
    }

    pub fn is_member_separator(&self, sep: &str) -> bool {
        (self.path_separators.is_empty() && self.member_separators.is_empty()) || self.member_separators.iter().any(|s| s == sep)
    }

    /// Splits a callee at its last separator outside brackets:
    /// (qualifier, separator, name). `Greeter().greet` -> (`Greeter()`, `.`, `greet`).
    pub fn split_last<'a>(&self, text: &'a str) -> Option<(&'a str, &'a str, &'a str)> {
        let mut depth = 0i32;
        let mut found: Option<(usize, &str)> = None;
        let mut skip_until = 0;
        for (i, ch) in text.char_indices() {
            if i < skip_until {
                continue;
            }
            match ch {
                '(' | '[' | '<' | '{' => depth += 1,
                ')' | ']' | '>' | '}' => depth -= 1,
                _ if depth == 0 => {
                    if let Some(sep) = self.call_separators.iter().filter(|s| text[i..].starts_with(s.as_str())).max_by_key(|s| s.len()) {
                        found = Some((i, sep.as_str()));
                        skip_until = i + sep.len();
                    }
                }
                _ => {}
            }
        }
        let (at, sep) = found?;
        Some((&text[..at], &text[at..at + sep.len()], &text[at + sep.len()..]))
    }

    /// `text` without leading reference markers and noise words (`&mut x` -> `x`).
    pub fn strip_references<'a>(&self, text: &'a str) -> &'a str {
        let mut rest = text.trim();
        loop {
            let before = rest;
            for marker in &self.reference_markers {
                rest = rest.strip_prefix(marker.as_str()).unwrap_or(rest).trim_start();
            }
            for word in &self.type_noise {
                if let Some(after) = rest.strip_prefix(word.as_str())
                    && after.starts_with(char::is_whitespace)
                {
                    rest = after.trim_start();
                }
            }
            if rest == before {
                return rest;
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct DefinitionSpec {
    /// The component type it maps to (generic or specialized).
    #[serde(rename = "type")]
    pub type_name: String,
    pub rule: toml::Value,
    pub name_field: String,
    #[serde(default)]
    pub params_field: Option<String>,
    #[serde(default)]
    pub return_field: Option<String>,
    /// Base classes/interfaces it extends (each named child becomes Implements).
    #[serde(default)]
    pub bases_field: Option<String>,
    /// Base classes/interfaces as an ast-grep rule matched inside the
    /// definition (for syntax where they aren't one field).
    #[serde(default)]
    pub bases_rule: Option<toml::Value>,
    /// The type this definition belongs to, named by the first `owner_kind`
    /// node under this field (methods declared outside their type).
    #[serde(default)]
    pub owner_field: Option<String>,
    #[serde(default)]
    pub owner_kind: Option<String>,
    /// The name the owner field binds for the owner value (a Go receiver's
    /// `s`): the first node of this kind under `owner_field`. Calls on it
    /// are calls on the owner type, like `self`.
    #[serde(default)]
    pub receiver_name_kind: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ScopeSpec {
    pub rule: toml::Value,
    /// Definitions inside belong to the component named by this field.
    pub target_field: String,
    /// The named target implements the component named by this field.
    #[serde(default)]
    pub implements_field: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ParamSpec {
    /// Syntax kind of a parameter node inside the params field.
    pub kind: String,
    #[serde(default)]
    pub name_field: Option<String>,
    /// Use the first child of this kind as the name (when there's no field).
    #[serde(default)]
    pub name_child_kind: Option<String>,
    #[serde(default)]
    pub type_field: Option<String>,
    /// Parameters of this kind may be left out by callers.
    #[serde(default)]
    pub optional: bool,
    /// A parameter with this field (a default value) may be left out.
    #[serde(default)]
    pub default_field: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CallSpec {
    pub rule: toml::Value,
    pub callee_field: String,
    /// The argument list; arguments produced by other calls become data flow.
    #[serde(default)]
    pub args_field: Option<String>,
}

/// A local name bound to a value (`let x = f()`), so `g(x)` shows f's output
/// flowing into g.
#[derive(Clone, Debug, Deserialize)]
pub struct BindingSpec {
    pub rule: toml::Value,
    pub name_field: String,
    pub value_field: String,
    /// The declared type, when written (`let s: Store = ...`).
    #[serde(default)]
    pub type_field: Option<String>,
}

/// A local name for something imported (`import helpers as h`,
/// `use x::shout as yell`).
#[derive(Clone, Debug, Deserialize)]
pub struct AliasSpec {
    pub rule: toml::Value,
    /// The alias: this field, or the first child of `alias_child_kind`.
    #[serde(default)]
    pub alias_field: Option<String>,
    #[serde(default)]
    pub alias_child_kind: Option<String>,
    /// What it stands for: this field on the node, or on the nearest
    /// `original_ancestor` (e.g. the import statement's source).
    pub original_field: String,
    #[serde(default)]
    pub original_ancestor: Option<String>,
}

/// Local macros (or similar) that expand to a placeholder: their uses are
/// placeholders too.
#[derive(Clone, Debug, Deserialize)]
pub struct PlaceholderAliasSpec {
    /// Matches a definition whose body holds a placeholder.
    pub rule: toml::Value,
    pub name_field: String,
    /// Matches a use; its `invocation_name_field` names what's used.
    pub invocation: toml::Value,
    pub invocation_name_field: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RuleSpec {
    pub rule: toml::Value,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PackSpec {
    pub language: LanguageSpec,
    #[serde(default)]
    pub definitions: Vec<DefinitionSpec>,
    #[serde(default)]
    pub scopes: Vec<ScopeSpec>,
    #[serde(default)]
    pub params: Vec<ParamSpec>,
    #[serde(default)]
    pub calls: Vec<CallSpec>,
    #[serde(default)]
    pub bindings: Vec<BindingSpec>,
    #[serde(default)]
    pub imports: Vec<RuleSpec>,
    #[serde(default)]
    pub aliases: Vec<AliasSpec>,
    #[serde(default)]
    pub placeholders: Vec<RuleSpec>,
    #[serde(default)]
    pub placeholder_aliases: Vec<PlaceholderAliasSpec>,
    /// Code that never runs as written (an always-false condition or cfg):
    /// calls inside aren't observed.
    #[serde(default)]
    pub dead_code: Vec<RuleSpec>,
}

/// A pack with its rules compiled for its language.
pub struct Pack {
    pub spec: PackSpec,
    pub lang: SupportLang,
    pub definitions: Vec<RuleCore>,
    /// Per definition: its compiled `bases_rule`, if any.
    pub bases: Vec<Option<RuleCore>>,
    pub scopes: Vec<RuleCore>,
    pub bindings: Vec<RuleCore>,
    pub calls: Vec<RuleCore>,
    pub imports: Vec<RuleCore>,
    pub aliases: Vec<RuleCore>,
    pub placeholders: Vec<RuleCore>,
    /// Per placeholder alias: (definition rule, invocation rule).
    pub placeholder_aliases: Vec<(RuleCore, RuleCore)>,
    pub dead_code: Vec<RuleCore>,
    pub package_name_regex: Option<regex::Regex>,
    /// Hash of the pack's text: extracts made with another version are stale.
    pub fingerprint: String,
}

impl std::fmt::Debug for Pack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pack").field("language", &self.spec.language.name).finish_non_exhaustive()
    }
}

pub(crate) fn compile(lang: SupportLang, rule: &toml::Value, what: &str) -> Result<RuleCore, IndexError> {
    let json = serde_json::json!({ "rule": rule });
    let core: SerializableRuleCore = serde_json::from_value(json).map_err(|e| IndexError::Pack(format!("{what}: {e}")))?;
    core.get_matcher(DeserializeEnv::new(lang)).map_err(|e| IndexError::Pack(format!("{what}: {e}")))
}

impl Pack {
    pub fn from_toml(text: &str) -> Result<Self, IndexError> {
        let spec: PackSpec = toml::from_str(text).map_err(|e| IndexError::Pack(e.to_string()))?;
        let name = spec.language.name.clone();
        let lang = SupportLang::from_str(&spec.language.ast_grep)
            .map_err(|_| IndexError::Pack(format!("pack '{name}': unknown ast-grep language '{}'", spec.language.ast_grep)))?;
        if spec.language.extensions.is_empty() {
            return Err(IndexError::Pack(format!("pack '{name}': no file extensions")));
        }
        if spec.definitions.is_empty() {
            return Err(IndexError::Pack(format!("pack '{name}': no definitions")));
        }
        for sep in spec.language.path_separators.iter().chain(&spec.language.member_separators) {
            if !spec.language.call_separators.contains(sep) {
                return Err(IndexError::Pack(format!("pack '{name}': separator '{sep}' isn't one of call_separators")));
            }
        }
        if !(spec.language.path_separators.is_empty() && spec.language.member_separators.is_empty())
            && let Some(sep) = spec.language.call_separators.iter().find(|s| !spec.language.is_path_separator(s) && !spec.language.is_member_separator(s))
        {
            return Err(IndexError::Pack(format!("pack '{name}': call separator '{sep}' is neither a path nor a member separator")));
        }
        for (i, alias) in spec.aliases.iter().enumerate() {
            if alias.alias_field.is_none() == alias.alias_child_kind.is_none() {
                return Err(IndexError::Pack(format!("pack '{name}' aliases[{i}]: set exactly one of alias_field and alias_child_kind")));
            }
        }
        let package_name_regex = spec
            .language
            .package_name_regex
            .as_deref()
            .map(|r| regex::RegexBuilder::new(r).multi_line(true).build().map_err(|e| IndexError::Pack(format!("pack '{name}' package_name_regex: {e}"))))
            .transpose()?;
        let rules = |items: Vec<&toml::Value>, what: &str| -> Result<Vec<RuleCore>, IndexError> {
            items.into_iter().enumerate().map(|(i, rule)| compile(lang, rule, &format!("pack '{name}' {what}[{i}]"))).collect()
        };
        Ok(Self {
            definitions: rules(spec.definitions.iter().map(|d| &d.rule).collect(), "definitions")?,
            bases: spec
                .definitions
                .iter()
                .enumerate()
                .map(|(i, d)| d.bases_rule.as_ref().map(|r| compile(lang, r, &format!("pack '{name}' definitions[{i}].bases_rule"))).transpose())
                .collect::<Result<_, _>>()?,
            scopes: rules(spec.scopes.iter().map(|d| &d.rule).collect(), "scopes")?,
            bindings: rules(spec.bindings.iter().map(|d| &d.rule).collect(), "bindings")?,
            calls: rules(spec.calls.iter().map(|d| &d.rule).collect(), "calls")?,
            imports: rules(spec.imports.iter().map(|d| &d.rule).collect(), "imports")?,
            aliases: rules(spec.aliases.iter().map(|d| &d.rule).collect(), "aliases")?,
            placeholders: rules(spec.placeholders.iter().map(|d| &d.rule).collect(), "placeholders")?,
            placeholder_aliases: spec
                .placeholder_aliases
                .iter()
                .enumerate()
                .map(|(i, a)| {
                    let what = format!("pack '{name}' placeholder_aliases[{i}]");
                    Ok((compile(lang, &a.rule, &what)?, compile(lang, &a.invocation, &format!("{what}.invocation"))?))
                })
                .collect::<Result<_, IndexError>>()?,
            dead_code: rules(spec.dead_code.iter().map(|d| &d.rule).collect(), "dead_code")?,
            package_name_regex,
            fingerprint: crate::content_hash(text),
            lang,
            spec,
        })
    }

    pub fn name(&self) -> &str {
        &self.spec.language.name
    }

    pub fn handles(&self, path: &Path) -> bool {
        path.extension().and_then(|e| e.to_str()).is_some_and(|ext| self.spec.language.extensions.iter().any(|x| x == ext))
    }
}

/// The packs in effect: built-ins plus any loaded from directories (a loaded
/// pack replaces a built-in of the same language name).
#[derive(Debug, Default)]
pub struct PackSet {
    pub packs: Vec<Pack>,
}

impl PackSet {
    pub fn built_in() -> Result<Self, IndexError> {
        let mut set = PackSet::default();
        for (file, text) in BUILT_IN {
            set.add(Pack::from_toml(text).map_err(|e| IndexError::Pack(format!("built-in {file}: {e}")))?);
        }
        Ok(set)
    }

    pub fn add(&mut self, pack: Pack) {
        self.packs.retain(|p| p.name() != pack.name());
        self.packs.push(pack);
    }

    /// Loads every `*.toml` pack in a directory.
    pub fn load_dir(&mut self, dir: &Path) -> Result<(), IndexError> {
        let entries = std::fs::read_dir(dir).map_err(|e| IndexError::Io(format!("{}: {e}", dir.display())))?;
        let mut paths: Vec<_> = entries.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|e| e == "toml")).collect();
        paths.sort();
        for path in paths {
            let text = std::fs::read_to_string(&path).map_err(|e| IndexError::Io(format!("{}: {e}", path.display())))?;
            self.add(Pack::from_toml(&text).map_err(|e| IndexError::Pack(format!("{}: {e}", path.display())))?);
        }
        Ok(())
    }

    pub fn for_path(&self, path: &Path) -> Option<&Pack> {
        self.packs.iter().find(|p| p.handles(path))
    }

    pub fn by_name(&self, name: &str) -> Option<&Pack> {
        self.packs.iter().find(|p| p.name() == name)
    }
}

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
    /// A file whose directory is a package root (e.g. Cargo.toml). Without it,
    /// module paths are relative to the repository root.
    #[serde(default)]
    pub package_manifest: Option<String>,
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
    pub placeholders: Vec<RuleSpec>,
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
    pub placeholders: Vec<RuleCore>,
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
            placeholders: rules(spec.placeholders.iter().map(|d| &d.rule).collect(), "placeholders")?,
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

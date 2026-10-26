//! Language packs: data that maps a language's syntax onto the generic
//! component types via ast-grep rules. They come from the pack catalog
//! (`nah-packs`): each language pack holds its syntax rules, its highlight
//! queries and the name of its server.

use std::path::Path;
use std::str::FromStr;

use ast_grep_config::{DeserializeEnv, RuleCore, SerializableRuleCore};
use ast_grep_language::SupportLang;
use serde::Deserialize;

use crate::IndexError;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// Names of functions a program starts at (e.g. "main").
    #[serde(default)]
    pub entry_names: Vec<String>,
    /// Receiver names meaning "the enclosing type" (self, this).
    #[serde(default)]
    pub self_names: Vec<String>,
    /// A path's first word naming the enclosing type itself (`Self::new`).
    #[serde(default)]
    pub self_type_names: Vec<String>,
    /// Leading qualifier segments that are relative markers, not names.
    #[serde(default)]
    pub relative_prefixes: Vec<String>,
    /// Of those, the ones meaning the package's root module (`crate`), the
    /// module above (`super`, once per word) and this module (`self`).
    #[serde(default)]
    pub root_prefixes: Vec<String>,
    #[serde(default)]
    pub parent_prefixes: Vec<String>,
    #[serde(default)]
    pub here_prefixes: Vec<String>,
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
    /// The component type a whole file (module) maps to.
    pub module_type: String,
    /// Characters besides letters and digits that identifiers may contain.
    pub identifier_chars: String,
    /// Words that can't name a definition (keywords).
    #[serde(default)]
    pub reserved_words: Vec<String>,
    /// Syntax kinds that never mean anything here (comments).
    #[serde(default)]
    pub ignored_kinds: Vec<String>,
    /// Text in an import statement that makes its path relative to the
    /// importer's package (`"./`, `from .`), besides `relative_prefixes`.
    #[serde(default)]
    pub relative_import_markers: Vec<String>,
    /// Words in an import statement that aren't names (`use`, `from`, `import`).
    #[serde(default)]
    pub import_keywords: Vec<String>,
    /// Receivers meaning "the base of the enclosing type" (`super()`, `super`).
    #[serde(default)]
    pub super_receivers: Vec<String>,
    /// How a written type says "or nothing": union separators and null types
    /// (`string | null`, `str | None`).
    #[serde(default)]
    pub union_separators: Vec<String>,
    #[serde(default)]
    pub null_types: Vec<String>,
    /// The same language under other extensions with another grammar
    /// (`.tsx`): the same rules, compiled for that grammar.
    #[serde(default)]
    pub dialects: Vec<DialectSpec>,
    /// Calls that just wrap their argument (`Ok(x)`, `Some(x)`): what's
    /// returned or bound through them is the argument.
    #[serde(default)]
    pub transparent_calls: Vec<String>,
    /// Kinds of the names a pattern binds (`let (a, b) = ...` binds a and b).
    #[serde(default)]
    pub pattern_name_kinds: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DialectSpec {
    pub ast_grep: String,
    pub extensions: Vec<String>,
}

impl LanguageSpec {
    pub fn is_identifier_char(&self, c: char) -> bool {
        c.is_alphanumeric() || self.identifier_chars.contains(c)
    }

    /// Why `name` can't name a definition in this language, if it can't: it
    /// must be one identifier (no separators), not start with a digit, and
    /// not be a reserved word.
    pub fn check_name(&self, name: &str) -> Result<(), String> {
        let first_ok = name.chars().next().is_some_and(|c| c.is_alphabetic() || self.identifier_chars.contains(c));
        if !first_ok || !name.chars().all(|c| self.is_identifier_char(c)) {
            return Err(format!("'{name}' isn't a single {} identifier", self.name));
        }
        if self.reserved_words.iter().any(|w| w == name) {
            return Err(format!("'{name}' is a reserved word in {}", self.name));
        }
        Ok(())
    }

    /// The identifier words in `text`, in order.
    pub fn words<'a>(&self, text: &'a str) -> Vec<&'a str> {
        text.split(|c: char| !self.is_identifier_char(c)).filter(|w| !w.is_empty()).collect()
    }

    pub fn ignored(&self, kind: &str) -> bool {
        self.ignored_kinds.iter().any(|k| k == kind)
    }

    pub fn is_path_separator(&self, sep: &str) -> bool {
        (self.path_separators.is_empty() && self.member_separators.is_empty()) || self.path_separators.iter().any(|s| s == sep)
    }

    pub fn is_member_separator(&self, sep: &str) -> bool {
        (self.path_separators.is_empty() && self.member_separators.is_empty()) || self.member_separators.iter().any(|s| s == sep)
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
#[serde(deny_unknown_fields)]
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
    /// Parameters and return type live on this child (`const f = (x) => ...`:
    /// the arrow function in `value`).
    #[serde(default)]
    pub signature_field: Option<String>,
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
#[serde(deny_unknown_fields)]
pub struct ScopeSpec {
    pub rule: toml::Value,
    /// Definitions inside belong to the component named by this field.
    pub target_field: String,
    /// The named target implements the component named by this field.
    #[serde(default)]
    pub implements_field: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// Parameters of this kind take any number of arguments (`*args`).
    #[serde(default)]
    pub variadic: bool,
    /// A parameter with this field (a default value) may be left out.
    #[serde(default)]
    pub default_field: Option<String>,
}

/// What an expression of some syntax kinds is, for reading a call's target
/// (see `Expr`).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpressionSpec {
    pub kinds: Vec<String>,
    pub shape: ExpressionShape,
    /// Path: the qualifier; member: the value; call: what's called; unwrap:
    /// what's inside (default: the first named child).
    #[serde(default)]
    pub of: Option<String>,
    /// Path or member: the field holding the last name.
    #[serde(default)]
    pub name: Option<String>,
    /// Path or member: how it's written between the parts.
    #[serde(default)]
    pub separator: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpressionShape {
    /// A name as written.
    Name,
    /// Names joined by a path separator.
    Path,
    /// A member of a value.
    Member,
    /// What calling something gives.
    Call,
    /// Only wraps another expression (`&x`, `x?`, `await x`, `(x)`).
    Unwrap,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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

/// Macros (or anything invoked by name that expands to code): which names
/// are placeholders, directly or through definitions and aliases anywhere
/// in the repo. Resolution decides, across files, to a fixpoint.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacroSpec {
    /// A use; `name_field` names what's used (its last path segment counts).
    pub invocation: toml::Value,
    pub name_field: String,
    /// A definition; `definition_name_field` names it.
    pub definition: toml::Value,
    pub definition_name_field: String,
    /// Names whose uses are placeholders.
    pub placeholders: Vec<String>,
    /// In a definition's text: the names it uses (first capture group).
    pub definition_uses: String,
    /// A definition whose text matches this is a placeholder itself.
    #[serde(default)]
    pub definition_placeholder: Option<String>,
}

/// Where a dead-code rule's condition is read from; the code is dead only
/// when the condition is constant false under `[constant_logic]`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConditionSpec {
    /// This field of the matched node's parent (`if <condition> { ... }`).
    #[serde(default)]
    pub parent_field: Option<String>,
    /// The node right before the matched one, if it matches this rule; its
    /// text through `capture` (first group) is the condition.
    #[serde(default)]
    pub preceding: Option<toml::Value>,
    #[serde(default)]
    pub capture: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeadCodeSpec {
    pub rule: toml::Value,
    /// Without a condition, a match is dead as is.
    #[serde(default)]
    pub condition: Option<ConditionSpec>,
}

/// How constant conditions are written: enough to tell code that can never
/// run from code that might. Anything not built only from these is unknown
/// (and never treated as dead).
/// Decides whether a condition is constant, from its text. Text, not the
/// syntax tree: a `cfg(...)` attribute's contents are a flat run of tokens
/// (tree-sitter doesn't parse them as an expression), and `if` conditions
/// are judged the same way so both follow one set of rules. It only says
/// "false" when every part is a constant; anything it can't read is
/// undecided, and undecided code counts as live.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConstantLogic {
    #[serde(default)]
    pub true_atoms: Vec<String>,
    #[serde(default)]
    pub false_atoms: Vec<String>,
    #[serde(default)]
    pub not_prefixes: Vec<String>,
    #[serde(default)]
    pub and_operators: Vec<String>,
    #[serde(default)]
    pub or_operators: Vec<String>,
    /// Function forms: `all(a, b)`, `any(a, b)`, `not(a)`, `f(a)` = a.
    #[serde(default)]
    pub all_functions: Vec<String>,
    #[serde(default)]
    pub any_functions: Vec<String>,
    #[serde(default)]
    pub not_functions: Vec<String>,
    #[serde(default)]
    pub identity_functions: Vec<String>,
}

impl ConstantLogic {
    /// The value of a constant condition, if it is one.
    pub fn eval(&self, text: &str) -> Option<bool> {
        let s: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        self.eval_norm(&s)
    }

    fn eval_norm(&self, s: &str) -> Option<bool> {
        let s = strip_parens(s);
        let norm = |list: &[String]| list.iter().map(|a| a.chars().filter(|c| !c.is_whitespace()).collect::<String>()).collect::<Vec<_>>();
        if norm(&self.true_atoms).iter().any(|a| a == s) {
            return Some(true);
        }
        if norm(&self.false_atoms).iter().any(|a| a == s) {
            return Some(false);
        }
        // Lowest precedence first: or, then and.
        for (ops, any) in [(&self.or_operators, true), (&self.and_operators, false)] {
            let parts = split_top(s, &norm(ops));
            if parts.len() > 1 {
                let values: Vec<Option<bool>> = parts.iter().map(|p| self.eval_norm(p)).collect();
                return combine(&values, any);
            }
        }
        for prefix in norm(&self.not_prefixes) {
            if let Some(rest) = s.strip_prefix(prefix.as_str())
                && !rest.is_empty()
                && let Some(v) = self.eval_norm(rest)
            {
                return Some(!v);
            }
        }
        let open = s.find('(')?;
        if !s.ends_with(')') {
            return None;
        }
        let (name, args) = (&s[..open], &s[open + 1..s.len() - 1]);
        let args: Vec<&str> = if args.is_empty() { Vec::new() } else { split_top(args, &[",".to_owned()]) };
        let values: Vec<Option<bool>> = args.iter().map(|a| self.eval_norm(a)).collect();
        let named = |list: &[String]| norm(list).iter().any(|n| n == name);
        if named(&self.all_functions) {
            combine(&values, false)
        } else if named(&self.any_functions) {
            combine(&values, true)
        } else if named(&self.not_functions) && values.len() == 1 {
            values[0].map(|v| !v)
        } else if named(&self.identity_functions) && values.len() == 1 {
            values[0]
        } else {
            None
        }
    }
}

/// `any` (or) / all (and) over values that may be unknown.
fn combine(values: &[Option<bool>], any: bool) -> Option<bool> {
    if any {
        if values.contains(&Some(true)) {
            Some(true)
        } else if values.iter().all(|v| *v == Some(false)) {
            Some(false)
        } else {
            None
        }
    } else if values.contains(&Some(false)) {
        Some(false)
    } else if values.iter().all(|v| *v == Some(true)) {
        Some(true)
    } else {
        None
    }
}

/// `(x)` -> `x`, when the parentheses wrap the whole text.
fn strip_parens(s: &str) -> &str {
    let mut s = s;
    while s.starts_with('(') && s.ends_with(')') {
        let mut depth = 0;
        let wraps = s.char_indices().all(|(i, c)| {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
            depth > 0 || i == s.len() - 1
        });
        if !wraps {
            break;
        }
        s = &s[1..s.len() - 1];
    }
    s
}

/// Splits at any of `ops` outside brackets.
fn split_top<'a>(s: &'a str, ops: &[String]) -> Vec<&'a str> {
    let mut out = Vec::new();
    let (mut depth, mut start, mut skip_until) = (0i32, 0usize, 0usize);
    for (i, c) in s.char_indices() {
        if i < skip_until {
            continue;
        }
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ if depth == 0 => {
                if let Some(op) = ops.iter().filter(|o| !o.is_empty() && s[i..].starts_with(o.as_str())).max_by_key(|o| o.len()) {
                    out.push(&s[start..i]);
                    skip_until = i + op.len();
                    start = skip_until;
                }
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleSpec {
    pub rule: toml::Value,
}

/// A named item that isn't a component, and the field holding its name.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemSpec {
    pub rule: toml::Value,
    pub name_field: String,
}

/// Making a value of a type (`Report { .. }`, `new Store()`): the type is
/// written in `type_field`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConstructSpec {
    pub rule: toml::Value,
    pub type_field: String,
}

/// Fields used: of a definition's own value (`self.x`, `this.x`), and of
/// other values the `other_object` rule takes (`c.count`: which type `c` is,
/// only the language's server can say). Written where they're assigned to,
/// read anywhere else.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateSpec {
    /// A field access.
    pub access: toml::Value,
    /// The value it's on and the field's name.
    pub object_field: String,
    /// Other values (not the definition's own) whose fields count.
    #[serde(default)]
    pub other_object: Option<toml::Value>,
    pub field_field: String,
    /// Assignments: an access in their `left_field` is written.
    #[serde(default)]
    pub assignments: Vec<toml::Value>,
    pub left_field: String,
}

/// A boundary data crosses without a direct call (events, HTTP routes,
/// messages): where it's sent and where it's received, matched by a key.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundarySpec {
    pub name: String,
    pub send: toml::Value,
    /// Over the sending node's text: the first group is the key.
    pub send_key: String,
    pub receive: toml::Value,
    /// Over the receiving node's text (or its `receive_field`): the first
    /// group is the key.
    pub receive_key: String,
    #[serde(default)]
    pub receive_field: Option<String>,
    /// The receivers named in the receiving node (route handlers), each
    /// match's first group; without it, the definition the node is in.
    #[serde(default)]
    pub receive_handlers: Option<String>,
    /// Parts of a key that vary (path parameters like `{id}`): written
    /// the same way on both sides before keys are compared.
    #[serde(default)]
    pub key_holes: Option<String>,
    /// Whether its receivers are where the code is entered (a view's
    /// starting points): none (an event handled inside), every one
    /// ("received": a route's handlers, called from outside), or those
    /// that take only keys the code declares ("declared": the call that
    /// takes the tools a server lists; a match that also takes keys no one
    /// declares reads keys of its own).
    #[serde(default)]
    pub entry: Option<BoundaryEntry>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BoundaryEntry {
    Received,
    Declared,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// How a call's target reads from the syntax tree.
    #[serde(default)]
    pub expressions: Vec<ExpressionSpec>,
    #[serde(default)]
    pub bindings: Vec<BindingSpec>,
    #[serde(default)]
    pub imports: Vec<RuleSpec>,
    #[serde(default)]
    pub aliases: Vec<AliasSpec>,
    #[serde(default)]
    pub placeholders: Vec<RuleSpec>,
    #[serde(default)]
    pub macros: Option<MacroSpec>,
    /// Code that never runs as written (an always-false condition or cfg):
    /// calls inside aren't observed.
    #[serde(default)]
    pub dead_code: Vec<DeadCodeSpec>,
    #[serde(default)]
    pub constant_logic: ConstantLogic,
    /// A function named as a value (`let f = stubbed;`, `run(stubbed)`):
    /// recorded as a deferred call when it names a known function.
    #[serde(default)]
    pub references: Vec<RuleSpec>,
    /// Named items that aren't components (constants, statics, type
    /// aliases): only their names are kept, so a use of one says what it is.
    #[serde(default)]
    pub items: Vec<ItemSpec>,
    /// Where a definition's result leaves it: the matched node is (or
    /// wraps) the value returned.
    #[serde(default)]
    pub returns: Vec<RuleSpec>,
    #[serde(default)]
    pub constructions: Vec<ConstructSpec>,
    #[serde(default)]
    pub state: Option<StateSpec>,
    #[serde(default)]
    pub boundaries: Vec<BoundarySpec>,
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
    pub macros: Option<CompiledMacros>,
    /// Per dead-code rule: its rule, and the rule a preceding node must match.
    pub dead_code: Vec<(RuleCore, Option<RuleCore>, Option<regex::Regex>)>,
    pub references: Vec<RuleCore>,
    /// Per item rule: its rule and the field holding the name.
    pub items: Vec<(RuleCore, String)>,
    pub returns: Vec<RuleCore>,
    pub constructions: Vec<RuleCore>,
    pub state: Option<CompiledState>,
    pub boundaries: Vec<CompiledBoundary>,
    pub package_name_regex: Option<regex::Regex>,
    /// Hash of the pack's files: facts read with another version are stale.
    pub fingerprint: String,
    /// tree-sitter highlight queries (from the language pack), as written.
    pub highlights: Option<String>,
    /// The same, compiled for this pack's grammar.
    pub highlight_query: Option<crate::highlight::HighlightQuery>,
    /// The server pack that knows this language exactly.
    pub server: Option<String>,
    /// The same pack compiled for its other grammars (by extension).
    pub dialects: Vec<Pack>,
}

pub struct CompiledState {
    pub access: RuleCore,
    pub other_object: Option<RuleCore>,
    pub assignments: Vec<RuleCore>,
}

pub struct CompiledBoundary {
    pub send: RuleCore,
    pub send_key: regex::Regex,
    pub receive: RuleCore,
    pub receive_key: regex::Regex,
    pub receive_handlers: Option<regex::Regex>,
    pub key_holes: Option<regex::Regex>,
}

impl CompiledBoundary {
    /// A key as compared: its varying parts written as `{}`.
    pub fn key(&self, raw: &str) -> String {
        self.key_holes.as_ref().map_or_else(|| raw.to_owned(), |re| re.replace_all(raw, "{}").into_owned())
    }
}

pub struct CompiledMacros {
    pub invocation: RuleCore,
    pub definition: RuleCore,
    pub uses: regex::Regex,
    pub placeholder: Option<regex::Regex>,
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
        let fingerprint = crate::content_hash(text);
        let mut base = Self::compile(spec.clone(), &fingerprint)?;
        for dialect in &spec.language.dialects {
            let mut variant = spec.clone();
            variant.language.ast_grep = dialect.ast_grep.clone();
            variant.language.extensions = dialect.extensions.clone();
            variant.language.dialects.clear();
            base.dialects.push(Self::compile(variant, &fingerprint)?);
        }
        Ok(base)
    }

    fn compile(spec: PackSpec, fingerprint: &str) -> Result<Self, IndexError> {
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
            && let Some(sep) =
                spec.language.call_separators.iter().find(|s| !spec.language.is_path_separator(s) && !spec.language.is_member_separator(s))
        {
            return Err(IndexError::Pack(format!("pack '{name}': call separator '{sep}' is neither a path nor a member separator")));
        }
        if !spec.calls.is_empty() && spec.expressions.is_empty() {
            return Err(IndexError::Pack(format!("pack '{name}': calls need expressions (how a call's target reads)")));
        }
        for (i, e) in spec.expressions.iter().enumerate() {
            let what = format!("pack '{name}' expressions[{i}]");
            let needs = match e.shape {
                ExpressionShape::Path | ExpressionShape::Member => {
                    e.name.is_some() && e.separator.as_ref().is_some_and(|s| spec.language.call_separators.contains(s))
                }
                ExpressionShape::Call => e.of.is_some(),
                ExpressionShape::Name | ExpressionShape::Unwrap => true,
            };
            if e.kinds.is_empty() || !needs {
                return Err(IndexError::Pack(format!(
                    "{what}: a path or member needs a name field and one of call_separators; a call needs `of`"
                )));
            }
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
            .map(|r| {
                regex::RegexBuilder::new(r)
                    .multi_line(true)
                    .build()
                    .map_err(|e| IndexError::Pack(format!("pack '{name}' package_name_regex: {e}")))
            })
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
                .map(|(i, d)| {
                    d.bases_rule.as_ref().map(|r| compile(lang, r, &format!("pack '{name}' definitions[{i}].bases_rule"))).transpose()
                })
                .collect::<Result<_, _>>()?,
            scopes: rules(spec.scopes.iter().map(|d| &d.rule).collect(), "scopes")?,
            bindings: rules(spec.bindings.iter().map(|d| &d.rule).collect(), "bindings")?,
            calls: rules(spec.calls.iter().map(|d| &d.rule).collect(), "calls")?,
            imports: rules(spec.imports.iter().map(|d| &d.rule).collect(), "imports")?,
            aliases: rules(spec.aliases.iter().map(|d| &d.rule).collect(), "aliases")?,
            placeholders: rules(spec.placeholders.iter().map(|d| &d.rule).collect(), "placeholders")?,
            macros: spec
                .macros
                .as_ref()
                .map(|m| {
                    let what = format!("pack '{name}' macros");
                    let regex = |r: &str, field: &str| regex::Regex::new(r).map_err(|e| IndexError::Pack(format!("{what}.{field}: {e}")));
                    Ok::<_, IndexError>(CompiledMacros {
                        invocation: compile(lang, &m.invocation, &format!("{what}.invocation"))?,
                        definition: compile(lang, &m.definition, &format!("{what}.definition"))?,
                        uses: regex(&m.definition_uses, "definition_uses")?,
                        placeholder: m.definition_placeholder.as_deref().map(|r| regex(r, "definition_placeholder")).transpose()?,
                    })
                })
                .transpose()?,
            dead_code: spec
                .dead_code
                .iter()
                .enumerate()
                .map(|(i, d)| {
                    let what = format!("pack '{name}' dead_code[{i}]");
                    let preceding = d
                        .condition
                        .as_ref()
                        .and_then(|c| c.preceding.as_ref())
                        .map(|r| compile(lang, r, &format!("{what}.condition.preceding")))
                        .transpose()?;
                    let capture = d
                        .condition
                        .as_ref()
                        .and_then(|c| c.capture.as_deref())
                        .map(|r| regex::Regex::new(r).map_err(|e| IndexError::Pack(format!("{what}.condition.capture: {e}"))))
                        .transpose()?;
                    Ok((compile(lang, &d.rule, &what)?, preceding, capture))
                })
                .collect::<Result<_, IndexError>>()?,
            references: rules(spec.references.iter().map(|d| &d.rule).collect(), "references")?,
            items: spec
                .items
                .iter()
                .enumerate()
                .map(|(i, item)| Ok((compile(lang, &item.rule, &format!("items[{i}]"))?, item.name_field.clone())))
                .collect::<Result<_, IndexError>>()?,
            returns: rules(spec.returns.iter().map(|d| &d.rule).collect(), "returns")?,
            constructions: rules(spec.constructions.iter().map(|d| &d.rule).collect(), "constructions")?,
            state: spec
                .state
                .as_ref()
                .map(|st| {
                    Ok::<_, IndexError>(CompiledState {
                        access: compile(lang, &st.access, &format!("pack '{name}' state.access"))?,
                        other_object: st
                            .other_object
                            .as_ref()
                            .map(|r| compile(lang, r, &format!("pack '{name}' state.other_object")))
                            .transpose()?,
                        assignments: st
                            .assignments
                            .iter()
                            .enumerate()
                            .map(|(i, a)| compile(lang, a, &format!("pack '{name}' state.assignments[{i}]")))
                            .collect::<Result<_, _>>()?,
                    })
                })
                .transpose()?,
            boundaries: spec
                .boundaries
                .iter()
                .map(|b| {
                    let what = format!("pack '{name}' boundaries.{}", b.name);
                    let regex = |r: &str, field: &str| regex::Regex::new(r).map_err(|e| IndexError::Pack(format!("{what}.{field}: {e}")));
                    Ok::<_, IndexError>(CompiledBoundary {
                        send: compile(lang, &b.send, &format!("{what}.send"))?,
                        send_key: regex(&b.send_key, "send_key")?,
                        receive: compile(lang, &b.receive, &format!("{what}.receive"))?,
                        receive_key: regex(&b.receive_key, "receive_key")?,
                        receive_handlers: b.receive_handlers.as_deref().map(|r| regex(r, "receive_handlers")).transpose()?,
                        key_holes: b.key_holes.as_deref().map(|r| regex(r, "key_holes")).transpose()?,
                    })
                })
                .collect::<Result<_, IndexError>>()?,
            package_name_regex,
            fingerprint: fingerprint.to_owned(),
            highlights: None,
            highlight_query: None,
            server: None,
            dialects: Vec::new(),
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

    /// The compiled pack (this one or a dialect) for a file.
    pub fn for_path(&self, path: &Path) -> Option<&Pack> {
        if self.handles(path) { Some(self) } else { self.dialects.iter().find(|d| d.handles(path)) }
    }

    /// The compiled pack (this one or a dialect) a code block's fence
    /// names (in lower case): by the language's name, or one of its
    /// extensions.
    fn for_fence(&self, fence: &str) -> Option<&Pack> {
        let names = |p: &Pack| p.name() == fence || p.spec.language.extensions.iter().any(|x| x == fence);
        std::iter::once(self).chain(&self.dialects).find(|p| names(p))
    }

    /// Every definition's type must be known to `types`, and so must the
    /// module type: a pack mapping onto a type that doesn't exist is broken.
    pub fn check_types(&self, types: &arch_graph::TypeSystem) -> Result<(), IndexError> {
        let name = self.name();
        let mut wanted: Vec<&str> = self.spec.definitions.iter().map(|d| d.type_name.as_str()).collect();
        wanted.push(&self.spec.language.module_type);
        match wanted.into_iter().find(|t| !types.contains(t)) {
            Some(t) => Err(IndexError::Pack(format!("pack '{name}' maps onto type '{t}', which the type system doesn't have"))),
            None => Ok(()),
        }
    }
}

/// The language packs in effect.
#[derive(Debug, Default)]
pub struct PackSet {
    pub packs: Vec<Pack>,
}

impl PackSet {
    /// The language packs in a pack catalog: each one's syntax rules,
    /// highlight queries and server.
    pub fn from_catalog(catalog: &nah_packs::Catalog) -> Result<Self, IndexError> {
        let mut set = PackSet::default();
        for language in catalog.of_kind(nah_packs::Kind::Language) {
            let files =
                language.manifest.language.as_ref().ok_or_else(|| IndexError::Pack(format!("{}: no [language]", language.name())))?;
            let text = |file: &str| language.text(file).map_err(|e| IndexError::Pack(e.to_string()));
            let syntax = text(&files.syntax)?;
            let highlights = files.highlights.as_deref().map(text).transpose()?;
            let mut pack = Pack::from_toml(syntax).map_err(|e| IndexError::Pack(format!("{} ({}): {e}", language.name(), files.syntax)))?;
            if pack.name() != language.name() {
                return Err(IndexError::Pack(format!("pack '{}' describes language '{}'", language.name(), pack.name())));
            }
            let fingerprint =
                crate::content_hash(&format!("{}\n{syntax}\n{}", language.manifest.pack.version, highlights.unwrap_or_default()));
            let mut dialects = std::mem::take(&mut pack.dialects);
            for p in std::iter::once(&mut pack).chain(dialects.iter_mut()) {
                p.fingerprint = fingerprint.clone();
                p.highlights = highlights.map(str::to_owned);
                p.highlight_query = highlights
                    .map(|h| crate::highlight::HighlightQuery::compile(&ast_grep_language::LanguageExt::get_ts_language(&p.lang), h))
                    .transpose()
                    .map_err(|e| IndexError::Pack(format!("{} ({}): {e}", language.name(), p.lang)))?;
                p.server = files.server.clone();
            }
            pack.dialects = dialects;
            set.add(pack);
        }
        Ok(set)
    }

    fn add(&mut self, pack: Pack) {
        self.packs.retain(|p| p.name() != pack.name());
        self.packs.push(pack);
    }

    pub fn for_path(&self, path: &Path) -> Option<&Pack> {
        self.packs.iter().find_map(|p| p.for_path(path))
    }

    /// See `Pack::check_types`.
    pub fn check_types(&self, types: &arch_graph::TypeSystem) -> Result<(), IndexError> {
        self.packs.iter().try_for_each(|p| p.check_types(types))
    }

    pub fn by_name(&self, name: &str) -> Option<&Pack> {
        self.packs.iter().find(|p| p.name() == name)
    }

    /// The pack a code block's fence names (`rust`, `rs`, `tsx`; any case).
    pub fn for_fence(&self, fence: &str) -> Option<&Pack> {
        let fence = fence.trim().to_ascii_lowercase();
        self.packs.iter().find_map(|p| p.for_fence(&fence))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn constant_conditions_are_decided_only_when_built_from_constants() {
        let rust = crate::shipped_packs();
        let logic = &rust.by_name("rust").unwrap().spec.constant_logic;
        for (text, value) in [
            ("false", Some(false)),
            ("(false)", Some(false)),
            ("!true", Some(false)),
            ("any()", Some(false)),
            ("all()", Some(true)),
            ("not(all())", Some(false)),
            ("all(any())", Some(false)),
            ("any(all(), x)", Some(true)),
            ("cfg!(any())", Some(false)),
            ("true && false", Some(false)),
            ("false || true", Some(true)),
            ("feature = \"x\"", None),
            ("input.is_empty()", None),
            ("NEVER", None),
            ("1 == 2", None),
            ("false && input.is_empty()", Some(false)),
            ("\"🔄\").clicked(", None),
        ] {
            assert_eq!(logic.eval(text), value, "{text}");
        }
    }
}

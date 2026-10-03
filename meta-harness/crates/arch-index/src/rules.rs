//! Code rules: ast-grep patterns that are forbidden in, or required of,
//! components of given types. Rules are data (defaults plus project files);
//! they're checked over each component's code and produce report findings.

use std::collections::BTreeMap;
use std::path::Path;

use arch_graph::{Component, Finding, FindingCode, Graph, RelationKind, Severity, Span, Target, TypeSystem};
use ast_grep_config::RuleCore;
use ast_grep_language::LanguageExt;
use serde::Deserialize;

use crate::pack::compile;
use crate::{IndexError, PackSet};

const DEFAULTS: &str = include_str!("../rules/defaults.toml");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleKind {
    /// Must not appear in the component's own code (nested components excluded).
    Forbidden,
    /// Must appear somewhere in the component (nested components included).
    Required,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleSeverity {
    #[default]
    Error,
    Warning,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CodeRuleSpec {
    pub id: String,
    pub language: String,
    pub types: Vec<String>,
    pub kind: RuleKind,
    pub rule: toml::Value,
    pub message: String,
    #[serde(default)]
    pub severity: RuleSeverity,
}

#[derive(Deserialize)]
struct RuleFile {
    #[serde(default, rename = "rule")]
    rules: Vec<CodeRuleSpec>,
}

pub struct CodeRule {
    pub spec: CodeRuleSpec,
    matcher: RuleCore,
}

impl std::fmt::Debug for CodeRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodeRule").field("id", &self.spec.id).finish_non_exhaustive()
    }
}

/// The rules in effect. A rule loaded later replaces one with the same id.
#[derive(Debug, Default)]
pub struct CodeRules {
    pub rules: Vec<CodeRule>,
}

impl CodeRules {
    pub fn defaults(packs: &PackSet, types: &TypeSystem) -> Result<Self, IndexError> {
        let mut rules = Self::default();
        rules.add_toml(DEFAULTS, packs, types).map_err(|e| IndexError::Pack(format!("default rules: {e}")))?;
        Ok(rules)
    }

    /// Adds every rule in a TOML text, checking languages, types and patterns.
    pub fn add_toml(&mut self, text: &str, packs: &PackSet, types: &TypeSystem) -> Result<(), IndexError> {
        let file: RuleFile = toml::from_str(text).map_err(|e| IndexError::Pack(e.to_string()))?;
        for spec in file.rules {
            let what = format!("rule '{}'", spec.id);
            let pack = packs.by_name(&spec.language).ok_or_else(|| IndexError::Pack(format!("{what}: no language pack '{}'", spec.language)))?;
            if spec.types.is_empty() {
                return Err(IndexError::Pack(format!("{what}: no types")));
            }
            for t in &spec.types {
                types.resolve(t).map_err(|e| IndexError::Pack(format!("{what}: {e}")))?;
            }
            let matcher = compile(pack.lang, &spec.rule, &what)?;
            self.rules.retain(|r| r.spec.id != spec.id);
            self.rules.push(CodeRule { spec, matcher });
        }
        Ok(())
    }

    /// Loads every `*.toml` rule file in a directory (sorted by name).
    pub fn load_dir(&mut self, dir: &Path, packs: &PackSet, types: &TypeSystem) -> Result<(), IndexError> {
        let entries = std::fs::read_dir(dir).map_err(|e| IndexError::Io(format!("{}: {e}", dir.display())))?;
        let mut paths: Vec<_> = entries.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|e| e == "toml")).collect();
        paths.sort();
        for path in paths {
            let text = std::fs::read_to_string(&path).map_err(|e| IndexError::Io(format!("{}: {e}", path.display())))?;
            self.add_toml(&text, packs, types).map_err(|e| IndexError::Pack(format!("{}: {e}", path.display())))?;
        }
        Ok(())
    }

    /// Checks `components` (as they are in `graph`) against the rules that
    /// apply to their language and type. Files are read from `root`.
    pub fn check<'a>(&self, components: impl IntoIterator<Item = &'a Component>, graph: &Graph, root: &Path, packs: &PackSet, types: &TypeSystem) -> Vec<Finding> {
        let mut findings = Vec::new();
        let mut files: BTreeMap<String, Option<String>> = BTreeMap::new();
        for c in components {
            let Some(anchor) = &c.anchor else { continue };
            let Ok(ty) = types.resolve(&c.type_name) else { continue };
            let rules: Vec<&CodeRule> =
                self.rules.iter().filter(|r| r.spec.language == anchor.language && r.spec.types.iter().any(|t| ty.is_a(t))).collect();
            let Some(pack) = packs.by_name(&anchor.language) else { continue };
            if rules.is_empty() {
                continue;
            }
            let content = files.entry(anchor.file.as_str().to_owned()).or_insert_with(|| std::fs::read_to_string(root.join(anchor.file.as_str())).ok());
            let Some(content) = content else { continue };
            let doc = pack.lang.ast_grep(content.as_str());
            let own = anchor.span.start_byte as usize..anchor.span.end_byte as usize;
            // Code of nested components (in the same file) isn't this one's own.
            let nested: Vec<Span> = graph
                .outgoing(&c.id, Some(RelationKind::Contains))
                .filter_map(|r| graph.component(&r.target)?.anchor.as_ref().filter(|a| a.file == anchor.file).map(|a| a.span))
                .collect();
            for rule in rules {
                let hits: Vec<Span> = doc
                    .root()
                    .find_all(&rule.matcher)
                    .map(|m| {
                        let n = m.get_node();
                        Span {
                            start_line: n.start_pos().line() as u32 + 1,
                            end_line: n.end_pos().line() as u32 + 1,
                            start_byte: n.range().start as u32,
                            end_byte: n.range().end as u32,
                        }
                    })
                    .filter(|s| own.contains(&(s.start_byte as usize)) && s.end_byte as usize <= own.end)
                    .collect();
                let severity = match rule.spec.severity {
                    RuleSeverity::Error => Severity::Error,
                    RuleSeverity::Warning => Severity::Warning,
                };
                let target = Target::Component { id: c.id.clone() };
                let message = format!("[{}] {}", rule.spec.id, rule.spec.message);
                match rule.spec.kind {
                    RuleKind::Forbidden => {
                        for hit in hits.into_iter().filter(|h| !nested.iter().any(|n| n.contains(h))) {
                            let mut f = Finding::error(FindingCode::ForbiddenPattern, target.clone(), message.clone()).at(anchor.file.clone(), hit);
                            f.severity = severity;
                            findings.push(f);
                        }
                    }
                    RuleKind::Required if hits.is_empty() => {
                        let mut f = Finding::error(FindingCode::MissingRequiredPattern, target.clone(), message).at(anchor.file.clone(), anchor.span);
                        f.severity = severity;
                        findings.push(f);
                    }
                    RuleKind::Required => {}
                }
            }
        }
        findings
    }
}

//! Code rules: ast-grep patterns that are forbidden in, or required of,
//! components of given types. Rules are data (defaults plus project files).
//! They're matched while a file is read, on the same parse as everything
//! else (`hits`); checking a component looks at the matches kept with its
//! file's facts, without reading or parsing the file again.

use std::collections::BTreeMap;
use std::path::Path;

use arch_graph::{Component, Finding, FindingCode, Graph, RelationKind, RepoPath, Severity, Span, Target, TypeSystem};
use ast_grep_config::RuleCore;
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
    /// Per language: what its rules match, as a hash (facts read with other
    /// rules are read again).
    fingerprints: BTreeMap<String, String>,
}

/// Where a rule matched in a file.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, Deserialize)]
pub struct RuleHit {
    pub rule: String,
    pub span: Span,
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
            let pack =
                packs.by_name(&spec.language).ok_or_else(|| IndexError::Pack(format!("{what}: no language pack '{}'", spec.language)))?;
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
        self.fingerprints.clear();
        for rule in &self.rules {
            let entry = self.fingerprints.entry(rule.spec.language.clone()).or_default();
            entry.push_str(&format!("{}={:?};", rule.spec.id, rule.spec.rule));
        }
        for text in self.fingerprints.values_mut() {
            *text = crate::content_hash(text);
        }
        Ok(())
    }

    /// What `language`'s rules match, as a hash ("" when it has none).
    pub fn fingerprint(&self, language: &str) -> &str {
        self.fingerprints.get(language).map_or("", String::as_str)
    }

    /// Every match of `language`'s rules in a parsed file.
    pub(crate) fn hits(
        &self,
        language: &str,
        root: &ast_grep_core::Node<'_, ast_grep_core::tree_sitter::StrDoc<ast_grep_language::SupportLang>>,
    ) -> Vec<RuleHit> {
        let mut out = Vec::new();
        for rule in self.rules.iter().filter(|r| r.spec.language == language) {
            out.extend(
                root.find_all(&rule.matcher).map(|m| RuleHit { rule: rule.spec.id.clone(), span: crate::extract::span(m.get_node()) }),
            );
        }
        out
    }

    /// Loads every `*.toml` rule file in a directory (sorted by name).
    pub fn load_dir(&mut self, dir: &Path, packs: &PackSet, types: &TypeSystem) -> Result<(), IndexError> {
        let entries = std::fs::read_dir(dir).map_err(|e| IndexError::Io(format!("{}: {e}", dir.display())))?;
        let mut paths: Vec<_> =
            entries.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|e| e == "toml")).collect();
        paths.sort();
        for path in paths {
            let text = std::fs::read_to_string(&path).map_err(|e| IndexError::Io(format!("{}: {e}", path.display())))?;
            self.add_toml(&text, packs, types).map_err(|e| IndexError::Pack(format!("{}: {e}", path.display())))?;
        }
        Ok(())
    }

    /// Checks `components` (as they are in `graph`) against the rules that
    /// apply to their language and type, from the matches kept with each
    /// file's facts (`facts` finds a file's).
    pub fn check<'a, 'f>(
        &self,
        components: impl IntoIterator<Item = &'a Component>,
        graph: &Graph,
        facts: &dyn Fn(&RepoPath) -> Option<&'f crate::rows::FileRows>,
        types: &TypeSystem,
    ) -> Vec<Finding> {
        let mut findings = Vec::new();
        for c in components {
            let Some(anchor) = &c.anchor else { continue };
            let Ok(ty) = types.resolve(&c.type_name) else { continue };
            let rules: Vec<&CodeRule> =
                self.rules.iter().filter(|r| r.spec.language == anchor.language && r.spec.types.iter().any(|t| ty.is_a(t))).collect();
            if rules.is_empty() {
                continue;
            }
            // Facts read from other content (or with other rules) say nothing
            // about the code the anchor points at.
            let Some(rows) = facts(&anchor.file) else { continue };
            if rows.file.content_hash != anchor.content_hash
                || crate::rows::rules_read_with(&rows.file.read_version) != self.fingerprint(&anchor.language)
            {
                continue;
            }
            let own = anchor.span.start_byte as usize..anchor.span.end_byte as usize;
            // Code of nested components (in the same file) isn't this one's own.
            let nested: Vec<Span> = graph
                .outgoing(&c.id, Some(RelationKind::Contains))
                .filter_map(|r| graph.component(&r.target)?.anchor.as_ref().filter(|a| a.file == anchor.file).map(|a| a.span))
                .collect();
            for rule in rules {
                let hits: Vec<Span> = rows
                    .rule_hits
                    .iter()
                    .filter(|h| h.rule == rule.spec.id)
                    .map(|h| h.span)
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
                            let mut f =
                                Finding::error(FindingCode::ForbiddenPattern, target.clone(), message.clone()).at(anchor.file.clone(), hit);
                            f.severity = severity;
                            findings.push(f);
                        }
                    }
                    RuleKind::Required if hits.is_empty() => {
                        let mut f = Finding::error(FindingCode::MissingRequiredPattern, target.clone(), message)
                            .at(anchor.file.clone(), anchor.span);
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

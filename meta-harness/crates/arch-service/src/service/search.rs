//! One search for symbols, files and lines. Symbols are matched with the
//! same matcher the cockpit uses (`arch_search::rank`); files and lines
//! come from fff-search over the repo. Every hit says which card it stands
//! for, so picking it can make that card the root.

use std::path::PathBuf;
use std::sync::PoisonError;

use arch_graph::{Component, Graph, Source};
use arch_search::{Found, Hit, RepoFiles, SearchKind, TextLimits};

use super::ArchitectureService;
use crate::{ServiceError, checks};

impl ArchitectureService {
    /// Symbols (in the code, or the code with a plan), files or lines that
    /// match `query`, best first, at most `limit` (the configured default
    /// when none is given, never more than the configured most).
    pub fn search(&self, query: &str, kind: SearchKind, source: &Source, limit: Option<usize>) -> Result<Found, ServiceError> {
        self.timed("search", format!("{} {query:?} ({source})", kind.name()), |t| {
            let limits = &self.limits.search;
            let limit = limit.unwrap_or(limits.results).clamp(1, limits.most);
            let graph = t.stage("source", || self.graph(source))?;
            let mut found = t.stage(kind.name(), || -> Result<Found, ServiceError> {
                Ok(match kind {
                    SearchKind::Symbols => self.find_symbols(query, &graph, limit),
                    SearchKind::Files => self.find_files(query, &graph, limit)?,
                    SearchKind::Text => self.find_lines(query, &graph, limit)?,
                })
            })?;
            t.size(found.hits.len(), "hits");
            found.query = query.to_owned();
            found.kind = kind;
            Ok(found)
        })
    }

    fn find_symbols(&self, query: &str, graph: &Graph, limit: usize) -> Found {
        let components: Vec<&Component> = graph.components().collect();
        // A qualified query (`service::search`, `a/b.rs`) is matched against
        // the whole symbol and file; a plain one against names.
        let qualified = query.contains("::") || query.contains('/') || query.contains('.');
        let haystacks: Vec<String> = components
            .iter()
            .map(|c| match qualified {
                true => format!("{} {}", symbol(c), c.file().map(|f| f.as_str()).unwrap_or_default()),
                false => c.name.clone(),
            })
            .collect();
        let ranked = arch_search::rank(query, &haystacks, arch_search::Matching::shared());
        let total = ranked.len();
        let hits = ranked.into_iter().take(limit).map(|r| self.symbol_hit(components[r.index])).collect();
        Found { hits, total, ..Found::default() }
    }

    fn find_files(&self, query: &str, graph: &Graph, limit: usize) -> Result<Found, ServiceError> {
        self.with_repo_files(|files| {
            let (found, total) = files.files(query, limit);
            let hits = found
                .into_iter()
                .map(|f| {
                    let card = file_card(graph, &f.path);
                    Hit {
                        kind: SearchKind::Files,
                        key: card.map(|c| c.id.0.clone()),
                        detail: card.map(|c| c.type_name.clone()).unwrap_or_else(|| "file".into()),
                        label: f.path.clone(),
                        file: Some(f.path),
                        line: None,
                        callable: false,
                        data: None,
                    }
                })
                .collect();
            Found { hits, total, ..Found::default() }
        })
    }

    fn find_lines(&self, query: &str, graph: &Graph, limit: usize) -> Result<Found, ServiceError> {
        let limits = &self.limits.search;
        let text = TextLimits { per_file: limits.text_per_file, budget_ms: limits.text_ms, max_file_bytes: limits.text_max_file_kib * 1024 };
        self.with_repo_files(|files| {
            let (lines, partial) = files.text(query, limit, &text);
            let hits: Vec<Hit> = lines
                .into_iter()
                .map(|l| {
                    let inside = line_card(graph, &l.path, l.line);
                    let mut hit = inside.map(|c| self.symbol_hit(c)).unwrap_or_else(|| Hit {
                        kind: SearchKind::Text,
                        key: None,
                        label: String::new(),
                        detail: String::new(),
                        file: None,
                        line: None,
                        callable: false,
                        data: None,
                    });
                    hit.kind = SearchKind::Text;
                    hit.detail = match inside {
                        Some(c) => format!("{}:{} · in {}", l.path, l.line, c.name),
                        None => format!("{}:{}", l.path, l.line),
                    };
                    hit.label = l.text;
                    hit.file = Some(l.path);
                    hit.line = Some(l.line);
                    hit
                })
                .collect();
            Found { total: hits.len(), hits, partial, ..Found::default() }
        })
    }

    fn symbol_hit(&self, c: &Component) -> Hit {
        let types = self.types();
        let line = c.anchor.as_ref().map(|a| a.span.start_line);
        let at = match (c.file(), line) {
            (Some(f), Some(l)) => format!(" · {f}:{l}"),
            (Some(f), None) => format!(" · {f} (planned)"),
            _ => String::new(),
        };
        Hit {
            kind: SearchKind::Symbols,
            key: Some(c.id.0.clone()),
            label: c.name.clone(),
            detail: format!("{}{at}", c.type_name),
            file: c.file().map(|f| f.as_str().to_owned()),
            line,
            callable: types.resolve(&c.type_name).is_ok_and(|t| t.allows_signature),
            data: checks::is_data_type(types)(c).then(|| format!("type:{}", c.name)),
        }
    }

    /// The repo's file list, read on first use and kept up to date.
    fn with_repo_files<T>(&self, use_files: impl FnOnce(&RepoFiles) -> T) -> Result<T, ServiceError> {
        {
            let files = self.repo_files.read().unwrap_or_else(PoisonError::into_inner);
            if let Some(files) = files.as_ref() {
                return Ok(use_files(files));
            }
        }
        let mut files = self.repo_files.write().unwrap_or_else(PoisonError::into_inner);
        if files.is_none() {
            *files = Some(RepoFiles::scan(self.root()).map_err(ServiceError::Io)?);
        }
        Ok(use_files(files.as_ref().expect("just listed")))
    }

    /// Tells the file list what changed (paths the walk would skip aside).
    pub(super) fn files_changed(&self, paths: &[PathBuf]) {
        let mut files = self.repo_files.write().unwrap_or_else(PoisonError::into_inner);
        if let Some(files) = files.as_mut() {
            let kept: Vec<PathBuf> = paths.iter().filter(|p| !arch_index::ignored(self.root(), p)).cloned().collect();
            files.changed(&kept);
        }
    }

    /// Forgets the file list (it's read again on the next search).
    pub(super) fn forget_files(&self) {
        *self.repo_files.write().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

fn symbol(c: &Component) -> &str {
    c.anchor.as_ref().map(|a| a.symbol.as_str()).or(c.proposed.as_ref().map(|p| p.symbol.as_str())).unwrap_or(&c.name)
}

/// The card a file stands for: what in it isn't inside anything else in it.
fn file_card<'g>(graph: &'g Graph, file: &str) -> Option<&'g Component> {
    let in_file: Vec<&Component> = graph.in_file(file).collect();
    in_file
        .iter()
        .copied()
        .filter(|c| graph.parent(&c.id).is_none_or(|p| !in_file.iter().any(|o| &o.id == p)))
        .min_by_key(|c| c.anchor.as_ref().map(|a| a.span.start_line).unwrap_or(0))
}

/// The innermost card a line is inside of.
fn line_card<'g>(graph: &'g Graph, file: &str, line: u32) -> Option<&'g Component> {
    graph
        .in_file(file)
        .filter(|c| c.anchor.as_ref().is_some_and(|a| a.span.start_line <= line && line <= a.span.end_line))
        .min_by_key(|c| c.anchor.as_ref().map(|a| a.span.end_line - a.span.start_line).unwrap_or(u32::MAX))
}

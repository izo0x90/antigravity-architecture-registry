//! Where a component lives in code (an anchor, verified by ingestion) or is
//! meant to live (a proposed location, set by the plan).

use serde::{Deserialize, Serialize};

use crate::ModelError;

/// A repository-relative path with `/` separators, no `..`, not absolute.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RepoPath(String);

impl RepoPath {
    pub fn new(path: &str) -> Result<Self, ModelError> {
        let normalized = path.replace('\\', "/");
        let trimmed = normalized.trim_start_matches("./");
        let bad = trimmed.is_empty()
            || trimmed.starts_with('/')
            || trimmed.chars().nth(1) == Some(':')
            || trimmed.split('/').any(|part| part == ".." || part.is_empty());
        if bad {
            return Err(ModelError::InvalidPath(path.to_owned()));
        }
        Ok(Self(trimmed.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn extension(&self) -> Option<&str> {
        self.0.rsplit_once('.').map(|(_, ext)| ext).filter(|ext| !ext.contains('/'))
    }
}

impl TryFrom<String> for RepoPath {
    type Error = ModelError;
    fn try_from(value: String) -> Result<Self, ModelError> {
        Self::new(&value)
    }
}

impl From<RepoPath> for String {
    fn from(path: RepoPath) -> String {
        path.0
    }
}

impl std::fmt::Display for RepoPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A source range. Lines are 1-based, inclusive; bytes are a half-open range.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Span {
    pub start_line: u32,
    pub end_line: u32,
    pub start_byte: u32,
    pub end_byte: u32,
}

impl Span {
    pub fn contains(&self, other: &Span) -> bool {
        self.start_byte <= other.start_byte && other.end_byte <= self.end_byte
    }

    pub fn contains_line(&self, line: u32) -> bool {
        self.start_line <= line && line <= self.end_line
    }
}

/// Where a component is in the code. Its identity is the symbol path; the span
/// and hash are what ingestion saw last and may drift as the file changes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CodeAnchor {
    pub language: String,
    pub file: RepoPath,
    /// Fully qualified symbol, e.g. `meta_harness::server::open_thread`.
    pub symbol: String,
    /// The syntax kind it came from (e.g. `function_item`).
    pub syntax_kind: String,
    pub span: Span,
    /// Hash of the file content when this was observed.
    pub content_hash: String,
}

impl PartialEq for CodeAnchor {
    fn eq(&self, other: &Self) -> bool {
        self.language == other.language && self.file == other.file && self.symbol == other.symbol
    }
}

impl Eq for CodeAnchor {}

/// Where a planned component is meant to go.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposedLocation {
    pub language: String,
    pub file: RepoPath,
    /// The component that will contain it (module, class, ...), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<crate::ComponentId>,
    /// The symbol it will be found as once implemented.
    pub symbol: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor(symbol: &str, start: u32) -> CodeAnchor {
        CodeAnchor {
            language: "rust".into(),
            file: RepoPath::new("src/lib.rs").unwrap(),
            symbol: symbol.into(),
            syntax_kind: "function_item".into(),
            span: Span { start_line: start, end_line: start + 3, start_byte: start * 10, end_byte: start * 10 + 40 },
            content_hash: format!("h{start}"),
        }
    }

    // req: P2.3
    #[test]
    fn anchors_are_identified_by_symbol_not_span() {
        assert_eq!(anchor("crate::run", 3), anchor("crate::run", 40), "same symbol, code moved");
        assert_ne!(anchor("crate::run", 3), anchor("crate::stop", 3));
    }

    // req: P2.3
    #[test]
    fn repo_paths_are_normalized_and_confined_to_the_repo() {
        assert_eq!(RepoPath::new("./src\\lib.rs").unwrap().as_str(), "src/lib.rs");
        for bad in ["/etc/passwd", "../outside.rs", "src/../../x.rs", "C:/x.rs", "", "src//x.rs"] {
            assert!(RepoPath::new(bad).is_err(), "{bad}");
        }
        assert_eq!(RepoPath::new("a/b.test.rs").unwrap().extension(), Some("rs"));
        assert!(serde_json::from_str::<RepoPath>("\"../x\"").is_err());
    }
}

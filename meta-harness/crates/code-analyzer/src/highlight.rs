//! Syntax colouring, on the same parse as everything else read from a file:
//! the language pack's tree-sitter highlight query runs over the tree
//! ast-grep already built. The result is a list of runs (byte ranges, each
//! with a capture name such as `keyword` or `function`), innermost and
//! earliest pattern winning, as tree-sitter's own highlighter decides.

pub use arch_graph::code::Highlights;
use streaming_iterator::StreamingIterator;

use crate::IndexError;

/// A language's compiled highlight query.
pub struct HighlightQuery {
    query: tree_sitter::Query,
}

impl std::fmt::Debug for HighlightQuery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HighlightQuery").field("captures", &self.query.capture_names().len()).finish()
    }
}

impl HighlightQuery {
    pub fn compile(language: &tree_sitter::Language, source: &str) -> Result<Self, IndexError> {
        tree_sitter::Query::new(language, source).map(|query| Self { query }).map_err(|e| IndexError::Pack(format!("highlight query: {e}")))
    }

    /// The runs for a parsed file (`text` is its content).
    pub fn runs(&self, root: tree_sitter::Node<'_>, text: &str) -> Highlights {
        let mut captures: Vec<(u32, u32, usize, u32)> = Vec::new();
        let mut cursor = tree_sitter::QueryCursor::new();
        let mut matches = cursor.matches(&self.query, root, text.as_bytes());
        while let Some(m) = matches.next() {
            for c in m.captures() {
                let r = c.node.byte_range();
                if r.start < r.end {
                    captures.push((r.start as u32, r.end as u32, m.pattern_index, c.index));
                }
            }
        }
        // Outer ranges first, then later patterns before earlier ones: what's
        // painted last (inner, earlier) wins.
        captures.sort_by(|a, b| (b.1 - b.0).cmp(&(a.1 - a.0)).then(b.2.cmp(&a.2)));
        let mut painted: Vec<u32> = vec![0; text.len()];
        for &(start, end, _, capture) in &captures {
            painted[start as usize..end as usize].fill(capture + 1);
        }
        let names = self.query.capture_names();
        let mut used: Vec<u32> = Vec::new();
        let mut out = Highlights::default();
        let mut i = 0;
        while i < painted.len() {
            let c = painted[i];
            let start = i;
            while i < painted.len() && painted[i] == c {
                i += 1;
            }
            if c == 0 {
                continue;
            }
            let at = match used.iter().position(|u| *u == c) {
                Some(at) => at,
                None => {
                    used.push(c);
                    out.names.push(names[(c - 1) as usize].to_owned());
                    used.len() - 1
                }
            };
            out.runs.push([start as u32, i as u32, at as u32]);
        }
        out
    }
}

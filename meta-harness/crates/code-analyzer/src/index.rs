//! The index stage: a language's indexer (its pack's run contract) writes a
//! SCIP index of the project; what it says about each file is kept, with
//! the content hash of the file as it was indexed, so a file that changed
//! since is never matched against it. Positions are turned into byte
//! offsets in the file, by the encoding the indexer declares.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use arch_graph::RepoPath;
use protobuf::Message;
use serde::{Deserialize, Serialize};

use crate::IndexError;

/// SCIP's symbol roles we use.
pub const ROLE_DEFINITION: u32 = 1;
pub const ROLE_WRITE: u32 = 4;
pub const ROLE_READ: u32 = 8;

/// What an indexer said about one file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IndexedFile {
    pub path: RepoPath,
    /// The file's content hash when it was indexed.
    pub content_hash: String,
    /// The symbols named in this file (occurrences point into this list).
    pub symbols: Vec<String>,
    /// Where each symbol occurs: `[start byte, end byte, symbol, roles]`.
    pub occurrences: Vec<[u32; 4]>,
    /// The symbols defined here, with what the indexer knows of them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub defined: Vec<IndexedSymbol>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct IndexedSymbol {
    pub symbol: String,
    /// Its signature as the indexer writes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// What it implements (traits, interfaces, base methods).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub implements: Vec<String>,
}

/// One indexer run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IndexRun {
    /// The indexer pack, and the tool's version as checked.
    pub indexer: String,
    pub tool_version: Option<String>,
    pub files: Vec<IndexedFile>,
    /// Occurrences that couldn't be placed (an unknown encoding on a line
    /// that isn't plain ASCII): left out rather than guessed.
    pub dropped: usize,
    pub millis: u64,
}

/// Runs `program args` with a time limit; the output file is the caller's.
fn run_with_limit(program: &str, args: &[String], cwd: &Path, limit: Duration) -> Result<(), IndexError> {
    let mut child = std::process::Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| IndexError::Io(format!("{program}: {e}")))?;
    let started = Instant::now();
    loop {
        match child.try_wait().map_err(|e| IndexError::Io(format!("{program}: {e}")))? {
            Some(status) if status.success() => return Ok(()),
            Some(status) => {
                let mut err = String::new();
                if let Some(mut stderr) = child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut stderr, &mut err);
                }
                let tail: Vec<&str> = err.lines().rev().take(5).collect();
                return Err(IndexError::Io(format!("{program} failed ({status}): {}", tail.into_iter().rev().collect::<Vec<_>>().join(" | "))));
            }
            None if started.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(IndexError::Io(format!("{program} took longer than {}s", limit.as_secs())));
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// Runs indexer pack `indexer` over `root` and reads what it wrote. Only
/// files in `hashes` (path → content hash before the run) are kept, and
/// only if they still have that content after it.
pub fn run(catalog: &nah_packs::Catalog, lock: &nah_packs::Lock, indexer: &str, root: &Path, hashes: &BTreeMap<RepoPath, String>) -> Result<IndexRun, IndexError> {
    let started = Instant::now();
    let pack = catalog.get(indexer).map_err(|e| IndexError::Pack(e.to_string()))?;
    let tool_version = match nah_packs::state(catalog, lock, indexer).map_err(|e| IndexError::Pack(e.to_string()))? {
        nah_packs::State::Ready { tool_version } => tool_version,
        other => return Err(IndexError::Pack(format!("{indexer} isn't ready: {other:?}"))),
    };
    let out = std::env::temp_dir().join(format!("nah-index-{}-{}-{}.scip", pack.name(), std::process::id(), started.elapsed().as_nanos()));
    let invocation = nah_packs::invocation(catalog, lock, indexer, root, &out).map_err(|e| IndexError::Pack(e.to_string()))?;
    let ran = run_with_limit(&invocation.program, &invocation.args, root, invocation.timeout);
    let read = ran.and_then(|()| std::fs::read(&out).map_err(|e| IndexError::Io(format!("{}: {e}", out.display()))));
    let _ = std::fs::remove_file(&out);
    let bytes = read?;
    let mut run = read_index(&bytes, root, hashes)?;
    run.indexer = indexer.to_owned();
    run.tool_version = tool_version;
    run.millis = started.elapsed().as_millis() as u64;
    Ok(run)
}

/// Reads a SCIP index: the files in `hashes` whose content still matches.
pub fn read_index(bytes: &[u8], root: &Path, hashes: &BTreeMap<RepoPath, String>) -> Result<IndexRun, IndexError> {
    let index = scip::types::Index::parse_from_bytes(bytes).map_err(|e| IndexError::Format(format!("SCIP: {e}")))?;
    let mut run = IndexRun { indexer: String::new(), tool_version: None, files: Vec::new(), dropped: 0, millis: 0 };
    for doc in &index.documents {
        let Ok(path) = RepoPath::new(&doc.relative_path) else { continue };
        let Some(expected) = hashes.get(&path) else { continue };
        let absolute: PathBuf = root.join(path.as_str());
        let Ok(text) = std::fs::read_to_string(&absolute) else { continue };
        if &crate::content_hash(&text) != expected {
            continue;
        }
        let encoding = doc.position_encoding.enum_value_or_default();
        let lines = LineStarts::new(&text);
        let mut file = IndexedFile { path, content_hash: expected.clone(), symbols: Vec::new(), occurrences: Vec::new(), defined: Vec::new() };
        let mut ids: BTreeMap<&str, u32> = BTreeMap::new();
        for occ in &doc.occurrences {
            if occ.symbol.is_empty() || occ.symbol.starts_with("local ") {
                continue;
            }
            let Some((sl, sc, el, ec)) = range_of(occ) else { continue };
            let (Some(start), Some(end)) = (lines.byte(&text, sl, sc, encoding), lines.byte(&text, el, ec, encoding)) else {
                run.dropped += 1;
                continue;
            };
            let next = ids.len() as u32;
            let id = *ids.entry(occ.symbol.as_str()).or_insert_with(|| {
                file.symbols.push(occ.symbol.clone());
                next
            });
            file.occurrences.push([start, end, id, occ.symbol_roles as u32]);
        }
        for info in &doc.symbols {
            let signature = info.signature_documentation.as_ref().map(|s| s.text.clone()).filter(|t| !t.is_empty());
            let implements = info.relationships.iter().filter(|r| r.is_implementation).map(|r| r.symbol.clone()).collect();
            file.defined.push(IndexedSymbol { symbol: info.symbol.clone(), signature, implements });
        }
        file.occurrences.sort();
        run.files.push(file);
    }
    run.files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(run)
}

/// (start line, start character, end line, end character).
fn range_of(occ: &scip::types::Occurrence) -> Option<(usize, usize, usize, usize)> {
    use scip::types::occurrence::Typed_range;
    let u = |v: i32| usize::try_from(v).ok();
    match &occ.typed_range {
        Some(Typed_range::SingleLineRange(r)) => Some((u(r.line)?, u(r.start_character)?, u(r.line)?, u(r.end_character)?)),
        Some(Typed_range::MultiLineRange(r)) => Some((u(r.start_line)?, u(r.start_character)?, u(r.end_line)?, u(r.end_character)?)),
        // A range shape added after this was written: not placed.
        Some(_) => None,
        None => match occ.range.as_slice() {
            [l, s, e] => Some((u(*l)?, u(*s)?, u(*l)?, u(*e)?)),
            [sl, sc, el, ec] => Some((u(*sl)?, u(*sc)?, u(*el)?, u(*ec)?)),
            _ => None,
        },
    }
}

/// Byte offsets of line starts.
struct LineStarts(Vec<usize>);

impl LineStarts {
    fn new(text: &str) -> Self {
        Self(std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect())
    }

    /// The byte offset of `character` on `line`, counted in `encoding`.
    /// Unspecified: only when the line up to there is ASCII (all agree).
    fn byte(&self, text: &str, line: usize, character: usize, encoding: scip::types::PositionEncoding) -> Option<u32> {
        use scip::types::PositionEncoding as E;
        let start = *self.0.get(line)?;
        let end = self.0.get(line + 1).copied().unwrap_or(text.len());
        let line_text = &text[start..end];
        let offset = match encoding {
            E::UTF8CodeUnitOffsetFromLineStart => (character <= line_text.len() && line_text.is_char_boundary(character)).then_some(character)?,
            E::UTF16CodeUnitOffsetFromLineStart => {
                let mut units = 0;
                let mut found = None;
                for (i, c) in line_text.char_indices() {
                    if units == character {
                        found = Some(i);
                        break;
                    }
                    units += c.len_utf16();
                }
                found.or((units == character).then_some(line_text.len()))?
            }
            E::UTF32CodeUnitOffsetFromLineStart => line_text.char_indices().map(|(i, _)| i).chain(std::iter::once(line_text.len())).nth(character)?,
            E::UnspecifiedPositionEncoding => {
                let prefix = line_text.get(..character)?;
                prefix.is_ascii().then_some(character)?
            }
        };
        u32::try_from(start + offset).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scip::types::PositionEncoding as E;

    #[test]
    fn positions_become_byte_offsets_by_the_declared_encoding() {
        let text = "fn a() {}\nlet é = b();\n";
        let lines = LineStarts::new(text);
        let b = text.find("b()").unwrap() as u32;
        // `é` is 2 bytes, 1 UTF-16 unit, 1 code point.
        assert_eq!(lines.byte(text, 1, 9, E::UTF8CodeUnitOffsetFromLineStart), Some(b));
        assert_eq!(lines.byte(text, 1, 8, E::UTF16CodeUnitOffsetFromLineStart), Some(b));
        assert_eq!(lines.byte(text, 1, 8, E::UTF32CodeUnitOffsetFromLineStart), Some(b));
        assert_eq!(lines.byte(text, 1, 8, E::UnspecifiedPositionEncoding), None, "not plain ASCII and no encoding: not guessed");
        assert_eq!(lines.byte(text, 0, 3, E::UnspecifiedPositionEncoding), Some(3));
    }
}

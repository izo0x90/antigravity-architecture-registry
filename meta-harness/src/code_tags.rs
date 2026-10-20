//! The code as owner of tags' subjects: a component (a symbol: the index
//! follows it through edits) and lines of a file. Code lasts (git, the
//! changes' snapshots): nothing of it is written to the tags' logs.
//!
//! Lines are kept as they were at a snapshot of the files (taken when
//! they're tagged; one taken just before is used again when nothing
//! changed since). When the files change, where the lines are now is
//! worked out from what changed since that snapshot: lines before them
//! added or taken away move them (a new snapshot is kept for where they
//! are); a change among them leaves them where they were, said as
//! changed; their file gone, they're gone.

use std::sync::{Arc, Mutex, PoisonError};

use arch_graph::{ComponentId, Source};
use arch_service::ArchitectureService;
use harness_contracts::Subject;
use harness_contracts::changes::{Comparison, Group, Hunk, Point};
use nah_changes::{Changes, Snapshot};

pub struct Code {
    arch: Arc<ArchitectureService>,
    changes: Option<Arc<Changes>>,
    /// The snapshot taken last (used again while the files are as it has them).
    last: Mutex<Option<Snapshot>>,
}

/// Where lines kept at a snapshot are in the files now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Now {
    /// Here (moved by changes before them, or not at all).
    At(u32, u32),
    /// Changed among them since: where they were.
    Changed,
    /// Their file's gone.
    Gone,
}

/// Where lines `first..=last` are after `hunks` (the changes from where
/// they were to the files now, in order).
fn map_lines(hunks: &[Hunk], first: u32, last: u32) -> Now {
    let mut shift: i64 = 0;
    for hunk in hunks {
        // Lines it takes away (none: it adds after `old_start`).
        let (from, to) = if hunk.old_lines == 0 { (hunk.old_start + 1, hunk.old_start) } else { (hunk.old_start, hunk.old_start + hunk.old_lines - 1) };
        if to < first && (hunk.old_lines > 0 || hunk.old_start < first) {
            shift += i64::from(hunk.new_lines) - i64::from(hunk.old_lines);
        } else if from <= last {
            return Now::Changed;
        } else {
            break;
        }
    }
    let moved = |n: u32| (i64::from(n) + shift).max(1) as u32;
    Now::At(moved(first), moved(last))
}

impl Code {
    pub fn new(arch: Arc<ArchitectureService>, changes: Option<Arc<Changes>>) -> Arc<Self> {
        Arc::new(Self { arch, changes, last: Mutex::default() })
    }

    /// The files now, as a snapshot (the last one again when they're as it has them).
    fn snapshot(&self) -> Result<String, String> {
        let changes = self.changes.as_ref().ok_or("the project isn't a git repository: its lines can't be kept where they are")?;
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        let (snapshot, _) = changes.snapshot("tagged lines", last.as_ref()).map_err(|e| e.to_string())?;
        *last = Some(snapshot.clone());
        Ok(snapshot.id)
    }

    /// `path` as a file of the project: inside it however it's named or
    /// linked, and not one it leaves out (`.nah/ignore`, generated output);
    /// none otherwise.
    fn file(&self, path: &str) -> Option<std::path::PathBuf> {
        let root = self.arch.root().canonicalize().ok()?;
        let real = root.join(arch_graph::RepoPath::new(path).ok()?.as_str()).canonicalize().ok()?;
        (real.starts_with(&root) && real.is_file() && !self.arch.ignores().is_ignored_as(&real, false)).then_some(real)
    }

    /// Where lines kept at `snapshot` are now.
    fn now_at(&self, path: &str, first: u32, last: u32, snapshot: &str) -> Now {
        if self.file(path).is_none() {
            return Now::Gone;
        }
        let Some(changes) = &self.changes else { return Now::At(first, last) };
        let comparison = Comparison { from: Point::Snapshot(snapshot.to_owned()), to: Point::Files };
        match changes.file(&comparison, path, Group::Changes, 0) {
            Ok(file) => map_lines(&file.hunks, first, last),
            // The snapshot's gone (pruned): as they're numbered.
            Err(_) => Now::At(first, last),
        }
    }

    /// The text of lines `first..=last` of `path` now.
    fn lines_text(&self, path: &str, first: u32, last: u32) -> Option<String> {
        let text = std::fs::read_to_string(self.file(path)?).ok()?;
        let lines: Vec<&str> = text.lines().skip(first.saturating_sub(1) as usize).take((last + 1).saturating_sub(first) as usize).collect();
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    /// Lines `first..=last` of `path` as snapshot `snapshot` has them.
    fn as_tagged(&self, path: &str, first: u32, last: u32, snapshot: &str) -> Option<String> {
        let text = self.changes.as_ref()?.text_at(snapshot, path).ok()??;
        let lines: Vec<&str> = text.lines().skip(first.saturating_sub(1) as usize).take((last + 1).saturating_sub(first) as usize).collect();
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    /// A component: its kind, name and file's line, its lines.
    fn component(&self, id: &ComponentId) -> Option<(String, String, u32, u32)> {
        let graph = self.arch.graph(&Source::Code).ok()?;
        let c = graph.component(id)?;
        let anchor = c.anchor.as_ref()?;
        Some((format!("{} {}", c.type_name, c.name), anchor.file.as_str().to_owned(), anchor.span.start_line, anchor.span.end_line))
    }
}

impl nah_tags::Resolve for Code {
    fn name(&self, subject: &Subject) -> Option<String> {
        match subject {
            // Its id first: named back by it.
            Subject::Component { id } => Some(match self.component(id) {
                Some((what, file, start, _)) => format!("component {id} ({what}, {file}:{start})"),
                None => format!("component {id} (gone from the code)"),
            }),
            Subject::CodeLines { path, first, last, snapshot } => {
                let lines = if first == last { format!("{path} line {first}") } else { format!("{path} lines {first}-{last}") };
                Some(match self.now_at(path, *first, *last, snapshot) {
                    Now::At(..) => lines,
                    Now::Changed => format!("{lines} (changed since tagged)"),
                    Now::Gone => format!("{lines} (its file's gone)"),
                })
            }
            _ => None,
        }
    }

    fn now(&self, subject: &Subject) -> Option<String> {
        match subject {
            Subject::Component { id } => {
                let (_, file, start, end) = self.component(id)?;
                self.lines_text(&file, start, end)
            }
            // Where they are now; changed or gone since, as they were.
            Subject::CodeLines { path, first, last, snapshot } => match self.now_at(path, *first, *last, snapshot) {
                Now::At(f, l) => self.lines_text(path, f, l),
                Now::Changed | Now::Gone => self.as_tagged(path, *first, *last, snapshot),
            },
            _ => None,
        }
    }

    /// `component <id>`, or `<path> line <n>` / `<path> lines <a>-<b>` (the
    /// files as they are now).
    fn named(&self, words: &str) -> Option<Result<Subject, String>> {
        let words = words.trim();
        if let Some(id) = words.strip_prefix("component ") {
            let id = ComponentId::new(id.split(" (").next().unwrap_or(id).trim());
            return Some(if self.component(&id).is_some() { Ok(Subject::Component { id }) } else { Err(format!("there's no component {id} in the code")) });
        }
        let (path, lines) = words.split_once(" lines ").or_else(|| words.split_once(" line "))?;
        let lines = lines.split(" (").next().unwrap_or(lines);
        Some((|| {
            let number = |n: &str| n.trim().parse::<u32>().map_err(|_| format!("{n:?} isn't a line number"));
            let (first, last) = match lines.split_once('-') {
                Some((a, b)) => (number(a)?, number(b)?),
                None => (number(lines)?, number(lines)?),
            };
            if self.file(path).is_none() {
                return Err(format!("{path} isn't a file of the project"));
            }
            Ok(Subject::CodeLines { path: path.to_owned(), first: first.min(last), last: first.max(last), snapshot: String::new() })
        })())
    }

    fn there(&self, subject: &Subject) -> bool {
        match subject {
            Subject::Component { id } => self.component(id).is_some(),
            Subject::CodeLines { path, first, last, snapshot } => matches!(self.now_at(path, *first, *last, snapshot), Now::At(..)),
            _ => false,
        }
    }

    /// Lines tagged: kept with the snapshot of the files they're in.
    fn settle(&self, subject: &Subject) -> Result<Subject, String> {
        match subject {
            Subject::CodeLines { path, first, last, snapshot } if snapshot.is_empty() => Ok(Subject::CodeLines { path: path.clone(), first: *first, last: *last, snapshot: self.snapshot()? }),
            other => Ok(other.clone()),
        }
    }

    /// Lines moved by changes before them: where they are now (kept with
    /// a snapshot of the files as they are).
    fn moved(&self, subject: &Subject) -> Option<Subject> {
        let Subject::CodeLines { path, first, last, snapshot } = subject else { return None };
        match self.now_at(path, *first, *last, snapshot) {
            Now::At(f, l) if (f, l) != (*first, *last) => Some(Subject::CodeLines { path: path.clone(), first: f, last: l, snapshot: self.snapshot().ok()? }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hunk(old_start: u32, old_lines: u32, new_start: u32, new_lines: u32) -> Hunk {
        Hunk { old_start, old_lines, new_start, new_lines, lines: Vec::new() }
    }

    #[test]
    fn lines_move_with_changes_before_them_and_are_changed_by_changes_among_them() {
        assert_eq!(map_lines(&[], 10, 12), Now::At(10, 12));
        assert_eq!(map_lines(&[hunk(2, 0, 3, 3)], 10, 12), Now::At(13, 15), "three added before");
        assert_eq!(map_lines(&[hunk(2, 2, 2, 0)], 10, 12), Now::At(8, 10), "two taken away before");
        assert_eq!(map_lines(&[hunk(11, 1, 11, 1)], 10, 12), Now::Changed, "one among them");
        assert_eq!(map_lines(&[hunk(10, 0, 11, 2)], 10, 12), Now::Changed, "added inside");
        assert_eq!(map_lines(&[hunk(20, 1, 20, 4)], 10, 12), Now::At(10, 12), "after them");
        assert_eq!(map_lines(&[hunk(2, 0, 3, 1), hunk(30, 1, 31, 1)], 10, 12), Now::At(11, 13));
    }
}

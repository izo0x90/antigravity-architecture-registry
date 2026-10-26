//! Changes in the project's files (git), as every part speaks them: the
//! server that reads git, the cockpit's views, the agents' tools.
//!
//! A [`Comparison`] says what's compared with what. The default is the
//! uncommitted changes against the last commit, kept apart as git does:
//! staged (the index against `HEAD`), unstaged (the files against the
//! index) and new untracked files. Points are written as short strings in
//! URLs and config (`HEAD`, `index`, `files`, `rev:<name>`, `snapshot:<id>`).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// One side of a comparison.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub enum Point {
    /// The last commit.
    Head,
    /// The staged files (git's index).
    Index,
    /// The files as they are now.
    Files,
    /// A commit or a branch, by any name git takes.
    Rev(String),
    /// A snapshot the timeline took (its id).
    Snapshot(String),
}

impl fmt::Display for Point {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Head => f.write_str("HEAD"),
            Self::Index => f.write_str("index"),
            Self::Files => f.write_str("files"),
            Self::Rev(rev) => write!(f, "rev:{rev}"),
            Self::Snapshot(id) => write!(f, "snapshot:{id}"),
        }
    }
}

impl FromStr for Point {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "HEAD" | "head" => Ok(Self::Head),
            "index" | "staged" => Ok(Self::Index),
            "files" | "working" => Ok(Self::Files),
            _ => match s.split_once(':') {
                Some(("rev", rev)) if !rev.is_empty() => Ok(Self::Rev(rev.to_owned())),
                Some(("snapshot", id)) if !id.is_empty() => Ok(Self::Snapshot(id.to_owned())),
                _ => Err(format!("`{s}` isn't a point to compare (HEAD, index, files, rev:<name>, snapshot:<id>)")),
            },
        }
    }
}

impl From<Point> for String {
    fn from(point: Point) -> Self {
        point.to_string()
    }
}

impl TryFrom<String> for Point {
    type Error = String;

    fn try_from(s: String) -> Result<Self, String> {
        s.parse()
    }
}

/// What's compared with what.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Comparison {
    pub from: Point,
    pub to: Point,
}

impl Default for Comparison {
    /// The uncommitted changes against the last commit.
    fn default() -> Self {
        Self { from: Point::Head, to: Point::Files }
    }
}

impl Comparison {
    /// As one string, `from..to` (what `parse` reads).
    pub fn range(&self) -> String {
        format!("{}..{}", self.from, self.to)
    }

    /// The uncommitted changes, kept apart as staged, unstaged and
    /// untracked (as git status does).
    pub fn is_uncommitted(&self) -> bool {
        self.from == Point::Head && self.to == Point::Files
    }

    /// The files as they are against where this comparison starts, the
    /// uncommitted groups together: what a view of the files themselves
    /// (the code view) marks.
    pub fn on_files(&self) -> Comparison {
        let from = match &self.from {
            Point::Head => Point::Rev("HEAD".into()),
            p => p.clone(),
        };
        Comparison { from, to: Point::Files }
    }
}

impl std::str::FromStr for Comparison {
    type Err = String;

    /// `from..to` (`HEAD..files`, `rev:main..snapshot:42`).
    fn from_str(s: &str) -> Result<Self, String> {
        let (from, to) = s.split_once("..").ok_or_else(|| format!("{s}: write from..to (HEAD..files, rev:main..HEAD)"))?;
        Ok(Self { from: from.parse()?, to: to.parse()? })
    }
}

impl fmt::Display for Comparison {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_uncommitted() { f.write_str("uncommitted changes") } else { write!(f, "{} → {}", self.from, self.to) }
    }
}

/// Which of the uncommitted changes a file's are (git status's groups);
/// any other comparison's are `Changes`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    /// Staged: what the next commit takes.
    Staged,
    /// Edited since it was staged (or never staged).
    Unstaged,
    /// New, not yet known to git.
    Untracked,
    /// Between two points that aren't the uncommitted ones.
    Changes,
}

impl Group {
    /// What the user can do with its changes: stage them (`true`), unstage
    /// them (`false`); another comparison's, neither.
    pub fn staging(self) -> Option<bool> {
        match self {
            Self::Unstaged | Self::Untracked => Some(true),
            Self::Staged => Some(false),
            Self::Changes => None,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Staged => "Staged",
            Self::Unstaged => "Unstaged",
            Self::Untracked => "Untracked",
            Self::Changes => "Changes",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    /// A file became a link, or the other way.
    TypeChanged,
}

/// A changed file: where, how, and how much.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedFile {
    pub path: String,
    /// Its path before, when it was renamed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub group: Group,
    pub added: u32,
    pub removed: u32,
    #[serde(default)]
    pub binary: bool,
}

/// The files a comparison changes (a page of them: `offset` on, `total` in all).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeList {
    pub comparison: Comparison,
    pub files: Vec<ChangedFile>,
    pub offset: usize,
    pub total: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineKind {
    /// Unchanged, shown around a change.
    Context,
    Added,
    Removed,
}

/// One line of a change, with its number on each side it's on (1-based).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffLine {
    pub kind: LineKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new: Option<u32>,
    pub text: String,
}

/// The most text compared in one ask (bytes, both sides of every pair):
/// the server refuses more, the cockpit doesn't ask it.
pub const TEXTS_MOST: usize = 1024 * 1024;

/// A run of changed lines and the unchanged ones around them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hunk {
    /// Where it starts and how many lines it spans, on each side.
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

impl Hunk {
    /// The lines it covers in the new version (`new_start..new_start + new_lines`).
    pub fn new_range(&self) -> std::ops::Range<u32> {
        self.new_start..self.new_start + self.new_lines.max(1)
    }
}

/// One file's changes in a comparison and group.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChanges {
    pub comparison: Comparison,
    pub path: String,
    pub group: Group,
    /// How many unchanged lines show around each change.
    pub context: u32,
    #[serde(default)]
    pub binary: bool,
    pub hunks: Vec<Hunk>,
}

/// A component its file's changes touch: how many lines, and which runs
/// (in its file's new version).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentChanges {
    pub component: String,
    pub file: String,
    pub added: u32,
    pub removed: u32,
    /// The changed runs inside it (new side: start, lines).
    pub runs: Vec<(u32, u32)>,
    /// Some of them are its own: in none of its parts (a container whose
    /// changes are all in its functions isn't).
    #[serde(default)]
    pub direct: bool,
}

/// What a comparison changes in the components (where the index knows them).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentChangeList {
    pub comparison: Comparison,
    pub components: Vec<ComponentChanges>,
    /// Changed files the index has out of date: their components may have
    /// moved, so they're left out.
    pub out_of_date: Vec<String>,
}

/// A session that was working during a window: its slot, its thread and
/// the turn.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Working {
    pub slot: String,
    pub thread: String,
    pub turn: String,
}

/// Between two snapshots on the timeline: what changed, and who was
/// working. Snapshots are taken whenever any session's turn starts or ends.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    /// Its snapshots (`snapshot:<id>` points): before, and after (none
    /// while it's still open: up to the files now).
    pub from: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    pub started_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_ms: Option<u64>,
    /// The sessions working during it (none: nobody NAH runs; edits are
    /// yours or another tool's).
    pub working: Vec<Working>,
    pub files: u32,
}

impl Window {
    /// What it changed, as a comparison.
    pub fn comparison(&self) -> Comparison {
        Comparison { from: Point::Snapshot(self.from.clone()), to: self.to.clone().map_or(Point::Files, Point::Snapshot) }
    }
}

/// A turn's changes: from its start to its end, and who else was working
/// meanwhile (named, never guessed apart).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnChanges {
    pub thread: String,
    pub turn: String,
    pub comparison: Comparison,
    pub files: u32,
    /// Other sessions working while it ran.
    pub alongside: Vec<Working>,
}

/// When the project's files changed and who was working: the windows
/// between snapshots (oldest first; the last one open, up to the files
/// now), and each turn's changes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timeline {
    pub windows: Vec<Window>,
    pub turns: Vec<TurnChanges>,
}

impl Timeline {
    /// A turn's changes, if it was seen.
    pub fn turn(&self, turn: &str) -> Option<&TurnChanges> {
        self.turns.iter().find(|t| t.turn == turn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_read_and_write_as_short_strings() {
        for (text, point) in [
            ("HEAD", Point::Head),
            ("index", Point::Index),
            ("files", Point::Files),
            ("rev:main", Point::Rev("main".into())),
            ("snapshot:42", Point::Snapshot("42".into())),
        ] {
            assert_eq!(text.parse::<Point>().unwrap(), point);
            assert_eq!(point.to_string(), text);
        }
        assert!("nowhere".parse::<Point>().is_err());
        let json = serde_json::to_string(&Comparison::default()).unwrap();
        assert_eq!(json, r#"{"from":"HEAD","to":"files"}"#);
        assert!(Comparison::default().is_uncommitted());
        let c = Comparison { from: Point::Rev("main".into()), to: Point::Snapshot("42".into()) };
        assert_eq!(c.range().parse::<Comparison>().unwrap(), c);
        assert!("HEAD".parse::<Comparison>().is_err());
    }
}

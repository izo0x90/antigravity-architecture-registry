//! The repo's files and their contents, found with fff-search. fff lists
//! the files (its walk respects .gitignore) and the caller's one definition
//! of "ignored" takes out what it leaves out (`.nah/ignore`, generated
//! output); the list is kept up to date from the changes the server's
//! watcher sees, and listed again whenever fff says it's time to (it holds a
//! bounded number of files added since its walk). Nothing outside the repo
//! is searched. A folder of the server's own (its logs, which git ignores)
//! is listed the same way, its walk started in it.

use std::path::{Path, PathBuf};

use fff_search::file_picker::FilePicker;
use fff_search::{AiGrepConfig, ConstraintVec, FFFQuery, FilePickerOptions, FileSearchConfig, FuzzyQuery, FuzzySearchOptions, GitRecencyConfig, GrepMode, GrepSearchOptions, PaginationArgs, QueryParser};

/// Limits on a search through file contents.
#[derive(Clone, Debug, PartialEq)]
pub struct TextLimits {
    /// Lines kept from one file.
    pub per_file: usize,
    /// Time spent before answering with what was found.
    pub budget_ms: u64,
    /// Bigger files aren't searched.
    pub max_file_bytes: u64,
}

/// A file found by its path.
#[derive(Clone, Debug, PartialEq)]
pub struct FoundFile {
    /// Relative to the repo, with `/`.
    pub path: String,
}

/// A line found by its text.
#[derive(Clone, Debug, PartialEq)]
pub struct FoundLine {
    pub path: String,
    /// 1-based.
    pub line: u32,
    pub text: String,
}

/// Whether the caller's one definition of "ignored" leaves a path out.
pub type Ignored = Box<dyn Fn(&Path) -> bool + Send + Sync>;

pub struct RepoFiles {
    root: PathBuf,
    picker: FilePicker,
    ignored: Ignored,
}

impl RepoFiles {
    /// Lists the repo's files with fff, less what `ignored` leaves out.
    pub fn scan(root: &Path, ignored: Ignored) -> Result<Self, String> {
        let options = FilePickerOptions {
            base_path: root.to_string_lossy().into_owned(),
            // The server's own watcher says what changed (see `changed`).
            watch: false,
            git_recency: GitRecencyConfig { enabled: false, ..GitRecencyConfig::default() },
            ..FilePickerOptions::default()
        };
        let picker = FilePicker::new(options).map_err(|e| format!("listing {}: {e}", root.display()))?;
        let mut files = Self { root: root.to_path_buf(), picker, ignored };
        files.list()?;
        Ok(files)
    }

    /// fff's walk, then out with what the caller ignores.
    fn list(&mut self) -> Result<(), String> {
        self.picker.collect_files().map_err(|e| format!("listing {}: {e}", self.root.display()))?;
        let left_out: Vec<PathBuf> = self.picker.get_files().iter().map(|f| f.absolute_path(&self.picker, &self.root)).filter(|p| (self.ignored)(p)).collect();
        for path in left_out {
            self.picker.remove_file_by_path(&path);
        }
        Ok(())
    }

    /// How many files are listed.
    pub fn len(&self) -> usize {
        self.picker.live_file_count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The lines of these files, searched within `limits`.
    pub fn lines(&self, limits: TextLimits) -> Lines<'_> {
        Lines { files: self, limits }
    }

    /// Takes in changed paths (created, modified, removed; files or
    /// directories gone). Paths outside the repo, or ignored, are left out.
    /// Fails only when listing again does.
    pub fn changed(&mut self, paths: &[PathBuf]) -> Result<(), String> {
        // fff's walk lists hidden files only in a git repository; one that
        // arrives later is taken in the same way.
        let hidden_ok = self.picker.has_git_repo();
        let hidden = |p: &Path| p.strip_prefix(&self.root).is_ok_and(|rest| rest.components().any(|c| c.as_os_str().to_string_lossy().starts_with('.')));
        let kept: Vec<&PathBuf> = paths.iter().filter(|p| p.starts_with(&self.root) && !(self.ignored)(p) && (hidden_ok || !hidden(p))).collect();
        for path in kept {
            if path.is_file() {
                // fff holds only so many files added since its walk: when
                // one more wouldn't fit, its walk lists them all again (this
                // one with them).
                let new = self.picker.get_file_by_path(path).is_none();
                if new && self.picker.get_overflow_files().len() + 1 >= fff_search::constants::MAX_OVERFLOW_FILES {
                    self.list()?;
                } else {
                    self.picker.handle_create_or_modify(path);
                }
            } else if !path.exists() && !self.picker.remove_file_by_path(path) {
                self.picker.remove_all_files_in_dir(path);
            }
        }
        Ok(())
    }
}

impl crate::Search for RepoFiles {
    type Found = FoundFile;

    /// Files whose path matches `query`, best first.
    fn search(&self, query: &str, limit: usize) -> crate::Searched<FoundFile> {
        let parsed = QueryParser::new(FileSearchConfig).parse(query);
        let found = self.picker.fuzzy_search(&parsed, None, FuzzySearchOptions { max_threads: 0, current_file: None, pagination: PaginationArgs { offset: 0, limit }, ..FuzzySearchOptions::default() });
        let files = found.items.iter().map(|f| FoundFile { path: f.relative_path(&self.picker) }).collect();
        crate::Searched { found: files, total: found.total_matched, partial: false }
    }
}

/// The lines of a repo's files, searched within a time and size budget.
pub struct Lines<'a> {
    files: &'a RepoFiles,
    limits: TextLimits,
}

impl crate::Search for Lines<'_> {
    type Found = FoundLine;

    /// Lines containing `query` as written (case ignored unless it has
    /// capitals; `*.rs` and `src/` narrow the files), the first `limit`;
    /// partial when the time limit cut it short.
    fn search(&self, query: &str, limit: usize) -> crate::Searched<FoundLine> {
        self.grep(&QueryParser::new(AiGrepConfig).parse(query), limit)
    }
}

impl Lines<'_> {
    /// Lines containing `text`, all of it as it's written (nothing in it
    /// narrows the files; case ignored unless it has capitals), the first
    /// `limit`; partial when the time limit cut it short.
    pub fn containing(&self, text: &str, limit: usize) -> crate::Searched<FoundLine> {
        let text = text.trim();
        let query = FFFQuery { raw_query: text, constraints: ConstraintVec::new(), fuzzy_query: if text.is_empty() { FuzzyQuery::Empty } else { FuzzyQuery::Text(text) }, location: None };
        self.grep(&query, limit)
    }

    fn grep(&self, query: &FFFQuery<'_>, limit: usize) -> crate::Searched<FoundLine> {
        let picker = &self.files.picker;
        let options = GrepSearchOptions {
            max_file_size: self.limits.max_file_bytes,
            max_matches_per_file: self.limits.per_file,
            page_limit: limit,
            mode: GrepMode::PlainText,
            time_budget_ms: self.limits.budget_ms,
            enforce_time_budget: true,
            trim_whitespace: true,
            ..GrepSearchOptions::default()
        };
        let found = picker.grep(query, &options);
        let lines: Vec<FoundLine> = found
            .matches
            .iter()
            .filter_map(|m| {
                let file = found.files.get(m.file_index)?;
                Some(FoundLine { path: file.relative_path(picker), line: m.line_number as u32, text: m.line_content.clone() })
            })
            .collect();
        let partial = found.next_file_offset != 0 || found.total_files_searched < found.filtered_file_count;
        crate::Searched { total: lines.len(), found: lines, partial }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Search;

    fn limits() -> TextLimits {
        TextLimits { per_file: 5, budget_ms: 1000, max_file_bytes: 1 << 20 }
    }

    #[test]
    fn files_and_lines_are_found_in_the_repo_only_and_kept_up_to_date() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/service.rs"), "fn rebuild_observed() {}\nfn other() {}\n").unwrap();
        std::fs::write(root.join("src/store.rs"), "struct Store;\n").unwrap();
        // A file the caller's "ignored" leaves out isn't searched.
        std::fs::create_dir_all(root.join("ignored")).unwrap();
        std::fs::write(root.join("ignored/service.rs"), "fn rebuild_observed() {}\n").unwrap();
        std::fs::write(dir.path().join("outside.rs"), "fn rebuild_observed() {}\n").unwrap();
        let skip = root.join("ignored");
        let mut files = RepoFiles::scan(&root, Box::new(move |p| p.starts_with(&skip))).unwrap();

        let paths = files.search("servic", 10);
        assert_eq!(paths.found.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["src/service.rs"], "ignored and outside files aren't found");
        assert_eq!(paths.total, 1);
        let lines = files.lines(limits()).search("rebuild_observed", 10);
        assert_eq!(lines.found, [FoundLine { path: "src/service.rs".into(), line: 1, text: "fn rebuild_observed() {}".into() }]);
        assert!(!lines.partial);
        // Narrowed by kind and folder; case ignored unless asked for.
        let count = |q: &str| files.lines(limits()).search(q, 10).found.len();
        assert_eq!((count("STORE *.rs"), count("store src/"), count("store *.py")), (0, 1, 0));

        // A new file, a changed one, a removed one.
        std::fs::write(root.join("src/view.rs"), "fn rebuild_observed() {}\n").unwrap();
        std::fs::write(root.join("src/service.rs"), "fn other() {}\n").unwrap();
        std::fs::remove_file(root.join("src/store.rs")).unwrap();
        files.changed(&[root.join("src/view.rs"), root.join("src/service.rs"), root.join("src/store.rs"), dir.path().join("outside.rs"), root.join("ignored/service.rs")]).unwrap();
        let lines = files.lines(limits()).search("rebuild_observed", 10).found;
        assert_eq!(lines.iter().map(|l| l.path.as_str()).collect::<Vec<_>>(), ["src/view.rs"]);
        assert!(!files.search("store", 10).found.iter().any(|f| f.path == "src/store.rs"), "removed");
    }

    /// The server's own folder that git ignores (its logs): listed and
    /// searched all the same, with what's searched as written (`src/`
    /// narrows nothing), and a file that grew is searched to its end once
    /// it's said to have changed.
    #[test]
    fn a_folder_git_ignores_is_searched_for_text_as_written_as_it_grows() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert!(std::process::Command::new("git").args(["init", "-q"]).current_dir(root).status().is_ok_and(|s| s.success()));
        std::fs::create_dir_all(root.join(".nah/state/tagged")).unwrap();
        std::fs::write(root.join(".nah/.gitignore"), "cache/\nstate/\n").unwrap();
        let logs = root.join(".nah/state/tagged");
        std::fs::write(logs.join("a.log"), "== a\nsee src/ for it\n").unwrap();
        let mut files = RepoFiles::scan(&logs, Box::new(|_| false)).unwrap();
        assert_eq!(files.lines(limits()).containing("see src/", 10).found, [FoundLine { path: "a.log".into(), line: 2, text: "see src/ for it".into() }]);
        let mut grown = std::fs::OpenOptions::new().append(true).open(logs.join("a.log")).unwrap();
        std::io::Write::write_all(&mut grown, b"later: error 3\n").unwrap();
        std::fs::write(logs.join("b.log"), "== b\nerror 4\n").unwrap();
        files.changed(&[logs.join("a.log"), logs.join("b.log")]).unwrap();
        let mut found: Vec<String> = files.lines(limits()).containing("ERROR", 10).found.into_iter().map(|l| format!("{}:{}", l.path, l.line)).collect();
        found.sort();
        assert!(found.is_empty(), "capitals heeded when there are some: {found:?}");
        let mut found: Vec<String> = files.lines(limits()).containing("error", 10).found.into_iter().map(|l| format!("{}:{}", l.path, l.line)).collect();
        found.sort();
        assert_eq!(found, ["a.log:3", "b.log:2"]);
    }

    /// How many any kind of search finds: the one trait over all three.
    fn how_many<S: Search>(search: &S, query: &str) -> usize {
        search.search(query, 10).total
    }

    #[test]
    fn symbols_files_and_lines_are_searched_through_one_trait() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/store.rs"), "struct Store;\nfn save() {}\n").unwrap();
        let files = RepoFiles::scan(&root, Box::new(|_| false)).unwrap();
        let symbols = crate::Symbols { names: vec!["Store".into(), "save".into()], qualified: vec!["store::Store src/store.rs".into(), "store::save src/store.rs".into()] };
        assert_eq!(how_many(&symbols, "save"), 1);
        assert_eq!(how_many(&symbols, "store::Store"), 1, "a qualified query matches the whole symbol");
        assert_eq!(how_many(&files, "store"), 1);
        assert_eq!(how_many(&files.lines(limits()), "fn save"), 1);
        let first = symbols.search("s", 1);
        assert_eq!((first.found.len(), first.total), (1, 2), "at most the limit, and how many in all");
    }

    /// `n` files under `root/src`, named so each is found by its own name.
    fn many(root: &Path, from: usize, n: usize) -> Vec<PathBuf> {
        std::fs::create_dir_all(root.join("src")).unwrap();
        (from..from + n)
            .map(|i| {
                let path = root.join(format!("src/file_{i:05}.rs"));
                std::fs::write(&path, format!("fn only_in_{i:05}() {{}}\n")).unwrap();
                path
            })
            .collect()
    }

    #[test]
    fn however_many_files_the_list_has_and_however_they_arrive_all_are_searched() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        many(&root, 0, 1500);
        // A file the caller ignores is never listed.
        std::fs::write(root.join("src/ignored.rs"), "fn only_in_ignored() {}\n").unwrap();
        let skip = root.join("src/ignored.rs");
        let mut files = RepoFiles::scan(&root, Box::new(move |p| p == skip)).unwrap();
        assert_eq!(files.len(), 1500);
        assert_eq!(files.search("file_01499", 5).found.first().map(|f| f.path.as_str()), Some("src/file_01499.rs"));
        assert!(!files.search("ignored", 5).found.iter().any(|f| f.path == "src/ignored.rs"));
        assert_eq!(files.lines(limits()).search("only_in_01499", 5).found.len(), 1);
        // Created one by one, as a watcher sees them: more than fff holds
        // between walks.
        for path in many(&root, 1500, 1100) {
            files.changed(std::slice::from_ref(&path)).unwrap();
        }
        assert_eq!(files.len(), 2600);
        assert!(!files.search("ignored", 5).found.iter().any(|f| f.path == "src/ignored.rs"), "still left out after listing again");
        assert_eq!(files.search("file_02599", 5).found.first().map(|f| f.path.as_str()), Some("src/file_02599.rs"));
        // A folder gone: everything under it.
        std::fs::remove_dir_all(root.join("src")).unwrap();
        files.changed(&[root.join("src")]).unwrap();
        assert!(files.is_empty());
    }
}

//! The repo's files and their contents, found with fff-search. The file
//! list is read once (respecting .gitignore) and kept up to date from the
//! changes the server's watcher sees; nothing outside the repo is searched.

use std::path::{Path, PathBuf};

use fff_search::file_picker::FilePicker;
use fff_search::{AiGrepConfig, FilePickerOptions, FileSearchConfig, FuzzySearchOptions, GitRecencyConfig, GrepMode, GrepSearchOptions, PaginationArgs, QueryParser};

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

pub struct RepoFiles {
    root: PathBuf,
    picker: FilePicker,
}

impl RepoFiles {
    /// Lists the repo's files (not those .gitignore leaves out).
    pub fn scan(root: &Path) -> Result<Self, String> {
        let options = FilePickerOptions {
            base_path: root.to_string_lossy().into_owned(),
            // The server's own watcher says what changed (see `changed`).
            watch: false,
            git_recency: GitRecencyConfig { enabled: false, ..GitRecencyConfig::default() },
            ..FilePickerOptions::default()
        };
        let mut picker = FilePicker::new(options).map_err(|e| format!("listing {}: {e}", root.display()))?;
        picker.collect_files().map_err(|e| format!("listing {}: {e}", root.display()))?;
        Ok(Self { root: root.to_path_buf(), picker })
    }

    pub fn len(&self) -> usize {
        self.picker.live_file_count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Files whose path matches `query`, best first, and how many matched.
    pub fn files(&self, query: &str, limit: usize) -> (Vec<FoundFile>, usize) {
        let parsed = QueryParser::new(FileSearchConfig).parse(query);
        let found = self.picker.fuzzy_search(&parsed, None, FuzzySearchOptions { max_threads: 0, current_file: None, pagination: PaginationArgs { offset: 0, limit }, ..FuzzySearchOptions::default() });
        let files = found.items.iter().map(|f| FoundFile { path: f.relative_path(&self.picker) }).collect();
        (files, found.total_matched)
    }

    /// Lines containing `query` as written (case ignored unless it has
    /// capitals; `*.rs` and `src/` narrow the files), the first `limit`;
    /// whether the time limit cut it short.
    pub fn text(&self, query: &str, limit: usize, limits: &TextLimits) -> (Vec<FoundLine>, bool) {
        let parsed = QueryParser::new(AiGrepConfig).parse(query);
        let options = GrepSearchOptions {
            max_file_size: limits.max_file_bytes,
            max_matches_per_file: limits.per_file,
            page_limit: limit,
            mode: GrepMode::PlainText,
            time_budget_ms: limits.budget_ms,
            enforce_time_budget: true,
            trim_whitespace: true,
            ..GrepSearchOptions::default()
        };
        let found = self.picker.grep(&parsed, &options);
        let lines: Vec<FoundLine> = found
            .matches
            .iter()
            .filter_map(|m| {
                let file = found.files.get(m.file_index)?;
                Some(FoundLine { path: file.relative_path(&self.picker), line: m.line_number as u32, text: m.line_content.clone() })
            })
            .collect();
        let partial = found.next_file_offset != 0 || found.total_files_searched < found.filtered_file_count;
        (lines, partial)
    }

    /// Takes in changed paths (created, modified, removed; files or
    /// directories). Paths outside the repo are ignored.
    pub fn changed(&mut self, paths: &[PathBuf]) {
        for path in paths.iter().filter(|p| p.starts_with(&self.root)) {
            if path.is_file() {
                self.picker.handle_create_or_modify(path);
            } else if !path.exists() {
                if !self.picker.remove_file_by_path(path) {
                    self.picker.remove_all_files_in_dir(path);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> TextLimits {
        TextLimits { per_file: 5, budget_ms: 1000, max_file_bytes: 1 << 20 }
    }

    // req: M.13
    #[test]
    fn files_and_lines_are_found_in_the_repo_only_and_kept_up_to_date() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir_all(root.join("src")).unwrap();
        // .gitignore counts in a git repo, as the server's always is.
        assert!(std::process::Command::new("git").arg("init").arg("-q").arg(&root).status().unwrap().success());
        std::fs::write(root.join("src/service.rs"), "fn rebuild_observed() {}\nfn other() {}\n").unwrap();
        std::fs::write(root.join("src/store.rs"), "struct Store;\n").unwrap();
        std::fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
        std::fs::create_dir_all(root.join("ignored")).unwrap();
        std::fs::write(root.join("ignored/service.rs"), "fn rebuild_observed() {}\n").unwrap();
        std::fs::write(dir.path().join("outside.rs"), "fn rebuild_observed() {}\n").unwrap();
        let mut files = RepoFiles::scan(&root).unwrap();

        let (found, total) = files.files("servic", 10);
        assert_eq!(found.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["src/service.rs"], "ignored and outside files aren't found");
        assert_eq!(total, 1);
        let (lines, partial) = files.text("rebuild_observed", 10, &limits());
        assert_eq!(lines, [FoundLine { path: "src/service.rs".into(), line: 1, text: "fn rebuild_observed() {}".into() }]);
        assert!(!partial);

        // A new file, a changed one, a removed one.
        std::fs::write(root.join("src/view.rs"), "fn rebuild_observed() {}\n").unwrap();
        std::fs::write(root.join("src/service.rs"), "fn other() {}\n").unwrap();
        std::fs::remove_file(root.join("src/store.rs")).unwrap();
        files.changed(&[root.join("src/view.rs"), root.join("src/service.rs"), root.join("src/store.rs"), dir.path().join("outside.rs")]);
        let (lines, _) = files.text("rebuild_observed", 10, &limits());
        assert_eq!(lines.iter().map(|l| l.path.as_str()).collect::<Vec<_>>(), ["src/view.rs"]);
        assert!(!files.files("store", 10).0.iter().any(|f| f.path == "src/store.rs"), "removed");
    }
}

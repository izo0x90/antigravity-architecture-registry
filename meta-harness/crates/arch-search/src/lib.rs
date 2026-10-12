//! One way of matching what's typed, everywhere. Names (symbols, actions,
//! what's on screen) are matched with neo_frizbee; files and their contents
//! are found with fff-search (feature `files`, the server only). The cockpit and the server use the same `rank`.
//!
//! A query is words; each must match. A word can say how: `'foo` contains
//! it as written, `^foo` starts with it, `foo$` ends with it, `!foo` must
//! not match.

#[cfg(feature = "files")]
mod files;

#[cfg(feature = "files")]
pub use files::{FoundFile, FoundLine, Lines, RepoFiles, TextLimits};

use serde::{Deserialize, Serialize};

const MATCHING: &str = include_str!("../config/matching.toml");

/// How typed text is matched (from config).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Matching {
    /// One typo forgiven per this many letters typed (0: none), when
    /// nothing matches as typed.
    pub letters_per_typo: usize,
    /// Names matching much worse than the best one are left out: those
    /// scoring under this share of its score.
    pub keep_within: f32,
}

#[derive(Deserialize)]
struct MatchingFile {
    matching: Matching,
}

impl Matching {
    pub fn built_in() -> Self {
        toml::from_str::<MatchingFile>(MATCHING).expect("built-in matching config parses").matching
    }

    /// The built-in settings, read once.
    pub fn shared() -> &'static Matching {
        static SHARED: std::sync::OnceLock<Matching> = std::sync::OnceLock::new();
        SHARED.get_or_init(Self::built_in)
    }
}

impl Default for Matching {
    fn default() -> Self {
        Self::built_in()
    }
}

/// One haystack that matched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ranked {
    /// Its place in the list given.
    pub index: usize,
    /// Higher is better.
    pub score: u16,
    /// The query is the whole of it (case aside).
    pub exact: bool,
}

/// The names `query` matches, best first (ties: the shorter, then the
/// list's order). Letters match in order with gaps (`fbr` finds `fooBar`);
/// when nothing matches as typed, a typo is forgiven now and then. Names
/// matching much worse than the best are left out. An empty query matches
/// everything, in order.
pub fn rank<S: AsRef<str>>(query: &str, haystacks: &[S], matching: &Matching) -> Vec<Ranked> {
    let mut out = rank_as(query, haystacks, 0, neo_frizbee::Matching::Fuzzy);
    if out.is_empty() && matching.letters_per_typo > 0 {
        out = rank_as(query, haystacks, matching.letters_per_typo, neo_frizbee::Matching::Fuzzy);
    }
    if let Some(best) = out.iter().map(|r| r.score).max() {
        let floor = f32::from(best) * matching.keep_within;
        out.retain(|r| r.exact || f32::from(r.score) >= floor);
    }
    out
}

/// The texts (lines, messages) that contain every word of `query` as
/// written (case ignored unless it has capitals), best first. Long text
/// would match almost any letters in order, so words aren't fuzzy here;
/// the same word operators apply.
pub fn rank_text<S: AsRef<str>>(query: &str, haystacks: &[S]) -> Vec<Ranked> {
    rank_as(query, haystacks, 0, neo_frizbee::Matching::Substring)
}

/// `letters_per_typo`: one typo forgiven per this many letters (0: none).
fn rank_as<S: AsRef<str>>(query: &str, haystacks: &[S], letters_per_typo: usize, plain: neo_frizbee::Matching) -> Vec<Ranked> {
    let query = query.trim();
    if query.is_empty() {
        return (0..haystacks.len()).map(|index| Ranked { index, score: 0, exact: false }).collect();
    }
    let patterns: Vec<neo_frizbee::Pattern> = neo_frizbee::Pattern::parse_query(query)
        .into_iter()
        .map(|mut p| {
            // A plain word (no operator) matches the way asked.
            if p.config.matching.is_none() {
                p.config.matching = Some(plain);
            }
            let typos = match (plain, letters_per_typo) {
                (neo_frizbee::Matching::Fuzzy, n) if n > 0 => p.needle.chars().count() / n,
                _ => 0,
            };
            p.max_typos(Some(typos as u16))
        })
        .collect();
    let mut matcher = neo_frizbee::Matcher::from_patterns(&patterns, &neo_frizbee::Config::default());
    let whole = |i: usize| haystacks[i].as_ref().eq_ignore_ascii_case(query);
    let mut out: Vec<Ranked> = matcher.match_list(haystacks).into_iter().map(|m| Ranked { index: m.index as usize, score: m.score, exact: m.exact || whole(m.index as usize) }).collect();
    // The whole name first, then the best match, then the shortest name.
    let len = |r: &Ranked| haystacks[r.index].as_ref().len();
    out.sort_by(|a, b| b.exact.cmp(&a.exact).then(b.score.cmp(&a.score)).then(len(a).cmp(&len(b))).then(a.index.cmp(&b.index)));
    out
}

/// What one search found: the best first, at most the limit asked.
#[derive(Clone, Debug, PartialEq)]
pub struct Searched<T> {
    pub found: Vec<T>,
    /// How many matched in all (more than `found` when cut at the limit).
    pub total: usize,
    /// It stopped at its time limit: there may be more.
    pub partial: bool,
}

/// Symbols, files and lines are each searched through this.
pub trait Search {
    type Found;
    /// What matches `query`, best first, at most `limit`.
    fn search(&self, query: &str, limit: usize) -> Searched<Self::Found>;
}

/// A snapshot's symbols: each one's name, and its whole symbol with its file.
#[derive(Clone, Debug, Default)]
pub struct Symbols {
    pub names: Vec<String>,
    pub qualified: Vec<String>,
}

impl Search for Symbols {
    /// The place in the lists of each symbol that matched.
    type Found = usize;

    /// A qualified query (`service::search`, `a/b.rs`) is matched against
    /// the whole symbol and file; a plain one against names.
    fn search(&self, query: &str, limit: usize) -> Searched<usize> {
        let qualified = query.contains("::") || query.contains('/') || query.contains('.');
        let ranked = rank(query, if qualified { &self.qualified } else { &self.names }, Matching::shared());
        Searched { total: ranked.len(), found: ranked.into_iter().take(limit).map(|r| r.index).collect(), partial: false }
    }
}

/// What a search looks through.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchKind {
    /// Names in the code (and in a plan): functions, types, modules.
    #[default]
    Symbols,
    /// File paths in the repo.
    Files,
    /// Lines in the repo's files.
    Text,
}

impl SearchKind {
    pub const ALL: [SearchKind; 3] = [SearchKind::Symbols, SearchKind::Files, SearchKind::Text];

    pub fn name(self) -> &'static str {
        match self {
            Self::Symbols => "symbols",
            Self::Files => "files",
            Self::Text => "text",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }
}

/// One thing found.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hit {
    pub kind: SearchKind,
    /// The card it stands for (a component id), when the graph has one: a
    /// symbol, a file's module, or what a line is inside of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// A name, a path, or the line's text.
    pub label: String,
    /// What it is and where.
    #[serde(default)]
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// The card can be called (it can be a root in the calls view).
    #[serde(default)]
    pub callable: bool,
    /// The card is a data type (a root in the data flow view, as `type:<name>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
}

/// A search's answer.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Found {
    pub query: String,
    pub kind: SearchKind,
    pub hits: Vec<Hit>,
    /// How many matched in all (more than `hits` when cut at the limit).
    pub total: usize,
    /// The search stopped at its time limit: there may be more.
    #[serde(default)]
    pub partial: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_ranked_best_first_with_typos_forgiven_and_query_atoms() {
        let m = Matching::built_in();
        let names = ["search_step", "Search", "research", "resolve", "ArchitectureService::search"];
        let order = |q: &str| rank(q, &names, &m).into_iter().map(|r| names[r.index]).collect::<Vec<_>>();
        assert_eq!(order("search")[0], "Search", "the whole name first");
        assert!(order("search").contains(&"research"));
        assert!(!order("search").contains(&"resolve"));
        assert!(order("sch").contains(&"search_step"), "fuzzy: letters in order");
        let few = ["Search", "resolve"];
        assert_eq!(rank("serach", &few, &m).first().map(|r| few[r.index]), Some("Search"), "a typo is forgiven in a longer word, when nothing matches as typed");
        let long = ["rebuild_observed", "history_replay_builds_one_turn_per_user_message", "RequiredField", "upserts_replace_by_id_and_close_removes_thread"];
        let found: Vec<&str> = rank("rebuild", &long, &m).into_iter().map(|r| long[r.index]).collect();
        assert_eq!(found[0], "rebuild_observed");
        assert!(!found.contains(&"RequiredField") && !found.contains(&"upserts_replace_by_id_and_close_removes_thread"), "no typos while something matches as typed: {found:?}");
        assert!(order("^res").iter().all(|n| n.starts_with("res")));
        assert_eq!(order("search !step !^re !Arch"), ["Search"]);
        assert_eq!(rank("", &names, &m).len(), names.len(), "nothing typed: everything");
        assert!(rank("zzz", &names, &m).is_empty());
        let lines = ["let found = search(query);", "fn research() {}", "Search::new()"];
        let text = |q: &str| rank_text(q, &lines).into_iter().map(|r| lines[r.index]).collect::<Vec<_>>();
        assert_eq!(text("search").len(), 3, "as written, any case");
        assert_eq!(text("search")[2], "fn research() {}", "a whole word ranks above part of one");
        assert!(text("sch").is_empty(), "not letters in order: words as written");
        assert_eq!(text("Search"), ["Search::new()"], "capitals ask for case");
        assert_eq!(text("search query"), ["let found = search(query);"], "every word");
        let strict = Matching { letters_per_typo: 0, ..m.clone() };
        assert!(rank("serach", &few, &strict).is_empty());
    }
}

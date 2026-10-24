//! A search of the code index as it's typed, for every picker that asks
//! it (`@` where text is written, the graph's find). One round per query,
//! its kinds asked together, at most one round out at a time: what's
//! typed meanwhile is asked once it's back. A round is answered once every
//! kind has answered, or once its time is up. What's shown is the last
//! round answered, whole and in the kinds' order, so a list never empties
//! and fills again at each key. Its rows read the same in every picker.

use std::collections::BTreeMap;

use arch_search::{Found, Hit, SearchKind};

use crate::actions::Choice;
use crate::net::NetResult;

/// How long a round may take before it counts as answered with what came.
pub const ROUND_LIMIT_S: f64 = 8.0;

/// A round asked: its number, the query as typed, since when, what came.
struct Round {
    number: u64,
    typed: String,
    since: f64,
    came: BTreeMap<SearchKind, Found>,
}

/// The last round answered: its number, the query as typed (none: none
/// yet), what each kind found, in the kinds' order.
#[derive(Default)]
struct Answered {
    number: u64,
    typed: Option<String>,
    found: Vec<(SearchKind, Found)>,
}

/// What a frame's tending gives: the round to ask now (its number and
/// the query to send), and what to tell the user.
#[derive(Debug, Default, PartialEq)]
pub struct Tended {
    pub ask: Option<(u64, String)>,
    pub note: Option<String>,
}

pub struct CodeSearch {
    /// What's asked, in the order shown.
    kinds: &'static [SearchKind],
    typed: String,
    out: Option<Round>,
    rounds: u64,
    shown: Answered,
}

impl CodeSearch {
    pub fn new(kinds: &'static [SearchKind]) -> Self {
        Self { kinds, typed: String::new(), out: None, rounds: 0, shown: Answered::default() }
    }

    /// What's asked of each round, in the order shown.
    pub fn kinds(&self) -> &'static [SearchKind] {
        self.kinds
    }

    /// What's typed now.
    pub fn typed(&mut self, query: &str) {
        if self.typed != query {
            query.clone_into(&mut self.typed);
        }
    }

    /// Once a frame: a round answered (every kind, or its time up) becomes
    /// what's shown; then, when none is out and the rows aren't of what's
    /// typed, the round to ask. Nothing typed (or only spaces) is answered
    /// at once, with nothing; the same query with other spaces at its ends
    /// keeps its rows.
    pub fn tend(&mut self, now: f64) -> Tended {
        let mut tended = Tended::default();
        if let Some(out) = &self.out {
            let whole = self.kinds.iter().all(|k| out.came.contains_key(k));
            if whole || now - out.since >= ROUND_LIMIT_S {
                if !whole {
                    tended.note = Some("The code index didn't answer in time: what came is shown.".into());
                }
                let Round { number, typed, mut came, .. } = self.out.take().expect("a round is out");
                let found = self.kinds.iter().filter_map(|k| came.remove(k).map(|f| (*k, f))).collect();
                self.shown = Answered { number, typed: Some(typed), found };
            }
        }
        if self.out.is_some() || self.shown.typed.as_deref() == Some(self.typed.as_str()) {
            return tended;
        }
        let query = self.typed.trim();
        if self.shown.typed.as_deref().map(str::trim) == Some(query) {
            self.shown.typed = Some(self.typed.clone());
        } else if query.is_empty() {
            self.rounds += 1;
            self.shown = Answered { number: self.rounds, typed: Some(self.typed.clone()), found: Vec::new() };
        } else {
            self.rounds += 1;
            tended.ask = Some((self.rounds, query.to_owned()));
            self.out = Some(Round { number: self.rounds, typed: self.typed.clone(), since: now, came: BTreeMap::new() });
        }
        tended
    }

    /// A kind's answer for round `number`: kept when that round is out (a
    /// failure as nothing found, and what went wrong returned to be said).
    pub fn answer(&mut self, number: u64, kind: SearchKind, answer: NetResult<Found>) -> Option<String> {
        let out = self.out.as_mut().filter(|o| o.number == number)?;
        match answer {
            Ok(found) => {
                out.came.insert(kind, found);
                None
            }
            Err(e) => {
                out.came.insert(kind, Found::default());
                Some(format!("Couldn't search the code: {}", String::from(e)))
            }
        }
    }

    /// The query the rows shown are of, as typed (none: nothing answered).
    pub fn rows_of(&self) -> Option<&str> {
        self.shown.typed.as_deref()
    }

    /// A round is out (to see its time run out, draw again).
    pub fn is_out(&self) -> bool {
        self.out.is_some()
    }

    /// A hit shown: the round's number, its kind and its place among them.
    pub fn hit(&self, number: u64, kind: SearchKind, index: usize) -> Option<&Hit> {
        (number == self.shown.number).then(|| self.shown.found.iter().find(|(k, _)| *k == kind)?.1.hits.get(index))?
    }

    /// The rows shown, named the same in every picker: what each is (a
    /// symbol, a file, a line) and where, the last of a kind saying what
    /// wasn't listed. `value` names each with the round's number, its
    /// kind and its place (none: left out).
    pub fn rows(&self, value: impl Fn(u64, SearchKind, usize, &Hit) -> Option<String>) -> Vec<Choice> {
        let mut out = Vec::new();
        for (kind, found) in &self.shown.found {
            let (what, icon) = match kind {
                SearchKind::Symbols => ("symbol", egui_phosphor::regular::CODE),
                SearchKind::Files => ("file", egui_phosphor::regular::FILE),
                SearchKind::Text => ("line", egui_phosphor::regular::TEXT_ALIGN_LEFT),
            };
            for (i, hit) in found.hits.iter().enumerate() {
                let Some(value) = value(self.shown.number, *kind, i, hit) else { continue };
                let mut detail = if hit.detail.is_empty() { what.to_owned() } else { format!("{what} · {}", hit.detail) };
                // The last of a kind says what wasn't listed: more matches
                // than shown, or a search cut at its time limit.
                if i + 1 == found.hits.len() {
                    if found.total > found.hits.len() {
                        detail.push_str(&format!(" · {} of {} {what}s", found.hits.len(), found.total));
                    }
                    if found.partial {
                        detail.push_str(" · stopped at its time limit: there may be more");
                    }
                }
                out.push(Choice::new(value, hit.label.clone()).detail(Some(detail)).glyph(icon, crate::theme::colors().accent));
            }
        }
        out
    }

    /// Nothing asked or shown (a pick was made: what comes for the round
    /// out is dropped).
    pub fn clear(&mut self) {
        (self.typed, self.out, self.shown) = (String::new(), None, Answered::default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(labels: &[&str]) -> NetResult<Found> {
        let hit = |label: &str| Hit { kind: SearchKind::Files, key: None, label: label.into(), detail: String::new(), file: None, line: None, callable: false, data: None };
        let hits = labels.iter().map(|l| hit(l)).collect::<Vec<_>>();
        Ok(Found { total: hits.len(), hits, ..Found::default() })
    }

    fn labels(search: &CodeSearch) -> Vec<String> {
        search.rows(|_, _, i, _| Some(i.to_string())).into_iter().map(|c| c.label).collect()
    }

    const KINDS: &[SearchKind] = &[SearchKind::Symbols, SearchKind::Files];

    /// One round out at a time, its rows shown whole in the kinds' order
    /// whatever order they came in; what's typed meanwhile is asked once
    /// it's back, the rows of the last round shown until then.
    #[test]
    fn a_round_is_shown_whole_and_what_was_typed_meanwhile_is_asked_next() {
        let mut search = CodeSearch::new(KINDS);
        search.typed("");
        assert_eq!(search.tend(0.0), Tended::default(), "nothing typed: nothing asked");
        assert_eq!((search.rows_of(), labels(&search).len()), (Some(""), 0), "and answered at once");

        search.typed("a");
        let first = search.tend(0.0).ask.unwrap();
        assert_eq!(first.1, "a");
        search.typed("ab");
        assert_eq!(search.tend(0.1).ask, None, "one round out at a time");
        assert_eq!(search.answer(first.0, SearchKind::Files, found(&["a.rs"])), None);
        assert_eq!(search.tend(0.2).ask, None, "half a round: still out");
        assert_eq!(search.rows_of(), Some(""), "the rows shown are still the last round's");
        search.answer(first.0, SearchKind::Symbols, found(&["alpha"]));
        let next = search.tend(0.3).ask.unwrap();
        assert_eq!((search.rows_of(), labels(&search)), (Some("a"), vec!["alpha".to_owned(), "a.rs".to_owned()]), "whole, symbols then files");
        assert_eq!(next.1, "ab", "what was typed meanwhile, asked once it's back");

        // An answer for a round gone is dropped; a failure is said, as nothing found.
        assert_eq!(search.answer(first.0, SearchKind::Files, found(&["late.rs"])), None);
        search.answer(next.0, SearchKind::Files, found(&["ab.rs"]));
        assert!(search.answer(next.0, SearchKind::Symbols, Err(crate::net::NetError::other("down"))).unwrap().contains("down"));
        search.tend(0.4);
        assert_eq!((search.rows_of(), labels(&search)), (Some("ab"), vec!["ab.rs".to_owned()]));
        // The same query with a space after it keeps its rows.
        search.typed("ab ");
        assert_eq!((search.tend(0.5).ask, search.rows_of()), (None, Some("ab ")));
    }

    #[test]
    fn a_round_out_of_time_shows_what_came_and_says_so() {
        let mut search = CodeSearch::new(KINDS);
        search.typed("a");
        let (round, _) = search.tend(0.0).ask.unwrap();
        search.answer(round, SearchKind::Files, found(&["a.rs"]));
        assert_eq!(search.tend(ROUND_LIMIT_S - 0.1).note, None);
        let tended = search.tend(ROUND_LIMIT_S);
        assert!(tended.note.unwrap().contains("in time"));
        assert_eq!((search.rows_of(), labels(&search)), (Some("a"), vec!["a.rs".to_owned()]));
        assert!(!search.is_out());
        let value = search.rows(|n, k, i, _| Some(format!("{n}:{}:{i}", k.name())))[0].value.clone();
        assert_eq!(value, format!("{round}:files:0"));
        assert_eq!(search.hit(round, SearchKind::Files, 0).map(|h| h.label.as_str()), Some("a.rs"));
        assert!(search.hit(round + 1, SearchKind::Files, 0).is_none(), "a pick from another round finds nothing");
    }
}

//! Finding anything in the code from the graph: symbols, files and lines,
//! through the server's one search, listed in the palette as they're typed.
//! Picking one makes its card the root, in this view if it can show it.

use std::collections::BTreeMap;

use arch_graph::tree::ViewKind;
use arch_search::{Found, Hit, SearchKind};

use super::{GraphState, NavChange, Reply, push};
use crate::actions::Choice;
use crate::net::{NetClient, NetResult};

/// The search being typed, and what came back for it.
#[derive(Default)]
pub(crate) struct Find {
    query: String,
    /// Counts up with every query; answers to older ones are dropped.
    asked: u64,
    /// The query hasn't gone out yet.
    send: bool,
    found: BTreeMap<SearchKind, Found>,
}

impl GraphState {
    /// What's typed in the palette's search, as it changes.
    pub(crate) fn find_query(&mut self, query: &str) {
        let query = query.trim();
        if query == self.find.query && self.find.asked > 0 {
            return;
        }
        self.find.query = query.to_owned();
        self.find.asked += 1;
        self.find.found.clear();
        self.find.send = !query.is_empty();
    }

    /// Asks the server for each kind (symbols, files, lines) at once.
    pub(super) fn send_find(&mut self, net: &NetClient, ctx: &egui::Context) {
        if !std::mem::take(&mut self.find.send) {
            return;
        }
        let limits = &self.ui.find;
        for (kind, limit) in [(SearchKind::Symbols, limits.symbols), (SearchKind::Files, limits.files), (SearchKind::Text, limits.text)] {
            let (inbox, asked) = (self.inbox.clone(), self.find.asked);
            net.search(&self.find.query, kind, self.nav.plan.as_ref(), limit, ctx.clone(), move |r| push(&inbox, Reply::Found(asked, kind, Box::new(r))));
        }
    }

    pub(super) fn found(&mut self, asked: u64, kind: SearchKind, found: NetResult<Found>) {
        if asked != self.find.asked {
            return;
        }
        // A kind that failed lists nothing (the others still do).
        self.find.found.insert(kind, found.unwrap_or_default());
    }

    /// The palette's list: symbols, then files, then lines; and whether
    /// some are still on their way.
    pub(crate) fn find_choices(&self) -> (Vec<Choice>, bool) {
        let mut out = Vec::new();
        for kind in SearchKind::ALL {
            let Some(found) = self.find.found.get(&kind) else { continue };
            for (i, hit) in found.hits.iter().enumerate() {
                let what = match kind {
                    SearchKind::Symbols => "symbol",
                    SearchKind::Files => "file",
                    SearchKind::Text => "line",
                };
                let detail = format!("{what} · {}", hit.detail);
                out.push(Choice::new(format!("{}:{}:{i}", self.find.asked, kind.name()), hit.label.clone()).detail(Some(detail)));
            }
        }
        let waiting = !self.find.query.is_empty() && self.find.found.len() < SearchKind::ALL.len();
        (out, waiting)
    }

    /// Goes to a picked result (`value` from `find_choices`).
    pub(crate) fn pick_found(&mut self, value: &str) {
        let mut parts = value.splitn(3, ':');
        let (Some(asked), Some(kind), Some(index)) = (parts.next(), parts.next(), parts.next()) else { return };
        if asked.parse() != Ok(self.find.asked) {
            return;
        }
        let Some(hit) = SearchKind::from_name(kind).and_then(|k| self.find.found.get(&k)).and_then(|f| f.hits.get(index.parse::<usize>().ok()?)).cloned() else { return };
        self.go_to_hit(&hit);
    }

    fn go_to_hit(&mut self, hit: &Hit) {
        let Some(key) = hit.key.clone() else {
            // Nothing in the graph stands for it (a file no pack reads): show the code.
            if let Some(file) = &hit.file {
                let line = hit.line.unwrap_or(1);
                self.requests.open_code = Some((file.clone(), line, line));
            }
            return;
        };
        let here = self.nav.view;
        let (view, root) = match (here, &hit.data) {
            (ViewKind::Calls, _) if hit.callable => (here, key),
            (ViewKind::DataFlow, Some(data)) => (here, data.clone()),
            (ViewKind::Contains, _) => (here, key),
            // This view can't show it: the one that can.
            (_, _) if hit.callable => (ViewKind::Calls, key),
            (_, Some(data)) => (ViewKind::DataFlow, data.clone()),
            _ => (ViewKind::Contains, key),
        };
        self.pending_select = Some(root.clone());
        if view == here {
            self.navigate(NavChange::GoIn(root));
        } else {
            self.navigate(NavChange::View { view, root: Some(root) });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::{sample_tree, showing};

    fn hit(kind: SearchKind, key: Option<&str>, label: &str, callable: bool, data: Option<&str>) -> Hit {
        Hit { kind, key: key.map(Into::into), label: label.into(), detail: "d".into(), file: Some("src/a.rs".into()), line: Some(3), callable, data: data.map(Into::into) }
    }

    fn answer(g: &mut GraphState, kind: SearchKind, hits: Vec<Hit>) {
        let asked = g.find.asked;
        g.found(asked, kind, Ok(Found { total: hits.len(), hits, ..Found::default() }));
    }

    fn value_of(g: &GraphState, label: &str) -> String {
        g.find_choices().0.into_iter().find(|c| c.label == label).unwrap().value
    }

    // req: M.13
    #[test]
    fn finding_lists_symbols_files_and_lines_and_picking_one_makes_it_the_root() {
        let mut g = showing(sample_tree());
        assert_eq!(g.nav.view, ViewKind::Calls);
        g.find_query("step");
        assert!(g.find.send, "the query goes out on the next frame");
        assert_eq!(g.find_choices(), (Vec::new(), true), "waiting for the answers");
        answer(&mut g, SearchKind::Text, vec![hit(SearchKind::Text, None, "// step by step", false, None)]);
        answer(&mut g, SearchKind::Files, vec![hit(SearchKind::Files, Some("rust:a"), "src/a.rs", false, None)]);
        answer(&mut g, SearchKind::Symbols, vec![hit(SearchKind::Symbols, Some("rust:a::step"), "step", true, None)]);
        let (list, waiting) = g.find_choices();
        assert!(!waiting);
        assert_eq!(list.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), ["step", "src/a.rs", "// step by step"], "symbols, files, lines");

        // A function, in the calls view: the root there.
        g.command(crate::graph::actions::GraphCommand::Find, Some(value_of(&g, "step")));
        assert_eq!((g.nav.view, g.nav.root.as_deref()), (ViewKind::Calls, Some("rust:a::step")));

        // A file's module: the calls view can't show it, so the boxes do.
        let file = value_of(&g, "src/a.rs");
        g.command(crate::graph::actions::GraphCommand::Find, Some(file));
        assert_eq!((g.nav.view, g.nav.root.as_deref()), (ViewKind::Contains, Some("rust:a")));

        // A line nothing in the graph stands for opens in the code.
        g.command(crate::graph::actions::GraphCommand::Find, Some(value_of(&g, "// step by step")));
        assert_eq!(g.requests.open_code, Some(("src/a.rs".into(), 3, 3)));

        // Answers to an older query are dropped.
        let stale = g.find.asked;
        g.find_query("run");
        g.found(stale, SearchKind::Symbols, Ok(Found { hits: vec![hit(SearchKind::Symbols, Some("rust:a::step"), "step", true, None)], ..Found::default() }));
        assert!(g.find_choices().0.is_empty());
        // So is a pick from an older list.
        g.command(crate::graph::actions::GraphCommand::Find, Some(format!("{stale}:symbols:0")));
        assert_eq!(g.nav.root.as_deref(), Some("rust:a"));
    }

    // req: M.13
    #[test]
    fn a_data_type_found_from_the_data_flow_view_is_its_root_there() {
        let mut g = showing(sample_tree());
        g.command(crate::graph::actions::GraphCommand::SetView, Some("data_flow".into()));
        g.find_query("Order");
        answer(&mut g, SearchKind::Symbols, vec![hit(SearchKind::Symbols, Some("rust:a::Order"), "Order", false, Some("type:Order"))]);
        g.command(crate::graph::actions::GraphCommand::Find, Some(value_of(&g, "Order")));
        assert_eq!((g.nav.view, g.nav.root.as_deref()), (ViewKind::DataFlow, Some("type:Order")));
    }
}

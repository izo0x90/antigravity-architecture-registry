//! The selection's details, fetched when it's selected (for the code and,
//! when a plan changes it, for the plan too), and what the graph says about
//! a component's card (shown above its spec in the quick look). Only shows;
//! never changes what's on the graph.

use arch_graph::tree::{Mark, PillRole};
use arch_graph::{ComponentId, Signature, Source};

use super::{Detail, GraphState, Reply, push};
use crate::hosted::{Status, Tone};
use crate::net::NetClient;

fn signature(sig: &Signature) -> String {
    let params: Vec<String> =
        sig.params.iter().map(|p| p.ty.as_ref().map_or_else(|| p.name.clone(), |t| format!("{}: {t}", p.name))).collect();
    let returns = sig.returns.as_ref().map(|r| format!(" -> {r}")).unwrap_or_default();
    format!("({}){returns}", params.join(", "))
}

impl GraphState {
    /// Asks for the selection's details (for each source it's compared in).
    pub(super) fn want_details(&mut self, net: &NetClient, ctx: &egui::Context) {
        let Some(card) = self.selected_card().cloned() else { return };
        if card.component.is_none() {
            return;
        }
        let mut wanted = vec![self.nav.source.clone()];
        // A planned change: the code as it is too, to show both.
        if self.nav.source != Source::Code && matches!(card.mark, arch_graph::tree::Mark::Changed | arch_graph::tree::Mark::Mismatch) {
            wanted.push(Source::Code);
        }
        for source in wanted {
            let key = (card.key.clone(), source.clone());
            if self.details.contains_key(&key) {
                continue;
            }
            if self.details.len() >= self.ui.details_kept.max(1) {
                self.details.clear();
            }
            self.details.insert(key, Detail::Loading);
            let (inbox, k, asked) = (self.inbox.clone(), card.key.clone(), source.clone());
            net.graph_card(&card.key, &source, ctx.clone(), move |r| push(&inbox, Reply::Card(k, asked, Box::new(r))));
        }
    }

    pub(crate) fn detail(&self, key: &str, source: &Source) -> Option<&Detail> {
        self.details.get(&(key.to_owned(), source.clone()))
    }
}

impl GraphState {
    /// What the graph says about `component`'s card, beyond the component
    /// itself: its kind and plan mark, what else changes it, what's under
    /// it, its review marks, and what's on its lines. Nothing when it has no
    /// card on screen.
    pub(crate) fn card_says(&self, component: &ComponentId) -> Vec<Status> {
        let card = match self.selected_card().filter(|c| c.component.as_ref() == Some(component)) {
            Some(card) => card,
            None => match self.tree.as_ref().and_then(|t| t.cards.iter().find(|c| c.component.as_ref() == Some(component))) {
                Some(card) => card,
                None => return Vec::new(),
            },
        };
        let mut out = vec![Status::new(format!("In the graph: {}", card.kind), Tone::Muted)];
        if card.mark != Mark::Code {
            out.push(Status::new(crate::graph_style::mark_label(card.mark), mark_tone(card.mark)));
        }
        if card.outside_plan {
            out.push(Status::new("code the plan plugs into", Tone::Muted));
        }
        if card.more > 0 {
            out.push(Status::new(format!("{} more under it", card.more), Tone::Muted));
        }
        // A planned change to its signature, beside the code's own.
        let signature_in = |source: &Source| match self.detail(&card.key, source) {
            Some(Detail::Ready(d)) => d.component.signature.clone(),
            _ => None,
        };
        if self.nav.source != Source::Code
            && matches!(card.mark, Mark::Changed | Mark::Mismatch)
            && let (Some(planned), Some(real)) = (signature_in(&self.nav.source), signature_in(&Source::Code))
            && planned != real
        {
            out.push(Status::new(format!("In the plan: {}", signature(&planned)), Tone::Warn));
            out.push(Status::new(format!("In the code: {}", signature(&real)), Tone::Plain));
        }
        if !card.other_plans.is_empty() {
            let names: Vec<&str> = card.other_plans.iter().map(|p| p.0.as_str()).collect();
            out.push(Status::new(format!("Also changed by {}", names.join(", ")), Tone::Accent));
        }
        if let Some(mark) = self.marks.get(component) {
            if mark.changed {
                out.push(Status::new("changed since review", Tone::Info));
            }
            if mark.open > 0 {
                out.push(Status::new(format!("{} discussions not settled", mark.open), Tone::Warn));
            }
        }
        // What's on its lines (its pills, listed).
        if let Some(tree) = &self.tree {
            for line in tree.lines.iter().filter(|l| (l.from == card.key || l.to == card.key) && !l.pills.is_empty()) {
                let other = if line.from == card.key { &line.to } else { &line.from };
                let name = self.card(other).map_or(other.as_str(), |c| c.label.as_str());
                let pills: Vec<String> = line
                    .pills
                    .iter()
                    .map(|p| match p.role {
                        PillRole::In => format!("in {}", p.text),
                        PillRole::Out => format!("out {}", p.text),
                        PillRole::Step => format!("via {}", p.text),
                    })
                    .collect();
                out.push(Status::new(format!("{name}: {}", pills.join(" · ")), Tone::Muted));
            }
        }
        out
    }
}

/// How a plan's mark on a card is said.
fn mark_tone(mark: Mark) -> Tone {
    match mark {
        Mark::Code => Tone::Plain,
        Mark::New => Tone::Accent,
        Mark::Changed => Tone::Warn,
        Mark::Done => Tone::Ok,
        Mark::Mismatch => Tone::Error,
    }
}

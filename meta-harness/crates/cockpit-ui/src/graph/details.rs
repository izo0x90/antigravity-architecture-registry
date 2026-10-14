//! The selection's details, fetched when it's selected (for the code and,
//! when a plan changes it, for the plan too), and what the graph says about
//! a component's card (shown above its spec in the quick look). Only shows;
//! never changes what's on the graph.

use egui::RichText;

use arch_graph::tree::{Mark, PillRole};
use arch_graph::{ComponentId, Signature, Source};

use super::{Detail, GraphState, Reply, push};
use crate::net::NetClient;
use crate::theme::colors;

fn signature(sig: &Signature) -> String {
    let params: Vec<String> = sig.params.iter().map(|p| p.ty.as_ref().map_or_else(|| p.name.clone(), |t| format!("{}: {t}", p.name))).collect();
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
    pub(crate) fn render_card_context(&self, ui: &mut egui::Ui, component: &ComponentId) {
        let card = match self.selected_card().filter(|c| c.component.as_ref() == Some(component)) {
            Some(card) => card,
            None => match self.tree.as_ref().and_then(|t| t.cards.iter().find(|c| c.component.as_ref() == Some(component))) {
                Some(card) => card,
                None => return,
            },
        };
        let look = crate::theme::look();
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(format!("In the graph: {}", card.kind)).size(look.text_body).color(colors().muted));
            if card.mark != Mark::Code {
                ui.label(RichText::new(crate::graph_style::mark_label(card.mark)).size(look.text_body).color(style_color(card.mark)));
            }
            if card.outside_plan {
                ui.label(RichText::new("code the plan plugs into").size(look.text_body).color(colors().muted));
            }
            if card.more > 0 {
                ui.label(RichText::new(format!("{} more under it", card.more)).size(look.text_body).color(colors().muted));
            }
        });
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
            ui.label(RichText::new(format!("In the plan: {}", signature(&planned))).monospace().size(look.text_small).color(colors().warn));
            ui.label(RichText::new(format!("In the code: {}", signature(&real))).monospace().size(look.text_small).color(colors().text));
        }
        if !card.other_plans.is_empty() {
            let names: Vec<&str> = card.other_plans.iter().map(|p| p.0.as_str()).collect();
            ui.label(RichText::new(format!("Also changed by {}", names.join(", "))).size(look.text_body).color(colors().special));
        }
        if let Some(mark) = self.marks.get(component) {
            ui.horizontal_wrapped(|ui| {
                if mark.changed {
                    ui.label(RichText::new("changed since review").size(look.text_body).color(colors().info));
                }
                if mark.open > 0 {
                    ui.label(RichText::new(format!("{} open review points", mark.open)).size(look.text_body).color(colors().warn));
                }
            });
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
                ui.label(RichText::new(format!("{name}: {}", pills.join(" · "))).monospace().size(look.text_small).color(colors().muted));
            }
        }
    }
}

fn style_color(mark: Mark) -> egui::Color32 {
    crate::graph_style::style().node_outline(&colors(), mark, false).color
}

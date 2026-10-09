//! The details card: everything about the selection, fetched when it's
//! selected. It only shows; it never changes what's on the graph (its
//! links go somewhere the way a key would).

use egui::{RichText, Stroke, Vec2};

use arch_graph::tree::{Mark, PillRole};
use arch_graph::{Component, ComponentDetail, RelationKind, Signature};

use super::{Detail, GraphState};
use crate::theme::colors;

fn signature(sig: &Signature) -> String {
    let params: Vec<String> = sig.params.iter().map(|p| p.ty.as_ref().map_or_else(|| p.name.clone(), |t| format!("{}: {t}", p.name))).collect();
    let returns = sig.returns.as_ref().map(|r| format!(" -> {r}")).unwrap_or_default();
    format!("({}){returns}", params.join(", "))
}

fn location(c: &Component) -> String {
    match (&c.anchor, &c.proposed) {
        (Some(a), _) => crate::status::place(a),
        (None, Some(p)) => format!("planned in {}", p.file),
        (None, None) => "not placed yet".to_owned(),
    }
}

impl GraphState {
    pub(crate) fn render_drawer(&mut self, ui: &mut egui::Ui) {
        let Some(card) = self.selected_card().cloned() else { return };
        let look = crate::theme::look();
        let plan = self.nav.plan.clone();
        let detail = self.detail(&card.key, plan.as_ref()).cloned();
        // The code as it is, beside the plan's version, when the plan changes it.
        let in_code = (plan.is_some() && matches!(card.mark, Mark::Changed | Mark::Mismatch)).then(|| self.detail(&card.key, None).cloned()).flatten();
        let mut go: Option<String> = None;
        egui::Frame::NONE.fill(colors().surface).stroke(Stroke::new(look.line_thin, colors().border_strong)).corner_radius(look.radius_l).inner_margin(crate::theme::margin(look.space_m)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical().id_salt("graph_details").auto_shrink([false, false]).show(ui, |ui| {
                ui.allocate_ui_with_layout(Vec2::new(ui.available_width(), 22.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button(egui_phosphor::regular::X).on_hover_text("Close (x)").clicked() {
                        self.drawer_open = false;
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(egui::Label::new(RichText::new(&card.label).strong().size(look.text_heading).color(colors().text_strong)).truncate()).on_hover_text(&card.label);
                    });
                });
                ui.label(RichText::new(&card.key).monospace().size(look.text_small).color(colors().muted));
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&card.kind).size(look.text_body).color(colors().muted));
                    if card.mark != Mark::Code {
                        ui.label(RichText::new(crate::graph_style::mark_label(card.mark)).size(look.text_body).color(style_color(card.mark)));
                    }
                    if card.outside_plan {
                        ui.label(RichText::new("code the plan plugs into").size(look.text_body).color(colors().muted));
                    }
                });
                if !card.other_plans.is_empty() {
                    let names: Vec<&str> = card.other_plans.iter().map(|p| p.0.as_str()).collect();
                    ui.label(RichText::new(format!("Also changed by {}", names.join(", "))).size(look.text_body).color(colors().special));
                }
                if card.more > 0 {
                    ui.label(RichText::new(format!("{} more under it (z o to open)", card.more)).size(look.text_body).color(colors().muted));
                }
                if let Some(mark) = card.component.as_ref().and_then(|id| self.marks.get(id)) {
                    ui.horizontal_wrapped(|ui| {
                        if mark.changed {
                            ui.label(RichText::new("changed since review").size(look.text_body).color(colors().info));
                        }
                        if mark.open > 0 {
                            ui.label(RichText::new(format!("{} open review points", mark.open)).size(look.text_body).color(colors().warn));
                        }
                    });
                }
                // What's on its lines (the selection's pills, listed).
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
                ui.separator();
                match &detail {
                    None | Some(Detail::Loading) if card.component.is_some() => {
                        ui.label(RichText::new("Loading…").size(look.text_body).color(colors().muted));
                    }
                    None | Some(Detail::Loading) => {}
                    Some(Detail::Failed(e)) => {
                        ui.label(RichText::new(format!("Couldn't load its details: {e}")).size(look.text_body).color(colors().error));
                    }
                    Some(Detail::Ready(d)) => go = self.render_component(ui, d, in_code.as_ref()),
                }
            });
        });
        // A relation clicked: selected here if it's on screen, else gone into.
        if let Some(key) = go {
            if self.jumped.is_none() {
                self.jumped = Some(crate::nav::Navigable::place(self));
            }
            self.reveal(arch_graph::ComponentId::new(key));
        }
    }

    /// A component's details; with the code's own version beside it when
    /// the plan changes it. Returns a card to go into, if one was clicked.
    fn render_component(&mut self, ui: &mut egui::Ui, d: &ComponentDetail, in_code: Option<&Detail>) -> Option<String> {
        let look = crate::theme::look();
        let c = &d.component;
        let code = match in_code {
            Some(Detail::Ready(code)) => Some(&code.component),
            _ => None,
        };
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(format!("{} · {:?}", c.type_name, c.status)).size(look.text_body).color(colors().muted));
            crate::status::chip(ui, crate::status::stage(c.stage), None);
        });
        ui.label(RichText::new(location(c)).monospace().size(look.text_body));
        match (&c.signature, code.and_then(|k| k.signature.as_ref())) {
            (Some(planned), Some(real)) if planned != real => {
                ui.label(RichText::new(format!("In the plan: {}", signature(planned))).monospace().size(look.text_small).color(colors().warn));
                ui.label(RichText::new(format!("In the code: {}", signature(real))).monospace().size(look.text_small).color(colors().text));
            }
            (Some(sig), _) => {
                ui.label(RichText::new(signature(sig)).monospace().size(look.text_small).color(colors().muted));
            }
            _ => {}
        }
        if !c.description.is_empty() {
            ui.label(RichText::new(&c.description).size(look.text_body));
        }
        let spec = c.spec.as_ref();
        ui.label(
            RichText::new(format!("{} logic steps · {} invariants · {} placeholders", spec.map_or(0, |s| s.logic_steps.len()), spec.map_or(0, |s| s.invariants.len()), c.placeholders.len()))
                .size(look.text_body)
                .color(colors().muted),
        );
        if d.stale {
            ui.label(RichText::new("Its file changed since it was read.").size(look.text_body).color(colors().warn));
        }
        ui.horizontal(|ui| {
            if ui.button(format!("{} Spec view", egui_phosphor::regular::ARTICLE)).on_hover_text("g s").clicked() {
                self.requests.open_spec = true;
            }
            if ui.button(format!("{} Code", egui_phosphor::regular::CODE)).on_hover_text("g d").clicked() {
                self.requests.open_code = match (&c.anchor, &c.proposed) {
                    (Some(a), _) if crate::status::is_folder(a) => None,
                    (Some(a), _) => Some((a.file.to_string(), a.span.start_line, a.span.end_line)),
                    (None, Some(p)) => Some((p.file.to_string(), 1, 1)),
                    (None, None) => None,
                };
            }
        });
        ui.separator();
        let mut go = None;
        for kind in RelationKind::ALL {
            let out: Vec<&arch_graph::Relation> = d.outgoing.iter().filter(|r| r.kind == kind).collect();
            let inc: Vec<&arch_graph::Relation> = d.incoming.iter().filter(|r| r.kind == kind).collect();
            let (to, from) = (egui_phosphor::regular::ARROW_RIGHT, egui_phosphor::regular::ARROW_LEFT);
            for (label, list, outward) in [(format!("{} {to}", kind.name()), out, true), (format!("{from} {}", kind.name()), inc, false)] {
                if list.is_empty() {
                    continue;
                }
                ui.label(RichText::new(format!("{label} ({})", list.len())).size(look.text_small).color(colors().muted));
                for r in list {
                    let other = if outward { &r.target } else { &r.source };
                    let name = other.as_str().rsplit([':', '.']).next().unwrap_or(other.as_str());
                    // A planned relation the code doesn't have is said so; a
                    // call found in the code, how it's known.
                    let planned_only = r.provenance.planned && !r.provenance.observed;
                    let (text, how) = match arch_graph::tree::Evidence::of(r) {
                        _ if planned_only => (format!("{name} (planned)"), String::new()),
                        Some(arch_graph::tree::Evidence::Named) if r.kind == RelationKind::Calls => (format!("{name} (matched by name)"), "\nNot confirmed by the language's own analyzer.".to_owned()),
                        Some(arch_graph::tree::Evidence::Proven(by)) => (name.to_owned(), format!("\nConfirmed by {by}.")),
                        _ => (name.to_owned(), String::new()),
                    };
                    if ui.link(RichText::new(text).size(look.text_body)).on_hover_text(format!("{}{how}", other.as_str())).clicked() {
                        go = Some(other.0.clone());
                    }
                }
            }
        }
        go
    }
}

fn style_color(mark: Mark) -> egui::Color32 {
    crate::graph_style::style().node_outline(&colors(), mark, false).color
}

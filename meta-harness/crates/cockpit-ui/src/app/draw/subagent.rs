//! A sub-agent over the panels (one of what's over them: a tap outside or
//! the way back out closes it): who it is, how it's doing, what it was
//! told, its steps and messages as they come; stopping it alone, where its
//! harness can (said, where it can't); where it started, in its chat.

use egui::{Frame, RichText, Stroke};
use egui_phosphor::regular as icon;
use harness_contracts::{ContentBlock, ItemBody, ItemStatus};

use super::super::*;
use crate::chat::subagents::{elapsed, state_words};

/// Its steps shown, at the most (the latest).
const STEPS_MOST: usize = 200;

impl CockpitApp {
    pub(in crate::app) fn render_subagent(&mut self, ctx: &egui::Context) {
        let Some((slot, id)) = self.subagent_look.clone() else { return };
        let now = ctx.input(|i| i.time);
        let look = crate::theme::look();
        let screen = ctx.content_rect();
        let width = (screen.width() - 2.0 * look.space_l).min(640.0);
        let mut asked: Option<Asked> = None;
        let shown = {
            let agents = self.desk.shared.agents.get();
            let Some((view, seen)) = agents.thread_in(&slot).and_then(|v| v.subagent(&id).map(|s| (v, s.clone()))) else {
                drop(agents);
                self.subagent_look = None;
                return;
            };
            let s = &seen.subagent;
            let steps: Vec<(String, Option<ItemStatus>)> = view
                .under(&s.item_id)
                .filter_map(|item| match &item.body {
                    ItemBody::ToolCall { name, .. } => Some((item.title.clone().unwrap_or_else(|| name.clone()), Some(item.status))),
                    ItemBody::AssistantMessage { content, .. } => {
                        let text: String = content.iter().filter_map(|b| if let ContentBlock::Text { text } = b { Some(text.as_str()) } else { None }).collect();
                        (!text.trim().is_empty()).then_some((text, None))
                    }
                    _ => None,
                })
                .collect();
            let thread = view.thread.id.clone();
            egui::Area::new(egui::Id::new("subagent_look")).order(egui::Order::Foreground).anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO).constrain(true).show(ctx, |ui| {
                Frame::NONE.fill(colors().panel).stroke(Stroke::new(look.line_thin, colors().focus)).corner_radius(look.radius_xl).inner_margin(crate::theme::margin(look.space_m)).show(ui, |ui| {
                    ui.set_width(width);
                    let (state, colour) = state_words(s.status);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(format!("{} {}", icon::ROBOT, s.title)).strong().size(look.text_title).color(colors().text_strong));
                        if let Some(kind) = &s.kind {
                            ui.label(RichText::new(kind).size(look.text_small).color(colors().muted));
                        }
                        ui.label(RichText::new(format!("{} {state}", icon::CIRCLE)).size(look.text_body).color(colour));
                        ui.label(RichText::new(elapsed(&seen, now)).size(look.text_small).color(colors().muted));
                    });
                    if let Some(task) = &s.description {
                        ui.label(RichText::new(task).size(look.text_body));
                    }
                    if let Some(doing) = s.doing.as_ref().filter(|_| s.status.working()) {
                        ui.label(RichText::new(format!("Doing: {doing}")).size(look.text_small).color(colors().info));
                    }
                    if let Some(prompt) = &s.prompt {
                        egui::CollapsingHeader::new(RichText::new("What it was told").size(look.text_small)).id_salt(("subagent_prompt", &s.id)).show(ui, |ui| {
                            egui::ScrollArea::vertical().id_salt(("subagent_prompt_text", &s.id)).max_height(screen.height() * 0.2).show(ui, |ui| {
                                ui.label(RichText::new(prompt).size(look.text_small));
                            });
                        });
                    }
                    ui.separator();
                    egui::ScrollArea::vertical().id_salt(("subagent_steps", &s.id)).max_height(screen.height() * 0.4).stick_to_bottom(true).auto_shrink([false, true]).show(ui, |ui| {
                        if steps.is_empty() {
                            ui.label(RichText::new("Its steps show here as they come, when its harness says which are its.").size(look.text_small).color(colors().muted));
                        }
                        for (text, status) in steps.iter().rev().take(STEPS_MOST).rev() {
                            match status {
                                Some(status) => ui.label(RichText::new(format!("{} {text}", crate::chat::status_tag(*status).0)).monospace().size(look.text_small)),
                                None => ui.label(RichText::new(text).size(look.text_body)),
                            };
                        }
                    });
                    ui.separator();
                    ui.horizontal_wrapped(|ui| {
                        if s.status.working() {
                            if s.can_cancel {
                                if ui.add(egui::Button::new(format!("{} Stop it", icon::STOP))).on_hover_text("Stops its work alone; the agent's turn goes on").clicked() {
                                    asked = Some(Asked::Stop(thread.clone()));
                                }
                            } else {
                                ui.label(RichText::new("Its harness can't stop it alone (stopping the turn stops it).").size(look.text_small).color(colors().muted));
                            }
                        }
                        if ui.button("Where it started").clicked() {
                            asked = Some(Asked::GoTo);
                        }
                        if ui.button("Close").clicked() {
                            asked = Some(Asked::Close);
                        }
                    });
                });
            })
        };
        self.overlays.drawn_at(crate::overlay::Overlay::Subagent, shown.response.rect);
        match asked {
            Some(Asked::Stop(thread)) => self.desk.shared.agents.get_mut().act(slot, crate::agents::Act::StopSubagent { thread, id }, &self.desk.net, ctx),
            Some(Asked::GoTo) => {
                self.subagent_look = None;
                let jump = self.jump_of(self.focused_kind());
                self.land(Some(BufferKind::Chat), &crate::nav::Target::Subagent { slot, id }, jump, ctx);
            }
            Some(Asked::Close) => self.subagent_look = None,
            None => {}
        }
    }
}

/// What its controls asked for.
enum Asked {
    Stop(harness_contracts::ThreadId),
    GoTo,
    Close,
}

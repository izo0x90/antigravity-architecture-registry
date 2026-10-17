//! Talking to the agent from anywhere: a box over whatever is on screen.
//! Typing goes into it, Enter sends to the lead's session (opening one if
//! there's none), and the agent's last message shows above it; Escape
//! closes it. Nothing on screen moves.

use egui::{Frame, RichText, Stroke};

use crate::agents::{Act, Agents};
use crate::net::NetClient;
use crate::theme::colors;

#[derive(Default)]
pub struct Talk {
    /// What's typed and not sent yet.
    pub text: String,
    /// Sent from here at least once (the reply is worth showing).
    pub sent: bool,
}

/// What the user did with the box this frame.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    Open,
    Close,
}

impl Talk {
    /// Draws the box above the key bar; sends on the send key.
    /// what closes it, for its hint.
    pub fn render(&mut self, ctx: &egui::Context, agents: &mut Agents, net: &NetClient) -> Outcome {
        let look = crate::theme::look();
        let screen = ctx.content_rect();
        let width = (screen.width() - 2.0 * look.space_l).min(look.quick_look_width);
        let mut outcome = Outcome::Open;
        egui::Area::new(egui::Id::new("talk")).order(egui::Order::Foreground).anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -look.space_huge)).show(ctx, |ui| {
            Frame::NONE.fill(colors().panel).stroke(Stroke::new(look.line_thin, colors().focus)).corner_radius(look.radius_xl).inner_margin(crate::theme::margin(look.space_m)).show(ui, |ui| {
                ui.set_width(width);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{} {}", egui_phosphor::regular::CHAT_CIRCLE_TEXT, agents.who())).strong().size(look.text_label).color(colors().text_strong));
                });
                if self.sent && let Some(reply) = agents.last_reply() {
                    egui::ScrollArea::vertical().id_salt("talk_reply").max_height(screen.height() * look.quick_look_height / 3.0).stick_to_bottom(true).show(ui, |ui| {
                        ui.label(RichText::new(reply).size(look.text_body).color(colors().text));
                    });
                }
                let field = ui.add(crate::keymap::local().text_box(&mut self.text).id(egui::Id::new("talk_input")).desired_rows(2).desired_width(f32::INFINITY).hint_text("Ask the agent…"));
                field.request_focus();
                let enter = ui.input(|i| crate::keymap::local().input.sent(i).is_some());
                if enter && !self.text.trim().is_empty() {
                    let text = self.text.trim().to_owned();
                    self.text.clear();
                    self.sent = true;
                    agents.act(harness_contracts::SlotId::new(harness_contracts::LEAD), Act::Prompt { text, add: false }, net, ctx);
                } else if enter {
                    self.text.clear();
                }
            });
        });
        if ctx.input(|i| crate::keymap::local().back(i)) {
            outcome = Outcome::Close;
        }
        outcome
    }
}

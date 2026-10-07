//! The status vocabulary: every state the cockpit shows (indexing, plan
//! alignment, stages, review marks) has one icon, one word and one theme
//! colour, from the `[status]` section of the UI config. Views draw states
//! as chips through here, so a state looks the same wherever it appears.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use egui::{Color32, RichText};
use serde::Deserialize;

use crate::theme::{Theme, colors};

/// How one state looks: its word, its icon (by name) and its theme colour
/// (by token).
#[derive(Clone, Debug, Deserialize)]
pub struct StatusLook {
    pub word: String,
    pub icon: String,
    pub color: String,
}

pub type Statuses = BTreeMap<String, StatusLook>;

/// Icon names config may use, and their glyphs.
pub fn icon(name: &str) -> Option<&'static str> {
    use egui_phosphor::regular as i;
    Some(match name {
        "check-circle" => i::CHECK_CIRCLE,
        "arrows-clockwise" => i::ARROWS_CLOCKWISE,
        "clock-counter-clockwise" => i::CLOCK_COUNTER_CLOCKWISE,
        "warning" => i::WARNING,
        "warning-octagon" => i::WARNING_OCTAGON,
        "warning-circle" => i::WARNING_CIRCLE,
        "x-circle" => i::X_CIRCLE,
        "circle" => i::CIRCLE,
        "circle-dashed" => i::CIRCLE_DASHED,
        "circle-half" => i::CIRCLE_HALF,
        "hourglass" => i::HOURGLASS,
        "chat-circle" => i::CHAT_CIRCLE,
        "chat-circle-dots" => i::CHAT_CIRCLE_DOTS,
        "pencil-simple" => i::PENCIL_SIMPLE,
        "dot" => i::DOT,
        "dot-outline" => i::DOT_OUTLINE,
        "seal-check" => i::SEAL_CHECK,
        "file-dashed" => i::FILE_DASHED,
        "file-code" => i::FILE_CODE,
        "code" => i::CODE,
        "lightbulb" => i::LIGHTBULB,
        "path" => i::PATH,
        "stack" => i::STACK,
        "puzzle-piece" => i::PUZZLE_PIECE,
        "list-checks" => i::LIST_CHECKS,
        "robot" => i::ROBOT,
        "hand" => i::HAND,
        "play" => i::PLAY,
        "pause" => i::PAUSE,
        "question" => i::QUESTION,
        "eye" => i::EYE,
        "flag" => i::FLAG,
        "lock" => i::LOCK,
        "sparkle" => i::SPARKLE,
        _ => return None,
    })
}

/// Every state names a known icon and theme colour.
pub fn check(statuses: &Statuses, theme: &Theme) -> Result<(), String> {
    for (name, look) in statuses {
        if icon(&look.icon).is_none() {
            return Err(format!("status.{name}: unknown icon `{}`", look.icon));
        }
        if theme.token(&look.color).is_none() {
            return Err(format!("status.{name}: unknown theme colour `{}`", look.color));
        }
    }
    Ok(())
}

thread_local! {
    static CURRENT: RefCell<Rc<Statuses>> = RefCell::new(Rc::new(Statuses::new()));
}

pub fn set(statuses: Statuses) {
    CURRENT.with(|c| *c.borrow_mut() = Rc::new(statuses));
}

/// A state's glyph, word and colour (a missing state shows its own name).
pub fn look(name: &str) -> (&'static str, String, Color32) {
    let statuses = CURRENT.with(|c| c.borrow().clone());
    match statuses.get(name) {
        Some(look) => (icon(&look.icon).unwrap_or_default(), look.word.clone(), colors().token(&look.color).unwrap_or(colors().muted)),
        None => ("", name.to_owned(), colors().muted),
    }
}

/// A state as a chip: its icon and word (and an optional detail, e.g. a
/// count), in its colour.
pub fn chip(ui: &mut egui::Ui, name: &str, detail: Option<&str>) -> egui::Response {
    let (glyph, word, color) = look(name);
    let text = match detail {
        Some(d) => format!("{glyph} {d} {word}"),
        None => format!("{glyph} {word}"),
    };
    egui::Frame::NONE
        .fill(color.gamma_multiply(0.15))
        .stroke(egui::Stroke::new(crate::theme::look().line_thin, color.gamma_multiply(0.6)))
        .corner_radius(crate::theme::look().radius_xl)
        .inner_margin(crate::theme::margin2(crate::theme::look().space_sm, crate::theme::look().space_hair))
        .show(ui, |ui| ui.label(RichText::new(text).size(crate::theme::look().text_body).color(color)))
        .response
}

/// A state as inline text: icon and word, coloured (for tight spots).
pub fn text(name: &str) -> RichText {
    let (glyph, word, color) = look(name);
    RichText::new(format!("{glyph} {word}")).color(color)
}

/// The vocabulary's name for how plan and code line up.
pub fn alignment(a: arch_graph::Alignment) -> &'static str {
    match a {
        arch_graph::Alignment::Observed => "observed",
        arch_graph::Alignment::Planned => "planned",
        arch_graph::Alignment::Missing => "missing",
        arch_graph::Alignment::Present => "present",
    }
}

/// The vocabulary's name for a lifecycle stage.
pub fn stage(s: arch_graph::Stage) -> &'static str {
    match s {
        arch_graph::Stage::Declared => "stage_declared",
        arch_graph::Stage::ArchApproved => "stage_arch_approved",
        arch_graph::Stage::PlanApproved => "stage_plan_approved",
        arch_graph::Stage::Skeleton => "stage_skeleton",
        arch_graph::Stage::Implemented => "stage_implemented",
    }
}

/// The vocabulary's name for a queue job's state.
pub fn job(state: &arch_graph::JobState) -> &'static str {
    match state {
        arch_graph::JobState::Queued => "job_queued",
        arch_graph::JobState::Running { .. } => "job_running",
        arch_graph::JobState::AwaitingApproval => "job_awaiting_approval",
        arch_graph::JobState::Retrying => "job_retrying",
        arch_graph::JobState::Failed => "job_failed",
        arch_graph::JobState::Done => "job_done",
    }
}

/// The vocabulary's name for a finding's severity.
pub fn severity(s: arch_graph::Severity) -> &'static str {
    match s {
        arch_graph::Severity::Error => "severity_error",
        arch_graph::Severity::Warning => "severity_warning",
        arch_graph::Severity::Info => "severity_info",
    }
}

/// Steps in order, the ones before `current` done, `current` lit, the rest
/// ahead: dots joined by a line, each with its word under it.
pub fn track(ui: &mut egui::Ui, steps: &[(&str, &str)], current: Option<usize>) -> egui::Response {
    let look = crate::theme::look();
    let theme = colors();
    let gap = look.space_xxxl * 2.0;
    let dot = look.space_s;
    let font = egui::FontId::proportional(look.text_small);
    let height = dot * 4.0 + look.text_small * 1.6;
    let width = gap * (steps.len().max(1) - 1) as f32 + gap;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let painter = ui.painter_at(rect.expand(look.space_s));
    let x = |i: usize| rect.left() + gap / 2.0 + gap * i as f32;
    let y = rect.top() + dot * 2.0;
    for (i, (name, label)) in steps.iter().enumerate() {
        let (_, _, color) = look_of(name);
        let state = match current {
            Some(c) if i < c => 0,
            Some(c) if i == c => 1,
            _ => 2,
        };
        if i + 1 < steps.len() {
            let line = if state == 0 { color } else { theme.border_strong };
            painter.line_segment([egui::pos2(x(i) + dot, y), egui::pos2(x(i + 1) - dot, y)], egui::Stroke::new(look.line_bold, line));
        }
        let center = egui::pos2(x(i), y);
        match state {
            0 => {
                painter.circle_filled(center, dot, color);
            }
            1 => {
                painter.circle_filled(center, dot * 1.4, color);
                painter.circle_stroke(center, dot * 2.0, egui::Stroke::new(look.line_thin, color.gamma_multiply(0.6)));
            }
            _ => {
                painter.circle_stroke(center, dot, egui::Stroke::new(look.line_thin, theme.faint));
            }
        }
        let text_color = match state {
            1 => color,
            0 => theme.muted,
            _ => theme.faint,
        };
        painter.text(egui::pos2(x(i), y + dot * 2.5), egui::Align2::CENTER_TOP, *label, font.clone(), text_color);
    }
    response
}

/// `look` under another name (inside `track`, where `look` is the sizes).
fn look_of(name: &str) -> (&'static str, String, Color32) {
    look(name)
}

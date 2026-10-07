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
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.6)))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| ui.label(RichText::new(text).size(11.0).color(color)))
        .response
}

/// A state as inline text: icon and word, coloured (for tight spots).
pub fn text(name: &str) -> RichText {
    let (glyph, word, color) = look(name);
    RichText::new(format!("{glyph} {word}")).color(color)
}

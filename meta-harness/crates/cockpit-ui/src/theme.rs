//! Semantic colors, from the `[theme]` section of the UI config. Views ask for
//! roles (`colors().warn`), never for literal colors.

use std::cell::RefCell;
use std::rc::Rc;

use egui::Color32;
use serde::{Deserialize, Deserializer};

/// "#rrggbb" or "#rrggbbaa" (alpha not premultiplied).
fn parse_hex(text: &str) -> Result<Color32, String> {
    let hex = text.strip_prefix('#').unwrap_or(text);
    let byte = |i: usize| hex.get(i..i + 2).and_then(|b| u8::from_str_radix(b, 16).ok());
    match (hex.len(), byte(0), byte(2), byte(4)) {
        (6, Some(r), Some(g), Some(b)) => Ok(Color32::from_rgb(r, g, b)),
        (8, Some(r), Some(g), Some(b)) => match byte(6) {
            Some(a) => Ok(Color32::from_rgba_unmultiplied(r, g, b, a)),
            None => Err(format!("bad color `{text}`")),
        },
        _ => Err(format!("bad color `{text}` (want #rrggbb or #rrggbbaa)")),
    }
}

fn hex<'de, D: Deserializer<'de>>(d: D) -> Result<Color32, D::Error> {
    let text = String::deserialize(d)?;
    parse_hex(&text).map_err(serde::de::Error::custom)
}

macro_rules! theme {
    ($($(#[$doc:meta])* $name:ident),* $(,)?) => {
        #[derive(Clone, Debug, Default, Deserialize)]
        pub struct Theme {
            $($(#[$doc])* #[serde(deserialize_with = "hex")] pub $name: Color32,)*
        }

        impl Theme {
            /// Every token name, as written in config.
            pub const TOKENS: &'static [&'static str] = &[$(stringify!($name)),*];

            /// A color by its token name (for config that refers to tokens).
            pub fn token(&self, name: &str) -> Option<Color32> {
                match name {
                    $(stringify!($name) => Some(self.$name),)*
                    _ => None,
                }
            }
        }
    };
}

theme! {
    /// App background and slot bodies.
    bg,
    /// Bars (top bar).
    panel,
    /// Cards, drawers, inputs.
    surface,
    /// Raised elements on a surface (buttons, chips).
    raised,
    /// Selected rows and active items.
    selection,
    border,
    border_strong,
    /// The focused slot's outline.
    focus,
    /// Graph grid lines.
    grid,
    text_strong,
    text,
    muted,
    faint,
    /// Text on a bright fill.
    on_bright,
    accent,
    info,
    /// Key hints in menus.
    key,
    ok,
    warn,
    error,
    /// Reasoning and other secondary agent output.
    special,
    info_bg,
    ok_bg,
    warn_bg,
    error_bg,
    special_bg,
    /// Floating panels (palette, which-key).
    overlay,
    overlay_raised,
    /// Hover highlight drawn over inspectable elements.
    highlight,
    /// egui's own panel and window background.
    chrome,
    /// egui's selected tabs, toggles and text selection.
    widget_selection,
    link,
    /// Graph cards by kind (component, call, plan step), plain and selected.
    card,
    card_selected,
    card_special,
    card_special_selected,
    card_ok,
    card_ok_selected,
}

impl Theme {
    /// Applies the theme to egui's own widgets (panels, windows, selection).
    pub fn apply(&self, ctx: &egui::Context) {
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = self.chrome;
        visuals.window_fill = self.chrome;
        visuals.selection.bg_fill = self.widget_selection;
        visuals.hyperlink_color = self.link;
        ctx.set_visuals(visuals);
    }
}

thread_local! {
    static CURRENT: RefCell<Rc<Theme>> = RefCell::new(Rc::new(Theme::default()));
}

/// The active theme (the UI is single-threaded).
pub fn colors() -> Rc<Theme> {
    CURRENT.with(|c| c.borrow().clone())
}

pub fn set(theme: Theme, ctx: &egui::Context) {
    theme.apply(ctx);
    CURRENT.with(|c| *c.borrow_mut() = Rc::new(theme));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_colors_parse() {
        assert_eq!(parse_hex("#fab432"), Ok(Color32::from_rgb(250, 180, 50)));
        assert_eq!(parse_hex("#ffffff80"), Ok(Color32::from_rgba_unmultiplied(255, 255, 255, 128)));
        assert!(parse_hex("#fab4").is_err());
        assert!(parse_hex("#zzzzzz").is_err());
    }
}

/// The cockpit's fonts: egui's defaults plus the phosphor icon font (also in
/// monospace, after the first face, so icons show in code text too).
pub fn font_definitions() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    if let Some(font_keys) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
        font_keys.insert(1, "phosphor".into());
    }
    fonts
}

#[cfg(test)]
mod glyph_tests {
    /// Every UI source file: whatever text they draw must have glyphs.
    const SOURCES: [(&str, &str); 15] = [
        ("actions.rs", include_str!("actions.rs")),
        ("app.rs", include_str!("app.rs")),
        ("chat.rs", include_str!("chat.rs")),
        ("code.rs", include_str!("code.rs")),
        ("feedback.rs", include_str!("feedback.rs")),
        ("graph.rs", include_str!("graph.rs")),
        ("graph_style.rs", include_str!("graph_style.rs")),
        ("keymap.rs", include_str!("keymap.rs")),
        ("palette.rs", include_str!("palette.rs")),
        ("queue.rs", include_str!("queue.rs")),
        ("review.rs", include_str!("review.rs")),
        ("spec.rs", include_str!("spec.rs")),
        ("tree.rs", include_str!("tree.rs")),
        ("touch.rs", include_str!("touch.rs")),
        ("../config/default.toml", include_str!("../config/default.toml")),
    ];

    #[test]
    fn every_character_the_ui_draws_has_a_glyph() {
        let ctx = egui::Context::default();
        ctx.set_fonts(super::font_definitions());
        let _ = ctx.run(Default::default(), |_| {});
        let mut missing = Vec::new();
        for (file, text) in SOURCES {
            for (n, line) in text.lines().enumerate() {
                let code = line.trim_start();
                if code.starts_with("//") || code.starts_with('#') {
                    continue;
                }
                for c in line.chars().filter(|c| !c.is_ascii()) {
                    let ok = ctx.fonts_mut(|f| f.has_glyph(&egui::FontId::proportional(12.0), c) && f.has_glyph(&egui::FontId::monospace(12.0), c));
                    if !ok {
                        missing.push(format!("{file}:{} {c:?} (U+{:04X})", n + 1, c as u32));
                    }
                }
            }
        }
        assert!(missing.is_empty(), "characters the fonts can't draw:\n{}", missing.join("\n"));
    }
}

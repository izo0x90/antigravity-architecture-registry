//! Semantic colors, from the `[theme]` section of the UI config, and the
//! look's sizes (text, spacing, corners, lines, effects) from `[look]`.
//! Views ask for roles (`colors().warn`, `look().text_body`), never for
//! literal colors or sizes.

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

macro_rules! look {
    ($($(#[$doc:meta])* $name:ident),* $(,)?) => {
        /// Sizes, from `[look]`: every one in points (or as noted).
        #[derive(Clone, Debug, Default, Deserialize)]
        pub struct Look {
            $($(#[$doc])* pub $name: f32,)*
        }

        impl Look {
            pub const TOKENS: &'static [&'static str] = &[$(stringify!($name)),*];
        }
    };
}

look! {
    /// Text sizes, smallest to largest.
    text_tiny,
    text_small,
    text_body,
    text_label,
    text_title,
    text_heading,
    text_display,
    /// The spacing scale.
    space_hair,
    space_xs,
    space_s,
    space_sm,
    space_m,
    space_ml,
    space_l,
    space_xl,
    space_xxl,
    space_xxxl,
    space_huge,
    /// Corner radii.
    radius_s,
    radius_m,
    radius_l,
    radius_xl,
    radius_pill,
    /// Line widths.
    line_thin,
    line_bold,
    /// The selection's glow: how wide, and how strong (0 to 1).
    glow_width,
    glow_alpha,
    /// The pulse around what the agent points at: one beat, in seconds.
    pulse_seconds,
    /// Planned connections' dashes move this many points a second.
    dash_speed,
    /// How strongly what's not selected fades (0: not at all, 1: gone).
    fade,
    /// The background grid's spacing (0: no grid).
    grid_step,
    /// Merged lines grow with how many relations they stand for: this much
    /// width per doubling, up to this many times their own width.
    line_count_gain,
    line_count_max,
    /// How long things take to move (cards rearranging, going into a
    /// level or out of it), in seconds (0: at once).
    move_seconds,
}

/// The built-in look (the `defaults_parse` test keeps it loadable).
fn built_in_look() -> Look {
    crate::config::UiConfig::with_overrides("").map(|c| c.look).unwrap_or_default()
}

/// Inner margins from look sizes (egui wants whole points).
pub fn margin(all: f32) -> egui::Margin {
    egui::Margin::same(all.round() as i8)
}

pub fn margin2(x: f32, y: f32) -> egui::Margin {
    egui::Margin::symmetric(x.round() as i8, y.round() as i8)
}

thread_local! {
    static CURRENT: RefCell<Rc<Theme>> = RefCell::new(Rc::new(Theme::default()));
    static LOOK: RefCell<Rc<Look>> = RefCell::new(Rc::new(built_in_look()));
}

/// The active look's sizes.
pub fn look() -> Rc<Look> {
    LOOK.with(|l| l.borrow().clone())
}

pub fn set_look(look: Look) {
    LOOK.with(|l| *l.borrow_mut() = Rc::new(look));
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

    // req: V.7
    #[test]
    fn no_view_writes_its_own_colours_or_sizes() {
        // Calls whose first argument must be a theme or look value, never a number.
        let sized = [".size(", "FontId::proportional(", "FontId::monospace(", "add_space(", "Margin::same(", "Margin::symmetric(", "inner_margin(", "corner_radius(", "Stroke::new("];
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if name == "theme.rs" || path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            // What the UI draws (tests check values, they don't draw).
            let drawn = text.split("#[cfg(test)]").next().unwrap_or_default();
            for (n, line) in drawn.lines().enumerate() {
                for call in sized {
                    for (at, _) in line.match_indices(call) {
                        if line[at + call.len()..].trim_start().starts_with(|c: char| c.is_ascii_digit()) {
                            found.push(format!("{name}:{} {}", n + 1, line.trim()));
                        }
                    }
                }
                if line.contains("Color32::from_") || (line.contains("Color32::") && !line.contains("Color32::TRANSPARENT") && line.contains("Color32::") && line.split("Color32::").skip(1).any(|rest| rest.starts_with(|c: char| c.is_ascii_uppercase()) && !rest.starts_with("TRANSPARENT"))) {
                    found.push(format!("{name}:{} {}", n + 1, line.trim()));
                }
            }
        }
        assert!(found.is_empty(), "colours and sizes come from the theme and look:\n{}", found.join("\n"));
    }

    #[test]
    fn presets_lay_their_sections_over_the_rest() {
        let config = crate::config::UiConfig::with_overrides("preset = \"big\"\n[presets.big.look]\ntext_body = 15.0\n[presets.big.theme]\naccent = \"#ff00ff\"\n").unwrap();
        assert_eq!((config.look.text_body, config.theme.accent), (15.0, egui::Color32::from_rgb(255, 0, 255)));
        assert!(crate::config::UiConfig::with_overrides("preset = \"nope\"\n").unwrap_err().contains("no preset `nope`"));
    }
}

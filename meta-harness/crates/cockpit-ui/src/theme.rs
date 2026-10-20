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
    /// A terminal's 16 named colours (what programs ask for by number,
    /// 0-7 then their bright 8-15); its default text and background are
    /// `text` and `bg`.
    term_black,
    term_red,
    term_green,
    term_yellow,
    term_blue,
    term_magenta,
    term_cyan,
    term_white,
    term_bright_black,
    term_bright_red,
    term_bright_green,
    term_bright_yellow,
    term_bright_blue,
    term_bright_magenta,
    term_bright_cyan,
    term_bright_white,
    /// Tags' colours: a tag is one of these, by its number (0-7).
    tag_0,
    tag_1,
    tag_2,
    tag_3,
    tag_4,
    tag_5,
    tag_6,
    tag_7,
}

impl Theme {
    /// A colour a terminal's program asked for, as this theme shows it:
    /// the named ones the theme's, the rest of the 256 by xterm's formula
    /// (a 6×6×6 cube, then greys), RGB as it is. None: the terminal's own.
    pub fn term_colour(&self, colour: harness_contracts::Colour) -> Option<Color32> {
        use harness_contracts::Colour;
        const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];
        match colour {
            Colour::Default => None,
            Colour::Named(n) => Some(self.term(n)),
            Colour::Indexed(i) if i < 16 => Some(self.term(i)),
            Colour::Indexed(i) if i < 232 => {
                let i = i - 16;
                Some(Color32::from_rgb(CUBE[usize::from(i / 36)], CUBE[usize::from((i / 6) % 6)], CUBE[usize::from(i % 6)]))
            }
            Colour::Indexed(i) => {
                let grey = 8 + 10 * (i - 232);
                Some(Color32::from_rgb(grey, grey, grey))
            }
            Colour::Rgb(r, g, b) => Some(Color32::from_rgb(r, g, b)),
        }
    }

    /// A terminal's named colour `n` (0-15).
    pub fn term(&self, n: u8) -> Color32 {
        [self.term_black, self.term_red, self.term_green, self.term_yellow, self.term_blue, self.term_magenta, self.term_cyan, self.term_white, self.term_bright_black, self.term_bright_red, self.term_bright_green, self.term_bright_yellow, self.term_bright_blue, self.term_bright_magenta, self.term_bright_cyan, self.term_bright_white][usize::from(n & 15)]
    }

    /// Tag colour `n` (one of `TAG_COLOURS`).
    pub fn tag(&self, n: u8) -> Color32 {
        [self.tag_0, self.tag_1, self.tag_2, self.tag_3, self.tag_4, self.tag_5, self.tag_6, self.tag_7][usize::from(n % harness_contracts::TAG_COLOURS)]
    }

    /// Tag colour `n` in a word, as this theme has it ("orange").
    pub fn tag_name(&self, n: u8) -> &'static str {
        colour_name(self.tag(n))
    }

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
    /// With something selected, how far what isn't related to it dims
    /// (0: not at all, 1: gone).
    dim,
    /// Merged lines grow with how many relations they stand for: this much
    /// width per doubling, up to this many times their own width.
    line_count_gain,
    line_count_max,
    /// How long things take to move (cards rearranging, going into a
    /// level or out of it), in seconds (0: at once).
    move_seconds,
    /// The quick look: at least this wide (or its panel's width, when
    /// wider), and this share of the screen's height.
    quick_look_width,
    quick_look_height,
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

    /// WCAG contrast of `fg` drawn on `bg` (both opaque here).
    fn contrast(fg: Color32, bg: Color32) -> f32 {
        let lum = |c: Color32| {
            let ch = |v: u8| {
                let v = v as f32 / 255.0;
                if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
        };
        let (a, b) = (lum(fg), lum(bg));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn each_themes_tag_colours_have_names_of_their_own() {
        for preset in ["", "preset = \"classic\"\n"] {
            let theme = crate::config::UiConfig::with_overrides(preset).unwrap().theme;
            let names: Vec<&str> = (0..harness_contracts::TAG_COLOURS).map(|n| theme.tag_name(n)).collect();
            let mut unique = names.clone();
            unique.sort_unstable();
            unique.dedup();
            assert_eq!(unique.len(), names.len(), "{preset:?}: {names:?}");
        }
        assert_eq!((colour_name(Color32::from_rgb(56, 189, 248)), colour_name(Color32::from_rgb(128, 128, 130))), ("blue", "grey"));
    }

    #[test]
    fn keys_on_pills_are_readable_in_every_theme() {
        for preset in ["", "preset = \"classic\"\n"] {
            let theme = crate::config::UiConfig::with_overrides(preset).unwrap().theme;
            let opaque = |c: Color32| Color32::from_rgb(c.r(), c.g(), c.b());
            // A highlighted key (a bright pill), a plain one, a label on the popup.
            for (fg, bg, what) in [(theme.on_bright, theme.focus, "on_bright on focus"), (theme.key, opaque(theme.overlay_raised), "key on overlay_raised"), (theme.text_strong, opaque(theme.overlay), "text_strong on overlay")] {
                assert!(contrast(fg, bg) >= 4.5, "{preset}: {what} is {:.1}:1", contrast(fg, bg));
            }
        }
    }
}

/// The cockpit's fonts: egui's defaults plus the phosphor icon font (also in
/// monospace, after the first face, so icons show in code text too), and
/// its filled style as a family of its own (`solid`).
pub fn font_definitions() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    if let Some(font_keys) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
        font_keys.insert(1, "phosphor".into());
    }
    fonts.font_data.insert("phosphor-fill".into(), egui_phosphor::Variant::Fill.font_data().into());
    fonts.families.insert(egui::FontFamily::Name(SOLID.into()), vec!["phosphor-fill".into()]);
    fonts
}

/// The filled icons' family.
const SOLID: &str = "icons-solid";

/// A filled icon (`egui_phosphor::fill::…`) at `size`.
pub fn solid(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name(SOLID.into()))
}

/// Puts a text field's caret after its text (text put in it, not typed).
pub fn caret_to_end(ctx: &egui::Context, id: egui::Id, text: &str) {
    let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    let end = egui::text::CCursor::new(text.chars().count());
    state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
    state.store(ctx, id);
}

/// A tag's mark where it's named: a solid tag in its colour (the outline
/// one until the fonts are in, the first frame).
pub fn tag_mark(ui: &egui::Ui, colour: Color32, size: f32) -> egui::RichText {
    let solid_in = ui.fonts(|f| f.definitions().families.contains_key(&egui::FontFamily::Name(SOLID.into())));
    let mark = if solid_in { egui::RichText::new(egui_phosphor::fill::TAG).font(solid(size)) } else { egui::RichText::new(egui_phosphor::regular::TAG).size(size) };
    mark.color(colour)
}

#[cfg(test)]
mod glyph_tests {
    /// Every UI source file: whatever text they draw must have glyphs.
    const SOURCES: &[(&str, &str)] = &[
        ("actions.rs", include_str!("actions.rs")),
        ("app.rs", include_str!("app.rs")),
        ("inbox.rs", include_str!("inbox.rs")),
        ("app/capture.rs", include_str!("app/capture.rs")),
        ("app/commands.rs", include_str!("app/commands.rs")),
        ("app/going.rs", include_str!("app/going.rs")),
        ("app/keeping.rs", include_str!("app/keeping.rs")),
        ("app/keys.rs", include_str!("app/keys.rs")),
        ("app/layout.rs", include_str!("app/layout.rs")),
        ("app/lifecycle.rs", include_str!("app/lifecycle.rs")),
        ("app/look.rs", include_str!("app/look.rs")),
        ("app/outside.rs", include_str!("app/outside.rs")),
        ("app/views.rs", include_str!("app/views.rs")),
        ("app/draw/bars.rs", include_str!("app/draw/bars.rs")),
        ("app/draw/panels.rs", include_str!("app/draw/panels.rs")),
        ("app/draw/top_bar.rs", include_str!("app/draw/top_bar.rs")),
        ("app/draw/which_key.rs", include_str!("app/draw/which_key.rs")),
        ("chat.rs", include_str!("chat.rs")),
        ("chat/actions.rs", include_str!("chat/actions.rs")),
        ("chat/nav.rs", include_str!("chat/nav.rs")),
        ("chat/render.rs", include_str!("chat/render.rs")),
        ("chat/session.rs", include_str!("chat/session.rs")),
        ("code.rs", include_str!("code.rs")),
        ("feedback.rs", include_str!("feedback.rs")),
        ("graph/mod.rs", include_str!("graph/mod.rs")),
        ("graph/actions.rs", include_str!("graph/actions.rs")),
        ("graph/details.rs", include_str!("graph/details.rs")),
        ("graph/draw.rs", include_str!("graph/draw.rs")),
        ("graph/source.rs", include_str!("graph/source.rs")),
        ("graph/select.rs", include_str!("graph/select.rs")),
        ("graph_style.rs", include_str!("graph_style.rs")),
        ("keymap/mod.rs", include_str!("keymap/mod.rs")),
        ("palette.rs", include_str!("palette.rs")),
        ("queue.rs", include_str!("queue.rs")),
        ("review.rs", include_str!("review.rs")),
        ("spec.rs", include_str!("spec.rs")),
        ("tree/mod.rs", include_str!("tree/mod.rs")),
        ("tree/outline.rs", include_str!("tree/outline.rs")),
        ("diff.rs", include_str!("diff.rs")),
        ("agents.rs", include_str!("agents.rs")),
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

    #[test]
    fn card_widths_are_what_the_theme_draws() {
        let ctx = egui::Context::default();
        ctx.set_fonts(super::font_definitions());
        let _ = ctx.run(Default::default(), |_| {});
        let texts = ["CockpitApp", "fn · 12 more", "iiii", "WWWW", "AVATAR Tokyo", "arch_graph::tree::lay_out", "café → naïve", "ok ✅ done", "+6"];
        for size in [11.0, 14.0, 23.5] {
            for text in texts {
                let drawn = ctx.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), egui::FontId::proportional(size), egui::Color32::WHITE).size().x);
                let measured = arch_graph::tree::measure::text_width(text, size);
                // egui rounds the last glyph's place to a quarter pixel, puts
                // the whole pixel in the galley and draws the quarters into
                // the glyph: its width is up to an eighth over, under a pixel short.
                assert!(drawn - measured <= 0.13 && measured - drawn < 1.0, "{text:?} at {size}: drawn {drawn}, measured {measured}");
            }
        }
    }

    #[test]
    fn no_view_writes_its_own_colours_or_sizes() {
        // Calls whose first argument must be a theme or look value, never a number.
        let sized = [".size(", "FontId::proportional(", "FontId::monospace(", "add_space(", "Margin::same(", "Margin::symmetric(", "inner_margin(", "corner_radius(", "Stroke::new("];
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found = Vec::new();
        // Every file below src (the graph's own folder too).
        let mut files = Vec::new();
        let mut dirs = vec![src.clone()];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                } else {
                    files.push(path);
                }
            }
        }
        for path in files {
            let name = path.strip_prefix(&src).unwrap().to_string_lossy().into_owned();
            if name == "theme.rs" || name.ends_with("tests.rs") || path.extension().is_none_or(|e| e != "rs") {
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

/// A colour in a word, by its hue as written ("grey" when it has little).
pub fn colour_name(colour: Color32) -> &'static str {
    let [r, g, b] = [colour.r(), colour.g(), colour.b()].map(|c| f32::from(c) / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    if max - min < 0.2 * max.max(f32::EPSILON) {
        return "grey";
    }
    let sector = match max {
        m if m == r => (g - b) / (max - min),
        m if m == g => 2.0 + (b - r) / (max - min),
        _ => 4.0 + (r - g) / (max - min),
    };
    let hue = (sector * 60.0).rem_euclid(360.0);
    let names = [(15.0, "red"), (42.0, "orange"), (70.0, "yellow"), (160.0, "green"), (195.0, "teal"), (250.0, "blue"), (290.0, "violet"), (340.0, "pink"), (360.0, "red")];
    names.into_iter().find(|(below, _)| hue < *below).map_or("red", |(_, name)| name)
}

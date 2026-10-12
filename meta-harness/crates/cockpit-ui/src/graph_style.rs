//! How graph elements look, from the `[graph_style]` section of the UI
//! config: every color is a theme token name, so restyling is config only.
//! A line takes its color from what it is (a call, a data flow) and its
//! stroke from its plan mark; a card's outline comes from its plan mark;
//! lines to cards shown elsewhere (repeats, loops) and the pills on lines
//! have their own looks.

use std::cell::RefCell;
use std::rc::Rc;

use arch_graph::RelationKind;
use arch_graph::tree::{Evidence, LineKind, Mark, PillRole};
use egui::Color32;
use serde::Deserialize;

use crate::theme::Theme;

/// A theme token per relation kind (a line's color unless its mark says).
#[derive(Clone, Debug, Default, Deserialize)]
pub struct KindColors {
    pub contains: String,
    pub implements: String,
    pub calls: String,
    pub data_flow: String,
}

/// A line or outline: optional color (a theme token), width, dashing, and
/// how strongly it shows (0..1, default fully).
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Look {
    #[serde(default)]
    pub color: Option<String>,
    pub width: f32,
    #[serde(default)]
    pub dashed: bool,
    #[serde(default)]
    pub fade: Option<f32>,
}

/// Calls found in the code, by how they're known: a call only matched by
/// name (no language analyzer confirmed it) has its own look.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct EvidenceLooks {
    pub named: Look,
}

/// One look per plan mark.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct MarkLooks {
    pub code: Look,
    pub new: Look,
    pub changed: Look,
    pub done: Look,
    pub mismatch: Look,
}

impl MarkLooks {
    fn get(&self, mark: Mark) -> &Look {
        match mark {
            Mark::Code => &self.code,
            Mark::New => &self.new,
            Mark::Changed => &self.changed,
            Mark::Done => &self.done,
            Mark::Mismatch => &self.mismatch,
        }
    }

    fn all(&self) -> [(&'static str, &Look); 5] {
        [("code", &self.code), ("new", &self.new), ("changed", &self.changed), ("done", &self.done), ("mismatch", &self.mismatch)]
    }
}

/// Lines to a card shown elsewhere.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct BackLooks {
    pub repeat: Look,
    #[serde(rename = "loop")]
    pub loops: Look,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct NodeLooks {
    /// Outline per plan mark; `color` is required here.
    pub outline: MarkLooks,
    pub selected_outline: String,
    pub selected_width: f32,
    pub fill: String,
    pub selected_fill: String,
    /// Ring around the card the agent pointed at.
    pub agent: String,
    /// How strongly code a plan only plugs into shows (0..1).
    pub outside_plan_alpha: f32,
}

/// A theme token per pill role.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct PillColors {
    #[serde(rename = "in")]
    pub takes: String,
    #[serde(rename = "out")]
    pub gives: String,
    pub step: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct GraphStyle {
    pub kinds: KindColors,
    pub lines: MarkLooks,
    pub back: BackLooks,
    pub nodes: NodeLooks,
    pub pills: PillColors,
    pub evidence: EvidenceLooks,
}

/// A resolved line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    pub color: Color32,
    pub width: f32,
    pub dashed: bool,
}

impl GraphStyle {
    /// Every token names a theme color, outlines have colors, and every
    /// width is positive (a zero would hide lines).
    pub fn check(&self, theme: &Theme) -> Result<(), String> {
        let mut sizes: Vec<(String, f32)> = Vec::new();
        for (name, look) in self.lines.all() {
            sizes.push((format!("lines.{name}.width"), look.width));
        }
        for (name, look) in self.nodes.outline.all() {
            sizes.push((format!("nodes.outline.{name}.width"), look.width));
        }
        sizes.push(("back.repeat.width".into(), self.back.repeat.width));
        sizes.push(("back.loop.width".into(), self.back.loops.width));
        sizes.push(("nodes.selected_width".into(), self.nodes.selected_width));
        sizes.push(("evidence.named.width".into(), self.evidence.named.width));
        if let Some(fade) = self.evidence.named.fade
            && !(fade > 0.0 && fade <= 1.0)
        {
            return Err("graph_style.evidence.named.fade: must be in (0, 1]".into());
        }
        if let Some((at, value)) = sizes.into_iter().find(|(_, v)| !(*v > 0.0 && v.is_finite())) {
            return Err(format!("graph_style.{at}: must be a positive number, got {value}"));
        }
        if !(self.nodes.outside_plan_alpha > 0.0 && self.nodes.outside_plan_alpha <= 1.0) {
            return Err("graph_style.nodes.outside_plan_alpha: must be in (0, 1]".into());
        }
        let k = &self.kinds;
        let mut tokens: Vec<(String, &str)> = vec![("kinds.contains".into(), &k.contains), ("kinds.implements".into(), &k.implements), ("kinds.calls".into(), &k.calls), ("kinds.data_flow".into(), &k.data_flow)];
        tokens.extend(self.lines.all().into_iter().filter_map(|(n, l)| l.color.as_deref().map(|t| (format!("lines.{n}"), t))));
        tokens.extend([&self.back.repeat, &self.back.loops].into_iter().filter_map(|l| l.color.as_deref().map(|t| ("back".to_owned(), t))));
        tokens.extend(self.evidence.named.color.as_deref().map(|t| ("evidence.named".to_owned(), t)));
        for (name, look) in self.nodes.outline.all() {
            match look.color.as_deref() {
                Some(t) => tokens.push((format!("nodes.outline.{name}"), t)),
                None => return Err(format!("graph_style.nodes.outline.{name}: needs a color")),
            }
        }
        tokens.extend([
            ("nodes.selected_outline".to_owned(), self.nodes.selected_outline.as_str()),
            ("nodes.fill".into(), &self.nodes.fill),
            ("nodes.selected_fill".into(), &self.nodes.selected_fill),
            ("nodes.agent".into(), &self.nodes.agent),
            ("pills.in".into(), &self.pills.takes),
            ("pills.out".into(), &self.pills.gives),
            ("pills.step".into(), &self.pills.step),
        ]);
        match tokens.into_iter().find(|(_, t)| theme.token(t).is_none()) {
            Some((at, t)) => Err(format!("graph_style.{at}: '{t}' is not a theme color")),
            None => Ok(()),
        }
    }

    fn color(theme: &Theme, token: &str) -> Color32 {
        theme.token(token).unwrap_or(theme.text)
    }

    pub fn kind_color(&self, theme: &Theme, kind: RelationKind) -> Color32 {
        let token = match kind {
            RelationKind::Contains => &self.kinds.contains,
            RelationKind::Implements => &self.kinds.implements,
            RelationKind::Calls => &self.kinds.calls,
            RelationKind::DataFlow => &self.kinds.data_flow,
        };
        Self::color(theme, token)
    }

    /// A line: tree lines by their plan mark (a call in the code only
    /// matched by name has its own look), lines back by repeat or loop.
    pub fn line(&self, theme: &Theme, kind: RelationKind, line: LineKind, mark: Mark, evidence: Option<&Evidence>) -> Line {
        let look = match line {
            LineKind::Tree if mark == Mark::Code && evidence == Some(&Evidence::Named) => &self.evidence.named,
            LineKind::Tree => self.lines.get(mark),
            LineKind::Repeat => &self.back.repeat,
            LineKind::Loop => &self.back.loops,
        };
        let color = look.color.as_deref().map_or_else(|| self.kind_color(theme, kind), |t| Self::color(theme, t));
        let color = look.fade.map_or(color, |f| color.gamma_multiply(f));
        Line { color, width: look.width, dashed: look.dashed }
    }

    pub fn node_outline(&self, theme: &Theme, mark: Mark, selected: bool) -> Line {
        let look = self.nodes.outline.get(mark);
        if selected {
            return Line { color: Self::color(theme, &self.nodes.selected_outline), width: self.nodes.selected_width, dashed: look.dashed };
        }
        Line { color: look.color.as_deref().map_or(theme.text, |t| Self::color(theme, t)), width: look.width, dashed: look.dashed }
    }

    pub fn node_fill(&self, theme: &Theme, selected: bool) -> Color32 {
        Self::color(theme, if selected { &self.nodes.selected_fill } else { &self.nodes.fill })
    }

    pub fn agent_color(&self, theme: &Theme) -> Color32 {
        Self::color(theme, &self.nodes.agent)
    }

    pub fn pill_color(&self, theme: &Theme, role: PillRole) -> Color32 {
        Self::color(
            theme,
            match role {
                PillRole::In => &self.pills.takes,
                PillRole::Out => &self.pills.gives,
                PillRole::Step => &self.pills.step,
            },
        )
    }

    /// What every look means, generated from this table (the legend draws
    /// exactly these).
    pub fn legend(&self, theme: &Theme) -> Vec<LegendEntry> {
        let mut out = Vec::new();
        for kind in [RelationKind::Calls, RelationKind::DataFlow] {
            out.push(LegendEntry { section: "Lines", label: kind.name().replace('_', " "), swatch: Swatch::Line(self.line(theme, kind, LineKind::Tree, Mark::Code, None)) });
        }
        // How each kind of link is known.
        for (kind, what) in [(RelationKind::Calls, "a call"), (RelationKind::DataFlow, "a data flow"), (RelationKind::Implements, "an implementation (listed in the details card)")] {
            out.push(LegendEntry {
                section: "How it's known",
                label: format!("{what} only matched by name (the language's analyzer didn't confirm it)"),
                swatch: Swatch::Line(self.line(theme, kind, LineKind::Tree, Mark::Code, Some(&Evidence::Named))),
            });
        }
        out.push(LegendEntry {
            section: "How it's known",
            label: "confirmed by the language's analyzer: the plain line".into(),
            swatch: Swatch::Line(self.line(theme, RelationKind::Calls, LineKind::Tree, Mark::Code, Some(&Evidence::Proven(String::new())))),
        });
        out.push(LegendEntry {
            section: "How it's known",
            label: "a box inside a box: read from the code's structure (exact)".into(),
            swatch: Swatch::Outline(self.node_outline(theme, Mark::Code, false)),
        });
        out.push(LegendEntry { section: "Lines", label: "to a card shown elsewhere".into(), swatch: Swatch::Line(self.line(theme, RelationKind::Calls, LineKind::Repeat, Mark::Code, None)) });
        out.push(LegendEntry { section: "Lines", label: "back up its own path (a loop)".into(), swatch: Swatch::Line(self.line(theme, RelationKind::Calls, LineKind::Loop, Mark::Code, None)) });
        for mark in MARKS {
            out.push(LegendEntry { section: "Plan", label: mark_label(mark), swatch: Swatch::Line(self.line(theme, RelationKind::Calls, LineKind::Tree, mark, None)) });
        }
        for mark in MARKS {
            out.push(LegendEntry { section: "Cards", label: mark_label(mark), swatch: Swatch::Outline(self.node_outline(theme, mark, false)) });
        }
        out.push(LegendEntry { section: "Cards", label: "selected".to_owned(), swatch: Swatch::Outline(self.node_outline(theme, Mark::Code, true)) });
        out.push(LegendEntry { section: "Cards", label: "the agent pointed here".to_owned(), swatch: Swatch::Outline(Line { color: self.agent_color(theme), width: self.nodes.selected_width, dashed: false }) });
        for (role, label) in [(PillRole::In, "goes in"), (PillRole::Out, "comes back"), (PillRole::Step, "the step that turns one into the next")] {
            out.push(LegendEntry { section: "On the selection's lines", label: label.into(), swatch: Swatch::Pill(self.pill_color(theme, role)) });
        }
        out
    }
}

pub const MARKS: [Mark; 5] = [Mark::Code, Mark::New, Mark::Changed, Mark::Done, Mark::Mismatch];

/// A plan mark in words.
pub fn mark_label(mark: Mark) -> String {
    match mark {
        Mark::Code => "code (the plan doesn't touch it)",
        Mark::New => "new: planned, not in the code yet",
        Mark::Changed => "changed: the plan changes it",
        Mark::Done => "done: the code matches the plan",
        Mark::Mismatch => "doesn't match: the code differs from the plan",
    }
    .to_owned()
}

/// How a legend entry is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Swatch {
    Line(Line),
    Outline(Line),
    Pill(Color32),
}

#[derive(Clone, Debug, PartialEq)]
pub struct LegendEntry {
    pub section: &'static str,
    pub label: String,
    pub swatch: Swatch,
}

thread_local! {
    static CURRENT: RefCell<Rc<GraphStyle>> = RefCell::new(Rc::new(GraphStyle::default()));
}

/// The active graph style (the UI is single-threaded).
pub fn style() -> Rc<GraphStyle> {
    CURRENT.with(|c| c.borrow().clone())
}

pub fn set(style: GraphStyle) {
    CURRENT.with(|c| *c.borrow_mut() = Rc::new(style));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::UiConfig;

    #[test]
    fn the_legend_says_how_every_kind_of_link_is_known() {
        let config = UiConfig::with_overrides("").unwrap();
        let labels: Vec<String> = config.graph_style.legend(&config.theme).into_iter().map(|e| e.label).collect();
        for needle in ["a call only matched by name", "a data flow only matched by name", "an implementation (listed in the details card) only matched by name", "confirmed by the language's analyzer", "read from the code's structure"] {
            assert!(labels.iter().any(|l| l.contains(needle)), "{needle}: {labels:?}");
        }
    }

    #[test]
    fn a_call_only_matched_by_name_looks_different_and_the_legend_says_so() {
        let config = UiConfig::with_overrides("").unwrap();
        let (style, theme) = (&config.graph_style, &config.theme);
        let named = Evidence::Named;
        let proven = Evidence::Proven("rust-analyzer 1.90.0".into());
        let line = |e: Option<&Evidence>| style.line(theme, RelationKind::Calls, LineKind::Tree, Mark::Code, e);
        assert_eq!(line(Some(&proven)), line(None), "confirmed: the ordinary look");
        assert_ne!(line(Some(&named)), line(Some(&proven)), "matched by name: its own look");
        assert!(line(Some(&named)).width < line(Some(&proven)).width);
        // A plan's marks win over it (what a plan says matters more there).
        assert_eq!(style.line(theme, RelationKind::Calls, LineKind::Tree, Mark::Changed, Some(&named)), style.line(theme, RelationKind::Calls, LineKind::Tree, Mark::Changed, None));
        assert!(style.legend(theme).iter().any(|e| e.label.contains("only matched by name") && e.swatch == Swatch::Line(line(Some(&named)))));
        assert!(UiConfig::with_overrides("[graph_style.evidence.named]\nwidth = 1.0\nfade = 2.0").unwrap_err().contains("fade"));
    }

    #[test]
    fn every_mark_line_and_pill_has_a_look_from_theme_tokens() {
        let config = UiConfig::with_overrides("").unwrap();
        let (style, theme) = (&config.graph_style, &config.theme);
        let tokens: Vec<Color32> = Theme::TOKENS.iter().filter_map(|t| theme.token(t)).collect();
        for kind in RelationKind::ALL {
            for line in [LineKind::Tree, LineKind::Repeat, LineKind::Loop] {
                for mark in MARKS {
                    let l = style.line(theme, kind, line, mark, None);
                    assert!(l.width > 0.0 && tokens.contains(&l.color), "{kind:?} {line:?} {mark:?}");
                }
            }
        }
        for mark in MARKS {
            for selected in [false, true] {
                assert!(tokens.contains(&style.node_outline(theme, mark, selected).color));
            }
        }
        // The marks are told apart.
        let tree = |m| style.line(theme, RelationKind::Calls, LineKind::Tree, m, None);
        assert!(tree(Mark::New).dashed && !tree(Mark::Code).dashed, "planned is dashed");
        assert_eq!(tree(Mark::Mismatch).color, theme.error, "doesn't match stands out");
        assert_ne!(style.line(theme, RelationKind::Calls, LineKind::Loop, Mark::Code, None), tree(Mark::Code));
        assert_ne!(style.kind_color(theme, RelationKind::Calls), style.kind_color(theme, RelationKind::DataFlow));
        assert_eq!(style.legend(theme).iter().filter(|e| e.section == "Cards").count(), 7);
        // Restyling is config: tokens are checked when the config loads.
        let custom = UiConfig::with_overrides("[graph_style.kinds]\ncalls = \"warn\"").unwrap();
        assert_eq!(custom.graph_style.kind_color(&custom.theme, RelationKind::Calls), custom.theme.warn);
        assert!(UiConfig::with_overrides("[graph_style.kinds]\ncalls = \"chartreuse\"").unwrap_err().contains("'chartreuse' is not a theme color"));
        assert!(UiConfig::with_overrides("[graph_style.lines.code]\nwidth = 0.0").unwrap_err().contains("lines.code.width"));
    }
}

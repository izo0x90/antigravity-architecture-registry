//! How graph elements look, from the `[graph_style]` section of the UI
//! config: every color is a theme token name, so restyling is config only.
//! Relations get their color from their kind, their line from how plan and
//! code line up (alignment) and their weight from where they're known
//! (provenance); nodes get their outline from alignment.

use std::cell::RefCell;
use std::rc::Rc;

use arch_graph::{Alignment, Provenance, RelationKind};
use egui::Color32;
use serde::Deserialize;

use crate::theme::Theme;

/// A theme token per relation kind.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct KindColors {
    pub contains: String,
    pub implements: String,
    pub calls: String,
    pub data_flow: String,
}

/// A line: optional color override (a theme token), width and dashing.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Look {
    #[serde(default)]
    pub color: Option<String>,
    pub width: f32,
    #[serde(default)]
    pub dashed: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct AlignmentLooks {
    pub observed: Look,
    pub planned: Look,
    pub missing: Look,
    pub present: Look,
}

impl AlignmentLooks {
    fn get(&self, alignment: Alignment) -> &Look {
        match alignment {
            Alignment::Observed => &self.observed,
            Alignment::Planned => &self.planned,
            Alignment::Missing => &self.missing,
            Alignment::Present => &self.present,
        }
    }

    fn all(&self) -> [&Look; 4] {
        [&self.observed, &self.planned, &self.missing, &self.present]
    }
}

/// Width multipliers by where a relation is known from.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ProvenanceWeights {
    pub planned: f32,
    pub observed: f32,
    pub both: f32,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct OverlayLook {
    /// Width multiplier for overlay edges.
    pub width: f32,
    /// Opacity multiplier for overlay edges.
    pub alpha: f32,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct NodeLooks {
    /// Outline per alignment; `color` is required here.
    pub outline: AlignmentLooks,
    pub selected_outline: String,
    pub selected_width: f32,
    pub fill: String,
    pub selected_fill: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct GraphStyle {
    pub kinds: KindColors,
    pub edges: AlignmentLooks,
    pub provenance: ProvenanceWeights,
    pub overlay: OverlayLook,
    pub nodes: NodeLooks,
}

/// A resolved line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    pub color: Color32,
    pub width: f32,
    pub dashed: bool,
}

impl GraphStyle {
    /// Every token names a theme color, and node outlines have colors.
    pub fn check(&self, theme: &Theme) -> Result<(), String> {
        let k = &self.kinds;
        let mut tokens: Vec<(&str, &str)> = vec![("kinds.contains", &k.contains), ("kinds.implements", &k.implements), ("kinds.calls", &k.calls), ("kinds.data_flow", &k.data_flow)];
        tokens.extend(self.edges.all().into_iter().filter_map(|l| l.color.as_deref()).map(|t| ("edges", t)));
        for look in self.nodes.outline.all() {
            match look.color.as_deref() {
                Some(t) => tokens.push(("nodes.outline", t)),
                None => return Err("graph_style.nodes.outline: every alignment needs a color".into()),
            }
        }
        tokens.extend([("nodes.selected_outline", self.nodes.selected_outline.as_str()), ("nodes.fill", &self.nodes.fill), ("nodes.selected_fill", &self.nodes.selected_fill)]);
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

    pub fn edge(&self, theme: &Theme, kind: RelationKind, provenance: Provenance, alignment: Alignment, overlay: bool) -> Line {
        let look = self.edges.get(alignment);
        let color = look.color.as_deref().map_or_else(|| self.kind_color(theme, kind), |t| Self::color(theme, t));
        let weight = match (provenance.planned, provenance.observed) {
            (true, true) => self.provenance.both,
            (true, false) => self.provenance.planned,
            _ => self.provenance.observed,
        };
        let (width, color) = if overlay { (look.width * weight * self.overlay.width, color.gamma_multiply(self.overlay.alpha)) } else { (look.width * weight, color) };
        Line { color, width, dashed: look.dashed }
    }

    pub fn node_outline(&self, theme: &Theme, alignment: Alignment, selected: bool) -> Line {
        let look = self.nodes.outline.get(alignment);
        if selected {
            return Line { color: Self::color(theme, &self.nodes.selected_outline), width: self.nodes.selected_width, dashed: look.dashed };
        }
        Line { color: look.color.as_deref().map_or(theme.text, |t| Self::color(theme, t)), width: look.width, dashed: look.dashed }
    }

    pub fn node_fill(&self, theme: &Theme, selected: bool) -> Color32 {
        Self::color(theme, if selected { &self.nodes.selected_fill } else { &self.nodes.fill })
    }
}

/// Where a view edge is known from, by how it lines up.
pub fn provenance_of(alignment: Alignment) -> Provenance {
    match alignment {
        Alignment::Observed => Provenance::OBSERVED,
        Alignment::Planned | Alignment::Missing => Provenance::PLANNED,
        Alignment::Present => Provenance { planned: true, observed: true },
    }
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

    // req: P7.4
    #[test]
    fn every_kind_provenance_and_alignment_has_a_style_from_theme_tokens() {
        let config = UiConfig::with_overrides("").unwrap();
        let (style, theme) = (&config.graph_style, &config.theme);
        let tokens: Vec<Color32> = Theme::TOKENS.iter().filter_map(|t| theme.token(t)).collect();
        let alignments = [Alignment::Observed, Alignment::Planned, Alignment::Missing, Alignment::Present];
        let provenances = [Provenance::OBSERVED, Provenance::PLANNED, Provenance { planned: true, observed: true }, Provenance::default()];
        let mut seen = std::collections::BTreeSet::new();
        for kind in RelationKind::ALL {
            for provenance in provenances {
                for alignment in alignments {
                    for overlay in [false, true] {
                        let line = style.edge(theme, kind, provenance, alignment, overlay);
                        assert!(line.width > 0.0, "{kind:?} {provenance:?} {alignment:?}");
                        let base = style.edge(theme, kind, provenance, alignment, false).color;
                        assert!(tokens.contains(&base), "{kind:?} {alignment:?}: color comes from a theme token");
                        seen.insert((format!("{:?}", line.color), (line.width * 100.0) as u32, line.dashed));
                    }
                }
            }
        }
        // The dimensions are actually distinguishable.
        let line = |k, p, a| style.edge(theme, k, p, a, false);
        assert_ne!(line(RelationKind::Calls, Provenance::OBSERVED, Alignment::Observed).color, line(RelationKind::Implements, Provenance::OBSERVED, Alignment::Observed).color, "kinds differ");
        assert!(line(RelationKind::Calls, Provenance::PLANNED, Alignment::Planned).dashed && !line(RelationKind::Calls, Provenance::OBSERVED, Alignment::Observed).dashed, "planned is dashed");
        assert_eq!(line(RelationKind::Calls, Provenance::PLANNED, Alignment::Missing).color, theme.error, "missing stands out");
        let both = Provenance { planned: true, observed: true };
        assert!(line(RelationKind::Calls, both, Alignment::Present).width > line(RelationKind::Calls, Provenance::OBSERVED, Alignment::Observed).width, "agreed relations are heavier");
        assert!(style.edge(theme, RelationKind::Calls, Provenance::OBSERVED, Alignment::Observed, true).width < line(RelationKind::Calls, Provenance::OBSERVED, Alignment::Observed).width, "overlays are lighter");
        for alignment in alignments {
            for selected in [false, true] {
                assert!(tokens.contains(&style.node_outline(theme, alignment, selected).color));
                assert!(tokens.contains(&style.node_fill(theme, selected)));
            }
        }
        assert!(seen.len() > 8);
        // Restyling is config: tokens are checked when the config loads.
        let custom = UiConfig::with_overrides("[graph_style.kinds]\ncalls = \"warn\"").unwrap();
        assert_eq!(custom.graph_style.kind_color(&custom.theme, RelationKind::Calls), custom.theme.warn);
        assert!(UiConfig::with_overrides("[graph_style.kinds]\ncalls = \"chartreuse\"").unwrap_err().contains("'chartreuse' is not a theme color"));
    }
}

//! How the graph behaves (as opposed to how it looks, `graph_style`): touch
//! gesture thresholds, on-screen chips, long-press actions and how much a
//! node card shows at each zoom. From the `[graph]` section of the UI config.

use serde::Deserialize;

use crate::touch::TouchConfig;

/// When the on-screen chips show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChipMode {
    /// On narrow screens, or once a finger has touched the graph.
    #[default]
    Auto,
    Always,
    Never,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct Chips {
    pub show: ChipMode,
    /// Canvas width (points) below which the screen counts as narrow.
    pub narrow_below: f32,
    /// Smallest side of a chip (a comfortable touch target).
    pub size: f32,
    /// Action ids, in order.
    pub actions: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct GraphUi {
    pub touch: TouchConfig,
    pub chips: Chips,
    /// Actions offered on a long press, in order (those that apply).
    pub context: Vec<String>,
    /// Fitting never zooms out past this (text stays readable).
    pub readable_zoom: f32,
    /// Text smaller than this on screen isn't drawn.
    pub min_text_px: f32,
    /// Details kept for cards selected before.
    pub details_kept: usize,
    /// Results listed by "Find in the code", per kind.
    pub find: FindLimits,
    pub legend: LegendSize,
}

/// How wide the legend may be (its text wraps to fit).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct LegendSize {
    pub min_w: f32,
    pub max_w: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct FindLimits {
    pub symbols: usize,
    pub files: usize,
    pub text: usize,
}

impl Default for GraphUi {
    /// The built-in settings.
    fn default() -> Self {
        #[derive(Deserialize)]
        struct File {
            graph: GraphUi,
        }
        toml::from_str::<File>(crate::config::DEFAULTS).map(|f| f.graph).unwrap_or(GraphUi {
            touch: TouchConfig::default(),
            chips: Chips::default(),
            context: Vec::new(),
            readable_zoom: 1.0,
            min_text_px: 6.0,
            details_kept: 64,
            find: FindLimits::default(),
            legend: LegendSize::default(),
        })
    }
}

impl GraphUi {
    pub fn check(&self) -> Result<(), String> {
        let t = &self.touch;
        if t.tap_slop <= 0.0 || t.double_tap_ms <= 0.0 || t.long_press_ms <= 0.0 || t.swipe_min <= t.tap_slop || t.swipe_max_ms <= 0.0 {
            return Err("graph.touch: thresholds must be positive and a swipe longer than the tap slop".into());
        }
        if self.chips.size <= 0.0 {
            return Err("graph.chips.size must be positive".into());
        }
        if self.readable_zoom.is_nan() || self.readable_zoom <= 0.0 || self.min_text_px.is_nan() || self.min_text_px < 0.0 || self.details_kept == 0 {
            return Err("graph: readable_zoom > 0, min_text_px >= 0 and details_kept >= 1".into());
        }
        Ok(())
    }
}

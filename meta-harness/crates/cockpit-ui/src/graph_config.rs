//! How the graph behaves (as opposed to how it looks, `graph_style`): touch
//! gesture thresholds, long-press actions and how much a
//! node card shows at each zoom. From the `[graph]` section of the UI config.

use serde::Deserialize;

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct GraphUi {
    /// Actions under the selected card, in order (those that apply).
    pub context: Vec<String>,
    /// Fitting never zooms out past this (text stays readable).
    pub readable_zoom: f32,
    /// How much one zoom in (or out) changes the size.
    pub zoom_step: f32,
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
            context: Vec::new(),
            readable_zoom: 1.0,
            zoom_step: 1.2,
            min_text_px: 6.0,
            details_kept: 64,
            find: FindLimits::default(),
            legend: LegendSize::default(),
        })
    }
}

impl GraphUi {
    pub fn check(&self) -> Result<(), String> {
        if self.readable_zoom.is_nan()
            || self.readable_zoom <= 0.0
            || self.zoom_step.is_nan()
            || self.zoom_step <= 1.0
            || self.min_text_px.is_nan()
            || self.min_text_px < 0.0
            || self.details_kept == 0
        {
            return Err("graph: readable_zoom > 0, zoom_step > 1, min_text_px >= 0 and details_kept >= 1".into());
        }
        Ok(())
    }
}

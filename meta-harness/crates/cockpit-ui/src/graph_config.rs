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

/// Zoom levels at which node cards show more.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct DetailLevels {
    /// From here: type, stage and location under the name.
    pub mid: f32,
    /// From here: signature, alignment, placeholders and review points too.
    pub near: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct GraphUi {
    pub touch: TouchConfig,
    pub chips: Chips,
    /// Actions offered on a long press, in order (those that apply).
    pub context: Vec<String>,
    pub detail: DetailLevels,
}

impl GraphUi {
    pub fn check(&self) -> Result<(), String> {
        let t = &self.touch;
        if t.tap_slop <= 0.0 || t.double_tap_ms <= 0.0 || t.long_press_ms <= 0.0 || t.swipe_min <= t.tap_slop || t.swipe_max_ms <= 0.0 {
            return Err("graph.touch: thresholds must be positive and a swipe longer than the tap slop".into());
        }
        if !(self.detail.mid > 0.0 && self.detail.near > self.detail.mid) {
            return Err("graph.detail: 0 < mid < near".into());
        }
        if self.chips.size <= 0.0 {
            return Err("graph.chips.size must be positive".into());
        }
        Ok(())
    }
}

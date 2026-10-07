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

/// Moving between levels by zooming, and how many parts a level shows.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct Levels {
    /// Zooming in until a shut module fills this share of the view goes into it.
    pub enter_fill: f32,
    /// Zooming out until the level fills less than this share goes up a level.
    pub leave_fill: f32,
    /// More parts than this: the selection's neighbourhood, the rest in "+N more".
    pub max_cards: usize,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct GraphUi {
    pub touch: TouchConfig,
    pub chips: Chips,
    /// Actions offered on a long press, in order (those that apply).
    pub context: Vec<String>,
    pub detail: DetailLevels,
    /// How cards are sized and arranged, and lines routed.
    pub arrange: arch_graph::view::scene::Metrics,
    pub levels: Levels,
}

impl Default for GraphUi {
    /// The built-in settings.
    fn default() -> Self {
        #[derive(Deserialize)]
        struct File {
            graph: GraphUi,
        }
        toml::from_str::<File>(crate::config::DEFAULTS).map(|f| f.graph).unwrap_or_else(|_| GraphUi {
            touch: TouchConfig::default(),
            chips: Chips::default(),
            context: Vec::new(),
            detail: DetailLevels::default(),
            arrange: arch_graph::view::scene::Metrics::default(),
            levels: Levels::default(),
        })
    }
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
        let a = &self.arrange;
        if [a.grid, a.gap_x, a.gap_y, a.pad, a.header, a.lane, a.card_min_w, a.char_width, a.line_height].iter().any(|v| *v <= 0.0) || a.card_max_w < a.card_min_w || a.column == 0 {
            return Err("graph.arrange: sizes must be positive, card_min_w <= card_max_w and column >= 1".into());
        }
        let l = &self.levels;
        if !(l.leave_fill > 0.0 && l.leave_fill < l.enter_fill && l.enter_fill <= 1.0) || l.max_cards == 0 {
            return Err("graph.levels: 0 < leave_fill < enter_fill <= 1 and max_cards >= 1".into());
        }
        Ok(())
    }
}

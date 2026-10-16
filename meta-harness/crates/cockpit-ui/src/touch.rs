//! Touch gestures: raw finger events in, gestures out. Pure state (no egui
//! UI), so every gesture is testable on its own. The host runs one for the
//! whole cockpit: a swipe runs what the keymap binds to it in the view it
//! started on, and a long press lists what can be done with the item under
//! it (the graph's selects it). The rest of what fingers do is the
//! pointer's, as egui makes it of them, the same as a mouse in every view:
//! a tap clicks, a double tap double-clicks, a finger drags; two fingers
//! pan and pinch. Thresholds come from `[touch]`.

use std::collections::BTreeMap;

use egui::Pos2;
use serde::Deserialize;

/// Gesture thresholds (screen points and milliseconds).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct TouchConfig {
    /// How far a finger may wander and still long-press.
    pub tap_slop: f32,
    /// How long a finger rests for a long press.
    pub long_press_ms: f64,
    /// Shortest horizontal travel of a swipe.
    pub swipe_min: f32,
    /// Longest a swipe may take.
    pub swipe_max_ms: f64,
}

impl TouchConfig {
    pub fn check(&self) -> Result<(), String> {
        if self.tap_slop <= 0.0 || self.long_press_ms <= 0.0 || self.swipe_min <= self.tap_slop || self.swipe_max_ms <= 0.0 {
            return Err("touch: thresholds must be positive and a swipe longer than the tap slop".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Start,
    Move,
    End,
    Cancel,
}

impl From<egui::TouchPhase> for Phase {
    fn from(phase: egui::TouchPhase) -> Self {
        match phase {
            egui::TouchPhase::Start => Self::Start,
            egui::TouchPhase::Move => Self::Move,
            egui::TouchPhase::End => Self::End,
            egui::TouchPhase::Cancel => Self::Cancel,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Swipe {
    /// Finger moved right (history back).
    Right,
    /// Finger moved left (history forward).
    Left,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Gesture {
    LongPress(Pos2),
    /// One finger flicked sideways, starting at `from`.
    Swipe { way: Swipe, from: Pos2 },
}

impl Swipe {
    /// Its name in the keymap (`gestures = ["swipe_left"]`).
    pub fn name(self) -> &'static str {
        match self {
            Self::Left => "swipe_left",
            Self::Right => "swipe_right",
        }
    }
}

/// Every gesture the keymap can bind.
pub const BINDABLE: [&str; 2] = ["swipe_left", "swipe_right"];

#[derive(Clone, Copy, Debug)]
struct Track {
    start: Pos2,
    start_time: f64,
    pos: Pos2,
    /// Moved beyond the slop at some point.
    wandered: bool,
}

/// Turns finger events into gestures.
#[derive(Clone, Debug, Default)]
pub struct Gestures {
    tracks: BTreeMap<u64, Track>,
    /// More than one finger took part since the last time all were lifted.
    multi: bool,
    long_fired: bool,
}

impl Gestures {
    /// Whether any finger is down.
    pub fn active(&self) -> bool {
        !self.tracks.is_empty()
    }

    /// When a resting finger becomes a long press (seconds), if one is pending.
    pub fn long_press_due(&self, cfg: &TouchConfig) -> Option<f64> {
        if self.multi || self.long_fired || self.tracks.len() != 1 {
            return None;
        }
        let track = self.tracks.values().next()?;
        (!track.wandered).then_some(track.start_time + cfg.long_press_ms / 1000.0)
    }

    /// Feeds one finger event at `time` (seconds).
    pub fn feed(&mut self, id: u64, phase: Phase, pos: Pos2, time: f64, cfg: &TouchConfig) -> Vec<Gesture> {
        let mut out = Vec::new();
        match phase {
            Phase::Start => {
                self.tracks.insert(id, Track { start: pos, start_time: time, pos, wandered: false });
                if self.tracks.len() > 1 {
                    self.multi = true;
                }
                if self.tracks.len() == 1 {
                    self.long_fired = false;
                }
            }
            Phase::Move => {
                let Some(track) = self.tracks.get_mut(&id) else { return out };
                track.pos = pos;
                if track.pos.distance(track.start) > cfg.tap_slop {
                    track.wandered = true;
                }
            }
            Phase::End | Phase::Cancel => {
                let Some(track) = self.tracks.remove(&id) else { return out };
                let single = !self.multi && self.tracks.is_empty();
                if phase == Phase::End && single {
                    out.extend(self.lifted(track, pos, time, cfg));
                }
                if self.tracks.is_empty() {
                    self.multi = false;
                }
            }
        }
        out
    }

    /// Time passing with fingers down (long presses fire here).
    pub fn tick(&mut self, time: f64, cfg: &TouchConfig) -> Vec<Gesture> {
        match self.long_press_due(cfg) {
            Some(due) if time >= due => {
                self.long_fired = true;
                self.tracks.values().next().map(|t| vec![Gesture::LongPress(t.pos)]).unwrap_or_default()
            }
            _ => Vec::new(),
        }
    }

    /// A lone finger lifted: a tap, a double tap, a swipe, or nothing.
    /// A finger lifted on its own: a quick sideways flick is a swipe.
    fn lifted(&mut self, track: Track, pos: Pos2, time: f64, cfg: &TouchConfig) -> Vec<Gesture> {
        let held_ms = (time - track.start_time) * 1000.0;
        let travel = pos - track.start;
        if held_ms <= cfg.swipe_max_ms && travel.x.abs() >= cfg.swipe_min && travel.x.abs() > travel.y.abs() * 2.0 {
            return vec![Gesture::Swipe { way: if travel.x > 0.0 { Swipe::Right } else { Swipe::Left }, from: track.start }];
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> TouchConfig {
        crate::config::UiConfig::with_overrides("").unwrap().touch
    }

    fn p(x: f32, y: f32) -> Pos2 {
        Pos2::new(x, y)
    }

    #[test]
    fn a_resting_finger_long_presses_once_and_a_tap_is_the_pointers() {
        let (cfg, mut g) = (cfg(), Gestures::default());
        assert!(g.feed(1, Phase::Start, p(10.0, 10.0), 0.0, &cfg).is_empty());
        assert!(g.feed(1, Phase::End, p(12.0, 11.0), 0.05, &cfg).is_empty(), "a tap: the pointer's click");
        // Resting fires a long press once; lifting after it is nothing.
        g.feed(5, Phase::Start, p(50.0, 50.0), 3.0, &cfg);
        assert!(g.tick(3.1, &cfg).is_empty());
        assert_eq!(g.tick(3.0 + cfg.long_press_ms / 1000.0, &cfg), vec![Gesture::LongPress(p(50.0, 50.0))]);
        assert!(g.tick(4.5, &cfg).is_empty(), "once");
        assert!(g.feed(5, Phase::End, p(50.0, 50.0), 4.6, &cfg).is_empty());
        // Wandering cancels the press.
        g.feed(6, Phase::Start, p(50.0, 50.0), 5.0, &cfg);
        g.feed(6, Phase::Move, p(50.0 + cfg.tap_slop * 3.0, 50.0), 5.1, &cfg);
        assert!(g.tick(7.0, &cfg).is_empty());
        assert!(g.feed(6, Phase::End, p(50.0 + cfg.tap_slop * 3.0, 50.0), 7.1, &cfg).is_empty());
        // So does a second finger.
        g.feed(7, Phase::Start, p(50.0, 50.0), 8.0, &cfg);
        g.feed(8, Phase::Start, p(90.0, 50.0), 8.05, &cfg);
        assert!(g.tick(9.0, &cfg).is_empty());
    }

    #[test]
    fn quick_sideways_flicks_swipe_and_two_fingers_never_do() {
        let (cfg, mut g) = (cfg(), Gestures::default());
        g.feed(1, Phase::Start, p(100.0, 100.0), 0.0, &cfg);
        g.feed(2, Phase::Start, p(200.0, 100.0), 0.0, &cfg);
        g.feed(1, Phase::Move, p(100.0 + cfg.swipe_min + 10.0, 100.0), 0.1, &cfg);
        assert!(g.feed(1, Phase::End, p(100.0 + cfg.swipe_min + 10.0, 100.0), 0.2, &cfg).is_empty(), "two fingers: no swipe");
        assert!(g.feed(2, Phase::End, p(200.0, 140.0), 0.2, &cfg).is_empty());
        // A quick horizontal flick swipes; a slow drag doesn't.
        g.feed(3, Phase::Start, p(100.0, 300.0), 1.0, &cfg);
        g.feed(3, Phase::Move, p(100.0 + cfg.swipe_min, 305.0), 1.1, &cfg);
        assert_eq!(g.feed(3, Phase::End, p(100.0 + cfg.swipe_min + 10.0, 305.0), 1.15, &cfg), vec![Gesture::Swipe { way: Swipe::Right, from: p(100.0, 300.0) }]);
        g.feed(4, Phase::Start, p(300.0, 300.0), 2.0, &cfg);
        assert_eq!(g.feed(4, Phase::End, p(300.0 - cfg.swipe_min - 5.0, 300.0), 2.1, &cfg), vec![Gesture::Swipe { way: Swipe::Left, from: p(300.0, 300.0) }]);
        g.feed(5, Phase::Start, p(300.0, 300.0), 3.0, &cfg);
        assert!(g.feed(5, Phase::End, p(300.0 - cfg.swipe_min - 5.0, 300.0), 3.0 + cfg.swipe_max_ms / 1000.0 + 0.5, &cfg).is_empty(), "too slow");
        assert!(!g.active());
    }
}

//! Input modes and the keymap. Bindings come from config (see
//! `config/default.toml`); actions never carry keys themselves.

#[cfg(test)]
mod lint;
mod local;

pub use local::{InputKeys, ListKeys, LocalKeys, Modifier, PointerKeys, Sent, local, set_local};

use std::collections::BTreeMap;

use egui::Key;
use serde::Deserialize;

use crate::actions::{Action, Bound, Origin};

/// High-level application input mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputMode {
    /// Modal navigation: direct keys run actions.
    #[default]
    Normal,
    /// Leader menu open, awaiting menu keys (path kept by the app).
    Leader,
    /// The command palette owns the keyboard.
    Palette,
    /// Keystrokes flow into the text field of this slot.
    Insert { slot: usize },
    /// The view in this slot reads typed keys itself (e.g. hint labels).
    Capture { slot: usize },
}

/// Tells a leader tap from a leader hold (holding Space pans the graph).
#[derive(Default, Debug, Clone, Copy)]
pub struct TapTracker {
    down: bool,
    dragged: bool,
}

impl TapTracker {
    pub fn press(&mut self) {
        self.down = true;
        self.dragged = false;
    }

    pub fn drag(&mut self) {
        if self.down {
            self.dragged = true;
        }
    }

    /// True when the key went up without a drag in between.
    pub fn release(&mut self) -> bool {
        let tapped = self.down && !self.dragged;
        self.down = false;
        self.dragged = false;
        tapped
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ChordKey {
    /// Matched against typed text, so it follows the keyboard layout.
    Text(String),
    Key(Key),
}

/// One key, possibly with modifiers: "y", "Y", ":", "Tab", "shift+Tab", "ctrl+shift+c".
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct Chord {
    key: ChordKey,
    ctrl: bool,
    alt: bool,
    shift: bool,
}

fn named_key(name: &str) -> Option<Key> {
    Key::from_name(name).or_else(|| {
        let mut chars = name.chars();
        let first = chars.next()?;
        Key::from_name(&format!("{}{}", first.to_uppercase(), chars.as_str()))
    })
}

impl TryFrom<String> for Chord {
    type Error = String;

    fn try_from(spec: String) -> Result<Self, String> {
        let (mods, key) = match spec.strip_suffix("++") {
            Some(mods) => (mods, "+"),
            None => match spec.rsplit_once('+') {
                Some((mods, key)) if !key.is_empty() => (mods, key),
                _ => ("", spec.as_str()),
            },
        };
        let (mut ctrl, mut alt, mut shift) = (false, false, false);
        for m in mods.split('+').filter(|m| !m.is_empty()) {
            match m.to_lowercase().as_str() {
                "ctrl" | "control" | "cmd" | "command" | "super" | "meta" => ctrl = true,
                "alt" | "option" | "opt" => alt = true,
                "shift" => shift = true,
                other => return Err(format!("unknown modifier `{other}` in `{spec}`")),
            }
        }
        let single_char = key.chars().count() == 1;
        let key = if single_char && !ctrl && !alt && !shift {
            ChordKey::Text(key.to_owned())
        } else {
            ChordKey::Key(named_key(key).ok_or_else(|| format!("unknown key `{key}` in `{spec}`"))?)
        };
        Ok(Self { key, ctrl, alt, shift })
    }
}

impl Chord {
    pub fn pressed(&self, input: &egui::InputState) -> bool {
        match &self.key {
            ChordKey::Text(text) => input.events.iter().any(|e| matches!(e, egui::Event::Text(t) if t == text)),
            ChordKey::Key(key) => {
                let m = input.modifiers;
                input.key_pressed(*key) && m.alt == self.alt && m.shift == self.shift && (m.ctrl || m.command) == self.ctrl
            }
        }
    }

    /// The physical key, when there is one: a named key, or a typed
    /// letter or digit's own key (so it can be held and let go).
    fn physical(&self) -> Option<Key> {
        match &self.key {
            ChordKey::Key(key) => Some(*key),
            ChordKey::Text(text) => named_key(&text.to_uppercase()),
        }
    }

    /// Takes the key if it was pressed (so a text box under it doesn't act
    /// on it too): whether it was.
    pub fn consume(&self, input: &mut egui::InputState) -> bool {
        let pressed = self.pressed(input);
        if pressed && let ChordKey::Key(key) = &self.key {
            input.events.retain(|e| !matches!(e, egui::Event::Key { key: k, pressed: true, .. } if k == key));
        }
        pressed
    }

    pub fn released(&self, input: &egui::InputState) -> bool {
        self.physical().is_some_and(|key| input.key_released(key))
    }

    pub fn key_down(&self, input: &egui::InputState) -> bool {
        self.physical().is_some_and(|key| input.key_down(key))
    }

    pub fn label(&self) -> String {
        let key = match &self.key {
            ChordKey::Text(t) => t.clone(),
            ChordKey::Key(k) => k.name().to_owned(),
        };
        let mods = [(self.ctrl, "ctrl+"), (self.alt, "alt+"), (self.shift, "shift+")];
        mods.iter().filter(|(on, _)| *on).map(|(_, m)| *m).collect::<String>() + &key
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LeaderKey {
    Char(char),
    /// The next free key in this menu.
    Auto,
}

/// Leader path, e.g. "s n" or "m #".
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct LeaderPath(Vec<LeaderKey>);

impl TryFrom<String> for LeaderPath {
    type Error = String;

    fn try_from(spec: String) -> Result<Self, String> {
        let mut keys = Vec::new();
        for token in spec.split_whitespace() {
            let mut chars = token.chars();
            match (chars.next(), chars.next()) {
                (Some('#'), None) => keys.push(LeaderKey::Auto),
                (Some(c), None) => keys.push(LeaderKey::Char(c)),
                _ => return Err(format!("leader keys are single characters separated by spaces, got `{spec}`")),
            }
        }
        if keys.iter().rev().skip(1).any(|k| *k == LeaderKey::Auto) {
            return Err(format!("`#` can only be the last leader key, got `{spec}`"));
        }
        Ok(Self(keys))
    }
}

/// One or more chords pressed in order, written space-separated ("g c").
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct KeySeq(pub Vec<Chord>);

impl TryFrom<String> for KeySeq {
    type Error = String;

    fn try_from(spec: String) -> Result<Self, String> {
        let chords: Vec<Chord> = spec.split_whitespace().map(|c| Chord::try_from(c.to_owned())).collect::<Result<_, _>>()?;
        if chords.is_empty() {
            return Err("empty key sequence".into());
        }
        Ok(Self(chords))
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Binding {
    #[serde(default)]
    pub leader: Option<LeaderPath>,
    #[serde(default)]
    pub keys: Vec<KeySeq>,
}

/// Without a config: Space leader, Escape back, nothing bound.
impl Default for KeymapConfig {
    fn default() -> Self {
        let key = |key| Chord { key: ChordKey::Key(key), ctrl: false, alt: false, shift: false };
        Self {
            leader: key(Key::Space),
            back: key(Key::Escape),
            peek_hold_ms: default_peek_hold(),
            groups: BTreeMap::new(),
            bind: BTreeMap::new(),
            hint_chars: default_hint_chars(),
            which_key_delay_ms: default_which_key_delay(),
            view_menu: default_view_menu(),
            bar: KeyBar::default(),
            list: ListKeys::default(),
            input: InputKeys::default(),
            pointer: PointerKeys::default(),
        }
    }
}

/// The key bar: the keys that matter for what the cursor is on.
#[derive(Clone, Debug, Deserialize)]
pub struct KeyBar {
    pub show: bool,
    /// How many keys it shows.
    pub max: usize,
    /// Below this width its keys are big, tappable chips.
    pub narrow_below: f32,
}

impl Default for KeyBar {
    fn default() -> Self {
        Self { show: true, max: 8, narrow_below: 600.0 }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct KeymapConfig {
    pub leader: Chord,
    pub back: Chord,
    /// Held at least this long, the quick look's key is a peek: the quick
    /// look closes when it's let go (tapped, it stays open).
    #[serde(default = "default_peek_hold")]
    pub peek_hold_ms: u64,
    /// Leader submenu labels by key path ("s" -> "Session").
    #[serde(default)]
    pub groups: BTreeMap<String, String>,
    #[serde(default)]
    pub bind: BTreeMap<String, Binding>,
    /// Characters hint labels are made of, best first.
    #[serde(default = "default_hint_chars")]
    pub hint_chars: String,
    /// How long a half-typed sequence waits before its popup shows.
    #[serde(default = "default_which_key_delay")]
    pub which_key_delay_ms: u64,
    /// The leader key of the focused view's own menu.
    #[serde(default = "default_view_menu")]
    pub view_menu: char,
    #[serde(default)]
    pub bar: KeyBar,
    /// Moving and picking in a list (the palette).
    #[serde(default)]
    pub list: ListKeys,
    /// Sending from a typing box.
    #[serde(default)]
    pub input: InputKeys,
    /// Held with the pointer.
    #[serde(default)]
    pub pointer: PointerKeys,
}

fn default_peek_hold() -> u64 {
    250
}

fn default_hint_chars() -> String {
    "asdfghjkl".to_owned()
}

fn default_which_key_delay() -> u64 {
    300
}

fn default_view_menu() -> char {
    ','
}

/// Keys handed out to `#` entries, in order.
pub const AUTO_KEYS: &str = "123456789abcdefghijklmnopqrstuvwxyz";

impl KeymapConfig {
    /// The keys outside the views' bindings, as views read them.
    pub fn local(&self) -> LocalKeys {
        LocalKeys { list: self.list.clone(), input: self.input.clone(), pointer: self.pointer.clone(), back: Some(self.back.clone()) }
    }

    pub fn group_label(&self, path: &[char]) -> Option<&str> {
        let key: Vec<String> = path.iter().map(char::to_string).collect();
        self.groups.get(&key.join(" ")).or_else(|| self.groups.get(&key.concat())).map(String::as_str)
    }

    /// The binding for an action: by id, then aliases, then the closest wildcard.
    fn binding_for(&self, action: &Action) -> Option<&Binding> {
        let names: Vec<&str> = std::iter::once(action.id.as_str()).chain(action.aliases.iter().map(String::as_str)).collect();
        if let Some(found) = names.iter().find_map(|n| self.bind.get(*n)) {
            return Some(found);
        }
        names.iter().find_map(|name| {
            let mut prefix = *name;
            while let Some((head, _)) = prefix.rsplit_once('.') {
                if let Some(found) = self.bind.get(&format!("{head}.*")) {
                    return Some(found);
                }
                prefix = head;
            }
            None
        })
    }

    /// Resolves every action's leader path and direct keys. Fixed paths win;
    /// a path already taken, and every `#`, gets the next free key in its menu.
    pub fn bind(&self, actions: Vec<(Origin, Action)>) -> Vec<Bound> {
        let specs: Vec<Option<&Binding>> = actions.iter().map(|(_, a)| self.binding_for(a)).collect();
        let mut taken: Vec<Vec<char>> = Vec::new();
        let mut leaders: Vec<Option<Vec<char>>> = vec![None; actions.len()];
        let mut pending: Vec<(usize, Vec<char>)> = Vec::new();

        for (index, spec) in specs.iter().enumerate() {
            let Some(path) = spec.and_then(|b| b.leader.as_ref()) else { continue };
            let fixed: Vec<char> = path.0.iter().filter_map(|k| if let LeaderKey::Char(c) = k { Some(*c) } else { None }).collect();
            let auto = path.0.last() == Some(&LeaderKey::Auto);
            if fixed.is_empty() && !auto {
                continue;
            }
            if auto || taken.contains(&fixed) {
                let parent = if auto { fixed } else { fixed[..fixed.len() - 1].to_vec() };
                pending.push((index, parent));
            } else {
                taken.push(fixed.clone());
                leaders[index] = Some(fixed);
            }
        }
        for (index, parent) in pending {
            let free = AUTO_KEYS.chars().find(|c| {
                let mut candidate = parent.clone();
                candidate.push(*c);
                !taken.iter().any(|t| t.starts_with(&candidate) || candidate.starts_with(t))
            });
            if let Some(c) = free {
                let mut path = parent;
                path.push(c);
                taken.push(path.clone());
                leaders[index] = Some(path);
            }
        }

        actions
            .into_iter()
            .zip(specs)
            .zip(leaders)
            .map(|(((origin, action), spec), leader)| Bound {
                keys: spec.map(|b| b.keys.iter().map(|k| k.0.clone()).collect()).unwrap_or_default(),
                action,
                origin,
                leader,
            })
            .collect()
    }
}

/// What pressing a chord means given the chords already pending.
#[derive(Clone, Debug, PartialEq)]
pub enum SeqMatch {
    /// Completes this action's sequence (index into the bound list).
    Run(usize),
    /// Starts or continues a longer sequence: keep waiting.
    Pending(Chord),
    /// Matches nothing.
    None,
}

/// What can follow the chords typed so far: each action whose sequence starts
/// with them, with the rest of its sequence.
pub fn continuations<'a>(bound: &'a [Bound], pending: &[Chord]) -> Vec<(&'a Bound, Vec<Chord>)> {
    let mut out = Vec::new();
    for b in bound {
        if let Some(seq) = b.keys.iter().find(|seq| seq.len() > pending.len() && seq[..pending.len()] == *pending) {
            out.push((b, seq[pending.len()..].to_vec()));
        }
    }
    out
}

/// Matches the next pressed chord against every bound sequence, given what's
/// pending. A complete sequence wins over waiting for a longer one only when
/// no longer sequence shares the prefix (so `g c` beats a lone `g`).
pub fn match_sequence(bound: &[Bound], pending: &[Chord], input: &egui::InputState) -> SeqMatch {
    let mut complete: Option<usize> = None;
    let mut longer: Option<Chord> = None;
    for (index, b) in bound.iter().enumerate() {
        for seq in &b.keys {
            if seq.len() <= pending.len() || seq[..pending.len()] != *pending {
                continue;
            }
            let next = &seq[pending.len()];
            if !next.pressed(input) {
                continue;
            }
            if seq.len() == pending.len() + 1 {
                complete.get_or_insert(index);
            } else {
                longer.get_or_insert_with(|| next.clone());
            }
        }
    }
    match (longer, complete) {
        (Some(chord), _) => SeqMatch::Pending(chord),
        (None, Some(index)) => SeqMatch::Run(index),
        (None, None) => SeqMatch::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::Action;

    fn keymap(toml: &str) -> KeymapConfig {
        toml::from_str(toml).unwrap()
    }

    fn act(id: &str) -> (Origin, Action) {
        (Origin::Shell, Action::new(id, id, ()))
    }

    #[test]
    fn chords_parse_text_and_named_keys() {
        let text = Chord::try_from("Y".to_owned()).unwrap();
        assert_eq!(text.key, ChordKey::Text("Y".into()));
        let combo = Chord::try_from("ctrl+shift+c".to_owned()).unwrap();
        assert_eq!((combo.key.clone(), combo.ctrl, combo.shift), (ChordKey::Key(Key::C), true, true));
        assert_eq!(Chord::try_from("shift+tab".to_owned()).unwrap().key, ChordKey::Key(Key::Tab));
        assert!(Chord::try_from("hyper+x".to_owned()).is_err());
    }

    #[test]
    fn bindings_resolve_by_id_alias_then_wildcard() {
        let km = keymap(
            r##"
            leader = "Space"
            back = "Escape"
            [bind]
            "session.new" = { leader = "s n" }
            "config.category.model" = { leader = "m m" }
            "config.*" = { leader = "m #" }
            "##,
        );
        let mut model = act("config.model_id");
        model.1.aliases.push("config.category.model".into());
        let bound = km.bind(vec![act("session.new"), model, act("config.effort"), act("config.speed"), act("unbound")]);
        let paths: Vec<Option<String>> = bound.iter().map(|b| b.leader.as_ref().map(|p| p.iter().collect())).collect();
        assert_eq!(paths, vec![Some("sn".into()), Some("mm".into()), Some("m1".into()), Some("m2".into()), None]);
    }

    #[test]
    fn a_taken_path_falls_back_to_the_next_free_key() {
        let km = keymap(
            r##"
            leader = "Space"
            back = "Escape"
            [bind]
            "a" = { leader = "r y" }
            "b" = { leader = "r y" }
            "##,
        );
        let bound = km.bind(vec![act("a"), act("b")]);
        assert_eq!(bound[1].leader, Some(vec!['r', '1']));
    }
}

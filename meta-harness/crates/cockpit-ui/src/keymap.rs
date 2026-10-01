use egui::Key;
use serde::{Deserialize, Serialize};

/// High-level leader menu prefix for Which-Key navigation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LeaderPrefix {
    Root,
    Buffer,
    Layout,
}

/// High-level application input mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputMode {
    /// Modal navigation: single keys execute navigation and panel commands.
    Normal,
    /// Leader mode: Which-Key HUD is visible, awaiting menu chords.
    Leader(LeaderPrefix),
    /// Insert mode: keystrokes flow into text fields in the specified slot.
    Insert { slot: usize },
}

impl Default for InputMode {
    fn default() -> Self {
        Self::Normal
    }
}

/// Gesture tracker for Space tap vs Space hold+drag.
#[derive(Default, Debug, Clone, Copy)]
pub struct SpaceGestureTracker {
    pub is_space_down: bool,
    pub drag_occurred: bool,
}

impl SpaceGestureTracker {
    pub fn on_space_pressed(&mut self) {
        self.is_space_down = true;
        self.drag_occurred = false;
    }

    pub fn on_drag(&mut self) {
        if self.is_space_down {
            self.drag_occurred = true;
        }
    }

    pub fn on_space_released(&mut self) -> bool {
        let was_down = self.is_space_down;
        let dragged = self.drag_occurred;
        self.is_space_down = false;
        self.drag_occurred = false;
        was_down && !dragged
    }
}

/// Decoupled global application actions.
#[derive(Clone, Debug, PartialEq)]
pub enum GlobalAction {
    FocusSlot(usize),
    CycleSlot(i32),
    SwitchBuffer { slot: usize, kind: crate::app::BufferKind },
    SetLayout(usize),
    ToggleInspector,
    ToggleIssues,
    CloseActiveDrawer,
}

/// Decoupled graph actions.
#[derive(Clone, Debug, PartialEq)]
pub enum GraphAction {
    ZoomIn,
    ZoomOut,
    ResetLayout,
    CycleDisplayMode,
    CloseInspector,
}

/// Decoupled chat actions.
#[derive(Clone, Debug, PartialEq)]
pub enum ChatAction {
    EnterInsert,
    ClearPrompt,
}

/// Outcome of processing a leader keystroke.
#[derive(Clone, Debug, PartialEq)]
pub enum LeaderResult {
    Navigate(LeaderPrefix),
    Execute(GlobalAction),
    Dismiss,
    None,
}

pub fn resolve_leader_key(
    prefix: LeaderPrefix,
    key: Key,
    focused_slot: usize,
) -> LeaderResult {
    match prefix {
        LeaderPrefix::Root => match key {
            Key::Num1 => LeaderResult::Execute(GlobalAction::FocusSlot(0)),
            Key::Num2 => LeaderResult::Execute(GlobalAction::FocusSlot(1)),
            Key::Num3 => LeaderResult::Execute(GlobalAction::FocusSlot(2)),
            Key::B => LeaderResult::Navigate(LeaderPrefix::Buffer),
            Key::L => LeaderResult::Navigate(LeaderPrefix::Layout),
            Key::I => LeaderResult::Execute(GlobalAction::ToggleInspector),
            Key::D => LeaderResult::Execute(GlobalAction::ToggleIssues),
            Key::X => LeaderResult::Execute(GlobalAction::CloseActiveDrawer),
            Key::Escape => LeaderResult::Dismiss,
            _ => LeaderResult::None,
        },
        LeaderPrefix::Buffer => match key {
            Key::C => LeaderResult::Execute(GlobalAction::SwitchBuffer {
                slot: focused_slot,
                kind: crate::app::BufferKind::Chat,
            }),
            Key::V => LeaderResult::Execute(GlobalAction::SwitchBuffer {
                slot: focused_slot,
                kind: crate::app::BufferKind::Code,
            }),
            Key::G => LeaderResult::Execute(GlobalAction::SwitchBuffer {
                slot: focused_slot,
                kind: crate::app::BufferKind::Graph,
            }),
            Key::T => LeaderResult::Execute(GlobalAction::SwitchBuffer {
                slot: focused_slot,
                kind: crate::app::BufferKind::Tree,
            }),
            Key::Escape => LeaderResult::Navigate(LeaderPrefix::Root),
            _ => LeaderResult::None,
        },
        LeaderPrefix::Layout => match key {
            Key::Num1 => LeaderResult::Execute(GlobalAction::SetLayout(1)),
            Key::Num2 => LeaderResult::Execute(GlobalAction::SetLayout(2)),
            Key::Num3 => LeaderResult::Execute(GlobalAction::SetLayout(3)),
            Key::Escape => LeaderResult::Navigate(LeaderPrefix::Root),
            _ => LeaderResult::None,
        },
    }
}

pub fn which_key_entries(prefix: LeaderPrefix) -> &'static [(&'static str, &'static str)] {
    match prefix {
        LeaderPrefix::Root => &[
            ("1..3", "Slot"),
            ("b", "Buffer..."),
            ("l", "Layout..."),
            ("i", "Inspector"),
            ("d", "Issues"),
            ("x", "Close Drawer"),
            ("Esc", "Close"),
        ],
        LeaderPrefix::Buffer => &[
            ("c", "Chat"),
            ("v", "Code"),
            ("g", "Graph"),
            ("t", "Tree"),
            ("Esc", "Back"),
        ],
        LeaderPrefix::Layout => &[
            ("1", "1 Col"),
            ("2", "2 Cols"),
            ("3", "3 Cols"),
            ("Esc", "Back"),
        ],
    }
}

/// A configurable single key or key combination.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyBinding {
    pub key: String,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub command: bool,
}

impl KeyBinding {
    pub fn single(key: &str) -> Self {
        Self {
            key: key.to_string(),
            alt: false,
            ctrl: false,
            shift: false,
            command: false,
        }
    }

    pub fn alt(key: &str) -> Self {
        Self {
            key: key.to_string(),
            alt: true,
            ctrl: false,
            shift: false,
            command: false,
        }
    }

    pub fn shift(key: &str) -> Self {
        Self {
            key: key.to_string(),
            alt: false,
            ctrl: false,
            shift: true,
            command: false,
        }
    }

    pub fn ctrl_shift(key: &str) -> Self {
        Self {
            key: key.to_string(),
            alt: false,
            ctrl: true,
            shift: true,
            command: false,
        }
    }

    pub fn to_egui_key(&self) -> Option<Key> {
        match self.key.to_lowercase().as_str() {
            "0" | "num0" => Some(Key::Num0),
            "1" | "num1" => Some(Key::Num1),
            "2" | "num2" => Some(Key::Num2),
            "3" | "num3" => Some(Key::Num3),
            "4" | "num4" => Some(Key::Num4),
            "5" | "num5" => Some(Key::Num5),
            "6" | "num6" => Some(Key::Num6),
            "7" | "num7" => Some(Key::Num7),
            "8" | "num8" => Some(Key::Num8),
            "9" | "num9" => Some(Key::Num9),
            "a" => Some(Key::A),
            "b" => Some(Key::B),
            "c" => Some(Key::C),
            "d" => Some(Key::D),
            "e" => Some(Key::E),
            "f" => Some(Key::F),
            "g" => Some(Key::G),
            "h" => Some(Key::H),
            "i" => Some(Key::I),
            "j" => Some(Key::J),
            "k" => Some(Key::K),
            "l" => Some(Key::L),
            "m" => Some(Key::M),
            "n" => Some(Key::N),
            "o" => Some(Key::O),
            "p" => Some(Key::P),
            "q" => Some(Key::Q),
            "r" => Some(Key::R),
            "s" => Some(Key::S),
            "t" => Some(Key::T),
            "u" => Some(Key::U),
            "v" => Some(Key::V),
            "w" => Some(Key::W),
            "x" => Some(Key::X),
            "y" => Some(Key::Y),
            "z" => Some(Key::Z),
            "escape" | "esc" => Some(Key::Escape),
            "tab" => Some(Key::Tab),
            "space" | " " => Some(Key::Space),
            "enter" | "return" => Some(Key::Enter),
            "=" | "+" | "equals" => Some(Key::Equals),
            "-" | "_" | "minus" => Some(Key::Minus),
            "/" | "slash" => Some(Key::Slash),
            "f1" => Some(Key::F1),
            "f2" => Some(Key::F2),
            "arrowup" | "up" => Some(Key::ArrowUp),
            "arrowdown" | "down" => Some(Key::ArrowDown),
            "arrowleft" | "left" => Some(Key::ArrowLeft),
            "arrowright" | "right" => Some(Key::ArrowRight),
            _ => None,
        }
    }

    /// Evaluates if this binding is currently triggered in egui input state.
    pub fn is_pressed(&self, input: &egui::InputState) -> bool {
        let Some(target_key) = self.to_egui_key() else {
            return false;
        };

        if !input.key_pressed(target_key) {
            return false;
        }

        // Modifiers match (support Mac command or ctrl interchangeably when ctrl requested)
        if self.alt != input.modifiers.alt {
            return false;
        }
        let effective_ctrl = input.modifiers.ctrl || (input.modifiers.command && !self.command);
        if self.ctrl != effective_ctrl {
            return false;
        }
        if self.shift != input.modifiers.shift {
            return false;
        }
        if self.command && !input.modifiers.command {
            return false;
        }

        true
    }
}

/// Global application navigation keybindings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GlobalKeymap {
    pub focus_slot_1: KeyBinding,
    pub focus_slot_2: KeyBinding,
    pub focus_slot_3: KeyBinding,
    pub next_slot: KeyBinding,
    pub prev_slot: KeyBinding,

    pub switch_chat: KeyBinding,
    pub switch_code: KeyBinding,
    pub switch_graph: KeyBinding,
    pub switch_tree: KeyBinding,

    pub enter_insert: KeyBinding,
    pub exit_insert: KeyBinding,

    pub toggle_inspector: KeyBinding,
    pub toggle_issues: KeyBinding,

    pub layout_1: KeyBinding,
    pub layout_2: KeyBinding,
    pub layout_3: KeyBinding,
}

impl Default for GlobalKeymap {
    fn default() -> Self {
        Self {
            focus_slot_1: KeyBinding::single("1"),
            focus_slot_2: KeyBinding::single("2"),
            focus_slot_3: KeyBinding::single("3"),
            next_slot: KeyBinding::single("Tab"),
            prev_slot: KeyBinding::shift("Tab"),

            switch_chat: KeyBinding::single("c"),
            switch_code: KeyBinding::single("v"),
            switch_graph: KeyBinding::single("g"),
            switch_tree: KeyBinding::single("t"),

            enter_insert: KeyBinding::single("i"),
            exit_insert: KeyBinding::single("Escape"),

            toggle_inspector: KeyBinding::ctrl_shift("c"),
            toggle_issues: KeyBinding::single("F2"),

            layout_1: KeyBinding::alt("1"),
            layout_2: KeyBinding::alt("2"),
            layout_3: KeyBinding::alt("3"),
        }
    }
}

/// Graph view keybindings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GraphKeymap {
    pub zoom_in: KeyBinding,
    pub zoom_out: KeyBinding,
    pub reset_layout: KeyBinding,
    pub cycle_mode: KeyBinding,
    pub close_drawer: KeyBinding,
    pub search: KeyBinding,
}

impl Default for GraphKeymap {
    fn default() -> Self {
        Self {
            zoom_in: KeyBinding::single("="),
            zoom_out: KeyBinding::single("-"),
            reset_layout: KeyBinding::single("r"),
            cycle_mode: KeyBinding::single("m"),
            close_drawer: KeyBinding::single("x"),
            search: KeyBinding::single("/"),
        }
    }
}

/// Chat view keybindings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatKeymap {
    pub enter_prompt: KeyBinding,
    pub clear_prompt: KeyBinding,
}

impl Default for ChatKeymap {
    fn default() -> Self {
        Self {
            enter_prompt: KeyBinding::single("i"),
            clear_prompt: KeyBinding::single("x"),
        }
    }
}

/// Code view keybindings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CodeKeymap {
    pub scroll_down: KeyBinding,
    pub scroll_up: KeyBinding,
}

impl Default for CodeKeymap {
    fn default() -> Self {
        Self {
            scroll_down: KeyBinding::single("j"),
            scroll_up: KeyBinding::single("k"),
        }
    }
}

/// Tree view keybindings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TreeKeymap {
    pub next_item: KeyBinding,
    pub prev_item: KeyBinding,
    pub toggle: KeyBinding,
}

impl Default for TreeKeymap {
    fn default() -> Self {
        Self {
            next_item: KeyBinding::single("j"),
            prev_item: KeyBinding::single("k"),
            toggle: KeyBinding::single("Space"),
        }
    }
}

/// Root keymap configuration grouping all global and view-specific keymaps.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct KeymapConfig {
    pub global: GlobalKeymap,
    pub graph: GraphKeymap,
    pub chat: ChatKeymap,
    pub code: CodeKeymap,
    pub tree: TreeKeymap,
}

use std::collections::{HashMap, HashSet};
use std::path::Path;

use bevy_ecs::prelude::*;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use winit::keyboard::KeyCode;
use glam::Vec2;

use crate::ui;

// ============================================================================
// Game actions
// ============================================================================

/// Named game actions decoupled from physical inputs. The semantic layer systems
/// consume - never raw winit events. Movement is a derived screen-relative
/// [`InputState::move_vector`]; the directional `Move*` actions feed it.
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub enum GameAction {
    MoveForward,
    MoveBackward,
    MoveLeft,
    MoveRight,
    CameraRotateLeft,
    CameraRotateRight,
    CameraZoomIn,
    CameraZoomOut,
    ToggleUI,
    ToggleGraphEditor,
    ToggleFieldProbe,
    /// Debug: pour water at the picked cell.
    PourWater,
    // --- Player-semantic actions (consumed from Substep 11/12) ---
    Interact,
    PrimaryAction,
    SecondaryAction,
    /// Cycle the active tool by a signed step (+1 next, -1 prev).
    CycleTool(i8),
    /// Cycle the targeted layer/instance under the cursor by a signed step.
    CycleTargetLayer(i8),
    ToggleCutaway,
    /// Explicit Authoring <-> Play mode toggle (spawns/despawns the player).
    TogglePlayMode,
    // --- Armed authoring tools ---
    /// Disarm whatever tool is waiting on an in-world click. Named for the
    /// mechanism, not for any one tool: every armed tool answers to it.
    CancelTool,
    /// Held while an armed tool fires: keep it armed instead of disarming, so
    /// the same action can be repeated without re-arming between clicks.
    RepeatTool,
    /// Cycle the armed tool's orientation.
    RotateTool,
}

// ============================================================================
// Physical input triggers
// ============================================================================

/// How an action responds to its trigger.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ActionMode {
    /// Active every frame the key is held (WASD movement).
    Held,
    /// Fires once on the press edge (rotation snaps, toggles).
    EdgeTriggered,
    /// Fires with a float magnitude (scroll wheel zoom).
    Continuous,
}

/// A physical input that can trigger an action.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InputTrigger {
    Key(KeyCodeSerde),
    ScrollUp,
    ScrollDown,
}

// ============================================================================
// KeyCode serde wrapper
// ============================================================================

/// Newtype around `winit::keyboard::KeyCode` providing serde support
///
/// Serializes as the variant name string (e.g. `"KeyW"`, `"ArrowUp"`, `"F1"`)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyCodeSerde(pub KeyCode);

impl From<KeyCode> for KeyCodeSerde {
    fn from(code: KeyCode) -> Self {
        Self(code)
    }
}

impl Serialize for KeyCodeSerde {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // KeyCode's Debug output is its variant name (e.g. "KeyW").
        serializer.serialize_str(&format!("{:?}", self.0))
    }
}

impl<'de> Deserialize<'de> for KeyCodeSerde {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        parse_key_code(&s)
            .map(KeyCodeSerde)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown KeyCode: {}", s)))
    }
}

/// Parse a KeyCode from its Debug variant name.
fn parse_key_code(s: &str) -> Option<KeyCode> {
    Some(match s {
        // Letters
        "KeyA" => KeyCode::KeyA,
        "KeyB" => KeyCode::KeyB,
        "KeyC" => KeyCode::KeyC,
        "KeyD" => KeyCode::KeyD,
        "KeyE" => KeyCode::KeyE,
        "KeyF" => KeyCode::KeyF,
        "KeyG" => KeyCode::KeyG,
        "KeyH" => KeyCode::KeyH,
        "KeyI" => KeyCode::KeyI,
        "KeyJ" => KeyCode::KeyJ,
        "KeyK" => KeyCode::KeyK,
        "KeyL" => KeyCode::KeyL,
        "KeyM" => KeyCode::KeyM,
        "KeyN" => KeyCode::KeyN,
        "KeyO" => KeyCode::KeyO,
        "KeyP" => KeyCode::KeyP,
        "KeyQ" => KeyCode::KeyQ,
        "KeyR" => KeyCode::KeyR,
        "KeyS" => KeyCode::KeyS,
        "KeyT" => KeyCode::KeyT,
        "KeyU" => KeyCode::KeyU,
        "KeyV" => KeyCode::KeyV,
        "KeyW" => KeyCode::KeyW,
        "KeyX" => KeyCode::KeyX,
        "KeyY" => KeyCode::KeyY,
        "KeyZ" => KeyCode::KeyZ,
        // Digits
        "Digit0" => KeyCode::Digit0,
        "Digit1" => KeyCode::Digit1,
        "Digit2" => KeyCode::Digit2,
        "Digit3" => KeyCode::Digit3,
        "Digit4" => KeyCode::Digit4,
        "Digit5" => KeyCode::Digit5,
        "Digit6" => KeyCode::Digit6,
        "Digit7" => KeyCode::Digit7,
        "Digit8" => KeyCode::Digit8,
        "Digit9" => KeyCode::Digit9,
        // Arrows
        "ArrowUp" => KeyCode::ArrowUp,
        "ArrowDown" => KeyCode::ArrowDown,
        "ArrowLeft" => KeyCode::ArrowLeft,
        "ArrowRight" => KeyCode::ArrowRight,
        // Function keys
        "F1" => KeyCode::F1,
        "F2" => KeyCode::F2,
        "F3" => KeyCode::F3,
        "F4" => KeyCode::F4,
        "F5" => KeyCode::F5,
        "F6" => KeyCode::F6,
        "F7" => KeyCode::F7,
        "F8" => KeyCode::F8,
        "F9" => KeyCode::F9,
        "F10" => KeyCode::F10,
        "F11" => KeyCode::F11,
        "F12" => KeyCode::F12,
        // Modifiers
        "ShiftLeft" => KeyCode::ShiftLeft,
        "ShiftRight" => KeyCode::ShiftRight,
        "ControlLeft" => KeyCode::ControlLeft,
        "ControlRight" => KeyCode::ControlRight,
        "AltLeft" => KeyCode::AltLeft,
        "AltRight" => KeyCode::AltRight,
        // Common keys
        "Space" => KeyCode::Space,
        "Enter" => KeyCode::Enter,
        "Escape" => KeyCode::Escape,
        "Backspace" => KeyCode::Backspace,
        "Tab" => KeyCode::Tab,
        "Delete" => KeyCode::Delete,
        "Insert" => KeyCode::Insert,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        "PageUp" => KeyCode::PageUp,
        "PageDown" => KeyCode::PageDown,
        // Punctuation / symbols
        "Minus" => KeyCode::Minus,
        "Equal" => KeyCode::Equal,
        "BracketLeft" => KeyCode::BracketLeft,
        "BracketRight" => KeyCode::BracketRight,
        "Backslash" => KeyCode::Backslash,
        "Semicolon" => KeyCode::Semicolon,
        "Quote" => KeyCode::Quote,
        "Backquote" => KeyCode::Backquote,
        "Comma" => KeyCode::Comma,
        "Period" => KeyCode::Period,
        "Slash" => KeyCode::Slash,
        _ => return None,
    })
}

// ============================================================================
// Input binding & map
// ============================================================================

/// A single physical-input-to-action mapping.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InputBinding {
    pub trigger: InputTrigger,
    pub action: GameAction,
    pub mode: ActionMode,
}

/// Maps physical inputs to game actions. Serializable for user configuration.
#[derive(Resource, Clone, Debug, Serialize, Deserialize)]
pub struct InputMap {
    pub bindings: Vec<InputBinding>,
}

impl Default for InputMap {
    fn default() -> Self {
        use GameAction::*;
        use ActionMode::*;
        Self {
            bindings: vec![
                // WASD + arrows - held
                InputBinding { trigger: InputTrigger::Key(KeyCode::KeyW.into()),       action: MoveForward,  mode: Held },
                InputBinding { trigger: InputTrigger::Key(KeyCode::ArrowUp.into()),    action: MoveForward,  mode: Held },
                InputBinding { trigger: InputTrigger::Key(KeyCode::KeyS.into()),       action: MoveBackward, mode: Held },
                InputBinding { trigger: InputTrigger::Key(KeyCode::ArrowDown.into()),  action: MoveBackward, mode: Held },
                InputBinding { trigger: InputTrigger::Key(KeyCode::KeyA.into()),       action: MoveLeft,     mode: Held },
                InputBinding { trigger: InputTrigger::Key(KeyCode::ArrowLeft.into()),  action: MoveLeft,     mode: Held },
                InputBinding { trigger: InputTrigger::Key(KeyCode::KeyD.into()),       action: MoveRight,    mode: Held },
                InputBinding { trigger: InputTrigger::Key(KeyCode::ArrowRight.into()), action: MoveRight,    mode: Held },
                // Q/E rotation - edge triggered
                InputBinding { trigger: InputTrigger::Key(KeyCode::KeyQ.into()), action: CameraRotateLeft,  mode: EdgeTriggered },
                InputBinding { trigger: InputTrigger::Key(KeyCode::KeyE.into()), action: CameraRotateRight, mode: EdgeTriggered },
                // Scroll zoom - continuous
                InputBinding { trigger: InputTrigger::ScrollUp,   action: CameraZoomIn,  mode: Continuous },
                InputBinding { trigger: InputTrigger::ScrollDown, action: CameraZoomOut, mode: Continuous },
                // UI / editor toggles - edge triggered
                InputBinding { trigger: InputTrigger::Key(KeyCode::F1.into()), action: ToggleUI, mode: EdgeTriggered },
                InputBinding { trigger: InputTrigger::Key(KeyCode::F2.into()), action: ToggleGraphEditor, mode: EdgeTriggered },
                InputBinding { trigger: InputTrigger::Key(KeyCode::F3.into()), action: ToggleFieldProbe, mode: EdgeTriggered },
                InputBinding { trigger: InputTrigger::Key(KeyCode::F5.into()), action: TogglePlayMode, mode: EdgeTriggered },
                // Debug: pour watch at the cursor - edge triggered
                InputBinding { trigger: InputTrigger::Key(KeyCode::KeyG.into()), action: PourWater, mode: EdgeTriggered },
                // Player-semantic actions (provisional keys; rebind in input.ron)
                InputBinding { trigger: InputTrigger::Key(KeyCode::KeyF.into()), action: Interact, mode: EdgeTriggered },
                InputBinding { trigger: InputTrigger::Key(KeyCode::KeyR.into()), action: PrimaryAction, mode: EdgeTriggered },
                InputBinding { trigger: InputTrigger::Key(KeyCode::KeyT.into()), action: SecondaryAction, mode: EdgeTriggered },
                InputBinding { trigger: InputTrigger::Key(KeyCode::KeyX.into()), action: CycleTool(1), mode: EdgeTriggered },
                InputBinding { trigger: InputTrigger::Key(KeyCode::KeyZ.into()), action: CycleTool(-1), mode: EdgeTriggered },
                InputBinding { trigger: InputTrigger::Key(KeyCode::BracketRight.into()), action: CycleTargetLayer(1), mode: EdgeTriggered },
                InputBinding { trigger: InputTrigger::Key(KeyCode::BracketLeft.into()), action: CycleTargetLayer(-1), mode: EdgeTriggered },
                InputBinding { trigger: InputTrigger::Key(KeyCode::KeyC.into()), action: ToggleCutaway, mode: EdgeTriggered },
                InputBinding { trigger: InputTrigger::Key(KeyCode::Escape.into()),     action: CancelTool, mode: EdgeTriggered },
                InputBinding { trigger: InputTrigger::Key(KeyCode::ShiftLeft.into()),  action: RepeatTool, mode: Held },
                InputBinding { trigger: InputTrigger::Key(KeyCode::ShiftRight.into()), action: RepeatTool, mode: Held },
                InputBinding { trigger: InputTrigger::Key(KeyCode::KeyR.into()),       action: RotateTool, mode: EdgeTriggered },
            ],
        }
    }
}

impl InputMap {
    /// All bindings triggered by a given key code.
    pub fn actions_for_key(&self, code: KeyCode) -> impl Iterator<Item = &InputBinding> {
        self.bindings.iter().filter(move |b| {
            matches!(&b.trigger, InputTrigger::Key(k) if k.0 == code)
        })
    }

    /// All bindings triggered by scroll in the given direction.
    pub fn actions_for_scroll_up(&self) -> impl Iterator<Item = &InputBinding> {
        self.bindings.iter().filter(|b| b.trigger == InputTrigger::ScrollUp)
    }

    pub fn actions_for_scroll_down(&self) -> impl Iterator<Item = &InputBinding> {
        self.bindings.iter().filter(|b| b.trigger == InputTrigger::ScrollDown)
    }

    /// Load bindings from a RON file, falling back to (and writing) and defaults
    /// if the file is missing or unparseable. Matches the data-driven materials/
    /// prefabs pattern; edit the file and relaunch to rebind.
    pub fn load_or_default(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => match ron::from_str::<InputMap>(&text) {
                Ok(map) => {
                    log::info!("Loaded input bindings from {:?}", path);
                    map
                }
                Err(e) => {
                    log::warn!("input bindings parse error ({e}); using defaults");
                    Self::default()
                }
            },
            Err(_) => {
                let map = Self::default();
                match ron::ser::to_string_pretty(&map, ron::ser::PrettyConfig::default()) {
                    Ok(text) => match std::fs::write(path, text) {
                        Ok(()) => log::info!("Wrote default input bindings to {:?}", path),
                        Err(e) => log::warn!("could not write default input.ron: {e}"),
                    },
                    Err(e) => log::warn!("could not serialize default input bindings: {e}"),
                }
                map
            }
        }
    }
}

// ============================================================================
// Per-action runtime state
// ============================================================================

/// Tracks the state of a single action for the current frame.
#[derive(Clone, Debug, Default)]
pub struct ActionState {
    /// Currently held (true for the duration of a key press).
    pub pressed: bool,
    /// Transitioned to pressed this frame.
    pub just_pressed: bool,
    /// Transitioned to released this frame.
    pub just_released: bool,
    /// Analog magnitude this frame (scroll delta, or 1.0 for digital).
    pub value: f32,
}

/// Aggregate input state for all actions. Consumed by game systems.
#[derive(Resource)]
pub struct InputState {
    actions: HashMap<GameAction, ActionState>,
    move_vector: Vec2,
}

impl InputState {
    pub fn new() -> Self {
        Self {
            actions: HashMap::new(),
            move_vector: Vec2::ZERO,
        }
    }

    /// Screen-relative movement intent this frame (x = right, y = forward),
    /// magnitude 0..=1. The consumer maps it to world space (camera in Authoring,
    /// player in Play), so controller support is an input-mapping task, not a
    /// movement redesign.
    pub fn move_vector(&self) -> Vec2 {
        self.move_vector
    }

    pub fn pressed(&self, action: GameAction) -> bool {
        self.actions.get(&action).map_or(false, |s| s.pressed)
    }

    pub fn just_pressed(&self, action: GameAction) -> bool {
        self.actions.get(&action).map_or(false, |s| s.just_pressed)
    }

    #[allow(dead_code)] // input accessor; pairs with just_pressed
    pub fn just_released(&self, action: GameAction) -> bool {
        self.actions.get(&action).map_or(false, |s| s.just_released)
    }

    pub fn value(&self, action: GameAction) -> f32 {
        self.actions.get(&action).map_or(0.0, |s| s.value)
    }

    /// Reset transient flags at the start of each frame.
    fn begin_frame(&mut self) {
        for state in self.actions.values_mut() {
            state.just_pressed = false;
            state.just_released = false;
            state.value = 0.0;
        }
    }
}

// ============================================================================
// Pointer (mouse) state
// ============================================================================

/// Press/edge state for a single mouse button.
#[derive(Default, Clone, Copy, Debug)]
pub struct ButtonEdges {
    pub pressed: bool,
    pub just_pressed: bool,
    pub just_released: bool,
}

/// Mouse cursor + button state, produced by `process_input_system` and consumed
/// by world-interaction systems (picking, edits). Cursor is in window pixels.
#[derive(Resource, Default)]
pub struct PointerState {
    /// Cursor position in window pixels; `None` until the first cursor move.
    pub cursor: Option<(f32, f32)>,
    pub left: ButtonEdges,
    pub right: ButtonEdges,
}

impl PointerState {
    /// Clear per-frame button edges (call at frame start).
    fn begin_frame(&mut self) {
        self.left.just_pressed = false;
        self.left.just_released = false;
        self.right.just_pressed = false;
        self.right.just_released = false;
    }
}

// ============================================================================
// Raw event buffer (filled by main.rs event loop)
// ============================================================================

/// A raw input event buffered from the winit event loop for the ECS to process.
pub enum RawInputEvent {
    KeyPressed(KeyCode),
    KeyReleased(KeyCode),
    /// Scroll amount: positive = up (zoom in), negative = down (zoom out).
    Scroll(f32),
    /// Cursor moved to window-pixel position `(x, y)`.
    CursorMoved(f32, f32),
    /// Mouse button changed: `true` = pressed, `false` = released.
    MouseButton(PointerButton, bool),
}

/// A mouse button the game cares about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerButton {
    Left,
    Right,
}

/// Buffer of raw input events, drained each frame by `process_input_system`.
#[derive(Resource)]
pub struct RawInputBuffer {
    pub events: Vec<RawInputEvent>,
    /// Keys currently physically held, maintained across frames.
    held_keys: HashSet<KeyCode>,
    /// Accumulated scroll delta this frame.
    scroll_accumulator: f32,
    /// Set before schedule runs: true if egui wants keyboard focus.
    pub egui_wants_keyboard: bool,
    /// Set before schedule runs: true if egui wants pointer/scroll focus.
    pub egui_wants_pointer: bool,
}

impl Default for RawInputBuffer {
    fn default() -> Self {
        Self {
            events: Vec::new(),
            held_keys: HashSet::new(),
            scroll_accumulator: 0.0,
            egui_wants_keyboard: false,
            egui_wants_pointer: false,
        }
    }
}

// ============================================================================
// Input processing system (FrameStage::Input)
// ============================================================================

/// Processes buffered raw events through the input map to produce per-action state.
pub fn process_input_system(
    mut buffer: ResMut<RawInputBuffer>,
    map: Res<InputMap>,
    mut state: ResMut<InputState>,
    mut pointer: ResMut<PointerState>,
) {
    state.begin_frame();
    pointer.begin_frame();

    // Take ownership of buffered events to avoid double-borrow on `buffer`.
    let events = std::mem::take(&mut buffer.events);
    buffer.scroll_accumulator = 0.0;
    for event in events {
        match event {
            RawInputEvent::KeyPressed(code) => {
                buffer.held_keys.insert(code);
            }
            RawInputEvent::KeyReleased(code) => {
                buffer.held_keys.remove(&code);
            }
            RawInputEvent::Scroll(amount) => {
                buffer.scroll_accumulator += amount;
            }
            RawInputEvent::CursorMoved(x, y) => {
                pointer.cursor = Some((x, y));
            }
            RawInputEvent::MouseButton(button, pressed) => {
                let edges = match button {
                    PointerButton::Left => &mut pointer.left,
                    PointerButton::Right => &mut pointer.right,
                };
                if pressed {
                    if !edges.pressed {
                        edges.just_pressed = true;
                    }
                    edges.pressed = true;
                } else {
                    if edges.pressed {
                        edges.just_released = true;
                    }
                    edges.pressed = false;
                }
            }
        }
    }

    // Determine which actions are active based on held keys + input map.
    // Build a set of currently-pressed actions from held keys.
    let mut active_actions: HashMap<GameAction, f32> = HashMap::new();

    if !buffer.egui_wants_keyboard {
        for &code in &buffer.held_keys {
            for binding in map.actions_for_key(code) {
                active_actions
                    .entry(binding.action)
                    .or_insert(1.0);
            }
        }
    }

    // Process scroll
    if !buffer.egui_wants_pointer {
        let scroll = buffer.scroll_accumulator;
        if scroll > f32::EPSILON {
            for binding in map.actions_for_scroll_up() {
                *active_actions.entry(binding.action).or_insert(0.0) += scroll;
            }
        } else if scroll < -f32::EPSILON {
            for binding in map.actions_for_scroll_down() {
                *active_actions.entry(binding.action).or_insert(0.0) += scroll.abs();
            }
        }
    }

    // Update InputState: detect edges by comparing to previous pressed state.
    // First, handle actions that are now active.
    for (action, value) in &active_actions {
        let entry = state.actions.entry(*action).or_default();

        // Determine mode from any matching binding (they should all agree).
        let mode = map.bindings.iter()
            .find(|b| b.action == *action)
            .map(|b| b.mode)
            .unwrap_or(ActionMode::Held);

        match mode {
            ActionMode::Held => {
                if !entry.pressed {
                    entry.just_pressed = true;
                }
                entry.pressed = true;
                entry.value = *value;
            }
            ActionMode::EdgeTriggered => {
                if !entry.pressed {
                    entry.just_pressed = true;
                }
                entry.pressed = true;
                entry.value = *value;
            }
            ActionMode::Continuous => {
                entry.value = *value;
            }
        }
    }

    // Handle actions that were pressed but are no longer active.
    for (action, action_state) in state.actions.iter_mut() {
        if action_state.pressed && !active_actions.contains_key(action) {
            action_state.pressed = false;
            action_state.just_released = true;
        }
    }

    // Screen-relative movement vector from the directional actions (x = right,
    // y = forward). Normalized so diagonals aren't faster.
    let mv_x = (state.pressed(GameAction::MoveRight) as i32 - state.pressed(GameAction::MoveLeft) as i32) as f32;
    let mv_y = (state.pressed(GameAction::MoveForward) as i32 - state.pressed(GameAction::MoveBackward) as i32) as f32;
    let mut mv = Vec2::new(mv_x, mv_y);
    if mv.length_squared() > 1.0 {
        mv = mv.normalize();
    }
    state.move_vector = mv;
}

/// Handles UI / editor toggles via InputState (needs NonSend access for EguiRenderer).
pub fn toggle_ui_system(
    state: Res<InputState>,
    mut egui: NonSendMut<ui::EguiRenderer>,
    mut probe: ResMut<ui::field_probe::FieldProbe>,
) {
    if state.just_pressed(GameAction::ToggleUI) {
        egui.toggle_visibility();
    }
    if state.just_pressed(GameAction::ToggleGraphEditor) {
        egui.toggle_editor();
    }
    if state.just_pressed(GameAction::ToggleFieldProbe) {
        probe.enabled = !probe.enabled;
    }
}
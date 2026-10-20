//! TOML configuration: parsing, validation, and defaults.
//!
//! The config format is **TOML** (ADR-0003), parsed with `serde` and the `toml`
//! crate. It follows the mango-style section layout: `[input]`,
//! `[output.<name>]`, `[layout]`, `[animations]`, `[autostart]`,
//! `[window_rule]`, `[layer_rule]`, `[binds.default]`, `[binds.locked]`,
//! `[mousebinds]`, `[gesturebinds]` and `[axisbinds]`. The shipped example is
//! `resources/default-config.toml`; without `--config`, crawm reads
//! [`default_path`] when that file exists.
//!
//! Unknown keys are ignored, so a config keeps working across versions; a key
//! that is *present* but has the wrong type is an error, so a typo is reported
//! rather than silently defaulted. Sections whose feature is not implemented
//! yet are still parsed and validated, so a config can be written once and
//! pick up new behavior as milestones land.
//!
//! `src/config` has no Smithay/Wayland dependency (ROADMAP §5); it depends only
//! on [`crate::core`] for shared primitives like [`crate::core::Color`], and
//! only *downward* (`config → core`).
//!
//! ```
//! let config = crawm::config::parse("[layout]\ngaps = 4\n").unwrap();
//! assert_eq!(config.gaps, 4);
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::core::{Color, LayoutKind, LayoutParams, ScrollCenter};
use serde::Deserialize;

/// The compositor configuration.
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// Outer/inner gap in logical pixels (`[layout] gaps`).
    pub gaps: i32,
    /// Window border width in logical pixels (`[layout] border_width`; `0`
    /// disables borders).
    pub border_width: i32,
    /// Border color of the focused window (`[layout] border_color`).
    pub border_color: Color,
    /// Border color of unfocused windows (`[layout] unfocused_border_color`).
    pub unfocused_border_color: Color,
    /// Default number of windows in the master area (`[layout] tile_master_count`).
    pub master_count: i32,
    /// Default master-area fraction (`[layout] tile_master_ratio`), within
    /// `LayoutParams::MIN_RATIO..=LayoutParams::MAX_RATIO`.
    pub master_ratio: f64,
    /// Default layout name for new spaces (`[layout] layout`).
    pub layout: String,
    /// Animation defaults (`[animations]`).
    pub animations: Animations,
    /// Request server-side decorations from clients (`[layout]
    /// client_side_decorations`). Parsed, not applied: xdg-decoration is M6.
    pub client_side_decorations: bool,
    /// Where a new space is created, per layout family (`[layout]
    /// *_next_space_direction`). Parsed, not applied yet.
    pub space_directions: SpaceDirections,
    /// Scroll-family defaults (`[layout] scroll_*`).
    pub scroll: ScrollDefaults,
    /// Move focus to the window under the pointer (`[layout]
    /// focus_follows_cursor`). Parsed, not applied yet (M4).
    pub focus_follows_cursor: bool,
    /// Keyboard, pointer and touchpad defaults (`[input]`). Parsed and
    /// validated; applied when the libinput backend lands (M4).
    pub input: Input,
    /// Per-connector settings (`[output.<name>]`). Parsed; applied with the
    /// output backend (M4).
    pub outputs: BTreeMap<String, Output>,
    /// Commands to run once at startup (`[autostart]`). Parsed, not run yet.
    pub autostart: Autostart,
    /// Rules applied when a window maps (`[window_rule]` or `[[window_rule]]`).
    /// Parsed; matched when the rules engine lands (M5.4).
    pub window_rules: Vec<WindowRule>,
    /// Rules applied to layer-shell surfaces (`[layer_rule]` or
    /// `[[layer_rule]]`). Parsed, not applied yet (M5.4/M6.7).
    pub layer_rules: Vec<LayerRule>,
    /// Keyboard chords (`[binds.default]` and `[binds.locked]`), kept as the
    /// authored `chord = action` strings. Built-in binds still run until M5.3
    /// compiles these; `locked` bindings wait for a lock screen (M6).
    pub binds: Binds,
    /// Pointer-button binds (`[mousebinds]`), kept as action strings. Parsed,
    /// not applied yet.
    pub mouse_binds: BTreeMap<String, String>,
    /// Swipe/pinch binds (`[gesturebinds]`), kept as action strings. Parsed,
    /// not applied yet (M6.7).
    pub gesture_binds: BTreeMap<String, String>,
    /// Axis (scroll) binds (`[axisbinds]`), kept as action strings. Parsed,
    /// not applied yet.
    pub axis_binds: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            gaps: 8,
            border_width: 2,
            border_color: Color::from_hex("#7aa2f7").expect("static color is valid"),
            unfocused_border_color: Color::from_hex("#3b4261").expect("static color is valid"),
            master_count: 1,
            master_ratio: 0.6,
            layout: "tile".to_owned(),
            animations: Animations::default(),
            client_side_decorations: false,
            space_directions: SpaceDirections::default(),
            scroll: ScrollDefaults::default(),
            focus_follows_cursor: true,
            input: Input::default(),
            outputs: BTreeMap::new(),
            autostart: Autostart::default(),
            window_rules: Vec::new(),
            layer_rules: Vec::new(),
            binds: Binds::default(),
            mouse_binds: BTreeMap::new(),
            gesture_binds: BTreeMap::new(),
            axis_binds: BTreeMap::new(),
        }
    }
}

impl Config {
    /// Sections that parsed but whose behavior is not implemented yet.
    ///
    /// A hand-written config can carry sections ahead of their milestone; this
    /// names them so startup can say which parts are not doing anything yet.
    /// Entries disappear as their features land (M4–M6).
    #[must_use]
    pub fn pending_sections(&self) -> Vec<&'static str> {
        let defaults = Self::default();
        let mut pending = Vec::new();
        if self.input != defaults.input {
            pending.push("input");
        }
        if !self.outputs.is_empty() {
            pending.push("output");
        }
        if !self.autostart.exec_once.is_empty() {
            pending.push("autostart");
        }
        if !self.window_rules.is_empty() {
            pending.push("window_rule");
        }
        if !self.layer_rules.is_empty() {
            pending.push("layer_rule");
        }
        if !self.binds.default.is_empty() || !self.binds.locked.is_empty() {
            pending.push("binds");
        }
        if !self.mouse_binds.is_empty() {
            pending.push("mousebinds");
        }
        if !self.gesture_binds.is_empty() {
            pending.push("gesturebinds");
        }
        if !self.axis_binds.is_empty() {
            pending.push("axisbinds");
        }
        if self.focus_follows_cursor != defaults.focus_follows_cursor {
            pending.push("layout.focus_follows_cursor");
        }
        if self.client_side_decorations {
            pending.push("layout.client_side_decorations");
        }
        if self.space_directions != defaults.space_directions {
            pending.push("layout.*_next_space_direction");
        }
        if self.scroll.always_center_single_column != defaults.scroll.always_center_single_column {
            pending.push("layout.scroll_always_center_single_column");
        }
        if self.animations.open != defaults.animations.open
            || self.animations.close != defaults.animations.close
        {
            pending.push("animations.animation_type_*");
        }
        pending
    }
}

/// The `[animations]` table.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Animations {
    /// Whether animations run at all (also toggled at runtime by IPC).
    pub enabled: bool,
    /// How a window appears (`animation_type_open`).
    pub open: AnimationStyle,
    /// How a window disappears (`animation_type_close`).
    pub close: AnimationStyle,
}

impl Default for Animations {
    fn default() -> Self {
        Self {
            enabled: true,
            open: AnimationStyle::Slide,
            close: AnimationStyle::Slide,
        }
    }
}

/// A window open/close animation (`animation_type_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimationStyle {
    /// The window slides in from, or out toward, its edge.
    Slide,
    /// The window scales up from, or down toward, its center.
    Zoom,
}

impl AnimationStyle {
    /// Every variant, in documentation order.
    pub const ALL: [Self; 2] = [Self::Slide, Self::Zoom];

    /// A stable identifier used in the config.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Slide => "slide",
            Self::Zoom => "zoom",
        }
    }

    fn parse(value: &str, key: &str) -> Result<Self, ConfigError> {
        Self::ALL
            .into_iter()
            .find(|style| style.name() == value)
            .ok_or_else(|| ConfigError::Invalid(one_of(key, Self::ALL.map(Self::name))))
    }
}

/// The `[layout]` scroll-family defaults.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollDefaults {
    /// Where the view puts the focused column (`scroll_center_focused_column`).
    pub center_focused_column: ScrollCenter,
    /// Whether a view with a single column centres it
    /// (`scroll_always_center_single_column`). Parsed, not applied yet.
    pub always_center_single_column: bool,
    /// Width a new column gets, as a fraction of the view
    /// (`scroll_default_column_ratio`).
    pub default_column_ratio: f64,
}

impl Default for ScrollDefaults {
    fn default() -> Self {
        Self {
            center_focused_column: ScrollCenter::Never,
            always_center_single_column: true,
            default_column_ratio: 0.5,
        }
    }
}

/// The `[layout] *_next_space_direction` keys: where a new space is created,
/// per layout family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpaceDirections {
    /// Direction for the `stack` family.
    pub stack: Flow,
    /// Direction for the `tile` family.
    pub tile: Flow,
    /// Direction for the `scroll` family.
    pub scroll: Flow,
}

impl Default for SpaceDirections {
    fn default() -> Self {
        Self {
            stack: Flow::Horizontal,
            tile: Flow::Horizontal,
            scroll: Flow::Vertical,
        }
    }
}

/// A one-dimensional direction in which new spaces extend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    /// Spaces extend along the x axis.
    Horizontal,
    /// Spaces extend along the y axis.
    Vertical,
}

impl Flow {
    /// Every variant, in documentation order.
    pub const ALL: [Self; 2] = [Self::Horizontal, Self::Vertical];

    /// A stable identifier used in the config.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Horizontal => "horizontal",
            Self::Vertical => "vertical",
        }
    }

    fn parse(value: &str, key: &str) -> Result<Self, ConfigError> {
        Self::ALL
            .into_iter()
            .find(|flow| flow.name() == value)
            .ok_or_else(|| ConfigError::Invalid(one_of(key, Self::ALL.map(Self::name))))
    }
}

/// The `[input]` table: keyboard, pointer and touchpad defaults.
#[derive(Clone, Debug, PartialEq)]
pub struct Input {
    /// Key repeat rate in keys per second (`keyboard.repeat_rate`; `0` disables repeat).
    pub repeat_rate: i32,
    /// Key repeat delay in milliseconds (`keyboard.repeat_delay`).
    pub repeat_delay: i32,
    /// Whether NumLock starts enabled (`keyboard.numlock_on`).
    pub numlock_on: bool,
    /// xkb layout(s), as accepted by `xkbcommon` (`keyboard.xkb_rules_layout`).
    pub xkb_rules_layout: String,
    /// Whether the touchpad is disabled (`touchpad.disable`).
    pub disable_touchpad: bool,
    /// Tap-to-click (`touchpad.tap_to_click`).
    pub tap_to_click: bool,
    /// Tap-and-drag (`touchpad.tap_and_drag`).
    pub tap_and_drag: bool,
    /// Drag lock (`touchpad.drag_lock`).
    pub drag_lock: bool,
    /// Natural scrolling for touchpads (`touchpad.natural_scrolling`).
    pub touchpad_natural_scrolling: bool,
    /// Ignore touchpad events while typing (`touchpad.disable_while_typing`).
    pub touchpad_disable_while_typing: bool,
    /// Minimum travel for a swipe to count as a gesture
    /// (`touchpad.swipe_min_threshold`).
    pub swipe_min_threshold: f64,
    /// Natural scrolling for mice (`mouse.natural_scrolling`).
    pub mouse_natural_scrolling: bool,
}

impl Default for Input {
    fn default() -> Self {
        Self {
            repeat_rate: 25,
            repeat_delay: 600,
            numlock_on: false,
            xkb_rules_layout: "us".to_owned(),
            disable_touchpad: false,
            tap_to_click: true,
            tap_and_drag: true,
            drag_lock: true,
            touchpad_natural_scrolling: true,
            touchpad_disable_while_typing: true,
            swipe_min_threshold: 1.0,
            mouse_natural_scrolling: false,
        }
    }
}

/// One `[output.<name>]` table, where `<name>` is the connector (e.g. `eDP-1`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Output {
    /// Mode as `WIDTHxHEIGHT[@RATE]` (`mode`).
    pub mode: Option<String>,
    /// Logical scale factor (`scale`).
    pub scale: Option<f64>,
    /// Output transform, e.g. `normal`, `90` or `flipped` (`transform`).
    pub transform: Option<String>,
    /// Position in the global logical layout (`position`).
    pub position: Option<(i32, i32)>,
}

/// The `[autostart]` table.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Autostart {
    /// Commands to run once when the session starts (`exec_once`). Parsed, not
    /// run yet.
    pub exec_once: Vec<String>,
}

/// One `[window_rule]`/`[[window_rule]]` entry, matched when a window maps.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WindowRule {
    /// `app_id`s to match (`app_id`, one or many); empty matches any window.
    pub app_id: Vec<String>,
    /// Open the window floating (`open_floating`).
    pub open_floating: bool,
    /// Initial size in logical pixels (`size = [width, height]`).
    pub size: Option<(i32, i32)>,
    /// Initial x offset in logical pixels (`offset_x`).
    pub offset_x: Option<i32>,
    /// Initial y offset in logical pixels (`offset_y`).
    pub offset_y: Option<i32>,
}

/// One `[layer_rule]` entry, matched against layer-shell surfaces.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayerRule {
    /// Animation used when the layer opens (`animation_type_open`).
    pub animation_type_open: Option<String>,
    /// Layer-shell namespaces this rule applies to (`layer_name`).
    pub layer_name: Vec<String>,
}

/// The `[binds]` table: keyboard chords to action strings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Binds {
    /// Binds active in a normal session (`[binds.default]`).
    pub default: BTreeMap<String, String>,
    /// Binds that stay active while the screen is locked (`[binds.locked]`).
    pub locked: BTreeMap<String, String>,
}

/// Errors produced while loading a configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file could not be read.
    #[error("failed to read config {path}: {source}")]
    Io {
        /// Path that could not be read.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The TOML could not be parsed, or a value had the wrong type.
    #[error("failed to parse config: {0}")]
    Parse(String),
    /// The config parsed but is semantically invalid.
    #[error("invalid config: {0}")]
    Invalid(String),
}

/// The on-disk shape, before validation.
///
/// Every field is optional so a partial file inherits the defaults. Unknown keys
/// are ignored (`serde`'s default), which keeps older configs working as crawm
/// grows; present-but-wrongly-typed keys still fail.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawConfig {
    input: RawInput,
    output: BTreeMap<String, RawOutput>,
    layout: RawLayout,
    animations: RawAnimations,
    autostart: RawAutostart,
    window_rule: Option<OneOrMany<RawWindowRule>>,
    layer_rule: Option<OneOrMany<RawLayerRule>>,
    binds: RawBinds,
    mousebinds: BTreeMap<String, String>,
    gesturebinds: BTreeMap<String, String>,
    axisbinds: BTreeMap<String, String>,
}

/// The `[layout]` table.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawLayout {
    gaps: Option<i64>,
    border_width: Option<i64>,
    border_color: Option<String>,
    unfocused_border_color: Option<String>,
    client_side_decorations: Option<RawFlag>,
    focus_follows_cursor: Option<RawFlag>,
    default_layout: Option<String>,
    /// The pre-rename name for `default_layout`, rejected with a pointer to it.
    layout: Option<toml::Value>,
    stack: RawStackLayout,
    tile: RawTileLayout,
    scroll: RawScrollLayout,
}

/// The `[layout.stack]` table.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawStackLayout {
    next_space_direction: Option<String>,
}

/// The `[layout.tile]` table.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawTileLayout {
    master_count: Option<i64>,
    master_ratio: Option<RawNumber>,
    next_space_direction: Option<String>,
}

/// The `[layout.scroll]` table.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawScrollLayout {
    center_focused_column: Option<String>,
    always_center_single_column: Option<RawFlag>,
    default_column_ratio: Option<RawNumber>,
    next_space_direction: Option<String>,
}

/// The `[animations]` table.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawAnimations {
    enabled: Option<RawFlag>,
    animation_type_open: Option<String>,
    animation_type_close: Option<String>,
}

/// The `[input]` table.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawInput {
    keyboard: RawKeyboard,
    touchpad: RawTouchpad,
    mouse: RawMouse,
}

/// The `[input.keyboard]` table.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawKeyboard {
    repeat_rate: Option<i64>,
    repeat_delay: Option<i64>,
    numlock_on: Option<RawFlag>,
    xkb_rules_layout: Option<String>,
}

/// The `[input.touchpad]` table.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawTouchpad {
    disable: Option<RawFlag>,
    tap_to_click: Option<RawFlag>,
    tap_and_drag: Option<RawFlag>,
    drag_lock: Option<RawFlag>,
    natural_scrolling: Option<RawFlag>,
    disable_while_typing: Option<RawFlag>,
    swipe_min_threshold: Option<RawNumber>,
}

/// The `[input.mouse]` table.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawMouse {
    natural_scrolling: Option<RawFlag>,
}

/// One `[output.<name>]` table.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawOutput {
    mode: Option<String>,
    scale: Option<RawNumber>,
    transform: Option<String>,
    position: Option<RawPosition>,
}

/// An output `position = { x = .., y = .. }` inline table.
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(default)]
struct RawPosition {
    x: i32,
    y: i32,
}

/// The `[autostart]` table.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawAutostart {
    exec_once: Vec<String>,
}

/// One `[window_rule]` entry.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawWindowRule {
    app_id: Option<OneOrMany<String>>,
    open_floating: Option<RawFlag>,
    size: Option<[i64; 2]>,
    offset_x: Option<i64>,
    offset_y: Option<i64>,
}

/// One `[layer_rule]` entry.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawLayerRule {
    animation_type_open: Option<String>,
    layer_name: Vec<String>,
}

/// The `[binds]` table.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawBinds {
    default: BTreeMap<String, String>,
    locked: BTreeMap<String, String>,
}

/// A TOML table that may also be repeated as an array of tables.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

impl<T> OneOrMany<T> {
    fn into_vec(self) -> Vec<T> {
        match self {
            Self::One(one) => vec![one],
            Self::Many(many) => many,
        }
    }
}

/// A toggle written either as `true`/`false` or as mango's `0`/`1`.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(untagged)]
enum RawFlag {
    Bool(bool),
    Int(i64),
}

impl RawFlag {
    fn to_bool(self, key: &str) -> Result<bool, ConfigError> {
        match self {
            Self::Bool(value) => Ok(value),
            Self::Int(0) => Ok(false),
            Self::Int(1) => Ok(true),
            Self::Int(other) => Err(ConfigError::Invalid(format!(
                "`{key}` must be `true`/`false` or `0`/`1`, not {other}"
            ))),
        }
    }
}

/// A number written as a TOML integer or float.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(untagged)]
enum RawNumber {
    Int(i64),
    Float(f64),
}

impl RawNumber {
    fn get(self) -> f64 {
        match self {
            Self::Int(value) => value as f64,
            Self::Float(value) => value,
        }
    }
}

impl RawLayout {
    fn apply(self, config: &mut Config) -> Result<(), ConfigError> {
        if let Some(value) = self.gaps {
            config.gaps = checked_i32(value, "layout.gaps")?;
        }
        if let Some(value) = self.border_width {
            config.border_width = checked_i32(value, "layout.border_width")?;
        }
        if let Some(value) = self.border_color {
            config.border_color = parse_color(&value, "layout.border_color")?;
        }
        if let Some(value) = self.unfocused_border_color {
            config.unfocused_border_color = parse_color(&value, "layout.unfocused_border_color")?;
        }
        if let Some(value) = self.client_side_decorations {
            config.client_side_decorations = value.to_bool("layout.client_side_decorations")?;
        }
        if let Some(value) = self.focus_follows_cursor {
            config.focus_follows_cursor = value.to_bool("layout.focus_follows_cursor")?;
        }
        if self.layout.is_some() {
            return Err(ConfigError::Invalid(
                "`[layout] layout` was renamed to `default_layout`".to_owned(),
            ));
        }
        if let Some(value) = self.default_layout {
            config.layout = value;
        }
        self.stack.apply(config)?;
        self.tile.apply(config)?;
        self.scroll.apply(config)?;
        Ok(())
    }
}

impl RawStackLayout {
    fn apply(self, config: &mut Config) -> Result<(), ConfigError> {
        if let Some(value) = self.next_space_direction {
            config.space_directions.stack =
                Flow::parse(&value, "layout.stack.next_space_direction")?;
        }
        Ok(())
    }
}

impl RawTileLayout {
    fn apply(self, config: &mut Config) -> Result<(), ConfigError> {
        if let Some(value) = self.master_count {
            config.master_count = checked_i32(value, "layout.tile.master_count")?;
        }
        if let Some(value) = self.master_ratio {
            config.master_ratio = value.get();
        }
        if let Some(value) = self.next_space_direction {
            config.space_directions.tile = Flow::parse(&value, "layout.tile.next_space_direction")?;
        }
        Ok(())
    }
}

impl RawScrollLayout {
    fn apply(self, config: &mut Config) -> Result<(), ConfigError> {
        if let Some(value) = self.center_focused_column {
            config.scroll.center_focused_column =
                ScrollCenter::from_name(&value).ok_or_else(|| {
                    ConfigError::Invalid(one_of(
                        "layout.scroll.center_focused_column",
                        ScrollCenter::ALL.map(ScrollCenter::name),
                    ))
                })?;
        }
        if let Some(value) = self.always_center_single_column {
            config.scroll.always_center_single_column =
                value.to_bool("layout.scroll.always_center_single_column")?;
        }
        if let Some(value) = self.default_column_ratio {
            config.scroll.default_column_ratio = value.get();
        }
        if let Some(value) = self.next_space_direction {
            config.space_directions.scroll =
                Flow::parse(&value, "layout.scroll.next_space_direction")?;
        }
        Ok(())
    }
}

impl RawAnimations {
    fn apply(self, config: &mut Config) -> Result<(), ConfigError> {
        if let Some(value) = self.enabled {
            config.animations.enabled = value.to_bool("animations.enabled")?;
        }
        if let Some(value) = self.animation_type_open {
            config.animations.open =
                AnimationStyle::parse(&value, "animations.animation_type_open")?;
        }
        if let Some(value) = self.animation_type_close {
            config.animations.close =
                AnimationStyle::parse(&value, "animations.animation_type_close")?;
        }
        Ok(())
    }
}

impl RawInput {
    fn apply(self, config: &mut Config) -> Result<(), ConfigError> {
        self.keyboard.apply(config)?;
        self.touchpad.apply(config)?;
        self.mouse.apply(config)?;
        Ok(())
    }
}

impl RawKeyboard {
    fn apply(self, config: &mut Config) -> Result<(), ConfigError> {
        let input = &mut config.input;
        if let Some(value) = self.repeat_rate {
            input.repeat_rate = checked_i32(value, "input.keyboard.repeat_rate")?;
        }
        if let Some(value) = self.repeat_delay {
            input.repeat_delay = checked_i32(value, "input.keyboard.repeat_delay")?;
        }
        if let Some(value) = self.numlock_on {
            input.numlock_on = value.to_bool("input.keyboard.numlock_on")?;
        }
        if let Some(value) = self.xkb_rules_layout {
            input.xkb_rules_layout = value;
        }
        Ok(())
    }
}

impl RawTouchpad {
    fn apply(self, config: &mut Config) -> Result<(), ConfigError> {
        let input = &mut config.input;
        if let Some(value) = self.disable {
            input.disable_touchpad = value.to_bool("input.touchpad.disable")?;
        }
        if let Some(value) = self.tap_to_click {
            input.tap_to_click = value.to_bool("input.touchpad.tap_to_click")?;
        }
        if let Some(value) = self.tap_and_drag {
            input.tap_and_drag = value.to_bool("input.touchpad.tap_and_drag")?;
        }
        if let Some(value) = self.drag_lock {
            input.drag_lock = value.to_bool("input.touchpad.drag_lock")?;
        }
        if let Some(value) = self.natural_scrolling {
            input.touchpad_natural_scrolling = value.to_bool("input.touchpad.natural_scrolling")?;
        }
        if let Some(value) = self.disable_while_typing {
            input.touchpad_disable_while_typing =
                value.to_bool("input.touchpad.disable_while_typing")?;
        }
        if let Some(value) = self.swipe_min_threshold {
            input.swipe_min_threshold = value.get();
        }
        Ok(())
    }
}

impl RawMouse {
    fn apply(self, config: &mut Config) -> Result<(), ConfigError> {
        if let Some(value) = self.natural_scrolling {
            config.input.mouse_natural_scrolling =
                value.to_bool("input.mouse.natural_scrolling")?;
        }
        Ok(())
    }
}

impl RawOutput {
    fn into_output(self) -> Output {
        Output {
            mode: self.mode,
            scale: self.scale.map(RawNumber::get),
            transform: self.transform,
            position: self.position.map(|position| (position.x, position.y)),
        }
    }
}

impl RawWindowRule {
    fn into_rule(self, index: usize) -> Result<WindowRule, ConfigError> {
        let key = |field: &str| format!("window_rule[{index}].{field}");
        let size = match self.size {
            Some([width, height]) => Some((
                checked_i32(width, &key("size"))?,
                checked_i32(height, &key("size"))?,
            )),
            None => None,
        };
        Ok(WindowRule {
            app_id: self.app_id.map(OneOrMany::into_vec).unwrap_or_default(),
            open_floating: match self.open_floating {
                Some(flag) => flag.to_bool(&key("open_floating"))?,
                None => false,
            },
            size,
            offset_x: self
                .offset_x
                .map(|value| checked_i32(value, &key("offset_x")))
                .transpose()?,
            offset_y: self
                .offset_y
                .map(|value| checked_i32(value, &key("offset_y")))
                .transpose()?,
        })
    }
}

impl RawLayerRule {
    fn into_rule(self) -> LayerRule {
        LayerRule {
            animation_type_open: self.animation_type_open,
            layer_name: self.layer_name,
        }
    }
}

impl RawConfig {
    fn into_config(self) -> Result<Config, ConfigError> {
        let mut config = Config::default();
        self.layout.apply(&mut config)?;
        self.animations.apply(&mut config)?;
        self.input.apply(&mut config)?;

        for (name, output) in self.output {
            if name.trim().is_empty() {
                return Err(ConfigError::Invalid(
                    "an `[output.<name>]` table needs a connector name".to_owned(),
                ));
            }
            config.outputs.insert(name, output.into_output());
        }

        config.autostart.exec_once = self.autostart.exec_once;

        if let Some(rules) = self.window_rule {
            for (index, raw) in rules.into_vec().into_iter().enumerate() {
                config.window_rules.push(raw.into_rule(index)?);
            }
        }
        if let Some(rules) = self.layer_rule {
            config.layer_rules = rules
                .into_vec()
                .into_iter()
                .map(RawLayerRule::into_rule)
                .collect();
        }

        config.binds = Binds {
            default: self.binds.default,
            locked: self.binds.locked,
        };
        config.mouse_binds = self.mousebinds;
        config.gesture_binds = self.gesturebinds;
        config.axis_binds = self.axisbinds;

        Ok(config)
    }
}

/// Top-level keys that moved under `[layout]` when crawm adopted the
/// mango-style sections.
///
/// Unknown keys are ignored, so without this check an old config would lose
/// these settings silently; a clear error is better than a silent reset.
const LEGACY_TOP_LEVEL: [&str; 7] = [
    "gaps",
    "border_width",
    "border_color",
    "unfocused_border_color",
    "master_count",
    "master_ratio",
    "layout",
];

/// Parses a configuration from TOML text (does not validate).
///
/// # Errors
/// Returns [`ConfigError::Parse`] on malformed TOML or wrongly-typed values, and
/// [`ConfigError::Invalid`] for a value that parsed but is not a valid choice
/// (e.g. an unknown layout name).
pub fn parse(text: &str) -> Result<Config, ConfigError> {
    // Check the raw document first so the pre-`[layout]` form gets a helpful
    // message instead of "invalid type" for the `layout` key.
    if let Ok(document) = toml::from_str::<toml::Value>(text) {
        if let Some(table) = document.as_table() {
            if let Some(key) = LEGACY_TOP_LEVEL
                .iter()
                .find(|key| table.get(**key).is_some_and(|value| !value.is_table()))
            {
                return Err(ConfigError::Invalid(format!(
                    "`{key}` now lives under `[layout]`; move it into that table"
                )));
            }
        }
    }
    let raw: RawConfig = toml::from_str(text).map_err(|err| ConfigError::Parse(err.to_string()))?;
    raw.into_config()
}

/// Loads and validates a configuration from `path`.
///
/// # Errors
/// Returns a [`ConfigError`] if the file cannot be read, parsed, or validated.
pub fn load(path: &Path) -> Result<Config, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let config = parse(&text)?;
    validate(&config)?;
    Ok(config)
}

/// The config file crawm reads when `--config` is not given.
///
/// This is `$XDG_CONFIG_HOME/crawm/config.toml`, or
/// `$HOME/.config/crawm/config.toml` when `XDG_CONFIG_HOME` is unset or not
/// absolute. Returns `None` when neither variable is set.
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("crawm").join("config.toml"))
}

/// Checks semantic constraints that parsing cannot (e.g. non-negative sizes).
///
/// # Errors
/// Returns [`ConfigError::Invalid`] describing the first violated constraint.
pub fn validate(config: &Config) -> Result<(), ConfigError> {
    if config.gaps < 0 {
        return Err(ConfigError::Invalid(
            "`layout.gaps` must be >= 0".to_owned(),
        ));
    }
    if config.border_width < 0 {
        return Err(ConfigError::Invalid(
            "`layout.border_width` must be >= 0".to_owned(),
        ));
    }
    if config.master_count < 0 {
        return Err(ConfigError::Invalid(
            "`layout.tile.master_count` must be >= 0".to_owned(),
        ));
    }
    if !(LayoutParams::MIN_RATIO..=LayoutParams::MAX_RATIO).contains(&config.master_ratio) {
        return Err(ConfigError::Invalid(format!(
            "`layout.tile.master_ratio` must be within {}..={}",
            LayoutParams::MIN_RATIO,
            LayoutParams::MAX_RATIO
        )));
    }
    if LayoutKind::from_name(&config.layout).is_none() {
        return Err(ConfigError::Invalid(one_of(
            "layout.default_layout",
            LayoutKind::ALL.map(LayoutKind::name),
        )));
    }
    if !(config.scroll.default_column_ratio > 0.0 && config.scroll.default_column_ratio.is_finite())
    {
        return Err(ConfigError::Invalid(
            "`layout.scroll.default_column_ratio` must be a positive number".to_owned(),
        ));
    }
    if config.input.repeat_rate < 0 {
        return Err(ConfigError::Invalid(
            "`input.keyboard.repeat_rate` must be >= 0".to_owned(),
        ));
    }
    if config.input.repeat_delay < 0 {
        return Err(ConfigError::Invalid(
            "`input.keyboard.repeat_delay` must be >= 0".to_owned(),
        ));
    }
    if !(config.input.swipe_min_threshold >= 0.0 && config.input.swipe_min_threshold.is_finite()) {
        return Err(ConfigError::Invalid(
            "`input.touchpad.swipe_min_threshold` must be >= 0".to_owned(),
        ));
    }
    for (name, output) in &config.outputs {
        if let Some(scale) = output.scale {
            if !(scale > 0.0 && scale.is_finite()) {
                return Err(ConfigError::Invalid(format!(
                    "`output.{name}.scale` must be a positive number"
                )));
            }
        }
    }
    for (section, binds) in [
        ("binds.default", &config.binds.default),
        ("binds.locked", &config.binds.locked),
        ("mousebinds", &config.mouse_binds),
        ("gesturebinds", &config.gesture_binds),
        ("axisbinds", &config.axis_binds),
    ] {
        for (chord, action) in binds {
            if chord.trim().is_empty() || action.trim().is_empty() {
                return Err(ConfigError::Invalid(format!(
                    "`[{section}]` has a bind with an empty chord or action"
                )));
            }
        }
    }
    Ok(())
}

fn checked_i32(raw: i64, key: &str) -> Result<i32, ConfigError> {
    i32::try_from(raw).map_err(|_| ConfigError::Invalid(format!("`{key}` is out of range")))
}

fn parse_color(hex: &str, key: &str) -> Result<Color, ConfigError> {
    Color::from_hex(hex).map_err(|err| ConfigError::Parse(format!("`{key}`: {err}")))
}

fn one_of<const N: usize>(key: &str, choices: [&'static str; N]) -> String {
    format!("`{key}` must be one of {choices:?}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        validate(&Config::default()).unwrap();
    }

    #[test]
    fn parses_layout_tables() {
        let cfg = parse(
            r##"
            [layout]
            gaps = 4
            border_width = 3
            border_color = "#ff0000"
            unfocused_border_color = "#222222"
            client_side_decorations = 1
            focus_follows_cursor = 0
            default_layout = "floating"

            [layout.stack]
            next_space_direction = "vertical"

            [layout.tile]
            master_count = 2
            master_ratio = 0.4
            next_space_direction = "horizontal"

            [layout.scroll]
            center_focused_column = "always"
            always_center_single_column = 0
            default_column_ratio = 0.7
            next_space_direction = "horizontal"
            "##,
        )
        .unwrap();
        assert_eq!(cfg.gaps, 4);
        assert_eq!(cfg.border_width, 3);
        assert_eq!(cfg.border_color, Color::rgb(1.0, 0.0, 0.0));
        let grey = 0x22 as f32 / 255.0;
        assert_eq!(cfg.unfocused_border_color, Color::rgb(grey, grey, grey));
        assert!(cfg.client_side_decorations);
        assert!(!cfg.focus_follows_cursor);
        assert_eq!(cfg.layout, "floating");
        assert_eq!(cfg.space_directions.stack, Flow::Vertical);
        assert_eq!(cfg.space_directions.tile, Flow::Horizontal);
        assert_eq!(cfg.space_directions.scroll, Flow::Horizontal);
        assert_eq!(cfg.master_count, 2);
        assert!((cfg.master_ratio - 0.4).abs() < 1e-9);
        assert_eq!(cfg.scroll.center_focused_column, ScrollCenter::Always);
        assert!(!cfg.scroll.always_center_single_column);
        assert!((cfg.scroll.default_column_ratio - 0.7).abs() < 1e-9);
    }

    #[test]
    fn parses_animations() {
        let cfg = parse(
            r#"
            [animations]
            enabled = 1
            animation_type_open = "zoom"
            animation_type_close = "slide"
            "#,
        )
        .unwrap();
        assert!(cfg.animations.enabled);
        assert_eq!(cfg.animations.open, AnimationStyle::Zoom);
        assert_eq!(cfg.animations.close, AnimationStyle::Slide);
    }

    #[test]
    fn flags_accept_bools_and_zero_one() {
        let cfg =
            parse("[animations]\nenabled = false\n[layout]\nclient_side_decorations = true\n")
                .unwrap();
        assert!(!cfg.animations.enabled);
        assert!(cfg.client_side_decorations);

        let cfg = parse("[animations]\nenabled = 0\n").unwrap();
        assert!(!cfg.animations.enabled);
    }

    #[test]
    fn rejects_flags_other_than_zero_or_one() {
        let err = parse("[animations]\nenabled = 2\n").unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
        assert!(err.to_string().contains("`animations.enabled`"));
    }

    #[test]
    fn rejects_unknown_animation_style() {
        let err = parse("[animations]\nanimation_type_open = \"bounce\"\n").unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
        assert!(err.to_string().contains("animations.animation_type_open"));
    }

    #[test]
    fn rejects_unknown_space_direction() {
        let err = parse("[layout.stack]\nnext_space_direction = \"diagonal\"\n").unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
        assert!(
            err.to_string()
                .contains("layout.stack.next_space_direction")
        );
    }

    #[test]
    fn parses_input() {
        let cfg = parse(
            r#"
            [input.keyboard]
            repeat_rate = 40
            repeat_delay = 300
            numlock_on = 1
            xkb_rules_layout = "us,de"

            [input.touchpad]
            disable = 1
            tap_to_click = 0
            tap_and_drag = 1
            drag_lock = 1
            natural_scrolling = 1
            disable_while_typing = 1
            swipe_min_threshold = 2

            [input.mouse]
            natural_scrolling = 1
            "#,
        )
        .unwrap();
        assert_eq!(cfg.input.repeat_rate, 40);
        assert_eq!(cfg.input.repeat_delay, 300);
        assert!(cfg.input.numlock_on);
        assert_eq!(cfg.input.xkb_rules_layout, "us,de");
        assert!(cfg.input.disable_touchpad);
        assert!(!cfg.input.tap_to_click);
        assert!(cfg.input.tap_and_drag);
        assert!(cfg.input.drag_lock);
        assert!(cfg.input.touchpad_natural_scrolling);
        assert!(cfg.input.touchpad_disable_while_typing);
        assert!((cfg.input.swipe_min_threshold - 2.0).abs() < 1e-9);
        assert!(cfg.input.mouse_natural_scrolling);
    }

    #[test]
    fn parses_outputs() {
        let cfg = parse(
            r#"
            [output.eDP-1]
            mode = "2560x1440@144"
            scale = 1.25
            transform = "90"
            position = { x = -1920, y = 0 }
            "#,
        )
        .unwrap();
        let output = &cfg.outputs["eDP-1"];
        assert_eq!(output.mode.as_deref(), Some("2560x1440@144"));
        assert_eq!(output.scale, Some(1.25));
        assert_eq!(output.transform.as_deref(), Some("90"));
        assert_eq!(output.position, Some((-1920, 0)));
    }

    #[test]
    fn rejects_non_positive_output_scale() {
        let cfg = parse("[output.DP-1]\nscale = 0\n").unwrap();
        assert!(matches!(validate(&cfg), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn parses_autostart() {
        let cfg = parse("[autostart]\nexec_once = [\"waybar\", \"swaybg\"]\n").unwrap();
        assert_eq!(cfg.autostart.exec_once, ["waybar", "swaybg"]);
    }

    #[test]
    fn parses_window_rules_as_table_and_array() {
        let one = parse(
            r#"
            [window_rule]
            app_id = "Nautilus"
            open_floating = 1
            size = [800, 500]
            offset_x = -90
            offset_y = -90
            "#,
        )
        .unwrap();
        assert_eq!(one.window_rules.len(), 1);
        let rule = &one.window_rules[0];
        assert_eq!(rule.app_id, ["Nautilus"]);
        assert!(rule.open_floating);
        assert_eq!(rule.size, Some((800, 500)));
        assert_eq!(rule.offset_x, Some(-90));
        assert_eq!(rule.offset_y, Some(-90));

        let many = parse(
            r#"
            [[window_rule]]
            app_id = "a"
            size = [400, 300]

            [[window_rule]]
            app_id = ["b", "c"]
            "#,
        )
        .unwrap();
        assert_eq!(many.window_rules.len(), 2);
        assert_eq!(many.window_rules[0].app_id, ["a"]);
        assert_eq!(many.window_rules[0].size, Some((400, 300)));
        assert_eq!(many.window_rules[1].app_id, ["b", "c"]);
        assert_eq!(many.window_rules[1].size, None);
    }

    #[test]
    fn rejects_a_rule_size_that_is_not_a_pair() {
        assert!(matches!(
            parse("[[window_rule]]\nsize = [800]\n"),
            Err(ConfigError::Parse(_))
        ));
    }

    #[test]
    fn parses_layer_rules() {
        let cfg = parse(
            r#"
            [layer_rule]
            animation_type_open = "zoom"
            layer_name = ["rofi", "alacritty"]
            "#,
        )
        .unwrap();
        assert_eq!(cfg.layer_rules.len(), 1);
        assert_eq!(
            cfg.layer_rules[0].animation_type_open.as_deref(),
            Some("zoom")
        );
        assert_eq!(cfg.layer_rules[0].layer_name, ["rofi", "alacritty"]);
    }

    #[test]
    fn parses_binds() {
        let cfg = parse(
            r#"
            [binds.default]
            "SUPER+q" = "killclient"

            [binds.locked]
            "XF86AudioMute" = "spawn:wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle"

            [mousebinds]
            "btn_left" = "moveresize,curmove"

            [gesturebinds]
            "left+3" = "focusdir:left"

            [axisbinds]
            "SUPER+UP" = "viewtoleft_have_client"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.binds.default["SUPER+q"], "killclient");
        assert_eq!(cfg.binds.default.len(), 1);
        assert_eq!(
            cfg.binds.locked["XF86AudioMute"],
            "spawn:wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle"
        );
        assert_eq!(cfg.mouse_binds["btn_left"], "moveresize,curmove");
        assert_eq!(cfg.gesture_binds["left+3"], "focusdir:left");
        assert_eq!(cfg.axis_binds["SUPER+UP"], "viewtoleft_have_client");
    }

    #[test]
    fn rejects_empty_bind_action() {
        let cfg = parse("[binds.default]\n\"SUPER+q\" = \"\"\n").unwrap();
        assert!(matches!(validate(&cfg), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn partial_config_inherits_defaults() {
        let cfg = parse("[layout]\ngaps = 2\n").unwrap();
        assert_eq!(cfg.gaps, 2);
        assert_eq!(cfg.border_width, Config::default().border_width);
        assert_eq!(cfg.layout, Config::default().layout);
        assert_eq!(cfg.input, Input::default());
    }

    #[test]
    fn ignores_unknown_keys() {
        let cfg = parse("future_thing = 42\n[layout]\nfuture = 1\n").unwrap();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn rejects_wrong_type() {
        assert!(matches!(
            parse("[layout]\ngaps = \"lots\""),
            Err(ConfigError::Parse(_))
        ));
    }

    #[test]
    fn rejects_legacy_flat_keys() {
        let err = parse("gaps = 4\n").unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
        assert!(err.to_string().contains("[layout]"));
    }

    #[test]
    fn rejects_negative_gaps() {
        let cfg = parse("[layout]\ngaps = -1").unwrap();
        assert!(matches!(validate(&cfg), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn rejects_unknown_layout() {
        let cfg = parse("[layout]\ndefault_layout = \"spiral\"").unwrap();
        let err = validate(&cfg).unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
        assert!(err.to_string().contains("default_layout"));
    }

    #[test]
    fn rejects_the_old_layout_key_name() {
        let err = parse("[layout]\nlayout = \"tile\"").unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
        assert!(err.to_string().contains("default_layout"));
    }

    #[test]
    fn rejects_out_of_range_ratio() {
        let cfg = parse("[layout.tile]\nmaster_ratio = 1.5").unwrap();
        assert!(matches!(validate(&cfg), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn rejects_negative_master_count() {
        let cfg = parse("[layout.tile]\nmaster_count = -1").unwrap();
        assert!(matches!(validate(&cfg), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn rejects_non_positive_column_ratio() {
        let cfg = parse("[layout.scroll]\ndefault_column_ratio = 0.0").unwrap();
        assert!(matches!(validate(&cfg), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn names_sections_that_are_not_applied_yet() {
        assert!(Config::default().pending_sections().is_empty());

        let cfg = parse(include_str!("../../resources/default-config.toml")).unwrap();
        let pending = cfg.pending_sections();
        for section in [
            "output",
            "autostart",
            "window_rule",
            "layer_rule",
            "binds",
            "mousebinds",
            "gesturebinds",
            "axisbinds",
        ] {
            assert!(pending.contains(&section), "missing {section}: {pending:?}");
        }
        // The example's `[input]` and pointer-focus setting match the built-in
        // defaults, so nothing is lost by not applying them; `[layout]` is.
        assert!(!pending.contains(&"input"));
        assert!(!pending.contains(&"layout"));

        let cfg = parse(
            "[input.keyboard]\nrepeat_rate = 40\n[layout]\nclient_side_decorations = 1\nfocus_follows_cursor = 0\n",
        )
        .unwrap();
        let pending = cfg.pending_sections();
        assert!(pending.contains(&"input"));
        assert!(pending.contains(&"layout.client_side_decorations"));
        assert!(pending.contains(&"layout.focus_follows_cursor"));
    }

    #[test]
    fn shipped_default_is_valid() {
        let config = parse(include_str!("../../resources/default-config.toml")).unwrap();
        validate(&config).unwrap();
        assert_eq!(config.layout, "tile");
        assert!(config.focus_follows_cursor);
        assert!(config.animations.enabled);
        assert_eq!(config.animations.open, AnimationStyle::Slide);
        assert_eq!(config.space_directions.stack, Flow::Horizontal);
        assert_eq!(config.space_directions.tile, Flow::Horizontal);
        assert_eq!(config.space_directions.scroll, Flow::Vertical);
        assert_eq!(config.master_count, 1);
        assert!((config.master_ratio - 0.6).abs() < 1e-9);
        assert_eq!(config.scroll.center_focused_column, ScrollCenter::Never);
        assert!(config.scroll.always_center_single_column);
        assert!((config.scroll.default_column_ratio - 0.5).abs() < 1e-9);
        assert_eq!(config.input.repeat_rate, 25);
        assert!(config.input.tap_to_click);
        assert!(!config.input.disable_touchpad);
        assert!(config.input.touchpad_natural_scrolling);
        assert!(!config.input.mouse_natural_scrolling);
        let output = &config.outputs["monitor"];
        assert_eq!(output.mode.as_deref(), Some("1920x1080@120.030"));
        assert_eq!(output.scale, Some(1.0));
        assert_eq!(output.position, Some((1280, 0)));
        assert_eq!(config.autostart.exec_once, ["waybar"]);
        assert_eq!(config.window_rules.len(), 2);
        let nautilus = &config.window_rules[0];
        assert_eq!(nautilus.app_id, ["Nautilus"]);
        assert!(nautilus.open_floating);
        assert_eq!(nautilus.size, Some((800, 500)));
        assert_eq!(nautilus.offset_x, Some(-90));
        assert_eq!(nautilus.offset_y, Some(-90));
        let browsers = &config.window_rules[1];
        assert_eq!(browsers.app_id, ["brave", "obsidian"]);
        assert_eq!(browsers.size, Some((1200, 800)));
        assert_eq!(browsers.offset_x, None);
        assert_eq!(config.layer_rules.len(), 1);
        assert_eq!(config.layer_rules[0].layer_name, ["rofi", "alacritty"]);
        assert_eq!(config.binds.default["SUPER+Return"], "spawn:alacritty");
        assert_eq!(config.binds.default["SUPER+1"], "view_space:1");
        assert!(config.binds.locked.contains_key("XF86AudioRaiseVolume"));
        assert_eq!(config.mouse_binds["btn_right"], "moveresize,curresize");
        assert_eq!(config.gesture_binds["right+4"], "viewprev_have_client");
        assert_eq!(config.axis_binds["SUPER+UP"], "viewtoleft_have_client");
    }
}

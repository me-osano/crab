//! TOML configuration: parsing, validation, and defaults.
//!
//! The config format is **TOML** (ADR-0003), parsed with `serde` and the `toml`
//! crate. It follows the mango-style section layout: `[input]`,
//! `[output.<name>]`, `[layout]`, `[animations]`, `[autostart]`,
//! `[window_rule]`, `[layer_rule]`, `[binds.default]`, `[binds.locked]`,
//! `[mousebinds]`, `[gesturebinds]` and `[axisbinds]`. The shipped example is
//! `resources/default-config.toml`; without `--config`, crab reads
//! [`default_path`] when that file exists. [`load`] resolves an `include` list
//! relative to the including file before parsing, while [`parse`] handles a
//! single already-merged document.
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
//! let config = crab::config::parse("[layout]\ngaps = 4\n").unwrap();
//! assert_eq!(config.gaps, 4);
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::core::{
    Color, LayoutKind, OutputMode, OutputPreference, OutputSpecError, OutputTransform, ScrollCenter,
};
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
    /// Default layout name for new spaces (`[layout] default_layout`).
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
    /// focus_follows_cursor`).
    pub focus_follows_cursor: bool,
    /// Keyboard, pointer and touchpad defaults (`[input]`). Parsed and
    /// validated; applied when the libinput backend lands (M4).
    pub input: Input,
    /// Per-connector settings (`[output.<name>]`). `position` is applied when
    /// an output is registered; mode, scale, and transform are applied by the
    /// DRM backend (M4).
    pub outputs: BTreeMap<String, Output>,
    /// Commands to run once at startup (`[autostart]`). Run by the backend once
    /// crab's Wayland socket exists.
    pub autostart: Autostart,
    /// Rules applied when a window maps (`[window_rule]` or `[[window_rule]]`).
    pub window_rules: Vec<WindowRule>,
    /// Rules applied to layer-shell surfaces (`[layer_rule]` or
    /// `[[layer_rule]]`). The matching rule's open animation is used by the
    /// compositor when the surface first maps.
    pub layer_rules: Vec<LayerRule>,
    /// Keyboard chords (`[binds.default]` and `[binds.locked]`), compiled into
    /// the session's bind table at startup. `locked` bindings are active all
    /// the time until a lock screen exists (M6).
    pub binds: Binds,
    /// Pointer-button binds (`[mousebinds]`), compiled at startup over the
    /// built-in Super+drag chords.
    pub mouse_binds: BTreeMap<String, String>,
    /// Swipe binds (`[gesturebinds]`), compiled at startup and fired when a
    /// libinput swipe ends.
    pub gesture_binds: BTreeMap<String, String>,
    /// Axis (scroll) binds (`[axisbinds]`), compiled at startup.
    pub axis_binds: BTreeMap<String, String>,
    /// Key modes/submaps (`[modes.<name>]`), each with its own bind tables.
    ///
    /// A mode's binds are layered over the default tables while it is the
    /// active mode; leaving the mode returns to the stack below it.
    pub modes: BTreeMap<String, KeyMode>,
}

/// One `[modes.<name>]` table: binds that only apply in that mode.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KeyMode {
    /// Keyboard chords active in this mode (`[modes.<name>.binds]`).
    pub binds: BTreeMap<String, String>,
    /// Pointer-button binds active in this mode (`[modes.<name>.mousebinds]`).
    pub mouse_binds: BTreeMap<String, String>,
    /// Scroll binds active in this mode (`[modes.<name>.axisbinds]`).
    pub axis_binds: BTreeMap<String, String>,
    /// Gesture binds active in this mode (`[modes.<name>.gesturebinds]`).
    pub gesture_binds: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            gaps: 8,
            border_width: 2,
            border_color: Color::from_hex("#7aa2f7").expect("static color is valid"),
            unfocused_border_color: Color::from_hex("#3b4261").expect("static color is valid"),
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
            modes: BTreeMap::new(),
        }
    }
}

impl Config {
    /// Keys that parsed but whose behavior is not implemented yet.
    ///
    /// A hand-written config can carry keys ahead of their milestone; this names
    /// them so startup can say which parts are not doing anything yet. Entries
    /// disappear as their features land (M4–M6). Only fields that are genuinely
    /// ignored are listed: a field that is applied is never reported, so a
    /// non-empty list always means some configured behavior is missing.
    #[must_use]
    pub fn pending_sections(&self) -> Vec<&'static str> {
        let defaults = Self::default();
        let mut pending = Vec::new();

        // Key repeat and the xkb layout are applied; NumLock is not.
        if self.input.numlock_on != defaults.input.numlock_on {
            pending.push("input.keyboard.numlock_on");
        }
        // Every touchpad field is parsed but not wired to a device yet.
        let touchpad = |input: &Input| {
            (
                input.disable_touchpad,
                input.tap_to_click,
                input.tap_and_drag,
                input.drag_lock,
                input.touchpad_natural_scrolling,
                input.touchpad_disable_while_typing,
                input.swipe_min_threshold,
            )
        };
        if touchpad(&self.input) != touchpad(&defaults.input) {
            pending.push("input.touchpad");
        }
        if self.input.mouse_natural_scrolling != defaults.input.mouse_natural_scrolling {
            pending.push("input.mouse.natural_scrolling");
        }

        // Position and scale are applied when an output is registered; choosing
        // a mode or transform needs a real output backend.
        if self
            .outputs
            .values()
            .any(|output| output.mode.is_some() || output.transform.is_some())
        {
            pending.push("output");
        }

        if self.client_side_decorations != defaults.client_side_decorations {
            pending.push("layout.client_side_decorations");
        }
        if self.space_directions != defaults.space_directions {
            pending.push("layout.*_next_space_direction");
        }
        if self.scroll.always_center_single_column != defaults.scroll.always_center_single_column {
            pending.push("layout.scroll.always_center_single_column");
        }
        // `x11 = false` correctly matches Wayland windows; `x11 = true` can never
        // match until XWayland lands (M9).
        if self.window_rules.iter().any(|rule| rule.x11 == Some(true)) {
            pending.push("window_rule.x11");
        }

        pending
    }

    /// Resolves every `[output.<name>]` table into the pure planner's form.
    ///
    /// This parses the mode and transform strings once, so the backend can hand
    /// the result straight to [`crate::core::OutputPlanner`] without knowing the
    /// config syntax.
    ///
    /// # Errors
    /// Fails if a mode or transform string is malformed. [`validate`] checks the
    /// same values at startup; this method is for callers that construct a
    /// [`Config`] directly or skip validation.
    pub fn output_preferences(
        &self,
    ) -> Result<BTreeMap<String, OutputPreference>, OutputSpecError> {
        self.outputs
            .iter()
            .map(|(name, output)| {
                let mode = output.mode.as_deref().map(OutputMode::parse).transpose()?;
                let transform = output
                    .transform
                    .as_deref()
                    .map(OutputTransform::parse)
                    .transpose()?;
                Ok((
                    name.clone(),
                    OutputPreference {
                        mode,
                        scale: output.scale,
                        transform,
                        position: output.position,
                    },
                ))
            })
            .collect()
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

    /// Parses a configured animation style name.
    #[must_use]
    pub fn from_name(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|style| style.name() == value)
    }

    fn parse(value: &str, key: &str) -> Result<Self, ConfigError> {
        Self::from_name(value)
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
    /// Commands to run once when the session starts (`exec_once`).
    pub exec_once: Vec<String>,
}

/// One `[window_rule]`/`[[window_rule]]` entry, matched when a window maps.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WindowRule {
    /// `app_id`s to match (`app_id`, one or many); empty matches any window.
    ///
    /// app ids must match exactly, because they are stable protocol
    /// identifiers rather than text shown to the user.
    pub app_id: Vec<String>,
    /// Title fragments to match (`title`, one or many); empty matches any.
    ///
    /// A title matches when it contains one of these fragments, ignoring case.
    /// Titles are human text and often carry a document or status suffix.
    pub title: Vec<String>,
    /// Output connector names to match (`output`, one or many).
    pub output: Vec<String>,
    /// Require the window to be floating when the rule is matched
    /// (`is_floating`).
    pub is_floating: Option<bool>,
    /// Require the window to be fullscreen when the rule is matched
    /// (`is_fullscreen`).
    pub is_fullscreen: Option<bool>,
    /// Require the window to be X11 (`x11 = true`) or Wayland (`x11 = false`).
    pub x11: Option<bool>,
    /// Open the window floating (`open_floating`).
    pub open_floating: bool,
    /// Initial size in logical pixels (`size = [width, height]`).
    pub size: Option<(i32, i32)>,
    /// Initial x offset in logical pixels (`offset_x`).
    pub offset_x: Option<i32>,
    /// Initial y offset in logical pixels (`offset_y`).
    pub offset_y: Option<i32>,
    /// Initial space, as a 1-based dynamic index (`space`).
    pub space: Option<u8>,
    /// Pin the window to every space (`pinned`).
    pub pinned: bool,
    /// Draw the window above other floating windows (`always_on_top`).
    pub always_on_top: bool,
    /// Exclude the window from keyboard focus (`no_focus`).
    pub no_focus: bool,
    /// Re-evaluate the rule when app_id/title change.
    pub dynamic: bool,
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
    /// A specific file could not be parsed (used when resolving `include`).
    #[error("failed to parse config {path}: {message}")]
    ParseFile {
        /// The file that failed.
        path: PathBuf,
        /// The parser's message, including its line/column.
        message: String,
    },
    /// The config parsed but is semantically invalid.
    #[error("invalid config: {0}")]
    Invalid(String),
}

/// The on-disk shape, before validation.
///
/// Every field is optional so a partial file inherits the defaults. Unknown keys
/// are ignored (`serde`'s default), which keeps older configs working as crab
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
    modes: BTreeMap<String, RawMode>,
}

/// One `[modes.<name>]` table.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawMode {
    binds: BTreeMap<String, String>,
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
    title: Option<OneOrMany<String>>,
    output: Option<OneOrMany<String>>,
    is_floating: Option<RawFlag>,
    is_fullscreen: Option<RawFlag>,
    x11: Option<RawFlag>,
    open_floating: Option<RawFlag>,
    size: Option<[i64; 2]>,
    offset_x: Option<i64>,
    offset_y: Option<i64>,
    space: Option<i64>,
    pinned: Option<RawFlag>,
    always_on_top: Option<RawFlag>,
    no_focus: Option<RawFlag>,
    dynamic: Option<RawFlag>,
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
        let flag = |value: Option<RawFlag>, field: &str| -> Result<bool, ConfigError> {
            match value {
                Some(flag) => flag.to_bool(&key(field)),
                None => Ok(false),
            }
        };
        let space = self
            .space
            .map(|value| {
                u8::try_from(value)
                    .ok()
                    .filter(|value| (1..=crate::core::SpaceSet::COUNT).contains(value))
                    .ok_or_else(|| {
                        ConfigError::Invalid(format!(
                            "`{}` must be within 1..={}",
                            key("space"),
                            crate::core::SpaceSet::COUNT
                        ))
                    })
            })
            .transpose()?;
        Ok(WindowRule {
            app_id: self.app_id.map(OneOrMany::into_vec).unwrap_or_default(),
            title: self.title.map(OneOrMany::into_vec).unwrap_or_default(),
            output: self.output.map(OneOrMany::into_vec).unwrap_or_default(),
            is_floating: match self.is_floating {
                Some(flag) => Some(flag.to_bool(&key("is_floating"))?),
                None => None,
            },
            is_fullscreen: match self.is_fullscreen {
                Some(flag) => Some(flag.to_bool(&key("is_fullscreen"))?),
                None => None,
            },
            x11: match self.x11 {
                Some(flag) => Some(flag.to_bool(&key("x11"))?),
                None => None,
            },
            open_floating: flag(self.open_floating, "open_floating")?,
            size,
            offset_x: self
                .offset_x
                .map(|value| checked_i32(value, &key("offset_x")))
                .transpose()?,
            offset_y: self
                .offset_y
                .map(|value| checked_i32(value, &key("offset_y")))
                .transpose()?,
            space,
            pinned: flag(self.pinned, "pinned")?,
            always_on_top: flag(self.always_on_top, "always_on_top")?,
            no_focus: flag(self.no_focus, "no_focus")?,
            dynamic: flag(self.dynamic, "dynamic")?,
        })
    }
}

impl RawLayerRule {
    fn into_rule(self) -> Result<LayerRule, ConfigError> {
        if let Some(value) = &self.animation_type_open {
            AnimationStyle::parse(value, "layer_rule.animation_type_open")?;
        }
        Ok(LayerRule {
            animation_type_open: self.animation_type_open,
            layer_name: self.layer_name,
        })
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
                .collect::<Result<_, _>>()?;
        }

        config.binds = Binds {
            default: self.binds.default,
            locked: self.binds.locked,
        };
        config.mouse_binds = self.mousebinds;
        config.gesture_binds = self.gesturebinds;
        config.axis_binds = self.axisbinds;
        config.modes = self
            .modes
            .into_iter()
            .map(|(name, mode)| {
                (
                    name,
                    KeyMode {
                        binds: mode.binds,
                        mouse_binds: mode.mousebinds,
                        axis_binds: mode.axisbinds,
                        gesture_binds: mode.gesturebinds,
                    },
                )
            })
            .collect();

        Ok(config)
    }
}

/// Top-level keys that moved under `[layout]` when crab adopted the
/// mango-style sections.
///
/// Unknown keys are ignored, so without this check an old config would lose
/// these settings silently; a clear error is better than a silent reset.
const LEGACY_TOP_LEVEL: [&str; 5] = [
    "gaps",
    "border_width",
    "border_color",
    "unfocused_border_color",
    "layout",
];

/// Parses a configuration from TOML text (does not validate).
///
/// `include` is resolved only by [`load`], which knows the file's directory;
/// text passed here must already be fully merged.
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
            if table.contains_key("include") {
                return Err(ConfigError::Invalid(
                    "`include` is resolved only when loading from a file; use `load`".to_owned(),
                ));
            }
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

/// Loads and validates a configuration from `path`, resolving `include`.
///
/// `include` entries are paths relative to the including file (absolute paths
/// are accepted too). An entry prefixed with `?` is optional: a missing file is
/// skipped instead of failing the load. Includes are resolved depth-first, in
/// order, and a file may include others; cycles are rejected. Tables merge
/// recursively, arrays concatenate, and a later value wins over an earlier one,
/// so the including file overrides what it includes.
///
/// # Errors
/// Returns a [`ConfigError`] if a file cannot be read, parsed, or validated, if
/// an include names a missing required file, or if includes form a cycle.
pub fn load(path: &Path) -> Result<Config, ConfigError> {
    let mut stack = Vec::new();
    let document = load_document(path, &mut stack)?;
    let text = toml::to_string(&document).map_err(|err| ConfigError::Parse(err.to_string()))?;
    let config = parse(&text)?;
    validate(&config)?;
    Ok(config)
}

/// One entry of an `include` list.
struct Include {
    path: PathBuf,
    optional: bool,
}

/// Reads `path` and recursively merges the files it includes.
fn load_document(path: &Path, stack: &mut Vec<PathBuf>) -> Result<toml::Value, ConfigError> {
    let canonical = std::fs::canonicalize(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if stack.contains(&canonical) {
        let chain: Vec<String> = stack
            .iter()
            .chain(std::iter::once(&canonical))
            .map(|path| path.display().to_string())
            .collect();
        return Err(ConfigError::Invalid(format!(
            "config include cycle: {}",
            chain.join(" -> ")
        )));
    }
    let text = std::fs::read_to_string(&canonical).map_err(|source| ConfigError::Io {
        path: canonical.clone(),
        source,
    })?;
    let mut document: toml::Value =
        toml::from_str(&text).map_err(|err| ConfigError::ParseFile {
            path: canonical.clone(),
            message: err.to_string(),
        })?;
    let includes = take_includes(&mut document, &canonical)?;

    stack.push(canonical);
    let mut merged = toml::Value::Table(toml::Table::new());
    for include in includes {
        if include.optional && !include.path.exists() {
            continue;
        }
        let included = load_document(&include.path, stack)?;
        merge_values(&mut merged, included);
    }
    stack.pop();

    merge_values(&mut merged, document);
    Ok(merged)
}

/// Removes the `include` key from `document` and resolves its entries.
fn take_includes(document: &mut toml::Value, path: &Path) -> Result<Vec<Include>, ConfigError> {
    let Some(table) = document.as_table_mut() else {
        return Ok(Vec::new());
    };
    let Some(raw) = table.remove("include") else {
        return Ok(Vec::new());
    };
    let entries = match raw {
        toml::Value::String(value) => vec![value],
        toml::Value::Array(values) => values
            .into_iter()
            .map(|value| match value {
                toml::Value::String(value) => Ok(value),
                other => Err(invalid_include(path, &other)),
            })
            .collect::<Result<Vec<_>, _>>()?,
        other => return Err(invalid_include(path, &other)),
    };
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    Ok(entries
        .into_iter()
        .map(|entry| {
            let (entry, optional) = match entry.strip_prefix('?') {
                Some(rest) => (rest, true),
                None => (entry.as_str(), false),
            };
            Include {
                path: base.join(entry),
                optional,
            }
        })
        .collect())
}

/// The error for an `include` value that is not a string or array of strings.
fn invalid_include(path: &Path, value: &toml::Value) -> ConfigError {
    ConfigError::Invalid(format!(
        "`include` in {} must be a string or an array of strings, not {}",
        path.display(),
        value.type_str()
    ))
}

/// Merges `overlay` into `base`: tables recursively, arrays by appending, and
/// every other value by replacement (so the overlay wins).
fn merge_values(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base), toml::Value::Table(overlay)) => {
            for (key, value) in overlay {
                match base.get_mut(&key) {
                    Some(existing) => merge_values(existing, value),
                    None => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (toml::Value::Array(base), toml::Value::Array(mut overlay)) => base.append(&mut overlay),
        (base, overlay) => *base = overlay,
    }
}

/// The config file crab reads when `--config` is not given.
///
/// This is `$XDG_CONFIG_HOME/crab/config.toml`, or
/// `$HOME/.config/crab/config.toml` when `XDG_CONFIG_HOME` is unset or not
/// absolute. Returns `None` when neither variable is set.
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("crab").join("config.toml"))
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
        if let Some(mode) = &output.mode {
            OutputMode::parse(mode)
                .map_err(|err| ConfigError::Invalid(format!("`output.{name}.mode`: {err}")))?;
        }
        if let Some(transform) = &output.transform {
            OutputTransform::parse(transform)
                .map_err(|err| ConfigError::Invalid(format!("`output.{name}.transform`: {err}")))?;
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
    for (name, mode) in &config.modes {
        if name.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "`[modes]` has an unnamed mode".to_owned(),
            ));
        }
        if name == "default" {
            return Err(ConfigError::Invalid(
                "`modes.default` is reserved; put default-mode binds under `[binds.default]`"
                    .to_owned(),
            ));
        }
        for (section, binds) in [
            ("binds", &mode.binds),
            ("mousebinds", &mode.mouse_binds),
            ("gesturebinds", &mode.gesture_binds),
            ("axisbinds", &mode.axis_binds),
        ] {
            for (chord, action) in binds {
                if chord.trim().is_empty() || action.trim().is_empty() {
                    return Err(ConfigError::Invalid(format!(
                        "`[modes.{name}.{section}]` has a bind with an empty chord or action"
                    )));
                }
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
    fn rejects_malformed_output_modes_and_transforms() {
        for config in [
            "[output.DP-1]\nmode = \"1920\"\n",
            "[output.DP-1]\nmode = \"1920x0\"\n",
            "[output.DP-1]\nmode = \"1920x1080@0\"\n",
            "[output.DP-1]\ntransform = \"sideways\"\n",
        ] {
            let cfg = parse(config).unwrap();
            assert!(
                matches!(validate(&cfg), Err(ConfigError::Invalid(_))),
                "accepted {config:?}"
            );
        }
    }

    #[test]
    fn resolves_output_preferences_for_the_planner() {
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
        let preferences = cfg.output_preferences().unwrap();
        let preference = &preferences["eDP-1"];
        let mode = preference.mode.unwrap();
        assert_eq!(mode.size(), (2560, 1440));
        assert_eq!(mode.refresh_mhz, Some(144_000));
        assert_eq!(preference.scale, Some(1.25));
        assert_eq!(preference.transform, Some(OutputTransform::Rotate90));
        assert_eq!(preference.position, Some((-1920, 0)));
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
            title = ["Save As", "Properties"]
            is_floating = 0
            is_fullscreen = false
            open_floating = 1
            size = [800, 500]
            offset_x = -90
            offset_y = -90
            space = 2
            pinned = 1
            always_on_top = 1
            no_focus = 0
            "#,
        )
        .unwrap();
        assert_eq!(one.window_rules.len(), 1);
        let rule = &one.window_rules[0];
        assert_eq!(rule.app_id, ["Nautilus"]);
        assert_eq!(rule.title, ["Save As", "Properties"]);
        assert_eq!(rule.is_floating, Some(false));
        assert_eq!(rule.is_fullscreen, Some(false));
        assert!(rule.open_floating);
        assert_eq!(rule.size, Some((800, 500)));
        assert_eq!(rule.offset_x, Some(-90));
        assert_eq!(rule.offset_y, Some(-90));
        assert_eq!(rule.space, Some(2));
        assert!(rule.pinned);
        assert!(rule.always_on_top);
        assert!(!rule.no_focus);

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
    fn rejects_a_window_rule_space_out_of_range() {
        for space in ["0", "33", "-1"] {
            let error = parse(&format!("[[window_rule]]\nspace = {space}\n")).unwrap_err();
            assert!(
                matches!(&error, ConfigError::Invalid(message) if message.contains("1..=32")),
                "unexpected error for space {space}: {error}"
            );
        }
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
    fn rejects_an_unknown_layer_open_animation() {
        let err = parse("[layer_rule]\nanimation_type_open = \"bounce\"\n").unwrap_err();
        assert!(err.to_string().contains("layer_rule.animation_type_open"));
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
    fn modes_parse_and_layer_their_tables() {
        let cfg = parse(
            "[modes.resize.binds]\n\"h\" = \"preset_width\"\n[modes.resize.mousebinds]\n\"SUPER+btn_left\" = \"toggle_floating\"\n[modes.resize.axisbinds]\n\"SUPER+UP\" = \"viewprev\"\n[modes.resize.gesturebinds]\n\"left+3\" = \"focusdir:left\"\n",
        )
        .unwrap();
        validate(&cfg).unwrap();
        let mode = &cfg.modes["resize"];
        assert_eq!(mode.binds["h"], "preset_width");
        assert_eq!(mode.mouse_binds["SUPER+btn_left"], "toggle_floating");
        assert_eq!(mode.axis_binds["SUPER+UP"], "viewprev");
        assert_eq!(mode.gesture_binds["left+3"], "focusdir:left");
    }

    #[test]
    fn reserved_default_mode_is_rejected() {
        let cfg = parse("[modes.default.binds]\n\"h\" = \"preset_width\"\n").unwrap();
        assert!(matches!(validate(&cfg), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn empty_mode_bind_is_rejected() {
        let cfg = parse("[modes.resize.binds]\n\"h\" = \"\"\n").unwrap();
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
    fn rejects_removed_layout_modes() {
        for name in ["monocle", "vertical-scroll"] {
            let cfg = parse(&format!("[layout]\ndefault_layout = \"{name}\"")).unwrap();
            assert!(
                matches!(validate(&cfg), Err(ConfigError::Invalid(_))),
                "{name} should no longer be a valid layout"
            );
        }
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
        // The example sets a mode, which needs a real output backend.
        assert!(pending.contains(&"output"), "missing output: {pending:?}");
        assert!(!pending.contains(&"layer_rule"));
        assert!(!pending.contains(&"mousebinds"));
        assert!(!pending.contains(&"axisbinds"));
        assert!(!pending.contains(&"gesturebinds"));
        assert!(!pending.contains(&"modes"));
        // The example's `[input]` matches the built-in defaults, so nothing is
        // lost by not applying it; `[layout]` and the animation styles are
        // applied.
        assert!(!pending.iter().any(|name| name.starts_with("input")));
        assert!(!pending.contains(&"layout"));
        assert!(!pending.iter().any(|name| name.starts_with("animations")));

        // Key repeat and the xkb layout are applied, so they are not pending.
        let cfg = parse("[input.keyboard]\nrepeat_rate = 40\nxkb_rules_layout = \"de\"\n").unwrap();
        assert!(
            !cfg.pending_sections()
                .iter()
                .any(|name| name.starts_with("input"))
        );

        let cfg = parse("[input.keyboard]\nnumlock_on = 1\n").unwrap();
        assert!(
            cfg.pending_sections()
                .contains(&"input.keyboard.numlock_on")
        );

        let cfg = parse("[input.touchpad]\ntap_to_click = 0\n").unwrap();
        assert!(cfg.pending_sections().contains(&"input.touchpad"));

        let cfg = parse("[input.mouse]\nnatural_scrolling = 1\n").unwrap();
        assert!(
            cfg.pending_sections()
                .contains(&"input.mouse.natural_scrolling")
        );

        let cfg =
            parse("[layout]\nclient_side_decorations = 1\nfocus_follows_cursor = 0\n").unwrap();
        let pending = cfg.pending_sections();
        assert!(pending.contains(&"layout.client_side_decorations"));
        assert!(!pending.contains(&"layout"), "{pending:?}");

        let cfg = parse("[layout.scroll]\nalways_center_single_column = 0\n").unwrap();
        assert!(
            cfg.pending_sections()
                .contains(&"layout.scroll.always_center_single_column")
        );

        let cfg = parse("[[window_rule]]\nx11 = true\n").unwrap();
        assert!(cfg.pending_sections().contains(&"window_rule.x11"));
        // `x11 = false` matches Wayland windows today, so it is not pending.
        let cfg = parse("[[window_rule]]\nx11 = false\n").unwrap();
        assert!(!cfg.pending_sections().contains(&"window_rule.x11"));

        // A position/scale-only output table is applied when the output is
        // registered, so nothing is pending for it.
        let cfg = parse("[output.DP-1]\nposition = { x = 1920, y = 0 }\nscale = 1.5\n").unwrap();
        assert!(!cfg.pending_sections().contains(&"output"));
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
        assert_eq!(
            config.mouse_binds["SUPER+btn_right"],
            "moveresize,curresize"
        );
        assert_eq!(config.gesture_binds["right+4"], "viewprev_have_client");
        assert_eq!(config.axis_binds["SUPER+UP"], "viewtoleft_have_client");
    }

    /// A scratch directory that removes itself at the end of a test.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("crab-config-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn write(&self, name: &str, text: &str) -> PathBuf {
            let path = self.0.join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&path, text).unwrap();
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn includes_merge_in_order_and_the_main_file_wins() {
        let dir = TempDir::new("merge");
        dir.write(
            "base.toml",
            "[layout]\ngaps = 12\nborder_width = 1\n[autostart]\nexec_once = [\"base\"]\n",
        );
        dir.write(
            "theme.toml",
            "[layout]\nborder_width = 3\n[autostart]\nexec_once = [\"theme\"]\n",
        );
        let main = dir.write(
            "config.toml",
            "include = [\"base.toml\", \"theme.toml\"]\n[layout]\ngaps = 4\n",
        );

        let config = load(&main).unwrap();
        assert_eq!(config.gaps, 4, "the including file overrides an include");
        assert_eq!(
            config.border_width, 3,
            "later includes override earlier ones"
        );
        assert_eq!(
            config.autostart.exec_once,
            ["base", "theme"],
            "arrays concatenate"
        );
    }

    #[test]
    fn a_single_include_string_is_accepted() {
        let dir = TempDir::new("single");
        dir.write("base.toml", "[layout]\ngaps = 5\n");
        let main = dir.write("config.toml", "include = \"base.toml\"\n");
        assert_eq!(load(&main).unwrap().gaps, 5);
    }

    #[test]
    fn includes_are_relative_to_the_including_file() {
        let dir = TempDir::new("nested");
        dir.write("sub/deeper.toml", "[layout]\nborder_width = 7\n");
        dir.write("sub/nested.toml", "include = [\"deeper.toml\"]\n");
        let main = dir.write("config.toml", "include = [\"sub/nested.toml\"]\n");
        assert_eq!(load(&main).unwrap().border_width, 7);
    }

    #[test]
    fn optional_includes_may_be_missing() {
        let dir = TempDir::new("optional");
        let main = dir.write(
            "config.toml",
            "include = [\"?missing.toml\"]\n[layout]\ngaps = 6\n",
        );
        assert_eq!(load(&main).unwrap().gaps, 6);
    }

    #[test]
    fn a_missing_required_include_fails() {
        let dir = TempDir::new("missing");
        let main = dir.write("config.toml", "include = [\"missing.toml\"]\n");
        let err = load(&main).unwrap_err();
        assert!(matches!(err, ConfigError::Io { .. }), "{err:?}");
    }

    #[test]
    fn include_cycles_are_rejected() {
        let dir = TempDir::new("cycle");
        dir.write("a.toml", "include = [\"b.toml\"]\n");
        let a = dir.write("b.toml", "include = [\"a.toml\"]\n");
        let err = load(&a).unwrap_err();
        assert!(err.to_string().contains("cycle"), "{err}");
    }

    #[test]
    fn a_syntax_error_in_an_include_names_its_file() {
        let dir = TempDir::new("bad");
        dir.write("bad.toml", "[layout\ngaps = 1\n");
        let main = dir.write("config.toml", "include = [\"bad.toml\"]\n");
        let err = load(&main).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("bad.toml"), "{message}");
        assert!(message.contains("failed to parse config"), "{message}");
    }

    #[test]
    fn parse_rejects_include_because_it_has_no_base_directory() {
        let err = parse("include = [\"base.toml\"]\n").unwrap_err();
        assert!(err.to_string().contains("include"), "{err}");
    }

    #[test]
    fn include_must_be_a_string_or_an_array_of_strings() {
        let dir = TempDir::new("shape");
        let main = dir.write("config.toml", "include = 3\n");
        let err = load(&main).unwrap_err();
        assert!(err.to_string().contains("include"), "{err}");
    }
}

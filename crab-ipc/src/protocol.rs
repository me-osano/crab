//! Wire types for the crab IPC protocol.
//!
//! The schema is versioned (ADR-0005): during `0.x` a breaking change bumps
//! [`PROTOCOL_VERSION`]; from `1.0` the schema is frozen and **additive-only**
//! within a major version. Unknown events/fields must be ignored by clients.

use serde::{Deserialize, Serialize};

/// The protocol version understood by this crate.
///
/// Version 2 adds the canonical action vocabulary (the same enum the compositor
/// binds use), richer output/window payloads, and the initial event snapshot.
pub const PROTOCOL_VERSION: u32 = 2;

/// A request sent from a client to the compositor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Request {
    /// `version` — report the compositor name and protocol version.
    Version,
    /// `outputs` — list outputs.
    Outputs,
    /// `spaces` — list the spaces that currently exist on each output.
    Spaces,
    /// `windows` — list managed windows.
    Windows,
    /// `focused-window` — describe the focused window, if any.
    FocusedWindow,
    /// `layouts` — list the available layout families and modes.
    Layouts,
    /// `dispatch` — run a named action (the same set binds use).
    Dispatch {
        /// The action to perform.
        action: Action,
    },
    /// `subscribe` — switch the connection to an event stream.
    ///
    /// The compositor replies with a [`Event::Snapshot`] followed by deltas. A
    /// subscribed connection is event-only; use a second connection to dispatch
    /// actions while watching.
    Subscribe,
    /// `config-reload` — re-read the active config file and apply it.
    ///
    /// The reply is [`Response::Ok`] on success or [`Response::Error`] when the
    /// new file is invalid. A failed reload keeps the previous configuration.
    ConfigReload,
    /// `config-validate` — parse and validate the active config file without
    /// applying it.
    ConfigValidate,
}

/// An action, shared by keybinds and the IPC `dispatch` request.
///
/// The variant names below are the canonical vocabulary. Config action strings
/// are parsed into exactly this type, so a new action cannot land on only one
/// side of the IPC boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    /// Exit the compositor.
    Quit,
    /// Spawn a command.
    Spawn {
        /// argv of the command to run.
        command: Vec<String>,
    },
    /// Close the focused window.
    CloseWindow,
    /// Focus a window by id.
    FocusWindow {
        /// Target window id.
        id: u64,
    },
    /// Focus the nearest window in a direction.
    FocusDirection {
        /// Direction to move focus.
        direction: Direction,
    },
    /// Move the focused window toward a direction in the layout order.
    MoveWindow {
        /// Direction to move the window.
        direction: Direction,
    },
    /// Focus the next (`true`) or previous (`false`) window.
    FocusCycle {
        /// Whether to advance or go back in the focus order.
        forward: bool,
    },
    /// Switch the keyboard layout forward (`true`) or backward.
    SwitchKeyboardLayout {
        /// Whether to advance or go back in the layout list.
        forward: bool,
    },
    /// Show a single space (1-based dynamic index).
    ViewSpace {
        /// Space index.
        index: u8,
    },
    /// Move the focused window to a space (1-based dynamic index).
    MoveToSpace {
        /// Space index.
        index: u8,
    },
    /// Toggle a space's visibility in the current view (1-based dynamic index).
    ToggleSpaceView {
        /// Space index.
        index: u8,
    },
    /// Toggle a space on the focused window (1-based dynamic index).
    ToggleSpace {
        /// Space index.
        index: u8,
    },
    /// Step the view one space in a direction (`-1` previous, `1` next).
    ViewSpaceStep {
        /// Relative space step.
        step: i8,
    },
    /// Step the view to the next occupied space in a direction.
    ViewSpaceHaveClient {
        /// Relative space step.
        step: i8,
    },
    /// Swap to the previously-viewed space set.
    ToggleBack,
    /// Cycle the focused space's layout mode within its family.
    CycleLayout,
    /// Cycle the focused space to the next layout family's default mode.
    CycleLayoutFamily,
    /// Set the focused space's layout by mode name (a family name is accepted
    /// as an alias for that family's default mode).
    SetLayout {
        /// Layout mode or family name.
        name: String,
    },
    /// Toggle floating for the focused window.
    ToggleFloating,
    /// Toggle fullscreen for the focused window.
    ToggleFullscreen,
    /// Toggle maximized for the focused window.
    ToggleMaximized,
    /// Cycle the focused scroll column's width through the presets.
    PresetWidth,
    /// Scroll the focused scroll column to the middle of the view.
    CenterColumn,
    /// Move the focused scroll column one place in `direction`.
    MoveColumn {
        /// Direction along the strip.
        direction: Direction,
    },
    /// Move the focused window into the neighbouring column in `direction`.
    MoveWindowToColumn {
        /// Direction toward the target column.
        direction: Direction,
    },
    /// Consume the focused window into the column to its left.
    ConsumeWindowIntoColumn,
    /// Expel the focused window into a new column to its right.
    ExpelWindowFromColumn,
    /// Widen the focused scroll column to fill the view.
    ExpandColumnToAvailableWidth,
    /// Set the focused scroll column's width (`0.5`, `proportion 0.5`, or `800px`).
    SetColumnWidth {
        /// Width expression, parsed by the compositor.
        width: String,
    },
    /// Reload the configuration.
    ///
    /// A failed reload leaves the previous configuration in effect and is
    /// reported as [`Response::Error`].
    ReloadConfig,
    /// Enter a key mode/submap.
    EnterMode {
        /// Mode name.
        name: String,
    },
    /// Leave the current key mode.
    LeaveMode,
    /// Pop one mode from the mode stack.
    PopMode,
}

/// A direction for focus or window movement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// Left.
    Left,
    /// Right.
    Right,
    /// Up.
    Up,
    /// Down.
    Down,
}

/// A response to a [`Request`], or an acknowledgement for a stream.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Response {
    /// Reply to [`Request::Version`].
    Version {
        /// Compositor binary name.
        name: String,
        /// Protocol version.
        version: u32,
    },
    /// A bare success with no payload.
    Ok,
    /// A failure.
    Error {
        /// Human-readable reason.
        message: String,
    },
    /// Reply to [`Request::Outputs`].
    Outputs {
        /// All outputs.
        outputs: Vec<OutputInfo>,
    },
    /// Reply to [`Request::Spaces`].
    Spaces {
        /// All spaces across outputs.
        spaces: Vec<SpaceInfo>,
    },
    /// Reply to [`Request::Windows`] / [`Request::FocusedWindow`].
    Windows {
        /// Matching windows.
        windows: Vec<WindowInfo>,
    },
    /// Reply to [`Request::Layouts`].
    Layouts {
        /// Available layout modes, including family-name aliases.
        layouts: Vec<String>,
        /// Available layout families.
        families: Vec<String>,
    },
}

/// A rectangle in logical compositor coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RectInfo {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub width: f64,
    /// Height.
    pub height: f64,
}

/// An output (monitor).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutputInfo {
    /// Stable output id.
    pub id: u64,
    /// Connector name (e.g. `DP-1`).
    pub name: String,
    /// Human-readable description (make/model, or the connector name until the
    /// backend reports physical properties).
    pub description: String,
    /// Logical geometry in the compositor's coordinate space.
    pub geometry: RectInfo,
    /// Output scale factor.
    pub scale: f64,
    /// Whether this output currently owns focus.
    pub focused: bool,
    /// 1-based indices of the spaces currently shown on this output.
    pub visible_spaces: Vec<u8>,
    /// Layout mode that arranges the current view.
    pub layout: String,
}

/// A space and its state on some output.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpaceInfo {
    /// Stable id of the output this space belongs to.
    pub output: u64,
    /// 1-based space index.
    pub index: u8,
    /// Human-readable name, if the space is named.
    pub name: Option<String>,
    /// Whether the space is currently shown.
    pub visible: bool,
    /// Whether the space contains an urgent window.
    pub urgent: bool,
}

/// A managed window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowInfo {
    /// Stable window id.
    pub id: u64,
    /// Window title.
    pub title: String,
    /// Application id (Wayland `app_id` or X11 class).
    pub app_id: String,
    /// Output the window is arranged on.
    pub output: u64,
    /// 1-based indices of the spaces the window is on.
    pub spaces: Vec<u8>,
    /// Whether the window is focused.
    pub focused: bool,
    /// Whether the window requested attention.
    pub urgent: bool,
    /// Whether the window floats above the layout.
    pub floating: bool,
    /// Whether the window is fullscreen.
    pub fullscreen: bool,
    /// Whether the window is maximized.
    pub maximized: bool,
    /// Logical geometry (position and size) of the window.
    pub geometry: RectInfo,
}

/// An event delivered on a [`Request::Subscribe`] stream.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum Event {
    /// Initial state for a new subscription, followed by deltas.
    Snapshot {
        /// All outputs.
        outputs: Vec<OutputInfo>,
        /// All existing spaces.
        spaces: Vec<SpaceInfo>,
        /// All managed windows.
        windows: Vec<WindowInfo>,
        /// The focused window id, if any.
        focused: Option<u64>,
        /// The active keyboard layout, if one is configured.
        #[serde(default)]
        keyboard_layout: Option<String>,
        /// The last config reload error, if one is currently shown.
        #[serde(default)]
        config_error: Option<String>,
    },
    /// A window appeared.
    WindowOpened {
        /// The new window.
        window: WindowInfo,
    },
    /// A window disappeared.
    WindowClosed {
        /// The closed window id.
        id: u64,
    },
    /// A window's metadata, spaces, or geometry changed.
    WindowChanged {
        /// The window's new state.
        window: WindowInfo,
    },
    /// Focus changed (`None` when nothing is focused).
    WindowFocused {
        /// The newly focused window id.
        id: Option<u64>,
    },
    /// An output's visible space set changed.
    SpaceViewChanged {
        /// Output id.
        output: u64,
        /// The spaces now visible.
        visible: Vec<u8>,
    },
    /// An output's layout changed.
    LayoutChanged {
        /// Output id.
        output: u64,
        /// New layout name.
        layout: String,
    },
    /// An output appeared.
    OutputAdded {
        /// The new output.
        output: OutputInfo,
    },
    /// An output disappeared.
    OutputRemoved {
        /// The removed output id.
        id: u64,
    },
    /// An output's geometry, scale, focus, or description changed.
    OutputChanged {
        /// The output's new state.
        output: OutputInfo,
    },
    /// The keyboard layout changed.
    KeyboardLayoutChanged {
        /// New keyboard layout name.
        layout: String,
    },
    /// The configuration was reloaded successfully.
    ConfigReloaded,
    /// The configuration failed to reload; the previous one is still in effect.
    ConfigError {
        /// Human-readable reason.
        message: String,
    },
    /// A window's urgency changed.
    Urgent {
        /// Window id.
        window: u64,
        /// Whether the window now requests attention.
        urgent: bool,
    },
    /// An event this build does not understand.
    ///
    /// Clients must ignore it: a newer compositor may add event kinds within a
    /// major version, and an old client must keep working.
    #[serde(other)]
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actions() -> Vec<Action> {
        vec![
            Action::Quit,
            Action::Spawn {
                command: vec!["foot".to_owned()],
            },
            Action::CloseWindow,
            Action::FocusWindow { id: 3 },
            Action::FocusDirection {
                direction: Direction::Left,
            },
            Action::MoveWindow {
                direction: Direction::Down,
            },
            Action::FocusCycle { forward: true },
            Action::SwitchKeyboardLayout { forward: true },
            Action::ViewSpace { index: 2 },
            Action::MoveToSpace { index: 4 },
            Action::ToggleSpaceView { index: 1 },
            Action::ToggleSpace { index: 3 },
            Action::ViewSpaceStep { step: -1 },
            Action::ViewSpaceHaveClient { step: 1 },
            Action::ToggleBack,
            Action::CycleLayout,
            Action::CycleLayoutFamily,
            Action::SetLayout {
                name: "fair".to_owned(),
            },
            Action::ToggleFloating,
            Action::ToggleFullscreen,
            Action::ToggleMaximized,
            Action::PresetWidth,
            Action::CenterColumn,
            Action::MoveColumn {
                direction: Direction::Left,
            },
            Action::MoveWindowToColumn {
                direction: Direction::Right,
            },
            Action::ConsumeWindowIntoColumn,
            Action::ExpelWindowFromColumn,
            Action::ExpandColumnToAvailableWidth,
            Action::SetColumnWidth {
                width: "0.5".to_owned(),
            },
            Action::ReloadConfig,
            Action::EnterMode {
                name: "resize".to_owned(),
            },
            Action::LeaveMode,
            Action::PopMode,
        ]
    }

    #[test]
    fn every_action_round_trips_as_kebab_case() {
        for action in actions() {
            let json = serde_json::to_string(&action).unwrap();
            let back: Action = serde_json::from_str(&json).unwrap();
            assert_eq!(back, action, "{json}");
        }
    }

    #[test]
    fn actions_use_the_canonical_wire_names() {
        assert_eq!(
            serde_json::to_string(&Action::FocusDirection {
                direction: Direction::Left
            })
            .unwrap(),
            r#"{"focus-direction":{"direction":"left"}}"#
        );
        assert_eq!(
            serde_json::to_string(&Action::CloseWindow).unwrap(),
            r#""close-window""#
        );
        assert_eq!(
            serde_json::to_string(&Action::EnterMode {
                name: "resize".to_owned()
            })
            .unwrap(),
            r#"{"enter-mode":{"name":"resize"}}"#
        );
        assert_eq!(
            serde_json::to_string(&Action::LeaveMode).unwrap(),
            r#""leave-mode""#
        );
        assert_eq!(
            serde_json::to_string(&Action::PopMode).unwrap(),
            r#""pop-mode""#
        );
    }

    #[test]
    fn requests_and_snapshots_round_trip() {
        let request: Request =
            serde_json::from_str(r#"{"type":"dispatch","action":"toggle-floating"}"#).unwrap();
        assert_eq!(
            request,
            Request::Dispatch {
                action: Action::ToggleFloating
            }
        );

        let event = Event::Snapshot {
            outputs: vec![],
            spaces: vec![],
            windows: vec![],
            focused: None,
            keyboard_layout: None,
            config_error: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(
            json,
            r#"{"event":"snapshot","outputs":[],"spaces":[],"windows":[],"focused":null,"keyboard_layout":null,"config_error":null}"#
        );
        assert_eq!(serde_json::from_str::<Event>(&json).unwrap(), event);
    }

    #[test]
    fn unknown_events_are_ignored() {
        let event: Event =
            serde_json::from_str(r#"{"event":"added-in-a-future-version","payload":1}"#).unwrap();
        assert_eq!(event, Event::Unknown);
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let request: Request =
            serde_json::from_str(r#"{"type":"version","added-in-a-future-version":true}"#).unwrap();
        assert_eq!(request, Request::Version);
    }
}

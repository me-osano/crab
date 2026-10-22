//! Wire types for the crab IPC protocol.
//!
//! The schema is versioned and **additive-only** within a major version: new
//! requests, responses, and event fields may be added, but existing meanings are
//! never changed. Unknown events/fields must be ignored by clients.

use serde::{Deserialize, Serialize};

/// The protocol version understood by this crate.
pub const PROTOCOL_VERSION: u32 = 1;

/// A request sent from a client to the compositor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Request {
    /// `version` — report the compositor name and protocol version.
    Version,
    /// `outputs` — list outputs.
    Outputs,
    /// `tags` — list the tags visible on each output.
    Tags,
    /// `windows` — list managed windows.
    Windows,
    /// `focused-window` — describe the focused window, if any.
    FocusedWindow,
    /// `layouts` — list the names of available layouts.
    Layouts,
    /// `dispatch` — run a named action (the same set binds use).
    Dispatch {
        /// The action to perform.
        action: Action,
    },
    /// `subscribe` — switch the connection to an event stream.
    Subscribe,
}

/// An action, shared by keybinds and the IPC `dispatch` request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    /// Show a single tag (1-based).
    ViewTag {
        /// Tag index.
        index: u8,
    },
    /// Move the focused window to a tag (1-based).
    MoveToTag {
        /// Tag index.
        index: u8,
    },
    /// Toggle a tag's visibility (1-based).
    ToggleTag {
        /// Tag index.
        index: u8,
    },
    /// Switch to the next layout on the focused tag.
    CycleLayout,
    /// Set the layout on the focused tag by name.
    SetLayout {
        /// Layout name.
        name: String,
    },
    /// Toggle floating for the focused window.
    ToggleFloating,
    /// Toggle fullscreen for the focused window.
    ToggleFullscreen,
    /// Reload the configuration.
    ReloadConfig,
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
    /// Reply to [`Request::Tags`].
    Tags {
        /// All tags across outputs.
        tags: Vec<TagInfo>,
    },
    /// Reply to [`Request::Windows`] / [`Request::FocusedWindow`].
    Windows {
        /// Matching windows.
        windows: Vec<WindowInfo>,
    },
    /// Reply to [`Request::Layouts`].
    Layouts {
        /// Available layout names.
        layouts: Vec<String>,
    },
}

/// An output (monitor).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutputInfo {
    /// Stable output id.
    pub id: u64,
    /// Connector name (e.g. `DP-1`).
    pub name: String,
    /// Whether this output currently has focus.
    pub focused: bool,
}

/// A tag and its state on some output.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TagInfo {
    /// 1-based tag index.
    pub index: u8,
    /// Whether the tag is currently shown.
    pub active: bool,
    /// Whether the tag contains an urgent window.
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
    /// Whether the window is focused.
    pub focused: bool,
    /// Tags the window is on.
    pub tags: Vec<u8>,
}

/// An event delivered on a [`Request::Subscribe`] stream.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum Event {
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
    /// Focus changed (`None` when nothing is focused).
    WindowFocused {
        /// The newly focused window id.
        id: Option<u64>,
    },
    /// An output's visible tag set changed.
    TagViewChanged {
        /// Output id.
        output: u64,
        /// The tags now visible.
        active: Vec<u8>,
    },
    /// An output's layout changed.
    LayoutChanged {
        /// Output id.
        output: u64,
        /// New layout name.
        layout: String,
    },
    /// The configuration was reloaded successfully.
    ConfigReloaded,
    /// The configuration failed to reload; the previous one is still in effect.
    ConfigError {
        /// Human-readable reason.
        message: String,
    },
}

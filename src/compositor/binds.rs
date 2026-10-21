//! Compiling `[binds.*]` into a lookup table, and the actions they run.
//!
//! The config authors a chord as `"SUPER+SHIFT+Up" = "move_client:up"`. At
//! startup the table turns each chord into a modifier set plus an xkb keysym and
//! each action string into an [`Action`], then input looks up the keysym the
//! keyboard reports (its *base* symbol, so shift does not change which key
//! matches; the modifiers themselves are part of the chord).
//!
//! Built-in binds keep crawm usable with no config. Config binds are layered on
//! top and replace a built-in with the same chord, so the shipped example can
//! say `"SUPER+space" = "spawn:rofi -show drun"` and take that chord over.
//!
//! Actions whose feature has not landed yet (`toggle_overview`, `minimized`,
//! scratchpads, gestures) are accepted by the config but skipped here with a
//! startup warning: a bind for a future feature must not stop the session from
//! starting, and it must not fail `--validate`.

use std::collections::BTreeMap;

use crawm::core::{Direction, SpaceSet};
use smithay::input::keyboard::{Keysym, ModifiersState, keysyms, xkb};

use crate::compositor::spawn::parse_command;

/// A compositor action produced by a keybind or pointer grab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    /// View a single space (1-based dynamic index).
    ViewSpace(u8),
    /// Move the focused window to a space.
    MoveToSpace(u8),
    /// Toggle a space in the current view.
    ToggleSpaceView(u8),
    /// Toggle a space on the focused window.
    ToggleSpace(u8),
    /// Step the view one space left/normal or right/next.
    ViewSpaceStep(i8),
    /// Swap to the previously-viewed space set.
    ToggleBack,
    /// Directional focus.
    Direction(Direction),
    /// Move the focused window toward a direction in the layout order.
    MoveWindow(Direction),
    /// Focus the next (`true`) or previous (`false`) window.
    CycleFocus(bool),
    /// Cycle the space's layout mode within its family.
    CycleLayoutMode,
    /// Cycle the space's layout family (to the next family's default mode).
    CycleLayoutFamily,
    /// Flip the focused window between tiled and floating.
    ToggleFloating,
    /// Flip fullscreen for the focused window.
    ToggleFullscreen,
    /// Flip maximized for the focused window.
    ToggleMaximized,
    /// Grow (`true`) or shrink the master area.
    MasterRatio(bool),
    /// Add (`true`) or remove a master slot.
    MasterCount(bool),
    /// Cycle the focused scroll column's width through the presets.
    PresetWidth,
    /// Scroll the focused scroll column to the middle of the view.
    CenterColumn,
    /// Run a command (already split into argv).
    Spawn(Vec<String>),
    /// Ask the focused window to close.
    Close,
}

/// The modifier keys a chord requires, independent of lock state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
struct Mods(u8);

impl Mods {
    const CTRL: u8 = 1 << 0;
    const ALT: u8 = 1 << 1;
    const SHIFT: u8 = 1 << 2;
    const LOGO: u8 = 1 << 3;

    fn from_state(state: &ModifiersState) -> Self {
        let mut mods = Self::default();
        mods.set(Self::CTRL, state.ctrl);
        mods.set(Self::ALT, state.alt);
        mods.set(Self::SHIFT, state.shift);
        mods.set(Self::LOGO, state.logo);
        mods
    }

    fn set(&mut self, bit: u8, on: bool) {
        if on {
            self.0 |= bit;
        }
    }

    fn insert(&mut self, bit: u8) {
        self.0 |= bit;
    }

    /// Modifier names as written in a chord. `Super` and `Logo` are the same
    /// key, as are `Mod4`/`Super` and `Mod1`/`Alt` on the usual xkb layout.
    fn parse(name: &str) -> Option<u8> {
        match name.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => Some(Self::CTRL),
            "alt" | "mod1" => Some(Self::ALT),
            "shift" => Some(Self::SHIFT),
            "super" | "logo" | "win" | "mod4" => Some(Self::LOGO),
            _ => None,
        }
    }
}

/// The key a bind fires on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BindKey {
    mods: Mods,
    keysym: u32,
}

/// One compiled binding.
#[derive(Clone, Debug)]
struct Bind {
    key: BindKey,
    action: Action,
}

/// A chord/action pair that was skipped, with the reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BindIssue {
    /// The authored chord.
    pub(crate) chord: String,
    /// The authored action.
    pub(crate) action: String,
    /// Why the pair was not compiled.
    pub(crate) reason: String,
}

/// The compiled keyboard bindings for this session.
#[derive(Clone, Debug, Default)]
pub(crate) struct BindTable {
    binds: Vec<Bind>,
}

impl BindTable {
    /// Compiles the built-in binds plus `[binds.default]` and `[binds.locked]`.
    ///
    /// Locked binds are active now: crawm has no lock screen yet, and the
    /// volume/brightness chords users put there should still work. Gating them
    /// arrives with the lock protocol (M6).
    #[must_use]
    pub(crate) fn compile(
        default: &BTreeMap<String, String>,
        locked: &BTreeMap<String, String>,
    ) -> (Self, Vec<BindIssue>) {
        let mut table = Self::default();
        let mut issues = Vec::new();
        for (chord, action) in builtins() {
            if let Err(issue) = table.insert(&chord, &action) {
                issues.push(issue);
            }
        }
        for (chord, action) in default.iter().chain(locked) {
            if let Err(issue) = table.insert(chord, action) {
                issues.push(issue);
            }
        }
        (table, issues)
    }

    /// Number of compiled bindings (used by startup logging).
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.binds.len()
    }

    /// The action a chord triggers, if any.
    ///
    /// Lock-state modifiers (`caps_lock`, `num_lock`) are ignored: a chord means
    /// the same thing with NumLock on.
    #[must_use]
    pub(crate) fn action(&self, modifiers: &ModifiersState, keysym: Keysym) -> Option<Action> {
        let mods = Mods::from_state(modifiers);
        self.binds
            .iter()
            .rev()
            .find(|bind| bind.key.mods == mods && bind.key.keysym == keysym.raw())
            .map(|bind| bind.action.clone())
    }

    /// Adds one binding, replacing any existing bind with the same chord.
    fn insert(&mut self, chord: &str, action: &str) -> Result<(), BindIssue> {
        let issue = |reason: String| BindIssue {
            chord: chord.to_owned(),
            action: action.to_owned(),
            reason,
        };
        let key = parse_chord(chord).map_err(issue)?;
        let action = parse_action(action).map_err(issue)?;
        self.binds.retain(|bind| bind.key != key);
        self.binds.push(Bind { key, action });
        Ok(())
    }
}

/// Parses `MOD+MOD+KEY` into the modifier set and base key it fires on.
fn parse_chord(chord: &str) -> Result<BindKey, String> {
    let mut parts: Vec<&str> = chord.split('+').map(str::trim).collect();
    let key_name = parts.pop().unwrap_or_default();
    if key_name.is_empty() {
        return Err("missing key name".to_owned());
    }
    let mut mods = Mods::default();
    for part in parts {
        if part.is_empty() {
            return Err("empty modifier in chord".to_owned());
        }
        let bit = Mods::parse(part).ok_or_else(|| format!("unknown modifier `{part}`"))?;
        mods.insert(bit);
    }
    let keysym = lookup_keysym(key_name);
    if keysym == keysyms::KEY_NoSymbol {
        return Err(format!("unknown key `{key_name}`"));
    }
    Ok(BindKey { mods, keysym })
}

/// Turns a key name into a keysym.
///
/// Exact xkb names win (`Up`, `XF86AudioMute`); a case-insensitive retry makes
/// `SUPER+UP` and `alt+v` work without requiring the author to know xkb's
/// capitalisation.
fn lookup_keysym(name: &str) -> u32 {
    let exact = xkb::keysym_from_name(name, xkb::KEYSYM_NO_FLAGS).raw();
    if exact != keysyms::KEY_NoSymbol {
        return exact;
    }
    xkb::keysym_from_name(name, xkb::KEYSYM_CASE_INSENSITIVE).raw()
}

/// Turns an authored action string into an [`Action`].
fn parse_action(authored: &str) -> Result<Action, String> {
    let action = authored.trim();
    let action = action.strip_suffix(',').unwrap_or(action).trim();

    if let Some(command) = action.strip_prefix("spawn:") {
        return parse_command(command)
            .map(Action::Spawn)
            .map_err(|reason| format!("invalid spawn command: {reason}"));
    }
    if let Some(value) = action.strip_prefix("focusdir:") {
        return parse_direction(value).map(Action::Direction);
    }
    if let Some(value) = action.strip_prefix("move_client:") {
        return parse_direction(value).map(Action::MoveWindow);
    }
    if let Some(value) = action.strip_prefix("view_space:") {
        return parse_index(value).map(Action::ViewSpace);
    }
    if let Some(value) = action.strip_prefix("move_client_space:") {
        return parse_index(value).map(Action::MoveToSpace);
    }
    if let Some(value) = action
        .strip_prefix("toggle_space_view:")
        .or_else(|| action.strip_prefix("toggle_view_space:"))
    {
        return parse_index(value).map(Action::ToggleSpaceView);
    }
    if let Some(value) = action.strip_prefix("toggle_space:") {
        return parse_index(value).map(Action::ToggleSpace);
    }

    match action.to_ascii_lowercase().as_str() {
        "killclient" | "close" | "closewindow" | "close_window" => Ok(Action::Close),
        "toggle_back" | "view_back" => Ok(Action::ToggleBack),
        "view_space_left" | "view_space_prev" | "viewprev" => Ok(Action::ViewSpaceStep(-1)),
        "view_space_right" | "view_space_next" | "viewnext" => Ok(Action::ViewSpaceStep(1)),
        "switch_layout" | "cycle_layout_family" => Ok(Action::CycleLayoutFamily),
        "cycle_layout" | "cycle_layout_mode" => Ok(Action::CycleLayoutMode),
        "toogle_window_floating"
        | "toggle_window_floating"
        | "toggle_floating"
        | "togglefloating" => Ok(Action::ToggleFloating),
        "toggle_fullscreen" | "togglefullscreen" => Ok(Action::ToggleFullscreen),
        "toggle_maximize_window" | "toggle_maximized" | "togglemaximizescreen" => {
            Ok(Action::ToggleMaximized)
        }
        "focus_cycle_next" | "focusnext" => Ok(Action::CycleFocus(true)),
        "focus_cycle_prev" | "focusprev" => Ok(Action::CycleFocus(false)),
        "inc_master_ratio" | "master_ratio+" => Ok(Action::MasterRatio(true)),
        "dec_master_ratio" | "master_ratio-" => Ok(Action::MasterRatio(false)),
        "inc_master_count" | "master_count+" => Ok(Action::MasterCount(true)),
        "dec_master_count" | "master_count-" => Ok(Action::MasterCount(false)),
        "preset_width" | "switch_preset_width" => Ok(Action::PresetWidth),
        "center_column" => Ok(Action::CenterColumn),
        "" => Err("empty action".to_owned()),
        // Mango actions for features crawm does not implement yet. They are
        // accepted by the parser and skipped here with a startup warning.
        "toggle_overview" | "enteroverview" | "leaveoverview" => {
            Err("overview is not implemented yet".to_owned())
        }
        "minimized" | "minimize" | "restore_minimized" => {
            Err("minimizing windows is not implemented yet".to_owned())
        }
        "toggle_scratchpad" => Err("scratchpads are not implemented yet".to_owned()),
        "viewprev_have_client"
        | "viewnext_have_client"
        | "viewtoleft_have_client"
        | "viewtoright_have_client" => {
            Err("this gesture/axis action is not implemented yet".to_owned())
        }
        other => Err(format!("unknown action `{other}`")),
    }
}

fn parse_direction(value: &str) -> Result<Direction, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "left" => Ok(Direction::Left),
        "right" => Ok(Direction::Right),
        "up" => Ok(Direction::Up),
        "down" => Ok(Direction::Down),
        other => Err(format!("unknown direction `{other}`")),
    }
}

fn parse_index(value: &str) -> Result<u8, String> {
    let index: u8 = value
        .trim()
        .parse()
        .map_err(|_| format!("`{}` is not a space number", value.trim()))?;
    if (1..=SpaceSet::COUNT).contains(&index) {
        Ok(index)
    } else {
        Err(format!(
            "space {index} is out of range 1..={}",
            SpaceSet::COUNT
        ))
    }
}

/// The binds crawm has without a config, using the same chord/action syntax the
/// config uses. A config entry with the same chord replaces one of these.
fn builtins() -> Vec<(String, String)> {
    let mut binds = Vec::new();
    for index in 1..=9 {
        for (mods, action) in [
            ("SUPER", "view_space"),
            ("SUPER+SHIFT", "move_client_space"),
            ("SUPER+CTRL", "toggle_space_view"),
            ("SUPER+CTRL+SHIFT", "toggle_space"),
        ] {
            binds.push((format!("{mods}+{index}"), format!("{action}:{index}")));
        }
    }
    for (chord, action) in [
        ("SUPER+Tab", "toggle_back"),
        ("SUPER+Left", "focusdir:left"),
        ("SUPER+Right", "focusdir:right"),
        ("SUPER+Up", "focusdir:up"),
        ("SUPER+Down", "focusdir:down"),
        ("SUPER+j", "focus_cycle_next"),
        ("SUPER+k", "focus_cycle_prev"),
        ("SUPER+SHIFT+space", "toggle_floating"),
        ("SUPER+CTRL+space", "switch_layout"),
        ("SUPER+space", "cycle_layout_mode"),
        ("SUPER+f", "toggle_fullscreen"),
        ("SUPER+SHIFT+h", "dec_master_count"),
        ("SUPER+SHIFT+l", "inc_master_count"),
        ("SUPER+h", "dec_master_ratio"),
        ("SUPER+l", "inc_master_ratio"),
        ("SUPER+r", "preset_width"),
        ("SUPER+c", "center_column"),
        ("SUPER+q", "killclient"),
    ] {
        binds.push((chord.to_owned(), action.to_owned()));
    }
    binds
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(entries: &[(&str, &str)]) -> BindTable {
        let map: BTreeMap<String, String> = entries
            .iter()
            .map(|(chord, action)| ((*chord).to_owned(), (*action).to_owned()))
            .collect();
        BindTable::compile(&map, &BTreeMap::new()).0
    }

    fn mods(ctrl: bool, alt: bool, shift: bool, logo: bool) -> ModifiersState {
        ModifiersState {
            ctrl,
            alt,
            shift,
            logo,
            ..ModifiersState::default()
        }
    }

    fn key(name: &str) -> Keysym {
        Keysym::new(lookup_keysym(name))
    }

    #[test]
    fn parses_modifiers_and_key() {
        let bind = parse_chord("SUPER+CTRL+SHIFT+Up").unwrap();
        assert_eq!(
            bind.mods,
            Mods(Mods::CTRL | Mods::SHIFT | Mods::LOGO),
            "alt is absent"
        );
        assert_eq!(bind.keysym, keysyms::KEY_Up);
    }

    #[test]
    fn key_names_are_case_insensitive_on_retry() {
        assert_eq!(parse_chord("alt+v").unwrap().keysym, keysyms::KEY_v);
        assert_eq!(parse_chord("SUPER+UP").unwrap().keysym, keysyms::KEY_Up);
        assert_eq!(
            parse_chord("XF86AudioMute").unwrap().keysym,
            keysyms::KEY_XF86AudioMute
        );
    }

    #[test]
    fn rejects_unknown_modifiers_keys_and_actions() {
        assert!(parse_chord("HYPER+q").unwrap_err().contains("HYPER"));
        assert!(parse_chord("SUPER+NoSuchKey").is_err());
        assert!(parse_action("nonsense").is_err());
        assert!(parse_action("view_space:0").is_err());
        assert!(parse_action("view_space:99").is_err());
    }

    #[test]
    fn config_overrides_a_builtin_chord() {
        let table = table(&[("SUPER+space", "spawn:rofi -show drun")]);
        assert_eq!(
            table.action(&mods(false, false, false, true), key("space")),
            Some(Action::Spawn(vec![
                "rofi".to_owned(),
                "-show".to_owned(),
                "drun".to_owned()
            ]))
        );
    }

    #[test]
    fn builtins_remain_for_unclaimed_chords() {
        let table = table(&[]);
        assert_eq!(
            table.action(&mods(false, false, false, true), key("Tab")),
            Some(Action::ToggleBack)
        );
    }

    #[test]
    fn extra_modifiers_do_not_match() {
        let table = table(&[("SUPER+q", "killclient")]);
        assert_eq!(
            table.action(&mods(false, false, false, true), key("q")),
            Some(Action::Close)
        );
        assert_eq!(
            table.action(&mods(false, false, true, true), key("q")),
            None
        );
    }

    #[test]
    fn mango_actions_and_trailing_commas_parse() {
        assert_eq!(
            parse_action("toogle_window_floating").unwrap(),
            Action::ToggleFloating
        );
        assert_eq!(
            parse_action("toggle_maximize_window,").unwrap(),
            Action::ToggleMaximized
        );
        assert_eq!(
            parse_action("toggle_fullscreen,").unwrap(),
            Action::ToggleFullscreen
        );
        assert_eq!(
            parse_action("view_space_left").unwrap(),
            Action::ViewSpaceStep(-1)
        );
        assert_eq!(
            parse_action("switch_layout").unwrap(),
            Action::CycleLayoutFamily
        );
    }

    #[test]
    fn unimplemented_actions_are_reported_not_fatal() {
        let map = BTreeMap::from([
            ("SUPER+o".to_owned(), "toggle_overview".to_owned()),
            ("ALT+w".to_owned(), "minimized,".to_owned()),
        ]);
        let (table, issues) = BindTable::compile(&map, &BTreeMap::new());
        assert_eq!(issues.len(), 2);
        assert!(
            issues
                .iter()
                .all(|issue| issue.reason.contains("not implemented"))
        );
        assert_eq!(
            table.action(&mods(false, false, false, true), key("o")),
            None
        );
    }

    #[test]
    fn locked_binds_are_compiled_too() {
        let locked = BTreeMap::from([(
            "XF86AudioMute".to_owned(),
            "spawn:wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle".to_owned(),
        )]);
        let (table, issues) = BindTable::compile(&BTreeMap::new(), &locked);
        assert!(issues.is_empty());
        assert!(matches!(
            table.action(&mods(false, false, false, false), key("XF86AudioMute")),
            Some(Action::Spawn(_))
        ));
    }

    #[test]
    fn builtins_are_all_valid() {
        let (table, issues) = BindTable::compile(&BTreeMap::new(), &BTreeMap::new());
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(table.len(), 36 + 18);
    }
}

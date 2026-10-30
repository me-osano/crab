//! Compiling `[binds.*]` into a lookup table, and the actions they run.
//!
//! The config authors a chord as `"SUPER+SHIFT+Up" = "move_client:up"`. At
//! startup the table turns each chord into a modifier set plus an xkb keysym and
//! each action string into an [`Action`], then input looks up the keysym the
//! keyboard reports (its *base* symbol, so shift does not change which key
//! matches; the modifiers themselves are part of the chord).
//!
//! Built-in binds keep crab usable with no config. Config binds are layered on
//! top and replace a built-in with the same chord, so the shipped example can
//! say `"SUPER+space" = "spawn:rofi -show drun"` and take that chord over.
//!
//! Actions whose feature has not landed yet (`toggle_overview`, `minimized`,
//! scratchpads, gestures) are accepted by the config but skipped here with a
//! startup warning: a bind for a future feature must not stop the session from
//! starting, and it must not fail `--validate`.
//!
//! `[mousebinds]` and `[axisbinds]` use the same modifier parser as the
//! keyboard table. A mouse chord ends in a Linux `BTN_*` name (`btn_left`) and
//! an axis chord in a scroll direction (`UP`, `DOWN`, `LEFT`, `RIGHT`). Mouse
//! binds replace the built-in Super+drag chords; bare left/right
//! `moveresize` binds are refused because they would make ordinary clicks
//! impossible (mango has the same restriction).

use std::collections::BTreeMap;

use crab::config::{Config, KeyMode};
use crab::core::{LayoutKind, SpaceSet};
use crab_ipc::Direction;
#[cfg(test)]
use smithay::input::keyboard::Keysym;
use smithay::input::keyboard::{ModifiersState, keysyms, xkb};

use crate::compositor::spawn::parse_command;

pub(crate) use crab_ipc::Action;

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

    fn is_empty(self) -> bool {
        self.0 == 0
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

/// Optional behavior flags written after a chord, as
/// `"SUPER+q:release,pass-through"`.
///
/// `allow_when_locked` and `allow_inhibiting` are parsed and carried so the
/// config vocabulary is stable, but crab has no lock screen or keyboard
/// inhibitor yet, so nothing gates on them today (M6).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct BindFlags {
    /// Re-fire the action while the key is held (key repeat).
    pub(crate) repeat: bool,
    /// Keep the bind active while the session is locked.
    pub(crate) allow_when_locked: bool,
    /// Keep the bind active while a client holds a keyboard inhibitor.
    pub(crate) allow_inhibiting: bool,
    /// Run the action when the key is released rather than pressed.
    pub(crate) release: bool,
    /// Also forward the key to the focused client.
    pub(crate) pass_through: bool,
}

/// A matched binding: the action to run and how input should treat the key.
#[derive(Clone, Debug)]
pub(crate) struct BoundAction {
    /// The action the chord runs.
    pub(crate) action: Action,
    /// The chord's behavior flags.
    pub(crate) flags: BindFlags,
}

/// One compiled binding.
#[derive(Clone, Debug)]
struct Bind {
    key: BindKey,
    action: Action,
    flags: BindFlags,
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
    /// Locked binds are active now: crab has no lock screen yet, and the
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
    #[cfg(test)]
    #[must_use]
    pub(crate) fn action(&self, modifiers: &ModifiersState, keysym: Keysym) -> Option<Action> {
        self.bind(modifiers, keysym.raw()).map(|bound| bound.action)
    }

    /// The binding a chord matches, with its flags, if any.
    #[must_use]
    pub(crate) fn bind(&self, modifiers: &ModifiersState, keysym: u32) -> Option<BoundAction> {
        let mods = Mods::from_state(modifiers);
        self.binds
            .iter()
            .rev()
            .find(|bind| bind.key.mods == mods && bind.key.keysym == keysym)
            .map(|bind| BoundAction {
                action: bind.action.clone(),
                flags: bind.flags,
            })
    }

    /// Compiles only `binds` (no built-ins), for one key mode's overlay.
    #[must_use]
    pub(crate) fn compile_overlay(binds: &BTreeMap<String, String>) -> (Self, Vec<BindIssue>) {
        let mut table = Self::default();
        let mut issues = Vec::new();
        for (chord, action) in binds {
            if let Err(issue) = table.insert(chord, action) {
                issues.push(issue);
            }
        }
        (table, issues)
    }

    /// Adds one binding, replacing any existing bind with the same chord.
    fn insert(&mut self, chord: &str, action: &str) -> Result<(), BindIssue> {
        let issue = |reason: String| BindIssue {
            chord: chord.to_owned(),
            action: action.to_owned(),
            reason,
        };
        let (key, flags) = parse_chord_with_flags(chord).map_err(issue)?;
        let action = parse_action(action).map_err(issue)?;
        self.binds.retain(|bind| bind.key != key);
        self.binds.push(Bind { key, action, flags });
        Ok(())
    }
}

/// A mouse binding's action: an ordinary [`Action`] or the start of a grab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MouseAction {
    /// Run a compositor action.
    Action(Action),
    /// Start an interactive move of the window under the pointer.
    Move,
    /// Start an interactive resize of the window under the pointer.
    Resize,
}

/// One compiled mouse binding.
#[derive(Clone, Debug)]
struct MouseBind {
    mods: Mods,
    button: u32,
    action: MouseAction,
}

/// The compiled `[mousebinds]` table (built-ins plus config).
#[derive(Clone, Debug, Default)]
pub(crate) struct MouseBindTable {
    binds: Vec<MouseBind>,
}

impl MouseBindTable {
    /// Compiles the built-in mouse binds plus `[mousebinds]`.
    ///
    /// The built-ins keep Super+drag moving and resizing; a config entry with
    /// the same chord replaces one. A bare left/right `moveresize` entry is
    /// skipped with an issue instead of swallowing every click.
    #[must_use]
    pub(crate) fn compile(binds: &BTreeMap<String, String>) -> (Self, Vec<BindIssue>) {
        let mut table = Self::default();
        let mut issues = Vec::new();
        for (chord, action) in mouse_builtins() {
            if let Err(issue) = table.insert(&chord, &action) {
                issues.push(issue);
            }
        }
        for (chord, action) in binds {
            if let Err(issue) = table.insert(chord, action) {
                issues.push(issue);
            }
        }
        (table, issues)
    }

    /// Number of compiled mouse bindings (used by startup logging).
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.binds.len()
    }

    /// Compiles only `binds` (no built-ins), for one key mode's overlay.
    #[must_use]
    pub(crate) fn compile_overlay(binds: &BTreeMap<String, String>) -> (Self, Vec<BindIssue>) {
        let mut table = Self::default();
        let mut issues = Vec::new();
        for (chord, action) in binds {
            if let Err(issue) = table.insert(chord, action) {
                issues.push(issue);
            }
        }
        (table, issues)
    }

    /// The action a button press triggers, if any.
    #[must_use]
    pub(crate) fn action(&self, modifiers: &ModifiersState, button: u32) -> Option<MouseAction> {
        let mods = Mods::from_state(modifiers);
        self.binds
            .iter()
            .rev()
            .find(|bind| bind.mods == mods && bind.button == button)
            .map(|bind| bind.action.clone())
    }

    /// Adds one binding, replacing any existing bind with the same chord.
    fn insert(&mut self, chord: &str, action: &str) -> Result<(), BindIssue> {
        let issue = |reason: String| BindIssue {
            chord: chord.to_owned(),
            action: action.to_owned(),
            reason,
        };
        let (mods, button) = parse_mouse_chord(chord).map_err(issue)?;
        let action = parse_mouse_action(action).map_err(issue)?;
        if mods.is_empty() && matches!(action, MouseAction::Move | MouseAction::Resize) {
            return Err(issue(
                "`moveresize` needs a modifier; bare left/right clicks stay with the client"
                    .to_owned(),
            ));
        }
        self.binds
            .retain(|bind| bind.mods != mods || bind.button != button);
        self.binds.push(MouseBind {
            mods,
            button,
            action,
        });
        Ok(())
    }
}

/// The scroll axis an axis bind fires on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScrollAxis {
    /// Horizontal scrolling (wheel tilt or a sideways two-finger swipe).
    Horizontal,
    /// Vertical scrolling.
    Vertical,
}

/// Gesture direction and finger count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GestureDir {
    Left,
    Right,
    Up,
    Down,
}

/// One compiled gesture binding.
#[derive(Clone, Debug)]
struct GestureBind {
    mods: Mods,
    fingers: u32,
    dir: GestureDir,
    action: Action,
}

/// The compiled `[gesturebinds]` table.
#[derive(Clone, Debug, Default)]
pub(crate) struct GestureBindTable {
    binds: Vec<GestureBind>,
}

impl GestureBindTable {
    /// Compiles `[gesturebinds]` into a lookup table.
    #[must_use]
    pub(crate) fn compile(binds: &BTreeMap<String, String>) -> (Self, Vec<BindIssue>) {
        let mut table = Self::default();
        let mut issues = Vec::new();
        for (chord, action) in binds {
            if let Err(issue) = table.insert(chord, action) {
                issues.push(issue);
            }
        }
        (table, issues)
    }

    /// Number of compiled gesture bindings (used by startup logging).
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.binds.len()
    }

    /// Compiles only `binds`, for one key mode's overlay.
    #[must_use]
    pub(crate) fn compile_overlay(binds: &BTreeMap<String, String>) -> (Self, Vec<BindIssue>) {
        let mut table = Self::default();
        let mut issues = Vec::new();
        for (chord, action) in binds {
            if let Err(issue) = table.insert(chord, action) {
                issues.push(issue);
            }
        }
        (table, issues)
    }

    /// The action a gesture triggers, if any.
    #[must_use]
    pub(crate) fn action(
        &self,
        modifiers: &ModifiersState,
        dir: GestureDir,
        fingers: u32,
    ) -> Option<Action> {
        let mods = Mods::from_state(modifiers);
        self.binds
            .iter()
            .rev()
            .find(|bind| bind.mods == mods && bind.dir == dir && bind.fingers == fingers)
            .map(|bind| bind.action.clone())
    }

    fn insert(&mut self, chord: &str, action: &str) -> Result<(), BindIssue> {
        let issue = |reason: String| BindIssue {
            chord: chord.to_owned(),
            action: action.to_owned(),
            reason,
        };
        let (mods, dir, fingers) = parse_gesture_chord(chord).map_err(issue)?;
        let action = parse_action(action).map_err(issue)?;
        self.binds
            .retain(|bind| bind.mods != mods || bind.dir != dir || bind.fingers != fingers);
        self.binds.push(GestureBind {
            mods,
            dir,
            fingers,
            action,
        });
        Ok(())
    }
}

/// One compiled axis binding.
#[derive(Clone, Debug)]
struct AxisBind {
    mods: Mods,
    axis: ScrollAxis,
    positive: bool,
    action: Action,
}

/// The compiled `[axisbinds]` table.
#[derive(Clone, Debug, Default)]
pub(crate) struct AxisBindTable {
    binds: Vec<AxisBind>,
}

impl AxisBindTable {
    /// Compiles `[axisbinds]` into a lookup table.
    #[must_use]
    pub(crate) fn compile(binds: &BTreeMap<String, String>) -> (Self, Vec<BindIssue>) {
        let mut table = Self::default();
        let mut issues = Vec::new();
        for (chord, action) in binds {
            if let Err(issue) = table.insert(chord, action) {
                issues.push(issue);
            }
        }
        (table, issues)
    }

    /// Number of compiled axis bindings (used by startup logging).
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.binds.len()
    }

    /// Compiles only `binds`, for one key mode's overlay.
    #[must_use]
    pub(crate) fn compile_overlay(binds: &BTreeMap<String, String>) -> (Self, Vec<BindIssue>) {
        let mut table = Self::default();
        let mut issues = Vec::new();
        for (chord, action) in binds {
            if let Err(issue) = table.insert(chord, action) {
                issues.push(issue);
            }
        }
        (table, issues)
    }

    /// The action a scroll event triggers, if any.
    ///
    /// `positive` is the direction the content scrolls in (`true` is down or
    /// right), so `SUPER+UP` maps to a negative vertical delta.
    #[must_use]
    pub(crate) fn action(
        &self,
        modifiers: &ModifiersState,
        axis: ScrollAxis,
        positive: bool,
    ) -> Option<Action> {
        let mods = Mods::from_state(modifiers);
        self.binds
            .iter()
            .rev()
            .find(|bind| bind.mods == mods && bind.axis == axis && bind.positive == positive)
            .map(|bind| bind.action.clone())
    }

    /// Adds one binding, replacing any existing bind with the same chord.
    fn insert(&mut self, chord: &str, action: &str) -> Result<(), BindIssue> {
        let issue = |reason: String| BindIssue {
            chord: chord.to_owned(),
            action: action.to_owned(),
            reason,
        };
        let (mods, axis, positive) = parse_axis_chord(chord).map_err(issue)?;
        let action = parse_action(action).map_err(issue)?;
        self.binds
            .retain(|bind| bind.mods != mods || bind.axis != axis || bind.positive != positive);
        self.binds.push(AxisBind {
            mods,
            axis,
            positive,
            action,
        });
        Ok(())
    }
}

/// All bind tables compiled for one key mode.
///
/// A mode holds only its own entries (no built-ins and no default table).
/// Lookup walks the mode stack top-down and falls back to the default tables,
/// so a mode composes with the modes below it and with the base config.
#[derive(Clone, Debug, Default)]
pub(crate) struct ModeBinds {
    /// Keyboard bindings declared for this mode.
    pub(crate) keys: BindTable,
    /// Pointer bindings declared for this mode.
    pub(crate) mouse: MouseBindTable,
    /// Scroll bindings declared for this mode.
    pub(crate) axis: AxisBindTable,
    /// Gesture bindings declared for this mode.
    pub(crate) gestures: GestureBindTable,
}

impl ModeBinds {
    /// Compiles one `[modes.<name>]` table.
    #[must_use]
    pub(crate) fn compile(mode: &KeyMode) -> (Self, Vec<BindIssue>) {
        let (keys, mut issues) = BindTable::compile_overlay(&mode.binds);
        let (mouse, mouse_issues) = MouseBindTable::compile_overlay(&mode.mouse_binds);
        issues.extend(mouse_issues);
        let (axis, axis_issues) = AxisBindTable::compile_overlay(&mode.axis_binds);
        issues.extend(axis_issues);
        let (gestures, gesture_issues) = GestureBindTable::compile_overlay(&mode.gesture_binds);
        issues.extend(gesture_issues);
        (
            Self {
                keys,
                mouse,
                axis,
                gestures,
            },
            issues,
        )
    }
}

/// Every compiled bind table for a config: the default tables plus each mode.
#[derive(Clone, Debug, Default)]
pub(crate) struct CompiledBinds {
    /// Built-in keyboard binds plus `[binds.*]`.
    pub(crate) keys: BindTable,
    /// Built-in pointer binds plus `[mousebinds]`.
    pub(crate) mouse: MouseBindTable,
    /// `[axisbinds]`.
    pub(crate) axis: AxisBindTable,
    /// `[gesturebinds]`.
    pub(crate) gestures: GestureBindTable,
    /// Per-mode overlays keyed by mode name.
    pub(crate) modes: BTreeMap<String, ModeBinds>,
}

impl CompiledBinds {
    /// Compiles every bind table in `config`.
    ///
    /// Skipped entries are returned with a section label (`"key"`, `"mouse"`,
    /// `"mode resize"`, …) so the caller can log them the same way for startup
    /// and reload.
    #[must_use]
    pub(crate) fn compile(config: &Config) -> (Self, Vec<(String, BindIssue)>) {
        let mut issues = Vec::new();

        let (keys, key_issues) = BindTable::compile(&config.binds.default, &config.binds.locked);
        issues.extend(
            key_issues
                .into_iter()
                .map(|issue| ("key".to_owned(), issue)),
        );

        let (mouse, mouse_issues) = MouseBindTable::compile(&config.mouse_binds);
        issues.extend(
            mouse_issues
                .into_iter()
                .map(|issue| ("mouse".to_owned(), issue)),
        );

        let (axis, axis_issues) = AxisBindTable::compile(&config.axis_binds);
        issues.extend(
            axis_issues
                .into_iter()
                .map(|issue| ("axis".to_owned(), issue)),
        );

        let (gestures, gesture_issues) = GestureBindTable::compile(&config.gesture_binds);
        issues.extend(
            gesture_issues
                .into_iter()
                .map(|issue| ("gesture".to_owned(), issue)),
        );

        let mut modes = BTreeMap::new();
        for (name, mode) in &config.modes {
            let (compiled, mode_issues) = ModeBinds::compile(mode);
            issues.extend(
                mode_issues
                    .into_iter()
                    .map(|issue| (format!("mode {name}"), issue)),
            );
            modes.insert(name.clone(), compiled);
        }

        (
            Self {
                keys,
                mouse,
                axis,
                gestures,
                modes,
            },
            issues,
        )
    }
}

fn parse_chord_with_flags(chord: &str) -> Result<(BindKey, BindFlags), String> {
    let mut flags = BindFlags::default();
    let mut chord_clean = chord.trim().to_owned();
    if let Some((keys, flag_part)) = chord_clean.clone().split_once(':') {
        chord_clean = keys.trim().to_owned();
        for flag in flag_part.split(',') {
            let flag = flag.trim();
            if flag.is_empty() {
                continue;
            }
            match flag.to_ascii_lowercase().as_str() {
                "repeat" => flags.repeat = true,
                "norepeat" | "no-repeat" => flags.repeat = false,
                "allow-when-locked" | "locked" => flags.allow_when_locked = true,
                "allow-inhibiting" => flags.allow_inhibiting = true,
                "release" => flags.release = true,
                "pass-through" | "pass_through" | "passthrough" => flags.pass_through = true,
                other => return Err(format!("unknown bind flag `{other}`")),
            }
        }
    }
    let parts_vec: Vec<String> = chord_clean
        .split('+')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if parts_vec.is_empty() {
        return Err("missing key name".to_owned());
    }
    let key_name = parts_vec[parts_vec.len() - 1].clone();
    let mod_parts = &parts_vec[0..parts_vec.len() - 1];
    let mod_refs: Vec<&str> = mod_parts.iter().map(String::as_str).collect();
    let mods = parse_mods(&mod_refs)?;
    let keysym = lookup_keysym(&key_name);
    if keysym == keysyms::KEY_NoSymbol {
        return Err(format!("unknown key `{key_name}`"));
    }
    Ok((BindKey { mods, keysym }, flags))
}

/// Parses `MOD+MOD+KEY` into the modifier set and base key it fires on.
#[cfg(test)]
fn parse_chord(chord: &str) -> Result<BindKey, String> {
    let (key, _) = parse_chord_with_flags(chord)?;
    Ok(key)
}

/// Parses `MOD+MOD+btn_left` into modifiers and a Linux `BTN_*` code.
fn parse_mouse_chord(chord: &str) -> Result<(Mods, u32), String> {
    let mut parts: Vec<&str> = chord.split('+').map(str::trim).collect();
    let button_name = parts.pop().unwrap_or_default();
    if button_name.is_empty() {
        return Err("missing button name".to_owned());
    }
    let mods = parse_mods(&parts)?;
    let button = lookup_button(button_name)
        .ok_or_else(|| format!("unknown mouse button `{button_name}`"))?;
    Ok((mods, button))
}

/// Parses `MOD+MOD+UP` into modifiers and a scroll direction.
fn parse_axis_chord(chord: &str) -> Result<(Mods, ScrollAxis, bool), String> {
    let mut parts: Vec<&str> = chord.split('+').map(str::trim).collect();
    let direction = parts.pop().unwrap_or_default();
    if direction.is_empty() {
        return Err("missing axis direction".to_owned());
    }
    let mods = parse_mods(&parts)?;
    let (axis, positive) = match direction.to_ascii_lowercase().as_str() {
        "up" | "wheel_up" => (ScrollAxis::Vertical, false),
        "down" | "wheel_down" => (ScrollAxis::Vertical, true),
        "left" | "wheel_left" => (ScrollAxis::Horizontal, false),
        "right" | "wheel_right" => (ScrollAxis::Horizontal, true),
        other => return Err(format!("unknown axis direction `{other}`")),
    };
    Ok((mods, axis, positive))
}

/// Parses `MOD+MOD+DIR+N` or `DIR+N` into modifiers, direction, and finger count.
fn parse_gesture_chord(chord: &str) -> Result<(Mods, GestureDir, u32), String> {
    let parts: Vec<String> = chord
        .split('+')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    let mut mods_list: Vec<String> = Vec::new();
    let mut dir_found: Option<GestureDir> = None;
    let mut fingers_found: u32 = 3;
    for p in &parts {
        let pl = p.to_ascii_lowercase();
        if pl == "none" {
            continue;
        }
        if Mods::parse(&pl).is_some() {
            mods_list.push(pl.clone());
            continue;
        }
        if matches!(pl.as_str(), "left" | "right" | "up" | "down") {
            dir_found = match pl.as_str() {
                "left" => Some(GestureDir::Left),
                "right" => Some(GestureDir::Right),
                "up" => Some(GestureDir::Up),
                "down" => Some(GestureDir::Down),
                _ => None,
            };
            continue;
        }
        if let Ok(n) = pl.parse::<u32>() {
            if (1..=5).contains(&n) {
                fingers_found = n;
                continue;
            }
        }
    }
    let dir = dir_found.ok_or_else(|| "missing gesture direction".to_string())?;
    let mods_refs: Vec<&str> = mods_list.iter().map(|s| s.as_str()).collect();
    let mods = parse_mods(&mods_refs)?;
    Ok((mods, dir, fingers_found))
}

/// Parses the modifier parts of a chord. `NONE` is accepted as an explicit
/// empty modifier set, as in mango's `none+left+3`.
fn parse_mods(parts: &[&str]) -> Result<Mods, String> {
    let mut mods = Mods::default();
    for part in parts {
        if part.is_empty() {
            return Err("empty modifier in chord".to_owned());
        }
        if part.eq_ignore_ascii_case("none") {
            continue;
        }
        let bit = Mods::parse(part).ok_or_else(|| format!("unknown modifier `{part}`"))?;
        mods.insert(bit);
    }
    Ok(mods)
}

/// Turns a mouse button name into a Linux `BTN_*` code.
fn lookup_button(name: &str) -> Option<u32> {
    Some(match name.to_ascii_lowercase().as_str() {
        "btn_left" => 0x110,
        "btn_right" => 0x111,
        "btn_middle" => 0x112,
        "btn_side" => 0x113,
        "btn_extra" => 0x114,
        "btn_forward" => 0x115,
        "btn_back" => 0x116,
        "btn_task" => 0x117,
        _ => return None,
    })
}

/// Turns a mouse action string into a [`MouseAction`].
fn parse_mouse_action(authored: &str) -> Result<MouseAction, String> {
    let action = authored.trim();
    let action = action.strip_suffix(',').unwrap_or(action).trim();
    match action.to_ascii_lowercase().as_str() {
        "moveresize,curmove" | "moveresize,move" => Ok(MouseAction::Move),
        "moveresize,curresize" | "moveresize,resize" => Ok(MouseAction::Resize),
        _ => parse_action(action).map(MouseAction::Action),
    }
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
///
/// Mango's spellings are aliases for the canonical action names where they
/// differ (`killclient` → close-window, `focusdir:left` → focus-direction).
fn parse_action(authored: &str) -> Result<Action, String> {
    let action = authored.trim();
    let action = action.strip_suffix(',').unwrap_or(action).trim();

    if let Some(command) = action.strip_prefix("spawn:") {
        return parse_command(command)
            .map(|command| Action::Spawn { command })
            .map_err(|reason| format!("invalid spawn command: {reason}"));
    }
    if let Some(value) = action
        .strip_prefix("focusdir:")
        .or_else(|| action.strip_prefix("focus-direction:"))
    {
        return parse_direction(value).map(|direction| Action::FocusDirection { direction });
    }
    if let Some(value) = action
        .strip_prefix("move_client:")
        .or_else(|| action.strip_prefix("move-window:"))
    {
        return parse_direction(value).map(|direction| Action::MoveWindow { direction });
    }
    if let Some(value) = action
        .strip_prefix("view_space:")
        .or_else(|| action.strip_prefix("view-space:"))
    {
        return parse_index(value).map(|index| Action::ViewSpace { index });
    }
    if let Some(value) = action
        .strip_prefix("move_client_space:")
        .or_else(|| action.strip_prefix("move-to-space:"))
    {
        return parse_index(value).map(|index| Action::MoveToSpace { index });
    }
    if let Some(value) = action
        .strip_prefix("toggle_space_view:")
        .or_else(|| action.strip_prefix("toggle_view_space:"))
        .or_else(|| action.strip_prefix("toggle-space-view:"))
    {
        return parse_index(value).map(|index| Action::ToggleSpaceView { index });
    }
    if let Some(value) = action
        .strip_prefix("toggle_space:")
        .or_else(|| action.strip_prefix("toggle-space:"))
    {
        return parse_index(value).map(|index| Action::ToggleSpace { index });
    }
    if let Some(value) = action
        .strip_prefix("set_layout:")
        .or_else(|| action.strip_prefix("set-layout:"))
    {
        let name = value.trim();
        if name.is_empty() {
            return Err("empty layout name".to_owned());
        }
        if LayoutKind::from_name(name).is_none() {
            return Err(format!("unknown layout `{name}`"));
        }
        return Ok(Action::SetLayout {
            name: name.to_owned(),
        });
    }
    if let Some(value) = action
        .strip_prefix("enter_mode:")
        .or_else(|| action.strip_prefix("enter-mode:"))
        .or_else(|| action.strip_prefix("mode:"))
    {
        let name = value.trim();
        if name.is_empty() {
            return Err("empty mode name".to_owned());
        }
        return Ok(Action::EnterMode {
            name: name.to_owned(),
        });
    }
    if let Some(value) = action
        .strip_prefix("focus_window:")
        .or_else(|| action.strip_prefix("focus-window:"))
    {
        let id = value
            .trim()
            .parse::<u64>()
            .map_err(|_| format!("`{}` is not a window id", value.trim()))?;
        if id == 0 {
            return Err("window id must be at least 1".to_owned());
        }
        return Ok(Action::FocusWindow { id });
    }

    match action.to_ascii_lowercase().as_str() {
        "killclient" | "close" | "closewindow" | "close_window" | "close-window" => {
            Ok(Action::CloseWindow)
        }
        "quit" | "exit" => Ok(Action::Quit),
        "reload_config" | "reload-config" => Ok(Action::ReloadConfig),
        "toggle_back" | "view_back" | "toggle-back" => Ok(Action::ToggleBack),
        "view_space_left" | "view_space_prev" | "viewprev" => {
            Ok(Action::ViewSpaceStep { step: -1 })
        }
        "view_space_right" | "view_space_next" | "viewnext" => {
            Ok(Action::ViewSpaceStep { step: 1 })
        }
        "switch_layout" | "cycle_layout_family" | "cycle-layout-family" => {
            Ok(Action::CycleLayoutFamily)
        }
        "cycle_layout" | "cycle_layout_mode" | "cycle-layout" => Ok(Action::CycleLayout),
        "toogle_window_floating"
        | "toggle_window_floating"
        | "toggle_floating"
        | "togglefloating"
        | "toggle-floating" => Ok(Action::ToggleFloating),
        "toggle_fullscreen" | "togglefullscreen" | "toggle-fullscreen" => {
            Ok(Action::ToggleFullscreen)
        }
        "toggle_maximize_window"
        | "toggle_maximized"
        | "togglemaximizescreen"
        | "toggle-maximized" => Ok(Action::ToggleMaximized),
        "focus_cycle_next" | "focusnext" => Ok(Action::FocusCycle { forward: true }),
        "focus_cycle_prev" | "focusprev" => Ok(Action::FocusCycle { forward: false }),
        "switch_keyboard_layout" | "next_keyboard_layout" | "switch-keyboard-layout" => {
            Ok(Action::SwitchKeyboardLayout { forward: true })
        }
        "switch_keyboard_layout_prev" | "prev_keyboard_layout" | "switch-keyboard-layout-prev" => {
            Ok(Action::SwitchKeyboardLayout { forward: false })
        }
        "leave_mode" | "leave-mode" | "exit_mode" | "exit-mode" => Ok(Action::LeaveMode),
        "pop_mode" | "pop-mode" => Ok(Action::PopMode),
        "preset_width" | "switch_preset_width" | "preset-width" => Ok(Action::PresetWidth),
        "center_column" | "center-column" => Ok(Action::CenterColumn),
        "" => Err("empty action".to_owned()),
        // Mango actions for features crab does not implement yet. They are
        // accepted by the parser and skipped here with a startup warning.
        "toggle_overview" | "enteroverview" | "leaveoverview" => {
            Err("overview is not implemented yet".to_owned())
        }
        "minimized" | "minimize" | "restore_minimized" => {
            Err("minimizing windows is not implemented yet".to_owned())
        }
        "toggle_scratchpad" => Err("scratchpads are not implemented yet".to_owned()),
        "viewprev_have_client" | "viewtoleft_have_client" => {
            Ok(Action::ViewSpaceHaveClient { step: -1 })
        }
        "viewnext_have_client" | "viewtoright_have_client" => {
            Ok(Action::ViewSpaceHaveClient { step: 1 })
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

/// The mouse binds crab has without a config: Super+drag moves, Super+right
/// drag resizes. A config entry with the same chord replaces one.
fn mouse_builtins() -> Vec<(String, String)> {
    vec![
        ("SUPER+btn_left".to_owned(), "moveresize,curmove".to_owned()),
        (
            "SUPER+btn_right".to_owned(),
            "moveresize,curresize".to_owned(),
        ),
    ]
}

/// The binds crab has without a config, using the same chord/action syntax the
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
        ("SUPER+ALT+space", "next_keyboard_layout"),
        ("SUPER+ALT+SHIFT+space", "prev_keyboard_layout"),
        ("SUPER+f", "toggle_fullscreen"),
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
            Some(Action::Spawn {
                command: vec!["rofi".to_owned(), "-show".to_owned(), "drun".to_owned()]
            })
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
            Some(Action::CloseWindow)
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
            Action::ViewSpaceStep { step: -1 }
        );
        assert_eq!(
            parse_action("switch_layout").unwrap(),
            Action::CycleLayoutFamily
        );
        assert_eq!(
            parse_action("next_keyboard_layout").unwrap(),
            Action::SwitchKeyboardLayout { forward: true }
        );
        assert_eq!(
            parse_action("prev_keyboard_layout").unwrap(),
            Action::SwitchKeyboardLayout { forward: false }
        );
    }

    #[test]
    fn canonical_action_names_parse_too() {
        assert_eq!(parse_action("quit").unwrap(), Action::Quit);
        assert_eq!(
            parse_action("toggle-floating").unwrap(),
            Action::ToggleFloating
        );
        assert_eq!(
            parse_action("focus-direction:left").unwrap(),
            Action::FocusDirection {
                direction: Direction::Left
            }
        );
        assert_eq!(
            parse_action("move-window:down").unwrap(),
            Action::MoveWindow {
                direction: Direction::Down
            }
        );
        assert_eq!(
            parse_action("set-layout:fair").unwrap(),
            Action::SetLayout {
                name: "fair".to_owned()
            }
        );
        assert_eq!(
            parse_action("focus-window:7").unwrap(),
            Action::FocusWindow { id: 7 }
        );
        assert!(parse_action("set-layout:nope").is_err());
        assert!(parse_action("focus-window:0").is_err());
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
            Some(Action::Spawn { .. })
        ));
    }

    #[test]
    fn builtins_are_all_valid() {
        let (table, issues) = BindTable::compile(&BTreeMap::new(), &BTreeMap::new());
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(table.len(), 36 + 16);
    }

    fn mouse_table(entries: &[(&str, &str)]) -> MouseBindTable {
        let map: BTreeMap<String, String> = entries
            .iter()
            .map(|(chord, action)| ((*chord).to_owned(), (*action).to_owned()))
            .collect();
        MouseBindTable::compile(&map).0
    }

    #[test]
    fn mouse_builtins_keep_super_drag_working() {
        let table = mouse_table(&[]);
        assert_eq!(table.len(), 2);
        assert_eq!(
            table.action(&mods(false, false, false, true), 0x110),
            Some(MouseAction::Move)
        );
        assert_eq!(
            table.action(&mods(false, false, false, true), 0x111),
            Some(MouseAction::Resize)
        );
    }

    #[test]
    fn mouse_binds_replace_builtins_and_allow_config_actions() {
        let table = mouse_table(&[
            ("SUPER+btn_left", "toggle_floating"),
            ("NONE+btn_middle", "togglemaximizescreen"),
        ]);
        assert_eq!(
            table.action(&mods(false, false, false, true), 0x110),
            Some(MouseAction::Action(Action::ToggleFloating))
        );
        assert_eq!(
            table.action(&mods(false, false, false, false), 0x112),
            Some(MouseAction::Action(Action::ToggleMaximized))
        );
        assert_eq!(
            table.action(&mods(false, false, false, true), 0x111),
            Some(MouseAction::Resize),
            "the untouched built-in remains"
        );
    }

    #[test]
    fn bare_left_or_right_moveresize_is_refused() {
        let map = BTreeMap::from([
            ("btn_left".to_owned(), "moveresize,curmove".to_owned()),
            ("btn_right".to_owned(), "moveresize,curresize".to_owned()),
        ]);
        let (table, issues) = MouseBindTable::compile(&map);
        assert_eq!(issues.len(), 2);
        assert!(issues.iter().all(|issue| issue.reason.contains("modifier")));
        assert_eq!(
            table.action(&mods(false, false, false, false), 0x110),
            None,
            "a bare left click must still reach the client"
        );
    }

    #[test]
    fn mouse_chords_reject_unknown_buttons_and_modifiers() {
        assert!(
            MouseBindTable::compile(&BTreeMap::from([(
                "SUPER+btn_nope".to_owned(),
                "killclient".to_owned()
            )]))
            .1[0]
                .reason
                .contains("button")
        );
        assert!(
            MouseBindTable::compile(&BTreeMap::from([(
                "HYPER+btn_left".to_owned(),
                "killclient".to_owned()
            )]))
            .1[0]
                .reason
                .contains("HYPER")
        );
    }

    fn axis_table(entries: &[(&str, &str)]) -> AxisBindTable {
        let map: BTreeMap<String, String> = entries
            .iter()
            .map(|(chord, action)| ((*chord).to_owned(), (*action).to_owned()))
            .collect();
        AxisBindTable::compile(&map).0
    }

    #[test]
    fn axis_binds_match_modifiers_axis_and_direction() {
        let table = axis_table(&[
            ("SUPER+UP", "viewtoleft_have_client"),
            ("SUPER+DOWN", "viewtoright_have_client"),
            ("ALT+LEFT", "view_space_left"),
        ]);
        let super_held = mods(false, false, false, true);
        assert_eq!(
            table.action(&super_held, ScrollAxis::Vertical, false),
            Some(Action::ViewSpaceHaveClient { step: -1 })
        );
        assert_eq!(
            table.action(&super_held, ScrollAxis::Vertical, true),
            Some(Action::ViewSpaceHaveClient { step: 1 })
        );
        assert_eq!(
            table.action(&super_held, ScrollAxis::Horizontal, false),
            None,
            "the configured LEFT chord needs Alt"
        );
        assert_eq!(
            table.action(
                &mods(false, true, false, false),
                ScrollAxis::Horizontal,
                false
            ),
            Some(Action::ViewSpaceStep { step: -1 })
        );
    }

    #[test]
    fn axis_chords_reject_unknown_directions() {
        let map = BTreeMap::from([("SUPER+DIAGONAL".to_owned(), "viewnext".to_owned())]);
        let (table, issues) = AxisBindTable::compile(&map);
        assert_eq!(table.len(), 0);
        assert!(issues[0].reason.contains("axis direction"));
    }

    #[test]
    fn have_client_actions_parse() {
        assert_eq!(
            parse_action("viewprev_have_client").unwrap(),
            Action::ViewSpaceHaveClient { step: -1 }
        );
        assert_eq!(
            parse_action("viewtoright_have_client").unwrap(),
            Action::ViewSpaceHaveClient { step: 1 }
        );
    }

    #[test]
    fn mode_actions_parse() {
        assert_eq!(
            parse_action("enter_mode:resize").unwrap(),
            Action::EnterMode {
                name: "resize".to_owned()
            }
        );
        assert_eq!(
            parse_action("enter-mode:resize").unwrap(),
            Action::EnterMode {
                name: "resize".to_owned()
            }
        );
        assert_eq!(parse_action("leave_mode").unwrap(), Action::LeaveMode);
        assert_eq!(parse_action("pop-mode").unwrap(), Action::PopMode);
        assert!(parse_action("enter_mode:").is_err());
    }

    #[test]
    fn bind_flags_parse_after_the_chord() {
        let (_, flags) = parse_chord_with_flags("SUPER+q:release,pass-through").unwrap();
        assert!(flags.release);
        assert!(flags.pass_through);
        assert!(!flags.repeat);

        let (key, flags) = parse_chord_with_flags("SUPER+r:repeat").unwrap();
        assert_eq!(key.keysym, keysyms::KEY_r);
        assert!(flags.repeat);
    }

    #[test]
    fn unknown_bind_flags_are_an_issue() {
        let map = BTreeMap::from([("SUPER+q:nonsense".to_owned(), "killclient".to_owned())]);
        let (table, issues) = BindTable::compile(&map, &BTreeMap::new());
        assert_eq!(table.len(), builtins().len(), "built-ins still compile");
        assert_eq!(issues.len(), 1);
        assert!(issues[0].reason.contains("unknown bind flag"));
    }

    #[test]
    fn mode_overlays_contain_only_their_own_binds() {
        let mode = KeyMode {
            binds: BTreeMap::from([("SUPER+q".to_owned(), "enter_mode:resize".to_owned())]),
            ..KeyMode::default()
        };
        let (compiled, issues) = ModeBinds::compile(&mode);
        assert!(issues.is_empty());
        assert_eq!(
            compiled
                .keys
                .bind(&mods(false, false, false, true), keysyms::KEY_q)
                .map(|bound| bound.action),
            Some(Action::EnterMode {
                name: "resize".to_owned()
            })
        );
        assert!(
            compiled
                .keys
                .bind(&mods(false, false, false, true), keysyms::KEY_Tab)
                .is_none(),
            "the default table is not copied into the mode"
        );
    }

    #[test]
    fn mode_overlays_compile_mouse_and_axis_binds() {
        let mode = KeyMode {
            mouse_binds: BTreeMap::from([(
                "SUPER+btn_left".to_owned(),
                "toggle_floating".to_owned(),
            )]),
            axis_binds: BTreeMap::from([("SUPER+UP".to_owned(), "viewprev".to_owned())]),
            ..KeyMode::default()
        };
        let (compiled, issues) = ModeBinds::compile(&mode);
        assert!(issues.is_empty());
        assert!(
            compiled
                .mouse
                .action(&mods(false, false, false, true), 0x110)
                .is_some()
        );
        assert_eq!(
            compiled.axis.action(
                &mods(false, false, false, true),
                ScrollAxis::Vertical,
                false
            ),
            Some(Action::ViewSpaceStep { step: -1 })
        );
    }

    #[test]
    fn compiled_binds_include_every_mode() {
        let mut config = Config::default();
        config.modes.insert(
            "resize".to_owned(),
            KeyMode {
                binds: BTreeMap::from([("h".to_owned(), "preset_width".to_owned())]),
                ..KeyMode::default()
            },
        );
        let (compiled, issues) = CompiledBinds::compile(&config);
        assert!(issues.is_empty());
        assert!(compiled.modes.contains_key("resize"));
        assert_eq!(compiled.keys.len(), builtins().len());
    }
}

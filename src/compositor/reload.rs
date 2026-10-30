//! Live config reloading for the compositor.
//!
//! Config files are watched via `notify` with debouncing, parsed and validated
//! atomically, and applied without restarting the compositor. A failed reload
//! keeps the last good configuration and surfaces the error on the
//! on-screen banner plus in logs and IPC events.

use std::path::Path;
use std::time::Instant;

use crab::config::{self, Config};

use crate::compositor::state::{Crab, apply_configured_position_if_empty, log_bind_issue};

/// Result of a reload attempt.
#[derive(Clone, Debug)]
pub(crate) enum ReloadOutcome {
    /// No config path was configured (or reload disabled).
    NoPath,
    /// The config was unchanged or not a file watch trigger; no swap occurred.
    #[allow(dead_code)]
    Unchanged,
    /// A new config was applied successfully.
    Reloaded {
        /// Whether any state change requires a redraw.
        needs_redraw: bool,
    },
    /// Reload failed; the previous configuration remains in effect.
    Failed {
        /// Human-readable error.
        message: String,
    },
}

/// Loads and validates a config from `path`.
pub(crate) fn load_from_path(path: &Path) -> Result<Config, String> {
    config::load(path).map_err(|err| err.to_string())
}

/// Applies a new config to `state` atomically.
///
/// Binds, mouse/axis binds, the keyboard keymap and repeat, and output
/// positions from config are updated. Window rules and animations take effect
/// on future events; existing windows remain where they are. A keymap that
/// fails to compile returns an error before any config state is swapped.
pub(crate) fn apply_config(state: &mut Crab, config: Config) -> Result<bool, String> {
    // Compile the new keymap before swapping any other state: an invalid xkb
    // layout must leave the previous config and keymap fully in place.
    if state.config.input.xkb_rules_layout != config.input.xkb_rules_layout {
        state.apply_keyboard_layout(&config.input.xkb_rules_layout)?;
    }

    let old = std::mem::replace(&mut state.config, config);
    let new = &state.config;

    let (compiled, issues) = crate::compositor::binds::CompiledBinds::compile(new);
    for (kind, issue) in issues {
        log_bind_issue(&kind, &issue);
    }
    state.binds = compiled.keys;
    state.mouse_binds = compiled.mouse;
    state.axis_binds = compiled.axis;
    state.gesture_binds = compiled.gestures;
    state.mode_binds = compiled.modes;

    // Update keyboard repeat if supported by the handle.
    state
        .keyboard
        .change_repeat_info(new.input.repeat_delay.max(0), new.input.repeat_rate.max(0));

    // Re-apply configured output positions/scales where the model still exists.
    for runtime in state.outputs.values_mut() {
        let model = &mut runtime.model;
        apply_configured_position_if_empty(new, model);
        if let Some(scale) = new.outputs.get(&model.name).and_then(|o| o.scale) {
            model.scale = scale;
        }
        // Transform is applied by the backend/Smithay in most paths; skip if not exposed.
    }
    // A reload that removed the active mode must not leave input in it.
    state.prune_modes();

    // Bump config epoch and clear error on success.
    state.config_epoch = state.config_epoch.wrapping_add(1);
    state.config_error = None;
    state.config_error_since = None;

    // Relayout existing windows for new gaps/borders.
    state.relayout();
    _ = old;
    Ok(true)
}

/// Reloads the config from `state.config_path` and updates state.
pub(crate) fn reload(state: &mut Crab) -> ReloadOutcome {
    let Some(path) = state.config_path.clone() else {
        return ReloadOutcome::NoPath;
    };
    match load_from_path(&path) {
        Ok(config) => match apply_config(state, config) {
            Ok(needs_redraw) => ReloadOutcome::Reloaded { needs_redraw },
            Err(message) => {
                record_failure(state, &message);
                ReloadOutcome::Failed { message }
            }
        },
        Err(message) => {
            record_failure(state, &message);
            ReloadOutcome::Failed { message }
        }
    }
}

/// Records a failed reload for the banner and IPC event streams.
fn record_failure(state: &mut Crab, message: &str) {
    state.config_epoch = state.config_epoch.wrapping_add(1);
    state.config_error = Some(message.to_owned());
    state.config_error_since = Some(Instant::now());
}

/// Validates a config at `path` without applying it.
pub(crate) fn validate_path(path: &Path) -> Result<(), String> {
    config::load(path)
        .map(|_| ())
        .map_err(|err| err.to_string())
}

/// Sets the banner message.
#[must_use]
pub(crate) fn banner_message(state: &Crab) -> Option<String> {
    state.config_error.clone()
}

/// Adds a banner element to a render list; not yet implemented (needs text rendering).
#[allow(dead_code)]
pub(crate) fn push_banner(_state: &Crab, _elements: &mut Vec<()>) {}

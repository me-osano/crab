//! Running the canonical [`Action`] vocabulary.
//!
//! Binds and the IPC `dispatch` request both produce [`crab_ipc::Action`], so
//! the behaviour of an action cannot diverge between the keyboard and a script.
//! Pointer-grab internals (`moveresize`) are the one thing that cannot be
//! expressed as a plain action; they stay in [`super::input`].
//!
//! `apply` returns an error for commands the current milestone cannot honour or
//! for a target that no longer exists, so the IPC server can report it instead
//! of silently doing nothing.

use crab::core::{
    ColumnWidth, Direction as CoreDirection, LayoutCommand, SpaceSet, WindowHeight, WindowId,
};
use crab_ipc::{Action, Direction};

use crate::compositor::reload;
use crate::compositor::spawn;
use crate::compositor::state::Crab;

/// Runs `action` against the compositor.
///
/// # Errors
/// Returns a human-readable reason when the action names a window or layout
/// that does not exist, or when the feature is not implemented yet.
pub(crate) fn apply(state: &mut Crab, action: Action) -> Result<(), String> {
    match action {
        Action::Quit => state.request_quit(),
        Action::Spawn { command } => {
            if command.is_empty() {
                return Err("spawn command is empty".to_owned());
            }
            spawn::spawn(
                &command,
                state.wayland_display.as_deref(),
                state.ipc_socket.as_deref(),
            );
        }
        Action::CloseWindow => state.close_focused(),
        Action::FocusWindow { id } => {
            let id = WindowId::new(id);
            if state.window(id).is_none() {
                return Err(format!("no window with id {id}"));
            }
            state.focus_window(Some(id));
        }
        Action::FocusDirection { direction } => state.focus_direction(core_direction(direction)),
        Action::MoveWindow { direction } => state.move_window(core_direction(direction)),
        Action::FocusCycle { forward } => state.focus_cycle(forward),
        Action::SwitchKeyboardLayout { forward } => {
            state.switch_keyboard_layout(forward);
        }
        Action::ViewSpace { index } => state.view_space(checked_space(index)?),
        Action::MoveToSpace { index } => state.move_to_space(checked_space(index)?),
        Action::ToggleSpaceView { index } => state.toggle_view_space(checked_space(index)?),
        Action::ToggleSpace { index } => state.toggle_space(checked_space(index)?),
        Action::ViewSpaceStep { step } => state.view_space_step(step),
        Action::ViewSpaceHaveClient { step } => state.view_space_have_client(step),
        Action::ToggleBack => state.toggle_back(),
        Action::CycleLayout => state.cycle_layout_mode(),
        Action::CycleLayoutFamily => state.cycle_layout_family(),
        Action::SetLayout { name } => return state.set_layout(&name),
        Action::ToggleFloating => state.toggle_floating(),
        Action::ToggleFullscreen => state.toggle_fullscreen(),
        Action::ToggleMaximized => state.toggle_maximized(),
        Action::PresetWidth => state.layout_command(LayoutCommand::SwitchPresetWidth),
        Action::CenterColumn => state.layout_command(LayoutCommand::CenterColumn),
        Action::MoveColumn { direction } => {
            state.layout_command(column_command(direction, true)?);
        }
        Action::MoveWindowToColumn { direction } => {
            state.layout_command(column_command(direction, false)?);
        }
        Action::ConsumeWindowIntoColumn => {
            state.layout_command(LayoutCommand::ConsumeWindowIntoColumn);
        }
        Action::ExpelWindowFromColumn => {
            state.layout_command(LayoutCommand::ExpelWindowFromColumn);
        }
        Action::ExpandColumnToAvailableWidth => {
            state.layout_command(LayoutCommand::ExpandColumnToAvailableWidth);
        }
        Action::SetColumnWidth { width } => {
            let Some(width) = ColumnWidth::parse(&width) else {
                return Err(format!("`{width}` is not a column width"));
            };
            state.layout_command(LayoutCommand::SetColumnWidth(width));
        }
        Action::PresetWindowHeight => {
            state.layout_command(LayoutCommand::SwitchPresetWindowHeight);
        }
        Action::SetWindowHeight { height } => {
            let Some(height) = WindowHeight::parse(&height) else {
                return Err(format!("`{height}` is not a window height"));
            };
            state.layout_command(LayoutCommand::SetWindowHeight(height));
        }
        Action::ReloadConfig => match reload::reload(state) {
            reload::ReloadOutcome::NoPath => return Err("no config file to reload".to_owned()),
            reload::ReloadOutcome::Unchanged => return Ok(()),
            reload::ReloadOutcome::Reloaded { .. } => return Ok(()),
            reload::ReloadOutcome::Failed { message } => return Err(message),
        },
        Action::EnterMode { name } => {
            state.enter_mode(&name);
            return Ok(());
        }
        Action::LeaveMode => {
            state.leave_mode();
            return Ok(());
        }
        Action::PopMode => {
            state.pop_mode();
            return Ok(());
        }
    }
    Ok(())
}

/// Rejects a space index that could never name an existing slot.
fn checked_space(index: u8) -> Result<u8, String> {
    if (1..=SpaceSet::COUNT).contains(&index) {
        Ok(index)
    } else {
        Err(format!(
            "space {index} is out of range 1..={}",
            SpaceSet::COUNT
        ))
    }
}

/// Converts the wire direction into the pure layout model's direction.
fn core_direction(direction: Direction) -> CoreDirection {
    match direction {
        Direction::Left => CoreDirection::Left,
        Direction::Right => CoreDirection::Right,
        Direction::Up => CoreDirection::Up,
        Direction::Down => CoreDirection::Down,
    }
}

/// Maps a wire direction onto a column action.
///
/// The exposed scroller runs horizontally, so only left/right name a place in
/// the strip; up/down are rejected rather than silently doing nothing.
fn column_command(direction: Direction, column: bool) -> Result<LayoutCommand, String> {
    let forward = match direction {
        Direction::Left => false,
        Direction::Right => true,
        Direction::Up | Direction::Down => {
            return Err("columns run horizontally: use left or right".to_owned());
        }
    };
    Ok(match (column, forward) {
        (true, false) => LayoutCommand::MoveColumnLeft,
        (true, true) => LayoutCommand::MoveColumnRight,
        (false, false) => LayoutCommand::MoveWindowToColumnLeft,
        (false, true) => LayoutCommand::MoveWindowToColumnRight,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_space_indices_are_rejected() {
        assert_eq!(checked_space(1), Ok(1));
        assert_eq!(checked_space(SpaceSet::COUNT), Ok(SpaceSet::COUNT));
        assert!(checked_space(0).is_err());
        assert!(checked_space(SpaceSet::COUNT + 1).is_err());
    }

    #[test]
    fn wire_directions_map_to_the_layout_model() {
        assert_eq!(core_direction(Direction::Left), CoreDirection::Left);
        assert_eq!(core_direction(Direction::Right), CoreDirection::Right);
        assert_eq!(core_direction(Direction::Up), CoreDirection::Up);
        assert_eq!(core_direction(Direction::Down), CoreDirection::Down);
    }
}

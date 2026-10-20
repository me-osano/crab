//! Input: turning backend events into seat events, and the M2 keybinds.
//!
//! M2 wires the keyboard and pointer (motion, buttons, scroll) into the seat
//! with click-to-focus, plus a **hardcoded keybind layer** so the space/layout
//! actions are reachable before the config-driven bind system lands in M5. The
//! binds below are temporary by design: when M5 adds `[[binds]]`, this table is
//! replaced by a lookup into the parsed config, and the [`Action`] enum stays.
//!
//! Touch, gestures, and pointer constraints land with the libinput backend (M4)
//! and protocol breadth (M3); the `_ => {}` arm of [`handle`] is where they will
//! arrive.

use crawm::core::{Direction, LayoutCommand};
use smithay::backend::input::{
    AbsolutePositionEvent, Axis, ButtonState, Event as _, InputBackend, InputEvent, KeyState,
    KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
};
use smithay::input::keyboard::{FilterResult, Keysym, ModifiersState, keysyms};
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge;
use smithay::utils::{Logical, Point, SERIAL_COUNTER};

use crate::compositor::state::{Crawm, GrabMode};

/// `BTN_LEFT`.
const BTN_LEFT: u32 = 0x110;
/// `BTN_RIGHT`.
const BTN_RIGHT: u32 = 0x111;

/// A compositor action produced by a keybind or pointer grab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    /// View a single space.
    ViewSpace(u8),
    /// Move the focused window to a space.
    MoveToSpace(u8),
    /// Toggle a space in the current view.
    ToggleSpaceView(u8),
    /// Toggle a space on the focused window.
    ToggleSpace(u8),
    /// Swap to the previously-viewed space set.
    ToggleBack,
    /// Directional focus.
    Direction(Direction),
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
    /// Grow (`true`) or shrink the master area.
    MasterRatio(bool),
    /// Add (`true`) or remove a master slot.
    MasterCount(bool),
    /// Cycle the focused scroll column's width through the presets.
    PresetWidth,
    /// Scroll the focused scroll column to the middle of the view.
    CenterColumn,
    /// Ask the focused window to close.
    Close,
}

/// Handles one input event from a backend, updating seat state and focus.
pub(crate) fn handle<B: InputBackend>(state: &mut Crawm, event: InputEvent<B>) {
    match event {
        InputEvent::Keyboard { event } => keyboard::<B>(state, event),
        InputEvent::PointerMotion { event } => motion_relative::<B>(state, &event),
        InputEvent::PointerMotionAbsolute { event } => {
            let position = event.position_transformed(state.size);
            motion(state, position, event.time_msec());
        }
        InputEvent::PointerButton { event } => button::<B>(state, &event),
        InputEvent::PointerAxis { event } => axis::<B>(state, &event),
        _ => {}
    }
}

/// Forwards a key to the focused client, intercepting crawm's own binds.
fn keyboard<B: InputBackend>(state: &mut Crawm, event: B::KeyboardKeyEvent) {
    let keycode = event.key_code();
    let key_state = event.state();

    // A key swallowed on press also swallows its release, so clients never see
    // a dangling modifier-combo half.
    if key_state == KeyState::Released && state.bound_keys.remove(&keycode.raw()) {
        return;
    }

    let keyboard = state.keyboard.clone();
    let action = keyboard.input::<Action, _>(
        state,
        keycode,
        key_state,
        0.into(),
        event.time_msec(),
        |state, modifiers, keysym| {
            // Track Super so pointer grabs know whether to grab.
            state.super_held = modifiers.logo;
            if key_state != KeyState::Pressed {
                return FilterResult::Forward;
            }
            // The *base* symbol (level 0, unshifted): `Super+1` and
            // `Super+Shift+1` both match `KEY_1`, and the modifiers pick the
            // space operation.
            let base = keysym
                .raw_syms()
                .first()
                .copied()
                .unwrap_or_else(|| keysym.modified_sym());
            match bind_action(modifiers, base) {
                Some(action) => {
                    state.bound_keys.insert(keycode.raw());
                    FilterResult::Intercept(action)
                }
                None => FilterResult::Forward,
            }
        },
    );

    if let Some(action) = action {
        apply(state, action);
    }
}

/// The action a bind maps to, or `None` to forward the key to the client.
fn bind_action(modifiers: &ModifiersState, keysym: Keysym) -> Option<Action> {
    if !modifiers.logo {
        return None;
    }
    let symbol = keysym.raw();
    if let Some(index) = digit_of(symbol) {
        return Some(match (modifiers.ctrl, modifiers.shift) {
            (false, false) => Action::ViewSpace(index),
            (false, true) => Action::MoveToSpace(index),
            (true, false) => Action::ToggleSpaceView(index),
            (true, true) => Action::ToggleSpace(index),
        });
    }
    let shift = modifiers.shift;
    let ctrl = modifiers.ctrl;
    Some(match symbol {
        keysyms::KEY_Tab => Action::ToggleBack,
        keysyms::KEY_Left => Action::Direction(Direction::Left),
        keysyms::KEY_Right => Action::Direction(Direction::Right),
        keysyms::KEY_Up => Action::Direction(Direction::Up),
        keysyms::KEY_Down => Action::Direction(Direction::Down),
        keysyms::KEY_j => Action::CycleFocus(true),
        keysyms::KEY_k => Action::CycleFocus(false),
        keysyms::KEY_space if shift => Action::ToggleFloating,
        keysyms::KEY_space if ctrl => Action::CycleLayoutFamily,
        keysyms::KEY_space => Action::CycleLayoutMode,
        keysyms::KEY_f => Action::ToggleFullscreen,
        keysyms::KEY_h if shift => Action::MasterCount(false),
        keysyms::KEY_l if shift => Action::MasterCount(true),
        keysyms::KEY_h => Action::MasterRatio(false),
        keysyms::KEY_l => Action::MasterRatio(true),
        keysyms::KEY_r => Action::PresetWidth,
        keysyms::KEY_c => Action::CenterColumn,
        keysyms::KEY_q => Action::Close,
        _ => return None,
    })
}

/// 1-based space index for `KEY_1`..`KEY_9`.
fn digit_of(symbol: u32) -> Option<u8> {
    let digit = symbol.checked_sub(keysyms::KEY_1)?;
    let digit = u8::try_from(digit).ok()?;
    (digit < 9).then_some(digit + 1)
}

/// Runs the action a bind produced.
fn apply(state: &mut Crawm, action: Action) {
    match action {
        Action::ViewSpace(index) => state.view_space(index),
        Action::MoveToSpace(index) => state.move_to_space(index),
        Action::ToggleSpaceView(index) => state.toggle_view_space(index),
        Action::ToggleSpace(index) => state.toggle_space(index),
        Action::ToggleBack => state.toggle_back(),
        Action::Direction(dir) => state.focus_direction(dir),
        Action::CycleFocus(forward) => state.focus_cycle(forward),
        Action::CycleLayoutMode => state.cycle_layout_mode(),
        Action::CycleLayoutFamily => state.cycle_layout_family(),
        Action::ToggleFloating => state.toggle_floating(),
        Action::ToggleFullscreen => state.toggle_fullscreen(),
        Action::MasterRatio(grow) => state.layout_command(if grow {
            LayoutCommand::IncMasterRatio
        } else {
            LayoutCommand::DecMasterRatio
        }),
        Action::MasterCount(add) => state.layout_command(if add {
            LayoutCommand::IncMasterCount
        } else {
            LayoutCommand::DecMasterCount
        }),
        Action::PresetWidth => state.layout_command(LayoutCommand::SwitchPresetWidth),
        Action::CenterColumn => state.layout_command(LayoutCommand::CenterColumn),
        Action::Close => state.close_focused(),
    }
}

/// Moves the pointer by a relative delta and updates focus.
fn motion_relative<B: InputBackend>(state: &mut Crawm, event: &B::PointerMotionEvent) {
    let position = state.pointer_location + event.delta();
    motion(state, position, event.time_msec());
}

/// Moves the pointer to an absolute position and updates focus.
fn motion(state: &mut Crawm, position: Point<f64, Logical>, time: u32) {
    let position = state.clamp_to_output(position);
    state.pointer_location = position;
    state.pointer_seen = true;

    // An active move/resize consumes motion; nothing is forwarded to clients.
    if state.update_interaction(position) {
        return;
    }

    // `motion` sends wl_pointer.enter/leave/motion as needed. The focus
    // carries the origin smithay subtracts `position` from, which for a
    // stretched window is the fit's (`compositor::fit`), not its placement.
    let focus = state.surface_under(position);
    state.pointer.clone().motion(
        state,
        focus,
        &MotionEvent {
            location: position,
            serial: SERIAL_COUNTER.next_serial(),
            time,
        },
    );
    state.pointer.clone().frame(state);
}

/// Forwards a button press/release, focusing the window under the pointer.
///
/// With Super held, the left button starts an interactive move and the right
/// button a resize; both are swallowed rather than sent to the client.
///
/// A move/resize that the *client* started (`xdg_toplevel.move` from its own
/// titlebar) ends on the release too, but that release is forwarded: the client
/// saw the press and would otherwise think the button is still down.
///
/// When a client holds a pointer grab (an open menu, `xdg_popup.grab`), the grab
/// owns the button instead: it dismisses the popup on a click outside of it and
/// forwards the rest, so crawm stays out of the way.
fn button<B: InputBackend>(state: &mut Crawm, event: &B::PointerButtonEvent) {
    let pressed = event.state();
    let grabbed = state.pointer.is_grabbed();
    let serial = SERIAL_COUNTER.next_serial();
    if pressed == ButtonState::Pressed && !grabbed {
        let position = state.pointer_location;
        if state.super_held {
            let mode = match event.button_code() {
                BTN_LEFT => Some(GrabMode::Move),
                BTN_RIGHT => Some(GrabMode::Resize {
                    edges: ResizeEdge::BottomRight,
                }),
                _ => None,
            };
            if let Some(mode) = mode {
                if state.begin_interaction(mode, position, event.button_code()) {
                    return;
                }
            }
        }
        state.focus_at(position);
    } else if pressed == ButtonState::Released {
        // The release ends an interaction even when `grabbed` is set: a press
        // we forwarded leaves smithay's implicit click grab active until this
        // very release, and that is exactly the case a client-started drag is
        // in. A real client grab (a popup) never starts an interaction, so
        // ending one here takes nothing away from it.
        if let Some(interaction) = state.end_interaction(event.button_code()) {
            if !interaction.from_client {
                // A compositor grab swallowed the press, so it swallows the
                // release too: the client never saw the button go down.
                return;
            }
        }
    }

    if pressed == ButtonState::Pressed {
        // A press the client can see is the only one an interactive request
        // may name.
        state.last_pointer_press = Some(serial);
    }
    state.pointer.clone().button(
        state,
        &ButtonEvent {
            serial,
            time: event.time_msec(),
            button: event.button_code(),
            state: pressed,
        },
    );
    state.pointer.clone().frame(state);
}

/// Forwards scroll to the focused client.
fn axis<B: InputBackend>(state: &mut Crawm, event: &B::PointerAxisEvent) {
    let mut frame = AxisFrame::new(event.time_msec()).source(event.source());
    for axis in [Axis::Horizontal, Axis::Vertical] {
        if let Some(amount) = event.amount(axis) {
            frame = frame.value(axis, amount);
        }
        if let Some(steps) = event.amount_v120(axis) {
            frame = frame.v120(axis, steps as i32);
        }
    }
    state.pointer.clone().axis(state, frame);
    state.pointer.clone().frame(state);
}

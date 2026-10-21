//! Input: turning backend events into seat events, and running keybinds.
//!
//! The keyboard path forwards keys to the focused client unless the compiled
//! [`BindTable`](crate::compositor::binds::BindTable) claims the chord, in which
//! case the key press (and its release) is swallowed and the resulting
//! [`Action`] runs. Binds come from `[binds.default]`/`[binds.locked]`, layered
//! over crawm's built-ins (`crate::compositor::binds`).
//!
//! Touch, gestures, and pointer constraints land with the libinput backend (M4)
//! and protocol breadth (M3); the `_ => {}` arm of [`handle`] is where they will
//! arrive.

use crawm::core::LayoutCommand;
use smithay::backend::input::{
    AbsolutePositionEvent, Axis, ButtonState, Event as _, InputBackend, InputEvent, KeyState,
    KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
};
use smithay::input::keyboard::FilterResult;
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge;
use smithay::utils::{Logical, Point, SERIAL_COUNTER};

use crate::compositor::binds::Action;
use crate::compositor::spawn;
use crate::compositor::state::{Crawm, GrabMode};

/// `BTN_LEFT`.
const BTN_LEFT: u32 = 0x110;
/// `BTN_RIGHT`.
const BTN_RIGHT: u32 = 0x111;

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
            match state.binds.action(modifiers, base) {
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

/// Runs the action a bind produced.
fn apply(state: &mut Crawm, action: Action) {
    match action {
        Action::ViewSpace(index) => state.view_space(index),
        Action::MoveToSpace(index) => state.move_to_space(index),
        Action::ToggleSpaceView(index) => state.toggle_view_space(index),
        Action::ToggleSpace(index) => state.toggle_space(index),
        Action::ViewSpaceStep(step) => state.view_space_step(step),
        Action::ToggleBack => state.toggle_back(),
        Action::Direction(dir) => state.focus_direction(dir),
        Action::MoveWindow(dir) => state.move_window(dir),
        Action::CycleFocus(forward) => state.focus_cycle(forward),
        Action::CycleLayoutMode => state.cycle_layout_mode(),
        Action::CycleLayoutFamily => state.cycle_layout_family(),
        Action::ToggleFloating => state.toggle_floating(),
        Action::ToggleFullscreen => state.toggle_fullscreen(),
        Action::ToggleMaximized => state.toggle_maximized(),
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
        Action::Spawn(argv) => spawn::spawn(&argv, state.wayland_display.as_deref()),
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

    // `[layout] focus_follows_cursor`: hovering a window focuses it. Hovering
    // empty space leaves the focus alone; only a click clears it.
    if state.config.focus_follows_cursor && !state.pointer.is_grabbed() {
        let under = state
            .surface_under(position)
            .and_then(|(surface, _)| state.window_id_for(&surface));
        if under.is_some() && under != state.focused_id() {
            state.focus_window(under);
        }
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

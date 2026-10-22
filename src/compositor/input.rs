//! Input: turning backend events into seat events, and running binds.
//!
//! The keyboard path forwards keys to the focused client unless the compiled
//! [`BindTable`](crate::compositor::binds::BindTable) claims the chord, in which
//! case the key press (and its release) is swallowed and the resulting
//! [`Action`] runs. Binds come from `[binds.default]`/`[binds.locked]`, layered
//! over crab's built-ins (`crate::compositor::binds`).
//!
//! Pointer buttons and scroll use the same idea: `[mousebinds]` and
//! `[axisbinds]` are compiled with the keyboard table's modifier rules. A bound
//! press or scroll is swallowed instead of reaching the client.
//!
//! Touch, gestures, and pointer constraints land with the libinput backend (M4)
//! and protocol breadth (M3); the `_ => {}` arm of [`handle`] is where they will
//! arrive.

use crab::core::LayoutCommand;
use smithay::backend::input::{
    AbsolutePositionEvent, Axis, ButtonState, Event as _, InputBackend, InputEvent, KeyState,
    KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
};
use smithay::input::keyboard::FilterResult;
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge;
use smithay::utils::{Logical, Point, SERIAL_COUNTER};

use crate::compositor::binds::{Action, MouseAction, ScrollAxis};
use crate::compositor::spawn;
use crate::compositor::state::{Crab, GrabMode, Interaction};

/// How much a continuous scroll delta must cover before it counts as one axis
/// bind step. Discrete wheel events use `amount_v120` and need only one notch.
const CONTINUOUS_AXIS_STEP: f64 = 10.0;

/// Handles one input event from a backend, updating seat state and focus.
pub(crate) fn handle<B: InputBackend>(state: &mut Crab, event: InputEvent<B>) {
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

/// Forwards a key to the focused client, intercepting crab's own binds.
fn keyboard<B: InputBackend>(state: &mut Crab, event: B::KeyboardKeyEvent) {
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
            // Both keyboard and pointer chords match the exact modifier set, so
            // remember what is down even when the key itself is forwarded.
            state.modifiers = *modifiers;
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
fn apply(state: &mut Crab, action: Action) {
    match action {
        Action::ViewSpace(index) => state.view_space(index),
        Action::MoveToSpace(index) => state.move_to_space(index),
        Action::ToggleSpaceView(index) => state.toggle_view_space(index),
        Action::ToggleSpace(index) => state.toggle_space(index),
        Action::ViewSpaceStep(step) => state.view_space_step(step),
        Action::ViewSpaceHaveClient(step) => state.view_space_have_client(step),
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
fn motion_relative<B: InputBackend>(state: &mut Crab, event: &B::PointerMotionEvent) {
    let position = state.pointer_location + event.delta();
    motion(state, position, event.time_msec());
}

/// Moves the pointer to an absolute position and updates focus.
fn motion(state: &mut Crab, position: Point<f64, Logical>, time: u32) {
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
/// A `[mousebinds]` chord (or the built-in Super+left/right move/resize) is
/// looked up with the current modifier set. A bound press is swallowed, its
/// action runs against the window under the pointer, and its release is
/// swallowed too so the client never sees half a click.
///
/// A move/resize that the *client* started (`xdg_toplevel.move` from its own
/// titlebar) ends on the release too, but that release is forwarded: the client
/// saw the press and would otherwise think the button is still down.
///
/// When a client holds a pointer grab (an open menu, `xdg_popup.grab`), the grab
/// owns the button instead: it dismisses the popup on a click outside of it and
/// forwards the rest, so crab stays out of the way.
fn button<B: InputBackend>(state: &mut Crab, event: &B::PointerButtonEvent) {
    let pressed = event.state();
    let grabbed = state.pointer.is_grabbed();
    let button = event.button_code();
    let serial = SERIAL_COUNTER.next_serial();
    if pressed == ButtonState::Pressed && !grabbed {
        let position = state.pointer_location;
        if let Some(action) = state.mouse_binds.action(&state.modifiers, button) {
            state.bound_buttons.insert(button);
            state.focus_at(position);
            match action {
                MouseAction::Move => {
                    state.begin_interaction(GrabMode::Move, position, button);
                }
                MouseAction::Resize => {
                    state.begin_interaction(
                        GrabMode::Resize {
                            edges: ResizeEdge::BottomRight,
                        },
                        position,
                        button,
                    );
                }
                MouseAction::Action(action) => apply(state, action),
            }
            return;
        }
        state.focus_at(position);
    } else if pressed == ButtonState::Released {
        // Remove the bind entry before any early return: a compositor grab
        // (move/resize) also inserted this button, and leaving it behind would
        // swallow the release of a later, ordinary click.
        let was_bound = state.bound_buttons.remove(&button);
        // The release ends an interaction even when `grabbed` is set: a press
        // we forwarded leaves smithay's implicit click grab active until this
        // very release, and that is exactly the case a client-started drag is
        // in. A real client grab (a popup) never starts an interaction, so
        // ending one here takes nothing away from it.
        let interaction = state.end_interaction(button);
        if release_outcome(was_bound, interaction.as_ref()) == ReleaseOutcome::Swallow {
            return;
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

/// What to do with a pointer-button release after the interaction ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReleaseOutcome {
    /// The client never saw the press, so it must not see the release either.
    Swallow,
    /// Forward the release to the focused client.
    Forward,
}

/// Decides whether a release stays inside the compositor.
///
/// This is separate from [`button`] so the two cases that used to leak or
/// mis-forward a release are directly testable: a mouse bind whose press was
/// swallowed, and a client-started drag whose press *was* forwarded.
fn release_outcome(was_bound: bool, interaction: Option<&Interaction>) -> ReleaseOutcome {
    if was_bound || interaction.is_some_and(|interaction| !interaction.from_client) {
        ReleaseOutcome::Swallow
    } else {
        ReleaseOutcome::Forward
    }
}

/// Forwards scroll to the focused client, unless an `[axisbinds]` chord claims
/// it (for example Super+wheel-up to view the previous occupied space).
///
/// One event is one bind step. Discrete wheel notches use `amount_v120`;
/// continuous sources need [`CONTINUOUS_AXIS_STEP`] so a tiny touchpad delta
/// does not fire on every frame.
fn axis<B: InputBackend>(state: &mut Crab, event: &B::PointerAxisEvent) {
    if let Some((axis, positive)) = axis_step::<B>(event) {
        if let Some(action) = state.axis_binds.action(&state.modifiers, axis, positive) {
            apply(state, action);
            return;
        }
    }

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

/// The axis and direction a scroll event names, if it is large enough to be a
/// bind step.
fn axis_step<B: InputBackend>(event: &B::PointerAxisEvent) -> Option<(ScrollAxis, bool)> {
    for (axis, bind_axis) in [
        (Axis::Vertical, ScrollAxis::Vertical),
        (Axis::Horizontal, ScrollAxis::Horizontal),
    ] {
        let amount = event
            .amount_v120(axis)
            .filter(|steps| steps.abs() >= 1.0)
            .or_else(|| {
                event
                    .amount(axis)
                    .filter(|amount| amount.abs() >= CONTINUOUS_AXIS_STEP)
            });
        if let Some(amount) = amount {
            return Some((bind_axis, amount > 0.0));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab::core::{Rect, WindowId};

    fn interaction(from_client: bool) -> Interaction {
        Interaction {
            id: WindowId::new(1),
            mode: GrabMode::Move,
            grab: (0.0, 0.0).into(),
            origin: Rect::new(0.0, 0.0, 100.0, 100.0),
            button: Some(0x110),
            from_client,
        }
    }

    #[test]
    fn ordinary_release_is_forwarded() {
        assert_eq!(release_outcome(false, None), ReleaseOutcome::Forward);
    }

    #[test]
    fn bound_press_swallows_its_release() {
        // Regression: the bound-button entry must be removed before this
        // decision, or every later ordinary click of the same button is eaten.
        assert_eq!(release_outcome(true, None), ReleaseOutcome::Swallow);
    }

    #[test]
    fn compositor_grab_swallows_its_release() {
        let interaction = interaction(false);
        assert_eq!(
            release_outcome(false, Some(&interaction)),
            ReleaseOutcome::Swallow
        );
    }

    #[test]
    fn client_started_drag_forwards_its_release() {
        let interaction = interaction(true);
        assert_eq!(
            release_outcome(false, Some(&interaction)),
            ReleaseOutcome::Forward
        );
    }

    #[test]
    fn a_bound_button_still_swallows_a_client_started_release() {
        let interaction = interaction(true);
        assert_eq!(
            release_outcome(true, Some(&interaction)),
            ReleaseOutcome::Swallow
        );
    }
}

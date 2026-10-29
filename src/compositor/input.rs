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

use smithay::backend::input::{
    AbsolutePositionEvent, Axis, ButtonState, Event as _, GestureBeginEvent,
    GestureSwipeUpdateEvent, InputBackend, InputEvent, KeyState, KeyboardKeyEvent,
    PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
};
use smithay::input::keyboard::FilterResult;
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge;
use smithay::utils::{Logical, Point, SERIAL_COUNTER};

use crate::compositor::actions;
use crate::compositor::binds::GestureDir;
use crate::compositor::binds::{Action, MouseAction, ScrollAxis};
use crate::compositor::state::{BoundKey, Crab, GrabMode, Interaction};

thread_local! {
    static GESTURE_FINGERS: std::cell::Cell<u32> = const { std::cell::Cell::new(3) };
    static GESTURE_DX: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
    static GESTURE_DY: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
}

/// How much a continuous scroll delta must cover before it counts as one axis
/// bind step. Discrete wheel events use `amount_v120` and need only one notch.
const CONTINUOUS_AXIS_STEP: f64 = 10.0;

/// Handles one input event from a backend, updating seat state and focus.
pub(crate) fn handle<B: InputBackend>(state: &mut Crab, event: InputEvent<B>) {
    match event {
        InputEvent::Keyboard { event } => keyboard::<B>(state, event),
        InputEvent::PointerMotion { event } => motion_relative::<B>(state, &event),
        InputEvent::PointerMotionAbsolute { event } => {
            // An absolute device reports a position in one output's local
            // space. Translate it into the compositor's global space before
            // hit-testing; a DRM backend that maps a device to another output
            // will pass that output's origin here.
            let runtime = state.output();
            let origin = runtime.model.geometry;
            let position = event.position_transformed(runtime.size)
                + Point::<f64, Logical>::from((origin.x, origin.y));
            motion(state, position, event.time_msec());
        }
        InputEvent::PointerButton { event } => button::<B>(state, &event),
        InputEvent::PointerAxis { event } => axis::<B>(state, &event),
        InputEvent::GestureSwipeBegin { event } => {
            GESTURE_FINGERS.with(|f| f.set(event.fingers()));
            GESTURE_DX.with(|v| v.set(0.0));
            GESTURE_DY.with(|v| v.set(0.0));
            let _ = state;
        }
        InputEvent::GestureSwipeUpdate { event } => {
            GESTURE_DX.with(|v| v.set(v.get() + event.delta_x()));
            GESTURE_DY.with(|v| v.set(v.get() + event.delta_y()));
            let _ = state;
        }
        InputEvent::GestureSwipeEnd { event } => {
            swipe_gesture_end::<B>(state, &event);
        }
        _ => {}
    }
}

/// Forwards a key to the focused client, intercepting crab's own binds.
///
/// Bind flags decide the details: [`release`](crate::compositor::binds::BindFlags::release)
/// runs the action on key-up, [`pass_through`](crate::compositor::binds::BindFlags::pass_through)
/// also sends the key to the client, and
/// [`repeat`](crate::compositor::binds::BindFlags::repeat) re-runs it on repeat
/// presses reported by the backend.
fn keyboard<B: InputBackend>(state: &mut Crab, event: B::KeyboardKeyEvent) {
    let keycode = event.key_code();
    let raw = keycode.raw();
    let key_state = event.state();
    let keyboard = state.keyboard.clone();
    // Actions are collected instead of returned through `FilterResult` so a
    // bind can both run and forward its key (`pass-through`).
    let mut actions: Vec<Action> = Vec::new();

    let _ = keyboard.input::<(), _>(
        state,
        keycode,
        key_state,
        0.into(),
        event.time_msec(),
        |state, modifiers, keysym| {
            // Both keyboard and pointer chords match the exact modifier set, so
            // remember what is down even when the key itself is forwarded.
            state.modifiers = *modifiers;
            // The *base* symbol (level 0, unshifted): `Super+1` and
            // `Super+Shift+1` both match `KEY_1`, and the modifiers pick the
            // space operation.
            let base = keysym
                .raw_syms()
                .first()
                .copied()
                .unwrap_or_else(|| keysym.modified_sym());

            match key_state {
                KeyState::Pressed => {
                    // A key still held from an earlier press is key repeat.
                    // Re-fire only when the bind opted into it, and always
                    // treat the repeat exactly like the original press.
                    if let Some(bound) = state.bound_keys.get(&raw) {
                        if bound.repeat {
                            if let Some(action) = bound.action.clone() {
                                actions.push(action);
                            }
                        }
                        return if bound.forwarded {
                            FilterResult::Forward
                        } else {
                            FilterResult::Intercept(())
                        };
                    }

                    let Some(bound) = state.key_bind(modifiers, base.raw()) else {
                        return FilterResult::Forward;
                    };
                    let forwarded = bound.flags.pass_through;
                    let repeat = bound.flags.repeat;
                    let (press_action, release_action) = if bound.flags.release {
                        (None, Some(bound.action))
                    } else {
                        (Some(bound.action), None)
                    };
                    if let Some(action) = press_action.clone() {
                        actions.push(action);
                    }
                    state.bound_keys.insert(
                        raw,
                        BoundKey {
                            action: press_action,
                            release_action,
                            forwarded,
                            repeat,
                        },
                    );
                    if forwarded {
                        FilterResult::Forward
                    } else {
                        FilterResult::Intercept(())
                    }
                }
                KeyState::Released => {
                    // A pressed bind always removes its entry, so a bound
                    // release is swallowed (or forwarded for pass-through) even
                    // when no action runs.
                    match state.bound_keys.remove(&raw) {
                        Some(bound) => {
                            if let Some(action) = bound.release_action {
                                actions.push(action);
                            }
                            if bound.forwarded {
                                FilterResult::Forward
                            } else {
                                FilterResult::Intercept(())
                            }
                        }
                        None => FilterResult::Forward,
                    }
                }
            }
        },
    );

    for action in actions {
        apply(state, action);
    }
}

/// Runs the action a bind produced.
fn apply(state: &mut Crab, action: Action) {
    if let Err(reason) = actions::apply(state, action) {
        tracing::warn!(%reason, "bind action failed");
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
        if let Some(action) = state.mouse_bind(&state.modifiers, button) {
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
        if let Some(action) = state.axis_bind(&state.modifiers, axis, positive) {
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

fn swipe_gesture_end<B: InputBackend>(state: &mut Crab, event: &B::GestureSwipeEndEvent) {
    let fingers = GESTURE_FINGERS.with(|f| f.get());
    let dx = GESTURE_DX.with(|v| v.get());
    let dy = GESTURE_DY.with(|v| v.get());
    let _ = event;
    if dx.abs() > dy.abs() && dx.abs() > 30.0 {
        let dir = if dx > 0.0 {
            GestureDir::Right
        } else {
            GestureDir::Left
        };
        if let Some(action) = state.gesture_bind(&state.modifiers, dir, fingers) {
            apply(state, action);
        }
    } else if dy.abs() > dx.abs() && dy.abs() > 30.0 {
        let dir = if dy > 0.0 {
            GestureDir::Down
        } else {
            GestureDir::Up
        };
        if let Some(action) = state.gesture_bind(&state.modifiers, dir, fingers) {
            apply(state, action);
        }
    }
    GESTURE_DX.with(|v| v.set(0.0));
    GESTURE_DY.with(|v| v.set(0.0));
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

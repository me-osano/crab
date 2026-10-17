//! Input: turning backend events into seat events.
//!
//! M1 wires the keyboard and pointer (motion, buttons, scroll) into the seat
//! with click-to-focus. Touch, gestures, and pointer constraints land with the
//! libinput backend (M4) and protocol breadth (M3); the `_ => {}` arm of
//! [`handle`] is where they will arrive.

use smithay::backend::input::{
    AbsolutePositionEvent, Axis, ButtonState, Event as _, InputBackend, InputEvent,
    KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
};
use smithay::input::keyboard::FilterResult;
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent};
use smithay::utils::{Logical, Point};

use crate::state::Crawm;

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

/// Forwards a key to the focused client.
fn keyboard<B: InputBackend>(state: &mut Crawm, event: B::KeyboardKeyEvent) {
    let keyboard = state.keyboard.clone();
    keyboard.input::<(), _>(
        state,
        event.key_code(),
        event.state(),
        0.into(),
        event.time_msec(),
        |_, _, _| FilterResult::Forward,
    );
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

    // `motion` sends wl_pointer.enter/leave/motion as needed.
    let focus = state
        .surface_under(position)
        .map(|(surface, location)| (surface, location.to_f64()));
    state.pointer.clone().motion(
        state,
        focus,
        &MotionEvent {
            location: position,
            serial: 0.into(),
            time,
        },
    );
}

/// Forwards a button press/release, focusing the surface under the pointer on
/// press (M1's click-to-focus).
fn button<B: InputBackend>(state: &mut Crawm, event: &B::PointerButtonEvent) {
    let pressed = event.state();
    if pressed == ButtonState::Pressed {
        let position = state.pointer_location;
        state.focus_at(position);
    }
    state.pointer.clone().button(
        state,
        &ButtonEvent {
            serial: 0.into(),
            time: event.time_msec(),
            button: event.button_code(),
            state: pressed,
        },
    );
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

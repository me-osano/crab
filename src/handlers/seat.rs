//! `wl_seat` — the keyboard/pointer/touch seat and its focus.

use smithay::delegate_seat;
use smithay::input::pointer::CursorImageStatus;
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;

use crate::state::Crawm;

impl SeatHandler for Crawm {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&WlSurface>) {}

    /// A client asked for a different pointer image with `wl_pointer.set_cursor`.
    ///
    /// [`crate::render`] draws it on top of the scene: a client's own cursor
    /// surface, the built-in arrow for a named icon, or nothing at all when the
    /// client asked for the pointer to be hidden.
    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        tracing::trace!(?image, "the pointer's image changed");
        self.cursor_image = image;
    }
}

delegate_seat!(Crawm);

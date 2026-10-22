//! `zxdg_decoration_manager_v1` — negotiate server- or client-side decorations.
//!
//! Crab does not draw a titlebar yet, so it negotiates client-side decorations.
//! Claiming `ServerSide` here would leave clients that honor the negotiation
//! without any titlebar or close button at all.

use smithay::delegate_xdg_decoration;
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
use smithay::wayland::shell::xdg::{ToplevelSurface, decoration::XdgDecorationHandler};

use crate::compositor::state::Crab;

impl XdgDecorationHandler for Crab {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(Mode::ClientSide);
        });
        toplevel.send_configure();
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: Mode) {
        // A client asking for server-side decorations is answered with the
        // compositor's choice; xdg-decoration lets the compositor override it.
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(Mode::ClientSide);
        });
        toplevel.send_configure();
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(Mode::ClientSide);
        });
        toplevel.send_configure();
    }
}

delegate_xdg_decoration!(Crab);

//! `xdg_shell` — toplevels, popups, and their configuration.

use smithay::delegate_xdg_shell;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::protocol::wl_seat;
use smithay::utils::Serial;
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
};

use crate::state::{Crawm, TOPLEVEL_SIZE};

impl XdgShellHandler for Crawm {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        // M1 asks for a fixed size and marks the window activated; sizing and
        // placement become the layout engine's job in M2.
        surface.with_pending_state(|state| {
            state.size = Some(TOPLEVEL_SIZE.into());
            state.states.set(xdg_toplevel::State::Activated);
        });
        surface.send_configure();
        self.add_toplevel(surface);
    }

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        self.add_popup(surface, positioner);
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
        // TODO(M3): implement popup grabs so bar/app menus dismiss correctly.
    }

    fn reposition_request(
        &mut self,
        _surface: PopupSurface,
        _positioner: PositionerState,
        _token: u32,
    ) {
        // TODO(M3): re-place reactive popups on xdg_popup.reposition.
    }
}

delegate_xdg_shell!(Crawm);

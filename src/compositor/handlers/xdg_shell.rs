//! `xdg_shell` — toplevels, popups, and their configuration.
//!
//! M2 routes every lifecycle event into the window/space model in `state.rs`: the
//! compositor now owns window positions and sizes, so creating, maximizing, or
//! destroying a toplevel is a layout change rather than a bookkeeping one.

use smithay::delegate_xdg_shell;
use smithay::desktop::{PopupKeyboardGrab, PopupKind, PopupPointerGrab, find_popup_root_surface};
use smithay::input::Seat;
use smithay::input::pointer::Focus;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Resource as _;
use smithay::reexports::wayland_server::protocol::wl_output;
use smithay::reexports::wayland_server::protocol::wl_seat;
use smithay::reexports::wayland_server::protocol::wl_surface;
use smithay::utils::{Logical, Point, Serial};
use smithay::wayland::shell::xdg::{
    Configure, PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
};

use crate::compositor::state::Crawm;

impl XdgShellHandler for Crawm {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        // `add_toplevel` asks a tiled window for a starting size, leaves a
        // floating (transient) one to its own choice, then lays the space out.
        self.add_toplevel(surface);
    }

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        self.add_popup(surface, positioner);
    }

    fn grab(&mut self, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let popup = PopupKind::Xdg(surface);
        let Some(seat) = Seat::<Self>::from_resource(&seat) else {
            tracing::debug!("ignored a popup grab on a seat we do not own");
            return;
        };
        let Ok(root) = find_popup_root_surface(&popup) else {
            return;
        };

        // xdg-shell lets the compositor refuse a grab, and a popup grab takes
        // the keyboard: deny one from a client that does not hold it, so popups
        // cannot steal focus. (With nothing focused there is nothing to steal.)
        let popup_client = root.client();
        if let Some(focused) = self.keyboard.current_focus() {
            if focused.client() != popup_client {
                tracing::debug!("denied a popup grab that would steal focus");
                return;
            }
        }

        // A menu belongs to the window that opened it: focus it, so the popup's
        // keyboard grab starts from the right client (`COMPOSITOR.md` §7.5).
        if let Some(id) = self.window_id_for(&root) {
            self.focus_window(Some(id));
        }

        let grab = match self.popup_manager.grab_popup(root, popup, &seat, serial) {
            Ok(grab) => grab,
            Err(err) => {
                tracing::debug!(?err, "denied a popup grab");
                return;
            }
        };

        // The grab dismisses the popup on an outside click and keeps the
        // keyboard on the topmost menu until it ends (the xdg-shell defaults).
        tracing::trace!("granted a popup grab");
        if let Some(keyboard) = seat.get_keyboard() {
            keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
        }
        if let Some(pointer) = seat.get_pointer() {
            pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
        }
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        // The client moved its anchor (a window moved under an open menu, or a
        // submenu flipped): re-place it and confirm with the token.
        self.reposition_popup(&surface, positioner, token);
    }

    fn move_request(&mut self, _surface: ToplevelSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
        // TODO(M3): honour client-initiated interactive moves, which arrive with
        // a pointer serial and need a real pointer grab. M2 only starts drags
        // from the compositor's own keybinds (see `src/compositor/input.rs`).
    }

    fn resize_request(
        &mut self,
        _surface: ToplevelSurface,
        _seat: wl_seat::WlSeat,
        _serial: Serial,
        _edges: xdg_toplevel::ResizeEdge,
    ) {
        // TODO(M3): honour client-initiated resizes from the requested edge.
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(id) = self.window_id_for(surface.wl_surface()) {
            self.set_maximized(id, true);
        }
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(id) = self.window_id_for(surface.wl_surface()) {
            self.set_maximized(id, false);
        }
    }

    fn fullscreen_request(
        &mut self,
        surface: ToplevelSurface,
        _output: Option<wl_output::WlOutput>,
    ) {
        if let Some(id) = self.window_id_for(surface.wl_surface()) {
            self.set_fullscreen(id, true);
        }
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        if let Some(id) = self.window_id_for(surface.wl_surface()) {
            self.set_fullscreen(id, false);
        }
    }

    fn minimize_request(&mut self, _surface: ToplevelSurface) {
        // TODO(M3): a space compositor has no "minimized" state; a bar could map
        // this onto hiding the window from its space.
    }

    fn show_window_menu(
        &mut self,
        _surface: ToplevelSurface,
        _seat: wl_seat::WlSeat,
        _serial: Serial,
        _location: Point<i32, Logical>,
    ) {
        // TODO(M3): draw a compositor-side window menu.
    }

    fn ack_configure(&mut self, _surface: wl_surface::WlSurface, _configure: Configure) {
        // Nothing to track: M2 keeps no per-window configure queue, because
        // every configure it sends is either a start size or a layout rect.
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.remove_window(surface.wl_surface());
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        self.update_toplevel_info(&surface);
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        self.update_toplevel_info(&surface);
    }

    fn parent_changed(&mut self, surface: ToplevelSurface) {
        // A toplevel that gains a parent is a transient dialog, so it floats
        // (`COMPOSITOR.md` §7.5).
        if surface.parent().is_none() {
            return;
        }
        let Some(id) = self.window_id_for(surface.wl_surface()) else {
            return;
        };
        let clocks = self.frame_clock;
        let mut changed = false;
        if let Some(window) = self.window_mut(id) {
            if !window.window.floating {
                let rect = window.visual_rect(&clocks);
                window.window.set_floating(true, rect);
                changed = true;
            }
        }
        if changed {
            self.relayout();
        }
    }
}

delegate_xdg_shell!(Crawm);

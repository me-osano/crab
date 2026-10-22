//! `zwlr_layer_shell_v1` — bars, launchers, notifications, and wallpapers.
//!
//! Placement and the exclusive zone are Smithay's layer map; these callbacks
//! only hand it the new surface and keep crab's focus bookkeeping in step
//! when a keyboard-interactive layer goes away.

use smithay::delegate_layer_shell;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::wayland::shell::wlr_layer::{
    Layer, LayerSurface as WlrLayerSurface, WlrLayerShellHandler, WlrLayerShellState,
};

use crate::compositor::state::Crab;

impl WlrLayerShellHandler for Crab {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: WlrLayerSurface,
        _output: Option<WlOutput>,
        _layer: Layer,
        namespace: String,
    ) {
        // crab has a single output for now, so an explicit or missing output
        // both name the same layer map.
        self.add_layer_surface(surface, namespace);
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        self.remove_layer_surface(&surface);
    }
}

delegate_layer_shell!(Crab);

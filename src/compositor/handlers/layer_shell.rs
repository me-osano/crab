//! `zwlr_layer_shell_v1` — bars, launchers, notifications, and wallpapers.
//!
//! Placement and the exclusive zone are Smithay's layer map; these callbacks
//! only hand it the new surface and keep crab's focus bookkeeping in step
//! when a keyboard-interactive layer goes away.

use smithay::delegate_layer_shell;
use smithay::output::Output;
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
        output: Option<WlOutput>,
        _layer: Layer,
        namespace: String,
    ) {
        // A client may name an output or leave it to the compositor. Resolve
        // the protocol object to crab's handle here; `add_layer_surface`
        // falls back to the primary output when it is unknown.
        let output = output.and_then(|output| Output::from_resource(&output));
        self.add_layer_surface(surface, namespace, output);
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        self.remove_layer_surface(&surface);
    }
}

delegate_layer_shell!(Crab);

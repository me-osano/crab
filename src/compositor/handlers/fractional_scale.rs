//! `wp_fractional_scale_manager_v1` — per-surface fractional scaling.

use smithay::delegate_fractional_scale;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::compositor::with_states;
use smithay::wayland::fractional_scale::{self, FractionalScaleHandler};

use crate::compositor::state::Crab;

impl FractionalScaleHandler for Crab {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        // The output is still fixed at integer scale 1 (see `Crab::new`), but a
        // surface that binds the global must receive a preferred scale or it
        // waits for one before committing. When output scale becomes real, this
        // moves to the same "update every surface" path as `wl_output.scale`.
        with_states(&surface, |states| {
            fractional_scale::with_fractional_scale(states, |scale| {
                scale.set_preferred_scale(1.0);
            });
        });
    }
}

delegate_fractional_scale!(Crab);

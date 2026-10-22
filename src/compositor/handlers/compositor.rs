//! `wl_compositor` — surface lifetimes, subsurfaces, and commit bookkeeping.

use smithay::backend::renderer::utils::on_commit_buffer_handler;
use smithay::delegate_compositor;
use smithay::reexports::wayland_server::Client;
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{CompositorClientState, CompositorHandler, CompositorState};

use crate::compositor::state::{ClientState, Crab};

impl CompositorHandler for Crab {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client
            .get_data::<ClientState>()
            .expect("client connected without crab's ClientState")
            .compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        // Keeps the buffer/viewport/subsurface bookkeeping the renderer reads.
        on_commit_buffer_handler::<Self>(surface);
        // Refreshes size hints, marks the surface mapped, and re-lays out when
        // something the layout cares about changed.
        self.on_commit(surface);
    }
}

impl BufferHandler for Crab {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

delegate_compositor!(Crab);

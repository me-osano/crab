//! Drawing one frame: clear, blit every mapped surface, send frame callbacks.
//!
//! M1 is exactly this: a full-surface redraw each frame with no damage tracking
//! (the `damage` rectangle is always the whole window). The damage/render
//! rework — elements, shaders, per-output redraw state — is M2's job;
//! `COMPOSITOR.md` §5 already names `render/{elements,damage}`.

use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::surface::{
    WaylandSurfaceRenderElement, render_elements_from_surface_tree,
};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::draw_render_elements;
use smithay::backend::renderer::{Frame as _, Renderer as _};
use smithay::backend::winit::WinitGraphicsBackend;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Rectangle, Transform};
use smithay::wayland::compositor::{
    SurfaceAttributes, TraversalAction, with_surface_tree_downward,
};

use crate::state::Crawm;

/// Background color of the compositor (dark slate).
const BACKGROUND: [f32; 4] = [0.05, 0.06, 0.09, 1.0];

/// Renders the whole scene into the winit window and presents it.
pub(crate) fn render(state: &Crawm, backend: &mut WinitGraphicsBackend<GlesRenderer>) {
    let size = backend.window_size();
    let damage = Rectangle::from_size(size);
    tracing::trace!(width = size.w, height = size.h, "rendering a frame");

    {
        let (renderer, mut framebuffer) = match backend.bind() {
            Ok(bound) => bound,
            Err(err) => {
                tracing::error!(?err, "failed to bind the framebuffer");
                return;
            }
        };

        // Positions are logical; with M1's `Scale::Integer(1)` output they map
        // one-to-one onto the renderer's physical pixels.
        //
        // Popups draw over toplevels, so they are appended last. Collecting in
        // two passes (rather than one chained iterator) lets the renderer be
        // reborrowed for each call.
        let mut elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = Vec::new();
        for toplevel in state.toplevels.iter().filter(|toplevel| toplevel.mapped) {
            elements.extend(render_elements_from_surface_tree(
                renderer,
                toplevel.surface.wl_surface(),
                toplevel.location.to_physical(1),
                1.0,
                1.0,
                Kind::Unspecified,
            ));
        }
        for popup in state.popups.iter().filter(|popup| popup.mapped) {
            elements.extend(render_elements_from_surface_tree(
                renderer,
                popup.surface.wl_surface(),
                popup.location.to_physical(1),
                1.0,
                1.0,
                Kind::Unspecified,
            ));
        }

        match renderer.render(&mut framebuffer, size, Transform::Flipped180) {
            Ok(mut frame) => {
                if let Err(err) = frame.clear(BACKGROUND.into(), &[damage]) {
                    tracing::error!(?err, "failed to clear the frame");
                }
                if let Err(err) = draw_render_elements(&mut frame, 1.0, &elements, &[damage]) {
                    tracing::error!(?err, "failed to draw the scene");
                }
                if let Err(err) = frame.finish() {
                    tracing::error!(?err, "failed to finish the frame");
                }
            }
            Err(err) => tracing::error!(?err, "failed to begin the frame"),
        }

        let time = state.start_time.elapsed().as_millis() as u32;
        for toplevel in state.toplevels.iter().filter(|toplevel| toplevel.mapped) {
            send_frame_callbacks(toplevel.surface.wl_surface(), time);
        }
        for popup in state.popups.iter().filter(|popup| popup.mapped) {
            send_frame_callbacks(popup.surface.wl_surface(), time);
        }
    }

    // Clients must have seen the frame before we block in the swap.
    if let Err(err) = backend.submit(Some(&[damage])) {
        tracing::error!(?err, "failed to submit the frame");
    }
}

/// Fires `wl_surface.frame` callbacks for a surface and its children.
///
/// Copied from Smithay's `minimal` example; M2 replaces this with the
/// frame-clock driver in `src/animation`.
fn send_frame_callbacks(surface: &WlSurface, time: u32) {
    with_surface_tree_downward(
        surface,
        (),
        |_, _, &()| TraversalAction::DoChildren(()),
        |_surface, states, &()| {
            // A subsurface may not have surface state until its first commit.
            for callback in states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .frame_callbacks
                .drain(..)
            {
                callback.done(time);
            }
        },
        |_, _, &()| true,
    );
}

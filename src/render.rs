//! Drawing one frame: clear, borders, blit every visible surface.
//!
//! M1 was a full-surface redraw with no damage tracking and no z-ordering. M2
//! adds the parts of the scene the pure layout engine deliberately does not
//! describe: **borders** around tiled and floating windows, the **z-order**
//! (tiled below floating below fullscreen below popups), and skipping the
//! windows the current view hides (`COMPOSITOR.md` §7.1). The frame-clock driver
//! in `src/animation` and real damage tracking land later.

use crawm::core::Color;
use crawm::core::WindowId;
use smithay::backend::renderer::Color32F;
use smithay::backend::renderer::element::Id;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::surface::{
    WaylandSurfaceRenderElement, render_elements_from_surface_tree,
};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::utils::draw_render_elements;
use smithay::backend::renderer::{Frame as _, Renderer as _};
use smithay::backend::winit::WinitGraphicsBackend;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Rectangle, Transform};
use smithay::wayland::compositor::{
    SurfaceAttributes, TraversalAction, with_surface_tree_downward,
};

use crate::state::{Crawm, ManagedWindow};

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

        // Positions are logical; with M2's `Scale::Integer(1)` output they map
        // one-to-one onto the renderer's physical pixels.
        let border_width = state
            .output_state
            .current_space_state()
            .map_or(0.0, |space| space.params.border_width)
            .max(0.0) as i32;
        let focused = state.focused_id();

        // Collect in stacking order: tiled, then floating, then fullscreen on
        // top. Popups draw over everything. Borders are a separate pass so the
        // window surfaces stay above their own frames.
        let mut borders: Vec<SolidColorRenderElement> = Vec::new();
        let mut surfaces: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = Vec::new();

        let tiled = state.windows.iter().filter(|window| {
            window.mapped && window.visible && window.window.is_tiled() && !window.window.maximized
        });
        let floating = state.windows.iter().filter(|window| {
            window.mapped
                && window.visible
                && window.window.floating
                && !window.window.fullscreen
                && !window.window.maximized
        });
        let covering = state.windows.iter().filter(|window| {
            window.mapped && window.visible && (window.window.fullscreen || window.window.maximized)
        });

        collect_pass(
            state,
            &mut *renderer,
            border_width,
            focused,
            tiled,
            &mut borders,
            &mut surfaces,
        );
        collect_pass(
            state,
            &mut *renderer,
            border_width,
            focused,
            floating,
            &mut borders,
            &mut surfaces,
        );
        collect_pass(
            state,
            &mut *renderer,
            border_width,
            focused,
            covering,
            &mut borders,
            &mut surfaces,
        );

        // Popups last: they draw over every toplevel.
        for popup in state.popups.iter().filter(|popup| popup.mapped) {
            surfaces.extend(render_elements_from_surface_tree(
                &mut *renderer,
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
                if let Err(err) =
                    draw_render_elements::<GlesRenderer, _, _>(&mut frame, 1.0, &borders, &[damage])
                {
                    tracing::error!(?err, "failed to draw the borders");
                }
                if let Err(err) = draw_render_elements::<GlesRenderer, _, _>(
                    &mut frame,
                    1.0,
                    &surfaces,
                    &[damage],
                ) {
                    tracing::error!(?err, "failed to draw the scene");
                }
                if let Err(err) = frame.finish() {
                    tracing::error!(?err, "failed to finish the frame");
                }
            }
            Err(err) => tracing::error!(?err, "failed to begin the frame"),
        }

        let time = state.start_time.elapsed().as_millis() as u32;
        for window in state
            .windows
            .iter()
            .filter(|window| window.mapped && window.visible)
        {
            send_frame_callbacks(window.wl_surface(), time);
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

/// Appends one pass of windows: their borders, then their surfaces.
#[allow(clippy::too_many_arguments)]
fn collect_pass<'a>(
    state: &Crawm,
    renderer: &mut GlesRenderer,
    border_width: i32,
    focused: Option<WindowId>,
    windows: impl Iterator<Item = &'a ManagedWindow>,
    borders: &mut Vec<SolidColorRenderElement>,
    surfaces: &mut Vec<WaylandSurfaceRenderElement<GlesRenderer>>,
) {
    for window in windows {
        // A fullscreen or maximized window covers the screen, so a border would
        // only clip it.
        if border_width > 0 && !window.window.fullscreen && !window.window.maximized {
            let color = if focused == Some(window.id()) {
                border_color(state.config.border_color)
            } else {
                border_color(state.config.unfocused_border_color)
            };
            borders.extend(borders_for(window, border_width, color));
        }
        surfaces.extend(render_elements_from_surface_tree(
            renderer,
            window.wl_surface(),
            window.location.to_physical(1),
            1.0,
            1.0,
            Kind::Unspecified,
        ));
    }
}

/// Converts a config color into the renderer's representation.
fn border_color(color: Color) -> Color32F {
    Color32F::new(color.r, color.g, color.b, color.a)
}

/// Four solid rectangles that frame `window`'s border width, in physical pixels.
fn borders_for(window: &ManagedWindow, bw: i32, color: Color32F) -> Vec<SolidColorRenderElement> {
    let x = window.location.x;
    let y = window.location.y;
    let width = window.size.w;
    let height = window.size.h;
    let rects = [
        Rectangle::new((x - bw, y - bw).into(), (width + 2 * bw, bw).into()),
        Rectangle::new((x - bw, y + height).into(), (width + 2 * bw, bw).into()),
        Rectangle::new((x - bw, y).into(), (bw, height).into()),
        Rectangle::new((x + width, y).into(), (bw, height).into()),
    ];
    rects
        .into_iter()
        .map(|geometry| {
            SolidColorRenderElement::new(
                Id::new(),
                geometry,
                CommitCounter::default(),
                color,
                Kind::Unspecified,
            )
        })
        .collect()
}

/// Fires `wl_surface.frame` callbacks for a surface and its children.
///
/// Copied from Smithay's `minimal` example; M3 replaces this with the
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

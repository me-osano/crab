//! Drawing one frame: clear, borders, every visible surface, then the cursor.
//!
//! M1 was a full-surface redraw with no damage tracking and no z-ordering. M2
//! adds the parts of the scene the pure layout engine deliberately does not
//! describe: **borders** around tiled and floating windows, the **z-order**
//! (tiled below floating below fullscreen below popups), skipping the windows
//! the current view hides (`COMPOSITOR.md` §7.1), and the compositor's own
//! **cursor**. The frame-clock driver in `src/animation` and real damage
//! tracking land later.
//!
//! The cursor is ours to draw: `wl_pointer.set_cursor` is a request, not a
//! buffer, and the backends hide the host cursor so only this one is visible
//! (TODO.md §1.3).

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
use smithay::input::pointer::{CursorImageStatus, CursorImageSurfaceData};
use smithay::reexports::wayland_server::Resource as _;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Physical, Point, Rectangle, Transform};
use smithay::wayland::compositor::{
    SurfaceAttributes, TraversalAction, with_states, with_surface_tree_downward,
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

        // Popups last: they draw over every toplevel. Each popup belongs to a
        // toplevel (`PopupManager` tracks the tree), so they are gathered per
        // window; a popup without a buffer yet yields no elements.
        for window in state.windows.iter().filter(|w| w.mapped && w.visible) {
            for (popup, offset) in crate::state::popup_offsets(window) {
                surfaces.extend(render_elements_from_surface_tree(
                    &mut *renderer,
                    popup.wl_surface(),
                    (window.location + offset).to_physical(1),
                    1.0,
                    1.0,
                    Kind::Unspecified,
                ));
            }
        }

        // And the cursor on top of all of it. A client's own cursor surface and
        // the built-in arrow are mutually exclusive, so they stay in separate
        // element lists (they would otherwise need one enum implementing
        // `RenderElement`). Until the backend reports where the pointer is
        // there is nothing to draw.
        let mut cursor_shape: Vec<SolidColorRenderElement> = Vec::new();
        let mut cursor_surface: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = Vec::new();
        if state.pointer_seen {
            match &state.cursor_image {
                CursorImageStatus::Hidden => {}
                CursorImageStatus::Named(_) => {
                    // One arrow for every named icon until the icon set ships (M6).
                    cursor_shape = arrow_cursor(state.pointer_location);
                }
                CursorImageStatus::Surface(surface) if surface.is_alive() => {
                    let hotspot = hotspot_of(surface);
                    // Everything is drawn at scale 1, so logical and physical
                    // pixels are the same and only the hotspot offset is applied.
                    let location = Point::<i32, Physical>::from((
                        (state.pointer_location.x - f64::from(hotspot.x)).round() as i32,
                        (state.pointer_location.y - f64::from(hotspot.y)).round() as i32,
                    ));
                    cursor_surface = render_elements_from_surface_tree(
                        &mut *renderer,
                        surface,
                        location,
                        1.0,
                        1.0,
                        Kind::Cursor,
                    );
                }
                // The client destroyed its cursor surface without setting a new one.
                CursorImageStatus::Surface(_) => {}
            }
            // A cursor surface with no buffer yet would leave the pointer
            // invisible; show the arrow until the client commits one.
            if cursor_shape.is_empty()
                && cursor_surface.is_empty()
                && !matches!(state.cursor_image, CursorImageStatus::Hidden)
            {
                cursor_shape = arrow_cursor(state.pointer_location);
            }
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
                if let Err(err) = draw_render_elements::<GlesRenderer, _, _>(
                    &mut frame,
                    1.0,
                    &cursor_shape,
                    &[damage],
                ) {
                    tracing::error!(?err, "failed to draw the cursor shape");
                }
                if let Err(err) = draw_render_elements::<GlesRenderer, _, _>(
                    &mut frame,
                    1.0,
                    &cursor_surface,
                    &[damage],
                ) {
                    tracing::error!(?err, "failed to draw the client cursor");
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
            for (popup, _) in crate::state::popup_offsets(window) {
                send_frame_callbacks(popup.wl_surface(), time);
            }
        }
        // A client's cursor surface animates like any other (busy spinners).
        if let CursorImageStatus::Surface(surface) = &state.cursor_image {
            send_frame_callbacks(surface, time);
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

/// Where in its own surface a client's cursor image is anchored.
///
/// `wl_pointer.set_cursor` stores the hotspot as smithay's
/// [`CursorImageSurfaceData`] on the surface it names; one that never got it
/// (or was destroyed) is anchored at its origin.
fn hotspot_of(surface: &WlSurface) -> Point<i32, Logical> {
    with_states(surface, |states| {
        states
            .data_map
            .get::<CursorImageSurfaceData>()
            .map(|data| data.lock().unwrap().hotspot)
    })
    .unwrap_or_else(|| (0, 0).into())
}

/// Height of the built-in pointer's head, where its belly meets the stem.
const HEAD_H: i32 = 15;
/// Width of the built-in pointer's stem, in pixels.
const STEM_W: i32 = 4;
/// Total height of the built-in pointer: head plus stem.
const CURSOR_H: i32 = 24;
/// Widest part of the built-in pointer (the belly of its head).
const CURSOR_W: i32 = HEAD_H;

/// Outline and fill of the built-in pointer.
const OUTLINE: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const FILL: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// The built-in pointer at one pixel: `None` off the shape, `Some(true)` on
/// its fill, `Some(false)` on its outline.
///
/// The shape is the classic arrow: a triangular head with a vertical back and
/// a 45° belly, then a stem of the same thickness stepping down and right.
const fn pointer_pixel(x: i32, y: i32) -> Option<bool> {
    if y < HEAD_H {
        // The head is the half of its bounding square below the diagonal.
        if x < 0 || x > y {
            return None;
        }
        // The underside stays open where the stem continues it, so head and
        // stem read as one shape.
        let outline = x == 0 || x == y || y == 0 || (y == HEAD_H - 1 && x >= STEM_W);
        return Some(!outline);
    }
    if y < CURSOR_H {
        let left = y - HEAD_H;
        if x < left || x >= left + STEM_W {
            return None;
        }
        let outline = x == left || x == left + STEM_W - 1 || y == CURSOR_H - 1;
        return Some(!outline);
    }
    None
}

/// The built-in pointer as merged horizontal runs of solid color.
///
/// `origin` is the hotspot — the tip — in logical coordinates. Rows of black
/// and white pixels are all the shape needs, so nothing is uploaded to the GPU
/// and no icon set is required.
fn arrow_cursor(origin: Point<f64, Logical>) -> Vec<SolidColorRenderElement> {
    let (ox, oy) = (origin.x.round() as i32, origin.y.round() as i32);
    let mut elements = Vec::new();
    for y in 0..CURSOR_H {
        let mut x = 0;
        while x < CURSOR_W {
            let Some(inside) = pointer_pixel(x, y) else {
                x += 1;
                continue;
            };
            let start = x;
            while pointer_pixel(x, y) == Some(inside) {
                x += 1;
            }
            let color = if inside { FILL } else { OUTLINE };
            elements.push(SolidColorRenderElement::new(
                Id::new(),
                Rectangle::new((ox + start, oy + y).into(), (x - start, 1).into()),
                CommitCounter::default(),
                Color32F::new(color[0], color[1], color[2], color[3]),
                Kind::Cursor,
            ));
        }
    }
    elements
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tip_sits_at_the_origin() {
        // The hotspot is the top-left pixel of the shape, so nothing is drawn
        // above or left of it.
        assert_eq!(pointer_pixel(0, 0), Some(false));
        assert!(pointer_pixel(-1, 0).is_none());
        assert!(pointer_pixel(0, -1).is_none());
    }

    #[test]
    fn the_head_widens_at_forty_five_degrees() {
        for y in 1..HEAD_H {
            assert_eq!(pointer_pixel(y, y), Some(false), "the belly at y={y}");
            assert!(
                pointer_pixel(y + 1, y).is_none(),
                "beyond the belly at y={y}"
            );
        }
    }

    #[test]
    fn the_stem_is_four_pixels_wide_and_steps_right() {
        for y in HEAD_H..CURSOR_H {
            let left = y - HEAD_H;
            assert!(pointer_pixel(left, y).is_some(), "the stem at y={y}");
            assert!(
                pointer_pixel(left - 1, y).is_none(),
                "left of the stem at y={y}"
            );
            assert!(
                pointer_pixel(left + STEM_W, y).is_none(),
                "right of the stem at y={y}"
            );
        }
        assert!(pointer_pixel(0, CURSOR_H).is_none(), "below the stem");
    }

    #[test]
    fn rows_merge_into_a_handful_of_rects() {
        use smithay::backend::renderer::element::Element as _;

        let rects = arrow_cursor((3.4, -2.6).into());
        // Three runs per row at most (back, belly, fill), so far fewer rects
        // than pixels, and rounding lands the tip on the nearest pixel.
        assert!(rects.len() <= usize::try_from(3 * CURSOR_H).unwrap());
        assert_eq!(
            rects[0].geometry(1.0.into()).loc,
            Point::<i32, Physical>::from((3, -3))
        );
    }
}

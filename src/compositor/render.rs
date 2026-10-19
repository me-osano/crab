//! Drawing one frame: clear, borders, every visible surface, then the cursor.
//!
//! M1 was a full-surface redraw with no damage tracking and no z-ordering. M2
//! adds the parts of the scene the pure layout engine deliberately does not
//! describe: **borders** around tiled and floating windows, the **z-order**
//! (tiled below floating below fullscreen below popups), skipping the windows
//! the current view hides (`COMPOSITOR.md` §7.1), and the compositor's own
//! **cursor**. Real damage tracking lands later.
//!
//! Windows are drawn at their **animated** rectangle (`ManagedWindow::visual_*`),
//! sampled from the frame clock the backend has just re-timed, so the borders
//! and the surfaces move together and the frame timestamps clients get refer to
//! the same instant the motion was sampled at (TODO.md §3.2).
//!
//! A window's *buffer* is a different size from that rectangle for as long as
//! the layout is moving it, and sometimes for longer: a client takes a few
//! frames to answer a configure, and some clients never do. Each window's
//! surface is therefore mapped onto its rectangle rather than drawn at its own
//! size — stretched when the rectangle has grown past what the client
//! committed, cropped when it has shrunk below it — so the content is never a
//! frame behind the border and never spills into a neighbour (TODO.md §3.5).
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
use smithay::backend::renderer::element::{Element, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::draw_render_elements;
use smithay::backend::renderer::utils::{CommitCounter, DamageSet};
use smithay::backend::renderer::{Frame as _, ImportAll, Renderer, Texture};
use smithay::backend::winit::WinitGraphicsBackend;
use smithay::input::pointer::{CursorImageStatus, CursorImageSurfaceData};
use smithay::reexports::wayland_server::Resource as _;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{
    Buffer as BufferCoords, Logical, Physical, Point, Rectangle, Scale, Size, Transform,
};
use smithay::wayland::compositor::{
    SurfaceAttributes, TraversalAction, with_states, with_surface_tree_downward,
};

use crate::compositor::state::{Crawm, ManagedWindow};

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
        // window surfaces stay above their own frames, and the popups are
        // separate again because they keep their own size: they are placed by
        // the client against the window geometry, not by the layout.
        let mut borders: Vec<SolidColorRenderElement> = Vec::new();
        let mut surfaces: Vec<FittedSurface<GlesRenderer>> = Vec::new();
        let mut popups: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = Vec::new();

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
            let origin = window.visual_origin(&state.frame_clock);
            for (popup, offset) in crate::compositor::state::popup_offsets(window) {
                popups.extend(render_elements_from_surface_tree(
                    &mut *renderer,
                    popup.wl_surface(),
                    (origin + offset).to_physical(1),
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
                if let Err(err) =
                    draw_render_elements::<GlesRenderer, _, _>(&mut frame, 1.0, &popups, &[damage])
                {
                    tracing::error!(?err, "failed to draw the popups");
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

        let time = state.frame_clock.now_ms();
        for window in state
            .windows
            .iter()
            .filter(|window| window.mapped && window.visible)
        {
            send_frame_callbacks(window.wl_surface(), time);
            for (popup, _) in crate::compositor::state::popup_offsets(window) {
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
    surfaces: &mut Vec<FittedSurface<GlesRenderer>>,
) {
    for window in windows {
        let location = window.visual_origin(&state.frame_clock);
        let size = window.visual_size(&state.frame_clock);
        // A fullscreen or maximized window covers the screen, so a border would
        // only clip it.
        if border_width > 0 && !window.window.fullscreen && !window.window.maximized {
            let color = if focused == Some(window.id()) {
                border_color(state.config.border_color)
            } else {
                border_color(state.config.unfocused_border_color)
            };
            borders.extend(borders_for(location, size, border_width, color));
        }
        // The toplevel is drawn from its geometry origin pulled back out by the
        // client's own decoration inset, so the *window geometry* — what the
        // layout placed and what `borders_for` frames — lands on the rect.
        // `WindowFit` then maps whatever the client has committed onto that
        // rect, which is the same thing while it is the size it was asked to
        // be.
        let fit = WindowFit {
            drawn: Rectangle::new(location.to_physical(1), size.to_physical(1)),
            committed: window.geometry.size,
            may_stretch: !window.is_size_refusing(),
        };
        surfaces.extend(fitted_surface_tree(
            renderer,
            window.wl_surface(),
            window.visual_surface_origin(&state.frame_clock),
            fit,
        ));
    }
}

/// Converts a config color into the renderer's representation.
fn border_color(color: Color) -> Color32F {
    Color32F::new(color.r, color.g, color.b, color.a)
}

/// The surface elements of one toplevel, each mapped onto the rectangle the
/// window is being drawn at.
///
/// They come from smithay at the window's surface origin — the rectangle's
/// origin pulled back by the client's decoration inset — and [`WindowFit`]
/// then moves and cuts them.
fn fitted_surface_tree(
    renderer: &mut GlesRenderer,
    surface: &WlSurface,
    location: Point<i32, Logical>,
    fit: WindowFit,
) -> Vec<FittedSurface<GlesRenderer>> {
    let elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
        render_elements_from_surface_tree(
            renderer,
            surface,
            location.to_physical(1),
            1.0,
            1.0,
            Kind::Unspecified,
        );
    elements
        .into_iter()
        .filter_map(|inner| {
            let natural = inner.geometry(Scale::from(1.0));
            let (src, dst) = fit.map_element(natural, inner.src())?;
            Some(FittedSurface { inner, src, dst })
        })
        .collect()
}

/// Where one window is being drawn this frame, and how big its client has
/// committed the window geometry to be.
#[derive(Clone, Copy, Debug)]
struct WindowFit {
    /// The rectangle the window is drawn at: its animated placement.
    drawn: Rectangle<i32, Physical>,
    /// The size the client last committed for its window geometry, which is
    /// what its buffer has to say for itself.
    committed: Size<i32, Logical>,
    /// Whether a rectangle bigger than [`Self::committed`] may be filled by
    /// growing the pixels the client drew.
    ///
    /// It may only while the client is being asked for exactly this rectangle:
    /// then the growth is the animation catching up and it is gone within the
    /// resize. A window that refused the size is cropped instead, because
    /// stretched pixels are pixels the client is not drawing — it would be
    /// answering clicks in places it never painted.
    may_stretch: bool,
}

impl WindowFit {
    /// Maps one element of the window's surface tree onto the rectangle.
    ///
    /// `natural` is where smithay would have drawn the element — its position
    /// in the tree, with the window geometry landing on the rectangle — and
    /// `src` the part of the buffer it samples. Nothing of the element inside
    /// the rectangle yields nothing: an element wholly outside it is not
    /// drawn.
    fn map_element(
        &self,
        natural: Rectangle<i32, Physical>,
        src: Rectangle<f64, BufferCoords>,
    ) -> Option<(Rectangle<f64, BufferCoords>, Rectangle<i32, Physical>)> {
        let x = Axis::fit(
            (f64::from(self.drawn.loc.x), f64::from(self.drawn.size.w)),
            f64::from(self.committed.w),
            self.may_stretch,
        );
        let y = Axis::fit(
            (f64::from(self.drawn.loc.y), f64::from(self.drawn.size.h)),
            f64::from(self.committed.h),
            self.may_stretch,
        );

        let ((dx, dw), (cut_x_lo, cut_x_hi)) = x.cut(
            x.map(f64::from(natural.loc.x)),
            f64::from(natural.size.w) * x.scale,
        )?;
        let ((dy, dh), (cut_y_lo, cut_y_hi)) = y.cut(
            y.map(f64::from(natural.loc.y)),
            f64::from(natural.size.h) * y.scale,
        )?;

        // How much of the buffer one drawn pixel of this element covers, which
        // is its buffer scale for an ordinary surface. Pixels cut from the
        // drawing take their share of the buffer with them; a stretched axis
        // cuts none, so the whole of it is still sampled.
        let (sw, sh) = (
            src.size.w / f64::from(natural.size.w).max(1.0),
            src.size.h / f64::from(natural.size.h).max(1.0),
        );
        let src = Rectangle::new(
            Point::from((src.loc.x + cut_x_lo * sw, src.loc.y + cut_y_lo * sh)),
            Size::from((
                src.size.w - (cut_x_lo + cut_x_hi) * sw,
                src.size.h - (cut_y_lo + cut_y_hi) * sh,
            )),
        );
        let dst = Rectangle::new(
            Point::from((dx, dy)).to_i32_round(),
            Size::from((dw, dh)).to_i32_round(),
        );
        Some((src, dst))
    }
}

/// One axis of a window's fit: how a coordinate on the client's surface moves
/// when it is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Axis {
    /// What is added to a surface coordinate times [`Self::scale`].
    offset: f64,
    /// Multiplier from surface pixels to drawn pixels.
    scale: f64,
    /// The extent the surface is cut to on this axis, present only when the
    /// window is cropped rather than stretched.
    cut: Option<(f64, f64)>,
}

impl Axis {
    /// Fits one axis of a window whose rectangle spans `origin` for `size`
    /// pixels and whose client committed `committed` of them.
    ///
    /// A rectangle the client has not filled yet is filled by growing what it
    /// drew, anchored on the rectangle's origin so the window's top-left
    /// corner stays put: the pixels that spill past the far edge are the
    /// client's own decoration shadows, which are meant to spill. A rectangle
    /// the client has overfilled is cut back, pixel for pixel, so nothing
    /// spills into whatever sits beside it.
    ///
    /// A window at rest is neither: it is drawn exactly as the client drew it,
    /// which is what keeps a client's decoration shadows around its edges.
    fn fit((origin, size): (f64, f64), committed: f64, may_stretch: bool) -> Self {
        if committed > 0.0 && size > committed && may_stretch {
            let scale = size / committed;
            // The map is `origin + (p - origin) * scale`, which over surface
            // coordinates is the affine `offset + p * scale`.
            return Axis {
                offset: origin - origin * scale,
                scale,
                cut: None,
            };
        }
        let cut = (committed > size).then_some((origin, origin + size));
        Axis {
            offset: 0.0,
            scale: 1.0,
            cut,
        }
    }

    /// Where a coordinate on the client's surface lands on this axis.
    fn map(&self, value: f64) -> f64 {
        self.offset + value * self.scale
    }

    /// Cuts the drawn extent `[at, at + size)` to the part the window's
    /// rectangle covers: what is left, and how much was taken from each end.
    ///
    /// An axis that is not cut keeps the whole extent, decorations and all; an
    /// element lying wholly past the rectangle's edges is cut to nothing.
    fn cut(&self, at: f64, size: f64) -> Option<((f64, f64), (f64, f64))> {
        let Some((floor, ceiling)) = self.cut else {
            return Some(((at, size), (0.0, 0.0)));
        };
        let (lo, hi) = (at, at + size);
        let kept_lo = lo.max(floor);
        let kept_hi = hi.min(ceiling);
        (kept_hi > kept_lo).then_some(((kept_lo, kept_hi - kept_lo), (kept_lo - lo, hi - kept_hi)))
    }
}

/// A surface element mapped onto the rectangle its window is drawn at.
///
/// Smithay's own element is the size of the client's buffer, which is not the
/// size of the window's rectangle while the layout is moving it or while the
/// client is behind. This one reports a different source and destination, so
/// the frame draws the client's pixels *into* the window's rectangle; every
/// other property — the element's id, its commit, its alpha — stays the
/// client's.
struct FittedSurface<R>
where
    R: Renderer + ImportAll,
    R::TextureId: Texture + 'static,
{
    /// The element smithay built for one surface of the tree.
    inner: WaylandSurfaceRenderElement<R>,
    /// What to take of the client's buffer.
    src: Rectangle<f64, BufferCoords>,
    /// Where to put it.
    dst: Rectangle<i32, Physical>,
}

impl<R> Element for FittedSurface<R>
where
    R: Renderer + ImportAll,
    R::TextureId: Texture + 'static,
{
    fn id(&self) -> &Id {
        self.inner.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn src(&self) -> Rectangle<f64, BufferCoords> {
        self.src
    }

    fn transform(&self) -> Transform {
        self.inner.transform()
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.dst
    }

    fn damage_since(
        &self,
        _scale: Scale<f64>,
        _commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        // The mapping moves with every animated frame, so damage from an
        // earlier commit says nothing about where this element draws now.
        // Real damage tracking (M5) will have to recompute it from the fit.
        DamageSet::from_slice(&[Rectangle::from_size(self.dst.size)])
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl<R> RenderElement<R> for FittedSurface<R>
where
    R: Renderer + ImportAll,
    R::TextureId: Texture + 'static,
{
    fn draw(
        &self,
        frame: &mut R::Frame<'_, '_>,
        _src: Rectangle<f64, BufferCoords>,
        _dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), R::Error> {
        // Whatever was computed from `Element` above is what the client's
        // buffer is mapped through, so the frame draws exactly what the fit
        // describes and the damage that goes with it.
        self.inner
            .draw(frame, self.src, self.dst, damage, opaque_regions)
    }

    fn underlying_storage(&self, renderer: &mut R) -> Option<UnderlyingStorage<'_>> {
        self.inner.underlying_storage(renderer)
    }
}

/// Four solid rectangles that frame a border width around a drawn window, in
/// physical pixels.
fn borders_for(
    location: Point<i32, Logical>,
    size: Size<i32, Logical>,
    bw: i32,
    color: Color32F,
) -> Vec<SolidColorRenderElement> {
    let x = location.x;
    let y = location.y;
    let width = size.w;
    let height = size.h;
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

/// Width of the built-in pointer's bounding box.
const CURSOR_W: i32 = 12;
/// Height of the built-in pointer, from its tip to the end of its tail.
const CURSOR_H: i32 = 19;
/// How thick the black edge around the arrow is drawn, in pixels.
const EDGE_W: f64 = 1.0;

/// Outline of the built-in pointer: the classic arrow, tip at the origin.
///
/// From the tip the back edge runs straight down, the tail notches in and juts
/// back out, and the barb comes up to meet the hypotenuse — the silhouette
/// every cursor theme draws, and the reason a bare triangle reads as a smudge.
/// Drawn at the same size as the arrow inside a 24×24 icon set.
const ARROW: [(f64, f64); 7] = [
    (0.0, 0.0),
    (0.0, 16.0),
    (4.5, 12.5),
    (7.5, 19.0),
    (10.0, 17.5),
    (6.5, 11.5),
    (12.0, 11.5),
];

/// Outline and fill of the built-in pointer.
const OUTLINE: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const FILL: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// Whether the point `(px, py)` is inside the arrow, by the even-odd rule.
fn inside_arrow(px: f64, py: f64) -> bool {
    let mut inside = false;
    // Walking each edge against the vertex before it, back edge included.
    let mut previous = ARROW[ARROW.len() - 1];
    for &(xi, yi) in &ARROW {
        let (xj, yj) = previous;
        if (yi > py) != (yj > py) && px < (xj - xi) * (py - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        previous = (xi, yi);
    }
    inside
}

/// Distance from `(px, py)` to the arrow's outline.
fn outline_distance(px: f64, py: f64) -> f64 {
    let mut best = f64::INFINITY;
    let mut previous = ARROW[ARROW.len() - 1];
    for &point in &ARROW {
        best = best.min(segment_distance(px, py, previous, point));
        previous = point;
    }
    best
}

/// Distance from `(px, py)` to the segment `a`–`b`.
fn segment_distance(px: f64, py: f64, a: (f64, f64), b: (f64, f64)) -> f64 {
    let (ax, ay) = a;
    let (bx, by) = b;
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        ((px - ax) * dx + (py - ay) * dy) / len2
    }
    .clamp(0.0, 1.0);
    let (cx, cy) = (ax + t * dx, ay + t * dy);
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

/// The built-in pointer at one pixel: `None` off the shape, `Some(true)` on
/// its fill, `Some(false)` on its outline.
///
/// Whatever lies within [`EDGE_W`] of the outline stays black, so the arrow
/// comes out as the black rim with a white core that the classic bitmap
/// cursors draw, thin tail included.
fn pointer_pixel(x: i32, y: i32) -> Option<bool> {
    // Sample pixel centres so whole-pixel measurements land on whole pixels.
    let (px, py) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
    if !inside_arrow(px, py) {
        return None;
    }
    Some(outline_distance(px, py) >= EDGE_W)
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
/// The timestamp is the frame clock's, i.e. the presentation time of the frame
/// the client's callback fired in — the same instant every animation drawn in
/// this frame was sampled at.
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

    /// A window drawn at `drawn`, its client having committed `committed`.
    fn fit(drawn: (i32, i32, i32, i32), committed: (i32, i32), may_stretch: bool) -> WindowFit {
        WindowFit {
            drawn: Rectangle::new((drawn.0, drawn.1).into(), (drawn.2, drawn.3).into()),
            committed: (committed.0, committed.1).into(),
            may_stretch,
        }
    }

    /// One element of a surface tree at `loc`, `size` big, sampling all of a
    /// `buffer`-sized buffer.
    fn element(
        loc: (i32, i32),
        size: (i32, i32),
        buffer: (f64, f64),
    ) -> (Rectangle<i32, Physical>, Rectangle<f64, BufferCoords>) {
        (
            Rectangle::new((loc.0, loc.1).into(), (size.0, size.1).into()),
            Rectangle::new((0.0, 0.0).into(), buffer.into()),
        )
    }

    #[test]
    fn a_window_at_rest_is_drawn_exactly_as_the_client_drew_it() {
        // A client-side-decorated window: its buffer is the geometry with a
        // 25 pixel shadow margin on every side, and at rest none of that may
        // move or shrink, shadows included.
        let (natural, src) = element((75, 175), (519, 1080), (519.0, 1080.0));
        let (src, dst) = fit((100, 200, 469, 1030), (469, 1030), true)
            .map_element(natural, src)
            .expect("the whole window is beside its rectangle");
        assert_eq!(dst, natural);
        assert_eq!(src, element((0, 0), (0, 0), (519.0, 1080.0)).1);
    }

    #[test]
    fn a_rectangle_the_client_has_not_filled_is_stretched_over() {
        // The rectangle has grown to twice what the client committed: the
        // buffer covers all of it, sampled from all of itself.
        let (natural, src) = element((100, 200), (469, 1030), (469.0, 1030.0));
        let (src, dst) = fit((100, 200, 940, 1030), (469, 1030), true)
            .map_element(natural, src)
            .expect("the window is inside its rectangle");
        assert_eq!(dst, Rectangle::new((100, 200).into(), (940, 1030).into()));
        assert_eq!(src, element((0, 0), (0, 0), (469.0, 1030.0)).1);
    }

    #[test]
    fn a_growing_rectangle_keeps_the_windows_top_left_corner() {
        // Stretching is anchored on the origin, so the geometry's top-left
        // corner stays exactly where it was. Everything else scales with it,
        // decoration shadows included: the margin above and left is still
        // above and left, just twice as wide.
        let (natural, src) = element((75, 175), (519, 1080), (519.0, 1080.0));
        let (_, dst) = fit((100, 200, 940, 1030), (469, 1030), true)
            .map_element(natural, src)
            .expect("the window is beside its rectangle");
        // The geometry's origin lands on the rectangle's origin, and the whole
        // element scales by 940/469 horizontally and 1:1 vertically.
        assert_eq!(dst.loc.x, 50);
        assert_eq!(dst.loc.y, 175);
        assert_eq!(dst.size.w, 1040);
        assert_eq!(dst.size.h, 1080);
    }

    #[test]
    fn a_rectangle_the_client_overfills_is_cut_back_to_it() {
        // The client committed 940 and the rectangle is 469 wide: nothing may
        // be drawn past the rectangle's right edge, and the buffer is sampled
        // from the part of itself that would have been there.
        let (natural, src) = element((100, 200), (940, 1030), (940.0, 1030.0));
        let (src, dst) = fit((100, 200, 469, 1030), (940, 1030), true)
            .map_element(natural, src)
            .expect("the window's start is inside its rectangle");
        assert_eq!(dst, Rectangle::new((100, 200).into(), (469, 1030).into()));
        assert_eq!(src, element((0, 0), (0, 0), (469.0, 1030.0)).1);
    }

    #[test]
    fn cutting_takes_the_pixels_behind_it_with_it() {
        // The same window with its shadow margins: the cut lands on the
        // rectangle's edges, so what survives horizontally is the geometry and
        // the margin before it — and the buffer is sampled from that, not from
        // its start. The other axis is left alone, since there the client
        // filled exactly what was asked, shadows included.
        let (natural, src) = element((75, 175), (990, 1080), (990.0, 1080.0));
        let (src, dst) = fit((100, 200, 469, 1030), (940, 1030), true)
            .map_element(natural, src)
            .expect("the window's start is inside its rectangle");
        assert_eq!(dst, Rectangle::new((100, 175).into(), (469, 1080).into()));
        assert_eq!(src.loc.x, 25.0);
        assert_eq!(src.size.w, 469.0);
        assert_eq!(src.loc.y, 0.0);
        assert_eq!(src.size.h, 1080.0);
    }

    #[test]
    fn a_client_that_refused_the_size_is_not_stretched() {
        // It committed 469 while its rectangle is 940 wide and is not going to
        // grow: its pixels stay its own size, and the empty part of the
        // rectangle is the honest picture of a client that said no. Stretched
        // pixels would put the pointer where nothing was drawn.
        let (natural, src) = element((100, 200), (469, 1030), (469.0, 1030.0));
        let (src, dst) = fit((100, 200, 940, 1030), (469, 1030), false)
            .map_element(natural, src)
            .expect("the window is inside its rectangle");
        assert_eq!(dst, natural);
        assert_eq!(src, element((0, 0), (0, 0), (469.0, 1030.0)).1);
    }

    #[test]
    fn an_element_beyond_the_rectangle_is_not_drawn() {
        // A subsurface sitting past the right edge of a window that overfills
        // its rectangle has nothing left of it inside.
        let (natural, src) = element((700, 300), (100, 100), (100.0, 100.0));
        assert!(
            fit((100, 200, 469, 1030), (940, 1030), true)
                .map_element(natural, src)
                .is_none()
        );
    }

    #[test]
    fn the_tip_sits_at_the_origin() {
        // The hotspot is the top-left pixel of the shape, so nothing is drawn
        // above or left of it.
        assert_eq!(pointer_pixel(0, 0), Some(false));
        assert!(pointer_pixel(-1, 0).is_none());
        assert!(pointer_pixel(0, -1).is_none());
    }

    #[test]
    fn the_arrow_stays_inside_its_bounding_box() {
        for y in 0..CURSOR_H {
            assert!(
                pointer_pixel(-1, y).is_none(),
                "nothing left of the back edge at y={y}"
            );
            assert!(
                pointer_pixel(CURSOR_W, y).is_none(),
                "nothing past the barb at y={y}"
            );
        }
        for x in -1..=CURSOR_W {
            assert!(
                pointer_pixel(x, -1).is_none(),
                "nothing above the tip at x={x}"
            );
            assert!(
                pointer_pixel(x, CURSOR_H).is_none(),
                "nothing below the tail at x={x}"
            );
        }
    }

    #[test]
    fn the_back_edge_is_straight_from_the_tip() {
        for y in 0..16 {
            assert_eq!(pointer_pixel(0, y), Some(false), "the back edge at y={y}");
        }
        assert!(
            pointer_pixel(0, 16).is_none(),
            "the back edge ends at (0,16)"
        );
    }

    #[test]
    fn the_tail_notches_in_below_the_head() {
        // Below the head's belly the back edge keeps going while the tail has
        // already stepped right, leaving the notch empty between them.
        assert!(pointer_pixel(0, 14).is_some(), "the back edge continues");
        assert!(pointer_pixel(2, 14).is_none(), "the notch under the head");
        assert!(pointer_pixel(6, 14).is_some(), "the tail past the notch");
        assert!(pointer_pixel(7, CURSOR_H - 1).is_some(), "the tail's tip");
        assert!(pointer_pixel(9, CURSOR_H - 1).is_none(), "and no wider");
    }

    #[test]
    fn the_barb_is_the_widest_row() {
        // The barb is level with where the tail leaves the head, and nothing
        // else reaches that far right.
        assert_eq!(pointer_pixel(10, 10), Some(false));
        assert!(pointer_pixel(11, 10).is_none());
        assert!(pointer_pixel(10, 0).is_none());
    }

    #[test]
    fn the_body_is_filled_inside_a_black_edge() {
        let mut fill = 0;
        let mut edge = 0;
        for y in 0..CURSOR_H {
            for x in 0..CURSOR_W {
                match pointer_pixel(x, y) {
                    Some(true) => fill += 1,
                    Some(false) => edge += 1,
                    None => {}
                }
            }
        }
        assert!(fill > 20, "a white core, got {fill} pixels");
        assert!(edge > 20, "a black edge around it, got {edge} pixels");
    }

    #[test]
    fn rows_merge_into_a_handful_of_rects() {
        let rects = arrow_cursor((3.4, -2.6).into());
        let drawn = (0..CURSOR_H)
            .flat_map(|y| (0..CURSOR_W).map(move |x| (x, y)))
            .filter(|&(x, y)| pointer_pixel(x, y).is_some())
            .count();
        // Rows merge same-colored runs, so far fewer rects than pixels, and at
        // most six runs per row (back edge, head, notch, tail, barb, tip).
        assert!(
            rects.len() < drawn,
            "{} rects for {drawn} pixels",
            rects.len()
        );
        assert!(rects.len() <= usize::try_from(6 * CURSOR_H).unwrap());
        assert_eq!(
            rects[0].geometry(1.0.into()).loc,
            Point::<i32, Physical>::from((3, -3))
        );
    }
}

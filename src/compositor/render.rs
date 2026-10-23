//! Drawing one frame: borders, every visible surface, then the cursor.
//!
//! The whole scene is one ordered list of [`SceneElement`]s, handed to
//! Smithay's [`OutputDamageTracker`](smithay::backend::renderer::damage::OutputDamageTracker).
//! The tracker clears and repaints only what changed, subtracts opaque content
//! from the layers below, and reports which elements were actually presented.
//! When it finds no damage the frame is not submitted and no `wl_surface.frame`
//! callbacks fire for a picture the clients never saw.
//!
//! The scene includes the parts the pure layout engine deliberately does not
//! describe: **borders** around tiled and floating windows, the **z-order**
//! (tiled below floating below fullscreen below popups), skipping the windows
//! the current view hides (`COMPOSITOR.md` §7.1), fading-out windows in their
//! former stacking group, and the compositor's own **cursor**.
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

use crab::animation::Clocks;
use crab::config::AnimationStyle;
use crab::core::Color;
use crab::core::{OutputId, WindowId};
use smithay::backend::SwapBuffersError;
use smithay::backend::renderer::Color32F;
use smithay::backend::renderer::element::Id;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::RenderElementStates;
use smithay::backend::renderer::element::render_elements;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::surface::{
    WaylandSurfaceRenderElement, render_elements_from_surface_tree,
};
use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::element::{Element, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{GlesFrame, GlesRenderer, GlesTarget, GlesTexture};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::backend::renderer::{ImportAll, Renderer, Texture};
use smithay::backend::winit::WinitGraphicsBackend;
use smithay::desktop::{LayerMap, PopupManager, layer_map_for_output};
use smithay::input::pointer::{CursorImageStatus, CursorImageSurfaceData};
use smithay::reexports::wayland_server::Resource as _;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{
    Buffer as BufferCoords, Logical, Physical, Point, Rectangle, Scale, Size, Transform,
};
use smithay::wayland::compositor::{
    SurfaceAttributes, TraversalAction, with_states, with_surface_tree_downward,
};
use smithay::wayland::shell::wlr_layer::Layer;

use crate::compositor::fit::WindowFit;
use crate::compositor::state::{ClosingWindow, Crab, ManagedWindow, WindowLayer};

/// Background color of the compositor (dark slate).
const BACKGROUND: [f32; 4] = [0.05, 0.06, 0.09, 1.0];

/// How far a sliding window falls while it closes, in logical pixels.
const CLOSE_SLIDE: f64 = 16.0;

/// How small a zooming window gets by the end of its close.
const CLOSE_ZOOM: f64 = 0.94;

render_elements! {
    /// Every kind of element the scene is made of, so a whole frame is one
    /// ordered list. That is what lets Smithay's damage tracker account for
    /// opaque regions and changed elements across borders, surfaces, snapshots
    /// and the cursor together.
    SceneElement<=GlesRenderer>;
    Solid=SolidColorRenderElement,
    Surface=WaylandSurfaceRenderElement<GlesRenderer>,
    Fitted=FittedSurface<GlesRenderer>,
    Closing=ClosingSurface,
}

/// A backend that can present one output's frame.
///
/// The winit backend implements this for its single nested output. The DRM
/// backend will implement it per connector, so the scene and damage code below
/// is shared by both.
pub(crate) trait OutputBackend {
    /// The error a failed bind or presentation reports.
    type Error: std::fmt::Debug;

    /// The output's current physical size.
    fn output_size(&self) -> Size<i32, Physical>;

    /// How many frames old the buffer being drawn into is, if known.
    fn buffer_age(&self) -> Option<usize>;

    /// Binds the renderer to the framebuffer for this frame.
    fn bind(&mut self) -> Result<(&mut GlesRenderer, GlesTarget<'_>), Self::Error>;

    /// Presents the bound frame with the given damage.
    fn submit(&mut self, damage: Option<&[Rectangle<i32, Physical>]>) -> Result<(), Self::Error>;
}

impl OutputBackend for WinitGraphicsBackend<GlesRenderer> {
    type Error = SwapBuffersError;

    fn output_size(&self) -> Size<i32, Physical> {
        self.window_size()
    }

    fn buffer_age(&self) -> Option<usize> {
        WinitGraphicsBackend::buffer_age(self)
    }

    fn bind(&mut self) -> Result<(&mut GlesRenderer, GlesTarget<'_>), Self::Error> {
        WinitGraphicsBackend::bind(self)
    }

    fn submit(&mut self, damage: Option<&[Rectangle<i32, Physical>]>) -> Result<(), Self::Error> {
        WinitGraphicsBackend::submit(self, damage)
    }
}

/// Renders the primary output into the winit window and presents it.
pub(crate) fn render(state: &mut Crab, backend: &mut WinitGraphicsBackend<GlesRenderer>) {
    let output = state.primary_output;
    render_output(state, output, backend);
}

/// Renders one output's whole scene through `backend` and presents it.
pub(crate) fn render_output<B: OutputBackend>(state: &mut Crab, output: OutputId, backend: &mut B) {
    let size = backend.output_size();
    tracing::trace!(width = size.w, height = size.h, "rendering a frame");
    // The buffer we are about to draw into may still hold an older frame. The
    // damage tracker must repaint everything that changed since that frame, not
    // just since the previous one. `0` means "nothing is known", which makes the
    // tracker repaint the whole output.
    let age = backend.buffer_age().unwrap_or(0);

    let (renderer, mut framebuffer) = match backend.bind() {
        Ok(bound) => bound,
        Err(err) => {
            tracing::error!(?err, "failed to bind the framebuffer");
            return;
        }
    };

    // Positions are logical; with M2's `Scale::Integer(1)` output they map
    // one-to-one onto the renderer's physical pixels.
    let output_origin = state.outputs.get(&output).map_or_else(
        || Point::<i32, Logical>::from((0, 0)),
        |runtime| {
            Point::from((
                runtime.model.geometry.x.round() as i32,
                runtime.model.geometry.y.round() as i32,
            ))
        },
    );
    let border_width = state
        .outputs
        .get(&output)
        .and_then(|runtime| runtime.model.current_space_state())
        .map_or(0.0, |space| space.params.border_width)
        .max(0.0) as i32;
    let focused = state.focused_id();

    // Build the scene back-to-front. The damage tracker wants front-to-back, so
    // the list is reversed once, after everything has been collected.
    let mut elements: Vec<SceneElement> = Vec::new();
    let mut frame_surfaces: Vec<WlSurface> = Vec::new();

    // Layer-shell surfaces are arranged by Smithay's layer map. Background and
    // bottom layers go under ordinary windows; top and overlay layers go over
    // them and over window popups, matching the protocol's order.
    let layer_map = {
        let Some(runtime) = state.outputs.get(&output) else {
            return;
        };
        layer_map_for_output(&runtime.wayland)
    };
    collect_layer_pass(
        state,
        &mut *renderer,
        &layer_map,
        &[Layer::Background, Layer::Bottom],
        &mut elements,
    );

    // Tiled, floating, always-on-top floating, then fullscreen/maximized. The
    // floating groups are ordered by the focus stack, least recently focused
    // first, so focusing a floating window raises it above its peers.
    let windows = state.stacked_windows_on(output);
    for layer in [
        WindowLayer::Tiled,
        WindowLayer::Floating,
        WindowLayer::FloatingOnTop,
        WindowLayer::Covering,
    ] {
        collect_window_layer(
            state,
            &mut *renderer,
            output,
            output_origin,
            border_width,
            focused,
            layer,
            &windows,
            &mut elements,
        );
    }

    // Popups draw over every toplevel. Each popup belongs to a toplevel
    // (`PopupManager` tracks the tree), so they are gathered per window; a popup
    // without a buffer yet yields no elements.
    for id in &windows {
        let Some(window) = state.window(*id) else {
            continue;
        };
        if window.window.output != output {
            continue;
        }
        let origin = window.visual_origin(&state.frame_clock) - output_origin;
        let alpha = window.open_alpha(&state.frame_clock);
        for (popup, offset) in crate::compositor::state::popup_offsets(window) {
            elements.extend(
                render_elements_from_surface_tree(
                    &mut *renderer,
                    popup.wl_surface(),
                    (origin + offset).to_physical(1),
                    1.0,
                    alpha,
                    Kind::Unspecified,
                )
                .into_iter()
                .map(SceneElement::Surface),
            );
        }
    }

    collect_layer_pass(
        state,
        &mut *renderer,
        &layer_map,
        &[Layer::Top, Layer::Overlay],
        &mut elements,
    );

    // Frame callbacks are only for surfaces that made it into a presented
    // frame. Collect the surface roots now; the per-surface check happens after
    // rendering, against the tracker's presentation states.
    for id in &windows {
        let Some(window) = state.window(*id) else {
            continue;
        };
        if !window.mapped || !window.visible || window.window.output != output {
            continue;
        }
        frame_surfaces.push(window.wl_surface().clone());
        for (popup, _) in crate::compositor::state::popup_offsets(window) {
            frame_surfaces.push(popup.wl_surface().clone());
        }
    }
    for layer in layer_map.layers() {
        frame_surfaces.push(layer.wl_surface().clone());
        for (popup, _) in PopupManager::popups_for_surface(layer.wl_surface()) {
            frame_surfaces.push(popup.wl_surface().clone());
        }
    }
    drop(layer_map);

    // And the cursor on top of all of it. A client's own cursor surface and the
    // built-in arrow are mutually exclusive; the arrow is made of solid runs so
    // no icon set or texture is needed.
    if output == state.primary_output && state.pointer_seen {
        let mut drew_cursor = false;
        match &state.cursor_image {
            CursorImageStatus::Hidden => {}
            CursorImageStatus::Named(_) => {
                // `wp_cursor_shape` is not advertised yet, so a named icon is
                // Smithay's default. Clients that want text or resize cursors
                // send their own surface through `wl_pointer.set_cursor` until
                // icon-theme loading lands.
                elements.extend(
                    arrow_cursor(state.pointer_location)
                        .into_iter()
                        .map(SceneElement::Solid),
                );
                drew_cursor = true;
            }
            CursorImageStatus::Surface(surface) if surface.is_alive() => {
                let hotspot = hotspot_of(surface);
                // Everything is drawn at scale 1, so logical and physical pixels
                // are the same and only the hotspot offset is applied.
                let location = Point::<i32, Physical>::from((
                    (state.pointer_location.x - f64::from(hotspot.x)).round() as i32,
                    (state.pointer_location.y - f64::from(hotspot.y)).round() as i32,
                ));
                let cursor = render_elements_from_surface_tree(
                    &mut *renderer,
                    surface,
                    location,
                    1.0,
                    1.0,
                    Kind::Cursor,
                );
                drew_cursor = !cursor.is_empty();
                elements.extend(cursor.into_iter().map(SceneElement::Surface));
                frame_surfaces.push(surface.clone());
            }
            // The client destroyed its cursor surface without setting a new one.
            CursorImageStatus::Surface(_) => {}
        }
        // A cursor surface with no buffer yet would leave the pointer invisible;
        // show the arrow until the client commits one.
        if !drew_cursor && !matches!(state.cursor_image, CursorImageStatus::Hidden) {
            elements.extend(
                arrow_cursor(state.pointer_location)
                    .into_iter()
                    .map(SceneElement::Solid),
            );
        }
    }

    // The scene was collected bottom-to-top; the tracker walks it top-to-bottom
    // so it can subtract opaque regions before deciding what is damaged.
    elements.reverse();

    let Some(runtime) = state.outputs.get_mut(&output) else {
        return;
    };
    let (damage, states) = match runtime.damage_tracker.render_output(
        &mut *renderer,
        &mut framebuffer,
        age,
        &elements,
        BACKGROUND,
    ) {
        Ok(result) => (result.damage.cloned(), result.states),
        Err(err) => {
            tracing::error!(?err, "failed to render the output");
            return;
        }
    };

    // Nothing changed: do not present a new frame, and do not fire frame
    // callbacks for a frame the clients never saw.
    let Some(damage) = damage.filter(|damage| !damage.is_empty()) else {
        return;
    };

    // The framebuffer borrows `backend`; end it before asking it to swap.
    drop(framebuffer);

    tracing::trace!(
        damage = ?damage,
        elements = elements.len(),
        surfaces = states.states.len(),
        "presenting a frame"
    );

    // Present first: a frame callback promises that its surface was shown, so
    // it must not fire if the swap fails.
    if let Err(err) = backend.submit(Some(&damage)) {
        tracing::error!(?err, "failed to submit the frame");
        return;
    }

    let time = state.frame_clock.now_ms();
    for surface in &frame_surfaces {
        send_frame_callbacks(surface, time, &states);
    }
}

/// Appends one layer-shell pass (background/bottom or top/overlay), including
/// each surface's popups.
fn collect_layer_pass(
    state: &Crab,
    renderer: &mut GlesRenderer,
    map: &LayerMap,
    kinds: &[Layer],
    out: &mut Vec<SceneElement>,
) {
    for &kind in kinds {
        for layer in map.layers_on(kind) {
            let Some(geometry) = map.layer_geometry(layer) else {
                continue;
            };
            let origin = geometry.loc + state.layer_offset(layer.wl_surface());
            let alpha = state.layer_alpha(layer.wl_surface());
            out.extend(
                render_elements_from_surface_tree(
                    renderer,
                    layer.wl_surface(),
                    origin.to_physical(1),
                    1.0,
                    alpha,
                    Kind::Unspecified,
                )
                .into_iter()
                .map(SceneElement::Surface),
            );
            // Popups are positioned against their parent's surface origin.
            for (popup, location) in PopupManager::popups_for_surface(layer.wl_surface()) {
                let offset = location - popup.geometry().loc;
                out.extend(
                    render_elements_from_surface_tree(
                        renderer,
                        popup.wl_surface(),
                        (origin + offset).to_physical(1),
                        1.0,
                        alpha,
                        Kind::Unspecified,
                    )
                    .into_iter()
                    .map(SceneElement::Surface),
                );
            }
        }
    }
}

/// Appends one stacking group of windows and the closing windows that belong
/// to it.
///
/// A closing window keeps the group and index it had when it went away, so it
/// fades out where it was instead of jumping above the windows that are still
/// alive. The index is clamped to the group's current length, so removing a
/// neighbour cannot make a snapshot disappear or jump to another group.
#[allow(clippy::too_many_arguments)]
fn collect_window_layer(
    state: &Crab,
    renderer: &mut GlesRenderer,
    output: crab::core::OutputId,
    output_origin: Point<i32, Logical>,
    border_width: i32,
    focused: Option<WindowId>,
    layer: WindowLayer,
    order: &[WindowId],
    out: &mut Vec<SceneElement>,
) {
    let live: Vec<&ManagedWindow> = order
        .iter()
        .filter_map(|id| state.window(*id))
        .filter(|window| {
            window.window.output == output
                && window.mapped
                && window.visible
                && window.stack_layer() == layer
        })
        .collect();

    let closings: Vec<&ClosingWindow> = state
        .closings
        .iter()
        .filter(|closing| closing.output == output && closing.layer == layer)
        .collect();
    let mut slots = closing_slots(
        closings.iter().map(|closing| closing.stack_index),
        live.len(),
    )
    .into_iter()
    .peekable();

    for index in 0..=live.len() {
        while let Some((position, closing)) = slots.peek().copied() {
            if position != index {
                break;
            }
            slots.next();
            if let Some(surface) = ClosingSurface::new(
                renderer,
                closings[closing],
                &state.frame_clock,
                output_origin,
            ) {
                out.push(SceneElement::Closing(surface));
            }
        }
        if let Some(window) = live.get(index) {
            collect_window(
                state,
                renderer,
                output_origin,
                border_width,
                focused,
                window,
                out,
            );
        }
    }
}

/// Places closing snapshots among a layer's live windows.
///
/// Each snapshot keeps the index it had when its window went away; the index
/// is clamped to the live group's current length, so removing a neighbour
/// cannot make the snapshot disappear or jump to another group. The stable
/// sort keeps several snapshots captured at the same index in the order they
/// closed.
fn closing_slots(indices: impl IntoIterator<Item = usize>, live_len: usize) -> Vec<(usize, usize)> {
    let mut slots: Vec<(usize, usize)> = indices
        .into_iter()
        .enumerate()
        .map(|(closing, index)| (index.min(live_len), closing))
        .collect();
    slots.sort_by_key(|(position, _)| *position);
    slots
}

/// Appends one live window's border and surfaces, in that order.
fn collect_window(
    state: &Crab,
    renderer: &mut GlesRenderer,
    output_origin: Point<i32, Logical>,
    border_width: i32,
    focused: Option<WindowId>,
    window: &ManagedWindow,
    out: &mut Vec<SceneElement>,
) {
    let location = window.visual_origin(&state.frame_clock) - output_origin;
    let size = window.visual_size(&state.frame_clock);
    let alpha = window.open_alpha(&state.frame_clock);
    let fit = window.fit(&state.frame_clock).relative_to(output_origin);
    // A fullscreen or maximized window covers the screen, so a border would
    // only clip it.
    if border_width > 0 && !window.window.fullscreen && !window.window.maximized {
        let color = if focused == Some(window.id()) {
            border_color(state.config.border_color)
        } else {
            border_color(state.config.unfocused_border_color)
        };
        let color = Color32F::new(color.r(), color.g(), color.b(), color.a() * alpha);
        out.extend(
            borders_for(location, size, border_width, color)
                .into_iter()
                .map(SceneElement::Solid),
        );
    }
    // The toplevel is drawn from its geometry origin pulled back out by the
    // client's own decoration inset, so the *window geometry* — what the layout
    // placed and what `borders_for` frames — lands on the rect. `WindowFit` then
    // maps whatever the client has committed onto that rect, which is the same
    // thing while it is the size it was asked to be; pointer input reads the
    // same fit backwards.
    out.extend(
        fitted_surface_tree(renderer, window.wl_surface(), fit, alpha)
            .into_iter()
            .map(SceneElement::Fitted),
    );
}

/// Converts a config color into the renderer's representation.
fn border_color(color: Color) -> Color32F {
    Color32F::new(color.r, color.g, color.b, color.a)
}

/// The surface elements of one toplevel, each mapped onto the rectangle the
/// window is being drawn at.
///
/// They come from smithay at the fit's surface origin — the rectangle's
/// origin pulled back by the client's decoration inset — and [`WindowFit`]
/// then moves and cuts them.
fn fitted_surface_tree(
    renderer: &mut GlesRenderer,
    surface: &WlSurface,
    fit: WindowFit,
    alpha: f32,
) -> Vec<FittedSurface<GlesRenderer>> {
    let elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
        render_elements_from_surface_tree(
            renderer,
            surface,
            fit.surface_origin().to_physical(1),
            1.0,
            alpha,
            Kind::Unspecified,
        );
    elements
        .into_iter()
        .filter_map(|inner| {
            let natural = inner.geometry(Scale::from(1.0));
            let (src, dst) = fit.map_element(natural, inner.src())?;
            Some(FittedSurface {
                inner,
                src,
                dst,
                natural,
                fit,
            })
        })
        .collect()
}

/// A closing window's kept buffer, mapped onto the rectangle its fade is
/// shrinking or sliding.
struct ClosingSurface {
    /// Smithay's texture element, built from the client's last buffer. Only its
    /// alpha, transform and kind are used; the geometry and source below
    /// override its placement.
    inner: TextureRenderElement<GlesTexture>,
    /// What to take of the kept buffer.
    src: Rectangle<f64, BufferCoords>,
    /// Where to put it, after the close style's slide/zoom.
    dst: Rectangle<i32, Physical>,
}

impl ClosingSurface {
    /// Imports `closing`'s buffer and maps it to where the window was drawn,
    /// moved by the close animation's progress.
    fn new(
        renderer: &mut GlesRenderer,
        closing: &ClosingWindow,
        clocks: &Clocks,
        output_origin: Point<i32, Logical>,
    ) -> Option<Self> {
        let fit = closing.fit.relative_to(output_origin);
        let texture = match renderer.import_buffer(&closing.buffer, None, &[]) {
            Some(Ok(texture)) => texture,
            Some(Err(err)) => {
                tracing::debug!(?err, "failed to import a closing window's last buffer");
                return None;
            }
            None => return None,
        };
        let progress = closing.progress.value(clocks).clamp(0.0, 1.0);
        let texture_size = texture.size();
        let natural_size = texture_size.to_logical(
            smithay::utils::Scale::from(closing.buffer_scale),
            closing.buffer_transform,
        );
        let natural = Rectangle::<i32, Physical>::new(
            fit.surface_origin().to_physical(1),
            natural_size.to_physical(1),
        );
        let (src, dst) = fit.map_element(natural, Rectangle::from_size(texture_size.to_f64()))?;
        let visual = closing
            .visual
            .translated(-f64::from(output_origin.x), -f64::from(output_origin.y));
        let dst = close_dst(dst, closing.style, visual, progress);
        let alpha = (f64::from(closing.alpha) * (1.0 - progress)) as f32;
        tracing::trace!(?dst, alpha, "drawing a closing window");
        let inner = TextureRenderElement::from_static_texture(
            Id::new(),
            renderer.context_id(),
            Point::<f64, Physical>::from((f64::from(dst.loc.x), f64::from(dst.loc.y))),
            texture,
            1,
            closing.buffer_transform,
            Some(alpha),
            None,
            None,
            None,
            Kind::Unspecified,
        );
        Some(Self { inner, src, dst })
    }
}

/// Applies the configured close style to a window's drawn rectangle.
///
/// The snapshot's source is left alone: slide moves the whole window down and
/// zoom shrinks it around the rectangle it had when it closed, and the fade
/// makes either read as the window leaving rather than being squashed.
fn close_dst(
    dst: Rectangle<i32, Physical>,
    style: AnimationStyle,
    visual: crab::core::Rect,
    progress: f64,
) -> Rectangle<i32, Physical> {
    match style {
        AnimationStyle::Slide => {
            let dy = (CLOSE_SLIDE * progress).round() as i32;
            Rectangle::new((dst.loc.x, dst.loc.y + dy).into(), dst.size)
        }
        AnimationStyle::Zoom => {
            let scale = 1.0 - (1.0 - CLOSE_ZOOM) * progress;
            let anchor = Point::<f64, Physical>::from((
                visual.x + visual.w / 2.0,
                visual.y + visual.h / 2.0,
            ));
            let location =
                Point::<f64, Physical>::from((f64::from(dst.loc.x), f64::from(dst.loc.y)));
            let location = anchor + (location - anchor).upscale(scale);
            let size = Size::<i32, Physical>::from((
                (f64::from(dst.size.w) * scale).round() as i32,
                (f64::from(dst.size.h) * scale).round() as i32,
            ));
            Rectangle::new(location.to_i32_round(), size)
        }
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
    /// Where smithay would have drawn the element before the fit moved it.
    natural: Rectangle<i32, Physical>,
    /// The fit this element was mapped through, kept so damage can be mapped
    /// through the same transform as the pixels.
    fit: WindowFit,
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
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        // The inner element reports damage relative to its own natural origin;
        // run each rectangle through the same fit as the pixels so the output
        // only repaints what actually changed *and* where it was drawn.
        self.inner
            .damage_since(scale, commit)
            .into_iter()
            .filter_map(|damage| {
                let natural = Rectangle::new(self.natural.loc + damage.loc, damage.size);
                self.fit
                    .map_rect(natural)
                    .map(|dst| Rectangle::new(dst.loc - self.dst.loc, dst.size))
            })
            .collect()
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        // Map the client's opaque regions through the fit, exactly like damage.
        // Without this the tracker cannot tell that a fully covered window is
        // hidden, so it would repaint it and fire its frame callbacks.
        self.inner
            .opaque_regions(scale)
            .into_iter()
            .filter_map(|region| {
                let natural = Rectangle::new(self.natural.loc + region.loc, region.size);
                self.fit
                    .map_rect(natural)
                    .map(|dst| Rectangle::new(dst.loc - self.dst.loc, dst.size))
            })
            .collect()
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

impl Element for ClosingSurface {
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
        // The rectangle moves every frame of the fade, so earlier damage says
        // nothing about where the snapshot is drawn now.
        DamageSet::from_slice(&[Rectangle::from_size(self.dst.size)])
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl RenderElement<GlesRenderer> for ClosingSurface {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        _src: Rectangle<f64, BufferCoords>,
        _dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), smithay::backend::renderer::gles::GlesError> {
        RenderElement::<GlesRenderer>::draw(
            &self.inner,
            frame,
            self.src,
            self.dst,
            damage,
            opaque_regions,
        )
    }

    fn underlying_storage(&self, renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
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
///
/// Only surfaces whose element was presented are answered. A surface that is
/// fully hidden behind opaque content, or wholly outside the output, is not
/// redrawn this frame, so telling its client it was would make it animate
/// against a picture nobody saw.
fn send_frame_callbacks(surface: &WlSurface, time: u32, states: &RenderElementStates) {
    with_surface_tree_downward(
        surface,
        (),
        |_, _, &()| TraversalAction::DoChildren(()),
        |surface, surface_states, &()| {
            if !states.element_was_presented(Id::from_wayland_resource(surface)) {
                return;
            }
            // A subsurface may not have surface state until its first commit.
            for callback in surface_states
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
    fn closing_snapshots_keep_their_slot_and_close_order() {
        assert_eq!(
            closing_slots([2, 0, 5, 1], 3),
            vec![(0, 1), (1, 3), (2, 0), (3, 2)],
            "indices past the live group clamp to its end"
        );
        assert_eq!(
            closing_slots([1, 1, 0], 2),
            vec![(0, 2), (1, 0), (1, 1)],
            "snapshots captured at the same index keep their close order"
        );
    }

    #[test]
    fn a_closing_slide_falls_and_keeps_its_size() {
        let dst = Rectangle::<i32, Physical>::new((10, 20).into(), (100, 50).into());
        let visual = crab::core::Rect::new(10.0, 20.0, 100.0, 50.0);
        let start = close_dst(dst, AnimationStyle::Slide, visual, 0.0);
        let end = close_dst(dst, AnimationStyle::Slide, visual, 1.0);
        assert_eq!(start, dst, "a close starts exactly where the window was");
        assert_eq!(end.size, dst.size, "sliding does not change the size");
        assert_eq!(end.loc.y - dst.loc.y, CLOSE_SLIDE as i32);
    }

    #[test]
    fn a_closing_zoom_shrinks_around_the_window_center() {
        let dst = Rectangle::<i32, Physical>::new((100, 100).into(), (200, 100).into());
        let visual = crab::core::Rect::new(100.0, 100.0, 200.0, 100.0);
        let end = close_dst(dst, AnimationStyle::Zoom, visual, 1.0);
        assert_eq!(end.size.w, (200.0 * CLOSE_ZOOM).round() as i32);
        assert_eq!(end.size.h, (100.0 * CLOSE_ZOOM).round() as i32);
        // The centre of the drawn rectangle stays put while it shrinks.
        assert!((f64::from(end.loc.x) + f64::from(end.size.w) / 2.0 - 200.0).abs() <= 1.0);
        assert!((f64::from(end.loc.y) + f64::from(end.size.h) / 2.0 - 150.0).abs() <= 1.0);
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

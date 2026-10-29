//! The single-owner compositor state — `calloop`'s `State` for crab.
//!
//! [`Crab`] owns every piece of Smithay state the compositor touches and is
//! owned by the main loop on one thread (ROADMAP.md §6 M1: *one
//! `calloop::EventLoop<State>` on the main thread; `State` is single-owner*).
//! Protocol handling is split per protocol under
//! [`crate::compositor::handlers`]; input lives in [`crate::compositor::input`]
//! and drawing in [`crate::compositor::render`].
//!
//! The pure model — windows, spaces, layouts, focus — lives in the `crab`
//! library ([`crab::core`], [`crab::layout`]); this file is the glue that
//! keeps a Smithay `ToplevelSurface` next to its [`Window`] and applies the
//! layout. That glue is binary-only, so it may depend on Smithay freely; the
//! library's four modules stay pure (`scripts/check-boundaries.sh`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

use crab::animation::{Animated, Clocks, Curve, Easing, Kind, SpringParams};
use crab::config::{AnimationStyle, Config, WindowRule};
use crab::core::{
    ColumnWidth, Direction, FocusStack, FocusTarget, LayerId, LayoutCommand, LayoutKind,
    LayoutParams, Output as OutputModel, OutputId, Rect, Size as CoreSize, SpaceSet, SpaceState,
    TilePosition, Window, WindowId,
};
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::utils::{Buffer, with_renderer_surface_state};
use smithay::desktop::utils::under_from_surface_tree;
use smithay::desktop::{
    LayerSurface, PopupKind, PopupManager, WindowSurfaceType, layer_map_for_output,
};
use smithay::input::SeatState;
use smithay::input::keyboard::{KeyboardHandle, ModifiersState, XkbConfig};
use smithay::input::pointer::{CursorImageStatus, PointerHandle};
use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{IsAlive, Logical, Physical, Point, Rectangle, Serial, Size, Transform};
use smithay::wayland::compositor::{CompositorClientState, CompositorState, with_states};
use smithay::wayland::fractional_scale::FractionalScaleManagerState;
use smithay::wayland::output::OutputManagerState;
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::shell::wlr_layer::{
    KeyboardInteractivity, Layer, LayerSurface as WlrLayerSurface, WlrLayerShellState,
};
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, SurfaceCachedState, ToplevelSurface, XdgShellState,
    XdgToplevelSurfaceData,
};
use smithay::wayland::shm::ShmState;
use smithay::wayland::viewporter::ViewporterState;
use smithay::wayland::xdg_activation::XdgActivationState;

use crate::compositor::binds::{
    Action, AxisBindTable, BindIssue, BindTable, BoundAction, CompiledBinds, GestureBindTable,
    ModeBinds, MouseBindTable,
};
use crate::compositor::fit::WindowFit;
use crate::compositor::layout;
use crate::compositor::spawn;

/// Size a new toplevel is configured to before the layout first sizes it, in
/// logical pixels. Floating windows pick their own size instead.
pub(crate) const DEFAULT_WINDOW_SIZE: (i32, i32) = (800, 600);

/// How much of a window must stay on screen when it is dragged, in pixels.
const MIN_VISIBLE: f64 = 32.0;

/// Deepest key-mode stack crab will keep. Entering a mode already on the stack
/// (but not on top) is allowed, so a cap prevents a dispatch loop or a
/// misbehaving client from growing it without bound.
const MAX_MODE_DEPTH: usize = 8;

/// How close a moved window's edge must be to an output edge before it snaps,
/// in logical pixels.
const SNAP_DISTANCE: f64 = 16.0;

/// How many different sizes a client may commit instead of the one it was
/// configured to before we stop asking it to resize.
///
/// A client that is merely behind commits its old size once (which is already
/// booked, so it does not count again) and then the new one; a client that
/// needs a second configure answers the first retry. Three is room for both,
/// and a client that keeps answering with fresh sizes of its own still only
/// buys itself two retries.
const MAX_SIZE_REFUSALS: u8 = 3;

/// The nearest occupied space before (`step < 0`) or after (`step > 0`)
/// `current`, skipping empty spaces.
fn occupied_target(occupied: SpaceSet, current: u8, step: i8) -> Option<u8> {
    if step < 0 {
        occupied
            .iter_indices()
            .filter(|&index| index < current)
            .max()
    } else if step > 0 {
        occupied.iter_indices().find(|&index| index > current)
    } else {
        None
    }
}

/// Applies a connector's configured position to its output model.
///
/// Mode, scale, and transform need the DRM backend, but position is known as
/// soon as the connector is named and must be in place before the first mode is
/// set, because [`Crab::set_output_size`] preserves the model's origin.
fn apply_configured_position(config: &Config, model: &mut OutputModel) {
    let Some((x, y)) = config
        .outputs
        .get(&model.name)
        .and_then(|output| output.position)
    else {
        return;
    };
    let geometry = model.geometry;
    model.set_geometry(Rect::new(
        f64::from(x),
        f64::from(y),
        geometry.w,
        geometry.h,
    ));
}

/// Applies configured position if the model is still at origin.
pub(crate) fn apply_configured_position_if_empty(config: &Config, model: &mut OutputModel) {
    let Some((x, y)) = config
        .outputs
        .get(&model.name)
        .and_then(|output| output.position)
    else {
        return;
    };
    let geometry = model.geometry;
    model.set_geometry(Rect::new(
        f64::from(x),
        f64::from(y),
        geometry.w,
        geometry.h,
    ));
}

/// The rectangle an output occupies in the compositor's global logical space.
fn output_rects(outputs: &HashMap<OutputId, OutputRuntime>) -> Vec<(OutputId, Rect)> {
    outputs
        .iter()
        .map(|(id, runtime)| (*id, runtime.model.geometry))
        .collect()
}

/// Whether `point` lies inside `rect`.
///
/// The right and bottom edges are exclusive: two outputs can share an edge
/// without both claiming the pixel on it.
fn rect_contains(rect: Rect, point: Point<f64, Logical>) -> bool {
    point.x >= rect.x && point.x < rect.right() && point.y >= rect.y && point.y < rect.bottom()
}

/// Squared distance from `point` to the nearest point of `rect`.
fn distance_to_rect_sq(rect: Rect, point: Point<f64, Logical>) -> f64 {
    let dx = if point.x < rect.x {
        rect.x - point.x
    } else if point.x > rect.right() {
        point.x - rect.right()
    } else {
        0.0
    };
    let dy = if point.y < rect.y {
        rect.y - point.y
    } else if point.y > rect.bottom() {
        point.y - rect.bottom()
    } else {
        0.0
    };
    dx * dx + dy * dy
}

/// The output under `position`, or the nearest one when the point falls in a
/// gap between outputs.
fn output_at_position(
    outputs: &[(OutputId, Rect)],
    position: Point<f64, Logical>,
) -> Option<OutputId> {
    outputs
        .iter()
        .find(|(_, rect)| rect_contains(*rect, position))
        .map(|(id, _)| *id)
        .or_else(|| {
            outputs
                .iter()
                .min_by(|(id_a, a), (id_b, b)| {
                    distance_to_rect_sq(*a, position)
                        .total_cmp(&distance_to_rect_sq(*b, position))
                        .then_with(|| id_a.cmp(id_b))
                })
                .map(|(id, _)| *id)
        })
}

/// Clamps a pointer position onto the output layout.
///
/// A point already on an output is left alone. A point in a gap between
/// outputs (or outside the layout entirely) is pulled onto the nearest output,
/// so the pointer cannot disappear into dead space when monitors are arranged
/// with a hole between them.
fn clamp_to_outputs(
    outputs: &[(OutputId, Rect)],
    position: Point<f64, Logical>,
) -> Point<f64, Logical> {
    if outputs.is_empty()
        || outputs
            .iter()
            .any(|(_, rect)| rect_contains(*rect, position))
    {
        return position;
    }
    let Some((_, rect)) = outputs.iter().min_by(|(id_a, a), (id_b, b)| {
        distance_to_rect_sq(*a, position)
            .total_cmp(&distance_to_rect_sq(*b, position))
            .then_with(|| id_a.cmp(id_b))
    }) else {
        return position;
    };
    let max_x = (rect.right() - 1.0).max(rect.x);
    let max_y = (rect.bottom() - 1.0).max(rect.y);
    (
        position.x.clamp(rect.x, max_x),
        position.y.clamp(rect.y, max_y),
    )
        .into()
}

/// Logs one bind the config author wrote but crab could not compile.
pub(crate) fn log_bind_issue(kind: &str, issue: &BindIssue) {
    tracing::warn!(
        kind,
        chord = %issue.chord,
        action = %issue.action,
        reason = %issue.reason,
        "ignoring a binding"
    );
}

/// The curve a window's rectangle moves along when the layout moves it.
///
/// One curve for every window rectangle for now; M5 makes it per-event config
/// (`COMPOSITOR.md` §9). Springs are the right default because retargeting
/// mid-flight then keeps the velocity, so a second layout switch blends out of
/// the first instead of restarting.
const WINDOW_CURVE: Curve = Curve::Spring(SpringParams::DEFAULT);

/// The curve a window follows when it appears.
///
/// Open and close are fixed-duration eases rather than springs: an appearance
/// should arrive without bouncing, and a disappearance should not overshoot the
/// point where the window is gone.
const OPEN_CURVE: Curve = Curve::Easing {
    duration_ms: 180,
    kind: Easing::EaseOutCubic,
};

/// The curve a window follows when it goes away.
const CLOSE_CURVE: Curve = Curve::Easing {
    duration_ms: 150,
    kind: Easing::EaseOutCubic,
};

/// How far a sliding window travels when it opens, in logical pixels.
const OPEN_SLIDE: f64 = 24.0;

/// How much smaller than its resting size a zooming window starts.
const OPEN_ZOOM: f64 = 0.92;

/// The stacking group a window belongs to, bottom to top.
///
/// The groups are the coarse part of the scene's z-order. Within the two
/// floating groups, windows are ordered by focus (most recently focused on
/// top); tiled and covering windows keep their layout order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum WindowLayer {
    /// Tiled windows, in window-list order.
    Tiled,
    /// Ordinary floating windows, in focus order.
    Floating,
    /// Floating windows that asked to stay above ordinary floating ones.
    FloatingOnTop,
    /// Fullscreen and maximized windows, above everything else.
    Covering,
}

/// One window's contribution to the scene's bottom-to-top stacking order.
#[derive(Clone, Copy, Debug)]
struct StackEntry {
    id: WindowId,
    layer: WindowLayer,
    visible: bool,
}

/// Orders visible windows bottom-to-top.
///
/// Tiled and covering windows keep their window-list order. Floating windows
/// are ordered by the focus stack instead, least recently focused first, so an
/// unfocused floating window sits below every focused one and focusing a peer
/// raises it above the rest. The two floating groups stay separate: an
/// always-on-top window remains above ordinary floating windows even when it is
/// not focused.
fn stacked_order(entries: &[StackEntry], focus_stack: &FocusStack) -> Vec<WindowId> {
    let in_focus_stack =
        |id: WindowId| focus_stack.iter().any(|target| target.window() == Some(id));

    let mut order = Vec::with_capacity(entries.len());
    order.extend(
        entries
            .iter()
            .filter(|entry| entry.visible && entry.layer == WindowLayer::Tiled)
            .map(|entry| entry.id),
    );
    for layer in [WindowLayer::Floating, WindowLayer::FloatingOnTop] {
        order.extend(
            entries
                .iter()
                .filter(|entry| entry.visible && entry.layer == layer && !in_focus_stack(entry.id))
                .map(|entry| entry.id),
        );
        order.extend(
            focus_stack
                .iter()
                .rev()
                .filter_map(FocusTarget::window)
                .filter_map(|id| {
                    entries
                        .iter()
                        .find(|entry| entry.id == id && entry.visible && entry.layer == layer)
                })
                .map(|entry| entry.id),
        );
    }
    order.extend(
        entries
            .iter()
            .filter(|entry| entry.visible && entry.layer == WindowLayer::Covering)
            .map(|entry| entry.id),
    );
    order
}

/// Where a closing window's snapshot belonged within its former layer.
///
/// `order` is the live bottom-to-top order; `layer_of` resolves each id's
/// current group. Falling back to the bottom of the group keeps a snapshot
/// visible even if its neighbours have already gone.
fn stack_index_in_layer(
    order: &[WindowId],
    id: WindowId,
    layer: WindowLayer,
    layer_of: impl Fn(WindowId) -> Option<WindowLayer>,
) -> usize {
    order
        .iter()
        .filter(|other| layer_of(**other) == Some(layer))
        .position(|other| *other == id)
        .unwrap_or(0)
}

/// One managed toplevel: the protocol object and its pure model.
pub(crate) struct ManagedWindow {
    /// The xdg toplevel this describes.
    pub(crate) surface: ToplevelSurface,
    /// Its place in the space/layout model.
    pub(crate) window: Window,
    /// Top-left corner of the *window geometry* in logical compositor
    /// coordinates: where the layout put the window.
    pub(crate) location: Point<i32, Logical>,
    /// Logical size of the window geometry, which is what the layout arranges
    /// and what clients are configured to.
    pub(crate) size: Size<i32, Logical>,
    /// The client's window geometry (`xdg_surface.set_window_geometry`, clamped
    /// to the surface tree's bounding box) in *surface* coordinates.
    ///
    /// This is the rectangle the layout places: `location` is its origin, while
    /// the surface itself usually extends past it on every side, since
    /// client-side decorations draw their shadow margins outside the geometry.
    /// Popups and the pointer anchor to the geometry, not to the surface.
    pub(crate) geometry: Rectangle<i32, Logical>,
    /// Whether the layout currently shows this window.
    pub(crate) visible: bool,
    /// Set once the client commits a buffer; unmapped windows are not drawn.
    pub(crate) mapped: bool,
    /// Where the window is *drawn*: the animated value of its placement.
    ///
    /// The layout's own `location`/`size` are the target it is heading for —
    /// what the client is configured with, what a drag grabs, what the next
    /// relocation starts from. While the two differ the window is in motion.
    visual: Animated<Rect>,
    /// How far the window has appeared: 0 at its first mapped frame, 1 at rest.
    ///
    /// Sampled by the renderer to fade in the surface, its border and its
    /// popups. It is separate from [`Self::visual`] because the open animation
    /// also moves the rectangle, and the two must share one clock
    /// ([`Kind::WindowOpen`]) so the fade and the motion end together.
    open: Animated<f64>,
    /// Whether [`Self::visual`] has had a real placement yet. The first one
    /// snaps, so a new window appears where the layout put it instead of flying
    /// in from its default rectangle.
    visual_placed: bool,
    /// The last buffer committed while the window had a real geometry.
    ///
    /// A client usually detaches its buffer (and then destroys the surface) as
    /// it closes, so by the time `toplevel_destroyed` runs there is nothing left
    /// to read. Keeping this between commits is what lets the close animation
    /// still draw the window's last frame (`COMPOSITOR.md` §9).
    last_frame: Option<FrameSnapshot>,
    /// Whether the matching `[window_rule]` (if any) has already been applied.
    rule_applied: bool,
    /// What the client has been asked to be, and how it has answered.
    configure: Configure,
}

/// The last committed frame of a window, kept for its close animation.
struct FrameSnapshot {
    /// The buffer, held so the client cannot free its pixels (see [`Buffer`]).
    buffer: Buffer,
    /// The scale the buffer was committed with.
    buffer_scale: i32,
    /// The transform the buffer was committed with.
    buffer_transform: Transform,
    /// The window geometry that went with it.
    geometry: Rectangle<i32, Logical>,
}

/// A toplevel whose protocol object is gone but whose last frame is still
/// fading out.
///
/// The client's buffer is kept alive with the [`Buffer`] handle, so its pixels
/// survive the `wl_surface` and `wl_buffer` being destroyed; the renderer
/// imports it into a texture on the frames the close animation runs and draws
/// it through the window's last [`WindowFit`]. That is the "snapshot" the
/// close animation needs without copying the framebuffer (`COMPOSITOR.md` §9).
pub(crate) struct ClosingWindow {
    /// The output the window was drawn on.
    pub(crate) output: OutputId,
    /// The last committed buffer, held so the client cannot free its pixels.
    pub(crate) buffer: Buffer,
    /// The scale and transform the buffer was committed with.
    pub(crate) buffer_scale: i32,
    /// The transform the buffer was committed with.
    pub(crate) buffer_transform: Transform,
    /// Where the window was drawn when it went away.
    pub(crate) fit: WindowFit,
    /// The window's drawn rectangle at that instant, used as a zoom anchor.
    pub(crate) visual: Rect,
    /// The configured `animation_type_close`.
    pub(crate) style: AnimationStyle,
    /// The opacity the window had when it went away, so closing one that is
    /// still appearing does not make it flash to fully opaque.
    pub(crate) alpha: f32,
    /// The stacking group the window was in, so it fades out between the same
    /// kind of windows rather than always above the live scene.
    pub(crate) layer: WindowLayer,
    /// Its index within that group when it went away, clamped to the group's
    /// current length at render time.
    pub(crate) stack_index: usize,
    /// How far the close has progressed: 0 at the last frame, 1 when gone.
    pub(crate) progress: Animated<f64>,
}

/// A layer-shell surface mapped onto the output.
///
/// Smithay's [`LayerMap`](smithay::desktop::LayerMap) owns the placement and
/// exclusive zone; this keeps the compositor-side identity and the optional
/// open animation from the matching `[layer_rule]`.
pub(crate) struct LayerEntry {
    /// Handle used by focus and the protocol layer.
    pub(crate) id: LayerId,
    /// The surface and its namespace.
    pub(crate) surface: LayerSurface,
    /// The output whose layer map owns the surface.
    pub(crate) output: OutputId,
    /// The rule's `animation_type_open`, if one matched the namespace.
    style: Option<AnimationStyle>,
    /// How far the surface has appeared: 0 at its first mapped frame, 1 at rest.
    open: Animated<f64>,
    /// Set once the client commits a buffer; the open animation starts then.
    mapped: bool,
}

impl LayerEntry {
    /// The opacity the layer is drawn with.
    fn alpha(&self, clocks: &Clocks) -> f32 {
        self.open.value(clocks).clamp(0.0, 1.0) as f32
    }

    /// How far the open animation has run, from 0 to 1.
    fn progress(&self, clocks: &Clocks) -> f64 {
        self.open.value(clocks).clamp(0.0, 1.0)
    }

    /// Starts the configured open animation on the first mapped frame.
    fn start_opening(&mut self, clocks: &Clocks) {
        let Some(style) = self.style else {
            return;
        };
        tracing::trace!(
            namespace = self.surface.namespace(),
            ?style,
            "starting the layer-surface open animation"
        );
        self.open.snap_to(0.0);
        self.open.animate(1.0, Kind::WindowOpen, clocks);
    }
}

/// Where a window's open animation starts, for the configured style.
///
/// Slide starts a little below the resting rectangle and rises into place;
/// zoom starts smaller around the resting rectangle's centre and grows. Both
/// are purely the rectangle the window is drawn at, so hit-testing and popup
/// placement follow the picture without a second transform.
fn open_start_rect(target: Rect, style: AnimationStyle) -> Rect {
    match style {
        AnimationStyle::Slide => Rect::new(target.x, target.y + OPEN_SLIDE, target.w, target.h),
        AnimationStyle::Zoom => {
            let w = target.w * OPEN_ZOOM;
            let h = target.h * OPEN_ZOOM;
            Rect::new(
                target.x + (target.w - w) / 2.0,
                target.y + (target.h - h) / 2.0,
                w,
                h,
            )
        }
    }
}

/// The buffer `surface` currently has attached, with its scale and transform.
///
/// Called on every commit to keep [`ManagedWindow::last_frame`] up to date
/// while the client still has pixels to show.
fn committed_frame(surface: &WlSurface) -> Option<(Buffer, i32, Transform)> {
    with_renderer_surface_state(surface, |state| {
        state
            .buffer()
            .cloned()
            .map(|buffer| (buffer, state.buffer_scale(), state.buffer_transform()))
    })
    .flatten()
}

impl ManagedWindow {
    /// The window's model id.
    pub(crate) fn id(&self) -> WindowId {
        self.window.id
    }

    /// Which z-order group the window is drawn in.
    pub(crate) fn stack_layer(&self) -> WindowLayer {
        if self.window.fullscreen || self.window.maximized {
            WindowLayer::Covering
        } else if self.window.floating {
            if self.window.always_on_top {
                WindowLayer::FloatingOnTop
            } else {
                WindowLayer::Floating
            }
        } else {
            WindowLayer::Tiled
        }
    }

    /// Whether window rules let this window take keyboard focus.
    pub(crate) fn is_focusable(&self) -> bool {
        !self.window.rules.no_focus
    }

    /// The underlying `wl_surface`.
    pub(crate) fn wl_surface(&self) -> &WlSurface {
        self.surface.wl_surface()
    }

    /// Current window geometry in logical coordinates.
    pub(crate) fn rect(&self) -> Rect {
        Rect::new(
            f64::from(self.location.x),
            f64::from(self.location.y),
            f64::from(self.size.w),
            f64::from(self.size.h),
        )
    }

    /// The rectangle the window is drawn at, sampled from its animation.
    ///
    /// This is the rectangle the user sees, and everything that has to agree
    /// with the picture — hit-testing, interaction origins, popup placement,
    /// borders — reads it rather than the layout target.
    pub(crate) fn visual_rect(&self, clocks: &Clocks) -> Rect {
        self.visual.value(clocks)
    }

    /// The drawn rectangle's origin, rounded to the pixel grid.
    pub(crate) fn visual_origin(&self, clocks: &Clocks) -> Point<i32, Logical> {
        let rect = self.visual_rect(clocks);
        point_i32(rect.x, rect.y)
    }

    /// The drawn rectangle's size, rounded to the pixel grid.
    pub(crate) fn visual_size(&self, clocks: &Clocks) -> Size<i32, Logical> {
        let rect = self.visual_rect(clocks);
        size_i32(rect.w, rect.h)
    }

    /// Where the toplevel's `wl_surface` origin sits while it is drawn: the
    /// rectangle's origin pulled back by the client's own decoration inset.
    ///
    /// This is where the renderer places the surface tree. Pointer input asks
    /// [`Self::fit`] for the origin it reports instead, because the two differ
    /// while the window is being stretched.
    pub(crate) fn visual_surface_origin(&self, clocks: &Clocks) -> Point<i32, Logical> {
        self.visual_origin(clocks) - self.geometry.loc
    }

    /// How this window's committed surface maps onto where it is drawn.
    ///
    /// Both readers of that mapping build it here — the renderer drawing the
    /// window forward, the seat answering the pointer backward — so what is
    /// drawn and what a click means cannot drift apart (`super::fit`).
    pub(crate) fn fit(&self, clocks: &Clocks) -> WindowFit {
        WindowFit::new(
            Rectangle::new(self.visual_origin(clocks), self.visual_size(clocks)),
            self.visual_surface_origin(clocks),
            self.geometry.size,
            !self.is_size_refusing(),
        )
    }

    /// Moves the drawn rectangle toward `rect`, for the reason `kind`.
    ///
    /// The first placement snaps rather than animates, and a placement that
    /// repeats the one already being animated toward is a no-op, so this is safe
    /// to call on every re-layout.
    pub(crate) fn animate_to(&mut self, rect: Rect, kind: Kind, clocks: &Clocks) {
        if self.visual_placed {
            // An appearance runs on its own fixed-duration curve. A real layout
            // change during it takes the rectangle over and returns to the
            // spring; repeating the same target must not swap the curve under
            // the running segment.
            if self.visual.target() != rect {
                self.visual.set_curve(WINDOW_CURVE);
            }
            self.visual.animate(rect, kind, clocks);
        } else {
            self.visual.snap_to(rect);
            self.visual_placed = true;
        }
    }

    /// Starts the configured open animation at the window's first mapped
    /// frame: the rectangle flies in from its slide/zoom start while the alpha
    /// fades up.
    ///
    /// Called only when `[animations] enabled` is true; without it the window
    /// is simply at rest on its placement and [`Self::open_alpha`] is 1.
    pub(crate) fn start_opening(&mut self, style: AnimationStyle, clocks: &Clocks) {
        tracing::trace!(?style, title = ?self.window.title, "starting the open animation");
        let target = self.visual.target();
        let start = open_start_rect(target, style);
        self.open.snap_to(0.0);
        self.open.animate(1.0, Kind::WindowOpen, clocks);
        if start != target {
            self.visual.set_curve(OPEN_CURVE);
            self.visual.snap_to(start);
            self.visual.animate(target, Kind::WindowOpen, clocks);
        }
    }

    /// How opaque the window is while it appears: 1 once open or when
    /// animations are off.
    pub(crate) fn open_alpha(&self, clocks: &Clocks) -> f32 {
        self.open.value(clocks).clamp(0.0, 1.0) as f32
    }

    /// Pins the drawn rectangle to `rect`, dropping any motion.
    ///
    /// For movement that must not animate — a window following the pointer in a
    /// drag has to track it 1:1 — and for re-layouts that would otherwise
    /// restart an animation that is already heading there.
    pub(crate) fn snap_visual_to(&mut self, rect: Rect) {
        self.visual.snap_to(rect);
        self.visual_placed = true;
    }

    /// Whether the client has refused enough sizes that we stopped asking it
    /// to be the size the layout drew it at.
    pub(crate) fn is_size_refusing(&self) -> bool {
        self.configure.gave_up()
    }

    /// Lets the window size itself, wherever it is now.
    ///
    /// A floating window owns its size, so the geometry it commits becomes the
    /// truth and there is nothing left to compare its commits against until
    /// the layout takes the size back over.
    pub(crate) fn float_at(&mut self, rect: Rect) {
        self.window.set_floating(true, rect);
        self.configure = Configure::default();
    }

    /// Asks the client to be `size`, the size its rectangle is about to have.
    ///
    /// The request is clamped to the client's own size hints: a window that
    /// has told us it cannot be smaller than `min_size` is never going to
    /// commit a smaller size, and asking for something it cannot be only earns
    /// a refusal. Asking twice for one size is skipped, which is what keeps a
    /// re-layout from configuring a window over and over.
    ///
    /// The size is only a request — [`Self::on_committed_size`] watches what
    /// the client actually commits.
    pub(crate) fn configure_to(&mut self, size: Size<i32, Logical>) {
        let size = clamp_to_hints(size, self.window.min_size, self.window.max_size);
        if self.configure.ask(size) {
            self.send_configure(size);
        }
    }

    /// Sends a configure for what was last asked, without forgetting what was
    /// refused for it: a retry asks again, it does not start over.
    fn retry_configure(&mut self) {
        let Some(size) = self.configure.asked() else {
            return;
        };
        self.send_configure(size);
    }

    fn send_configure(&mut self, size: Size<i32, Logical>) {
        tracing::trace!(title = ?self.window.title, ?size, "sending configure");
        self.surface
            .with_pending_state(|pending| pending.size = Some(size));
        self.surface.send_configure();
    }

    /// Books what a commit says about [`Self::configure_to`], asking again if
    /// the client is merely behind.
    fn on_committed_size(&mut self, size: Size<i32, Logical>) {
        match self.configure.commit(size) {
            Reply::TryAgain => {
                tracing::trace!(
                    title = ?self.window.title,
                    committed = ?size,
                    "the client answered with a size it was not asked for; asking again"
                );
                self.retry_configure();
            }
            Reply::GaveUp => {
                tracing::debug!(
                    title = ?self.window.title,
                    asked = ?self.configure.asked(),
                    committed = ?size,
                    "client refuses the size it is configured to"
                );
            }
            Reply::Complied | Reply::SameAsBefore => {}
        }
    }
}

/// What a commit says about the size a client was asked to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reply {
    /// It is the size it was asked for, and everything is forgotten.
    Complied,
    /// The same size it committed last time, wrong or right: nothing new.
    SameAsBefore,
    /// A size it has not committed before, and not the one asked for:
    /// worth asking once more.
    TryAgain,
    /// Enough different sizes have been answered with: stop asking.
    GaveUp,
}

/// What a client was asked to be, and how it has answered.
///
/// A configure is a request, not a command, so this is what tells "the client
/// has not caught up yet" — over within a frame or two, with the surface
/// drawn into its rectangle meanwhile — from "the client is never going to be
/// that size", which has to be given up on: configuring a client that ignores
/// us forever is exactly the loop this must not become (TODO.md §1.5).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Configure {
    /// The size last asked for, if any.
    asked: Option<Size<i32, Logical>>,
    /// The size last committed, if any.
    committed: Option<Size<i32, Logical>>,
    /// How many *different* sizes have been committed instead of `asked`.
    refusals: u8,
    /// Whether asking has been given up on.
    gave_up: bool,
}

impl Configure {
    /// The size last asked for.
    fn asked(&self) -> Option<Size<i32, Logical>> {
        self.asked
    }

    /// Whether the client has refused so many sizes that we stopped asking.
    fn gave_up(&self) -> bool {
        self.gave_up
    }

    /// Starts asking for `size`, unless it has already been asked for.
    ///
    /// A size that differs from the last one is a fresh start: the client
    /// refused nothing yet for *this* rectangle, so it gets a new allowance of
    /// refusals. Asking again for the same size returns false, which is what
    /// keeps a re-layout from configuring a window over and over.
    fn ask(&mut self, size: Size<i32, Logical>) -> bool {
        if self.asked == Some(size) {
            return false;
        }
        self.asked = Some(size);
        self.refusals = 0;
        self.gave_up = false;
        true
    }

    /// Books a commit of `size` and says what it is worth.
    ///
    /// Only a size the client has not committed before is a new answer: the
    /// same one repeated — which is what a client that ignores configure keeps
    /// giving, and what a terminal that rounds its window to whole character
    /// cells always gives — is not, so this cannot turn into a configure loop.
    /// A client that commits the size it was asked for starts over.
    fn commit(&mut self, size: Size<i32, Logical>) -> Reply {
        let previous = self.committed.replace(size);
        if self.asked == Some(size) {
            self.refusals = 0;
            self.gave_up = false;
            return Reply::Complied;
        }
        if self.gave_up || previous == Some(size) {
            return Reply::SameAsBefore;
        }
        self.refusals = self.refusals.saturating_add(1);
        if self.refusals >= MAX_SIZE_REFUSALS {
            self.gave_up = true;
            return Reply::GaveUp;
        }
        Reply::TryAgain
    }
}

/// What a pointer grab is doing to a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GrabMode {
    /// Moving the window around.
    Move,
    /// Resizing it: the named edges follow the pointer, the others stay put.
    Resize {
        /// The edges being dragged, as `xdg_toplevel.resize` names them.
        edges: xdg_toplevel::ResizeEdge,
    },
}

impl GrabMode {
    /// The rectangle a drag of `(dx, dy)` from `origin` produces.
    ///
    /// A move translates. A resize keeps the edges that are not being dragged
    /// fixed, so pulling the left or top edge moves the origin as well as
    /// changing the size. A dragged edge never crosses the fixed one — the
    /// result stays at least one pixel wide and tall — so a window cannot
    /// invert itself.
    fn rect_after(self, origin: Rect, dx: f64, dy: f64) -> Rect {
        match self {
            Self::Move => origin.translated(dx, dy),
            Self::Resize { edges } => {
                let Edges {
                    left,
                    right,
                    top,
                    bottom,
                } = edges.into();
                let mut rect = origin;
                if left {
                    rect.x = origin.x + dx;
                    rect.w = origin.right() - rect.x;
                } else if right {
                    rect.w = origin.w + dx;
                }
                if top {
                    rect.y = origin.y + dy;
                    rect.h = origin.bottom() - rect.y;
                } else if bottom {
                    rect.h = origin.h + dy;
                }
                if rect.w < 1.0 {
                    rect.w = 1.0;
                    if left {
                        rect.x = origin.right() - 1.0;
                    }
                }
                if rect.h < 1.0 {
                    rect.h = 1.0;
                    if top {
                        rect.y = origin.bottom() - 1.0;
                    }
                }
                rect
            }
        }
    }
}

/// Snaps a moved rectangle flush when an edge is within `threshold` of the
/// matching usable-area edge.
///
/// The nearest edge wins when both are in range (a window wider or taller than
/// the area), and the size never changes. Resizes do not use this: the pointer
/// names the exact edge the client asked for.
fn snap_move_to_edges(rect: Rect, area: Rect, threshold: f64) -> Rect {
    fn axis(near: f64, far: f64, area_near: f64, area_far: f64, threshold: f64) -> f64 {
        let near_gap = (near - area_near).abs();
        let far_gap = (far - area_far).abs();
        match (near_gap <= threshold, far_gap <= threshold) {
            (true, true) if far_gap < near_gap => area_far - (far - near),
            (true, _) => area_near,
            (false, true) => area_far - (far - near),
            (false, false) => near,
        }
    }

    Rect::new(
        axis(rect.x, rect.right(), area.x, area.right(), threshold),
        axis(rect.y, rect.bottom(), area.y, area.bottom(), threshold),
        rect.w,
        rect.h,
    )
}

/// Which sides of a rectangle a resize drags.
#[derive(Clone, Copy, Debug)]
struct Edges {
    left: bool,
    right: bool,
    top: bool,
    bottom: bool,
}

impl From<xdg_toplevel::ResizeEdge> for Edges {
    fn from(edge: xdg_toplevel::ResizeEdge) -> Self {
        use xdg_toplevel::ResizeEdge as E;

        Self {
            left: matches!(edge, E::Left | E::TopLeft | E::BottomLeft),
            right: matches!(edge, E::Right | E::TopRight | E::BottomRight),
            top: matches!(edge, E::Top | E::TopLeft | E::TopRight),
            bottom: matches!(edge, E::Bottom | E::BottomLeft | E::BottomRight),
        }
    }
}

/// An in-progress interactive move/resize.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Interaction {
    /// The window being manipulated.
    pub(crate) id: WindowId,
    /// Whether this is a move or a resize.
    pub(crate) mode: GrabMode,
    /// Where the grab started.
    pub(crate) grab: Point<f64, Logical>,
    /// The window's rectangle when the grab started.
    pub(crate) origin: Rect,
    /// The button that started a compositor grab, whose release ends it.
    ///
    /// `None` for a client request: `xdg_toplevel.move` names a press serial
    /// rather than a button, and the client already tracks its own press.
    pub(crate) button: Option<u32>,
    /// Whether the client asked for the grab (`xdg_toplevel.move`/`resize`)
    /// rather than a compositor bind.
    ///
    /// The difference is the press: a compositor grab swallowed it, so it
    /// swallows the release too, while a client-initiated one must see its own
    /// release or its titlebar stays stuck down.
    pub(crate) from_client: bool,
}

/// Per-client state carried through wayland-server's `ClientData` API.
///
/// `CompositorHandler::client_compositor_state` reads this out of the client
/// object, so it must exist for every connection.
#[derive(Default)]
pub(crate) struct ClientState {
    pub(crate) compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

/// Everything crab owns for one output.
///
/// The Wayland object, the pure space model, the damage tracker, and the size
/// travel together so a second output can be added without threading another
/// set of fields through the compositor. `render_transform` is not the
/// Wayland output transform: it is the backend's correction between crab's
/// logical coordinates and its framebuffer (winit is bottom-up).
pub(crate) struct OutputRuntime {
    /// The protocol object advertised to clients.
    pub(crate) wayland: Output,
    /// The pure output/space model.
    pub(crate) model: OutputModel,
    /// Damage and presentation state, carried from frame to frame.
    ///
    /// It is the source of truth for which parts of the output need repainting
    /// and which elements were actually shown, so frame callbacks are only sent
    /// to surfaces that made it into a presented frame.
    pub(crate) damage_tracker: OutputDamageTracker,
    /// The transform the renderer applies to this backend's framebuffer.
    pub(crate) render_transform: Transform,
    /// Logical size of the visible area; kept in sync by the backend.
    pub(crate) size: Size<i32, Logical>,
}

impl OutputRuntime {
    /// Creates the runtime for an output. `set_size` fills in the real size
    /// and rebuilds the tracker before the first frame.
    fn new(wayland: Output, model: OutputModel, render_transform: Transform) -> Self {
        Self {
            wayland,
            model,
            damage_tracker: OutputDamageTracker::new((1, 1), 1.0, render_transform),
            render_transform,
            size: (0, 0).into(),
        }
    }

    /// The output's top-left corner in the compositor's global logical space.
    pub(crate) fn origin(&self) -> Point<i32, Logical> {
        (
            self.model.geometry.x.round() as i32,
            self.model.geometry.y.round() as i32,
        )
            .into()
    }
}

/// What a bound key should do when it is released.
#[derive(Clone, Debug)]
pub(crate) struct BoundKey {
    /// Action re-fired on key repeat, if the bind opted into `repeat`.
    pub(crate) action: Option<Action>,
    /// Action to run on release, for a `release` bind.
    pub(crate) release_action: Option<Action>,
    /// Whether the press reached the client, so its release must too.
    pub(crate) forwarded: bool,
    /// Whether holding the key repeats the action.
    pub(crate) repeat: bool,
}

pub(crate) struct Crab {
    pub(crate) compositor_state: CompositorState,
    pub(crate) xdg_shell_state: XdgShellState,
    pub(crate) layer_shell_state: WlrLayerShellState,
    /// Kept alive for `zxdg_decoration_manager_v1` dispatch.
    pub(crate) _decoration_state: XdgDecorationState,
    /// Kept alive for `wp_fractional_scale_manager_v1` dispatch.
    pub(crate) _fractional_scale_state: FractionalScaleManagerState,
    /// Kept alive for `wp_viewporter` dispatch.
    pub(crate) _viewporter_state: ViewporterState,
    /// Kept alive for `xdg_activation_v1` dispatch.
    pub(crate) xdg_activation_state: XdgActivationState,
    /// Kept alive for `zxdg_output_manager_v1` dispatch; the global itself is
    /// owned by the display.
    pub(crate) _output_manager_state: OutputManagerState,
    pub(crate) shm_state: ShmState,
    pub(crate) data_device_state: DataDeviceState,
    pub(crate) seat_state: SeatState<Self>,
    pub(crate) keyboard: KeyboardHandle<Self>,
    /// Name of the active xkb layout, as reported by the keymap.
    pub(crate) keyboard_layout: Option<String>,
    pub(crate) pointer: PointerHandle<Self>,
    /// Every output, keyed by the stable id in its model.
    pub(crate) outputs: HashMap<OutputId, OutputRuntime>,
    /// The output that owns the current focus and receives input.
    /// Multi-output will move this with the pointer and focus.
    pub(crate) primary_output: OutputId,
    /// The loaded configuration.
    pub(crate) config: Config,
    /// The compiled keyboard binds (built-ins plus `[binds.*]`).
    pub(crate) binds: BindTable,
    /// The compiled pointer-button binds (built-ins plus `[mousebinds]`).
    pub(crate) mouse_binds: MouseBindTable,
    /// The compiled scroll binds (`[axisbinds]`).
    pub(crate) axis_binds: AxisBindTable,
    /// The compiled gesture binds (`[gesturebinds]`).
    pub(crate) gesture_binds: GestureBindTable,
    /// Per-mode overlay binds (`[modes.<name>]`), layered over the tables above
    /// while a mode is active.
    pub(crate) mode_binds: BTreeMap<String, ModeBinds>,
    /// Current keyboard modifiers, used by pointer and axis chords.
    pub(crate) modifiers: ModifiersState,
    /// Key mode stack; the default mode is always the first entry.
    pub(crate) mode_stack: Vec<String>,
    /// crab's own Wayland socket name, once the backend has bound it.
    ///
    /// `spawn:` and `[autostart]` pass this as `WAYLAND_DISPLAY` so launched
    /// clients connect to crab rather than to the host compositor.
    pub(crate) wayland_display: Option<String>,
    /// Path of the IPC Unix socket, once the backend has bound it.
    ///
    /// Spawned and autostarted processes receive it as `CRAB_SOCKET`, so they
    /// find the server without having to parse `WAYLAND_DISPLAY`.
    pub(crate) ipc_socket: Option<PathBuf>,
    /// Path of the loaded config file, if any.
    pub(crate) config_path: Option<PathBuf>,
    /// Set by the IPC `quit` action; the backend stops its event loop after the
    /// current dispatch.
    pub(crate) quit_requested: bool,
    /// Config epoch, incremented on each reload attempt.
    pub(crate) config_epoch: u64,
    /// Last config reload error, if any.
    pub(crate) config_error: Option<String>,
    /// When the current config error first appeared.
    pub(crate) config_error_since: Option<std::time::Instant>,
    /// Managed toplevels, in layout order (first = first tile).
    pub(crate) windows: Vec<ManagedWindow>,
    /// Layer-shell surfaces, in mapping order. Rendering groups them by layer.
    pub(crate) layers: Vec<LayerEntry>,
    /// Toplevels that are gone but still fading out, drawn above the scene.
    pub(crate) closings: Vec<ClosingWindow>,
    /// The xdg popup trees, one per toplevel, plus their grabs.
    ///
    /// Smithay's manager is the source of truth: it descends each popup's
    /// parent chain, so popups of popups (submenus) resolve correctly, and
    /// keeps the `xdg_popup.grab` state that input is redirected through.
    pub(crate) popup_manager: PopupManager,
    /// What currently holds keyboard focus.
    pub(crate) focus: FocusTarget,
    /// Most-recently-used focus order, used for stacking and fallback focus.
    pub(crate) focus_stack: FocusStack,
    /// Keys a bind claimed on press, kept until the matching release.
    ///
    /// The entry records what the release should do: run a `release` bind,
    /// forward the release because the press was a `pass-through`, or swallow
    /// it because the client never saw the press.
    pub(crate) bound_keys: HashMap<u32, BoundKey>,
    /// Buttons swallowed by a mouse bind, so the matching release is swallowed
    /// too.
    pub(crate) bound_buttons: HashSet<u32>,
    /// An in-progress interactive move/resize, if any.
    pub(crate) interaction: Option<Interaction>,
    /// Serial of the last pointer press sent to a client.
    ///
    /// `xdg_toplevel.move` and `resize` name the press that triggered them; a
    /// request whose serial is not this one is stale (or not ours) and is
    /// ignored.
    pub(crate) last_pointer_press: Option<Serial>,
    /// Time of the first frame, used for `wl_surface.frame` timestamps.
    pub(crate) start_time: Instant,
    /// The frame clock every animation is sampled against (COMPOSITOR.md §9).
    ///
    /// Backends re-time it once per frame, from that frame's presentation time;
    /// everything drawn in a frame therefore sees one instant.
    pub(crate) frame_clock: Clocks,
    /// Pointer position in logical compositor coordinates.
    pub(crate) pointer_location: Point<f64, Logical>,
    /// Whether [`Self::pointer_location`] means anything yet. Backends report
    /// nothing until the pointer moves (`wl_pointer` and winit both work this
    /// way), and an arrow stuck in a corner is worse than no cursor.
    pub(crate) pointer_seen: bool,
    /// The image the pointer is drawn as, as last requested by a client with
    /// `wl_pointer.set_cursor` (updated by `SeatHandler::cursor_image`).
    pub(crate) cursor_image: CursorImageStatus,
    /// Next window id to hand out (never reused, even after closes).
    next_window_id: u64,
    /// Next layer-surface id to hand out.
    next_layer_id: u64,
}

impl Crab {
    /// Builds every Wayland global on `display` and returns the initial state.
    ///
    /// The globals registered here (compositor, xdg-shell, layer-shell,
    /// xdg-output, shm, data-device, wl_seat, wl_output) are the protocols the
    /// compositor can serve today; the rest of `crab-protocols` is still to
    /// come.
    pub(crate) fn new(
        display: &DisplayHandle,
        config: &Config,
        render_transform: Transform,
    ) -> Self {
        let mut state = Self::new_without_outputs(display, config);
        let output = Output::new(
            "crab-0".to_owned(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "crab".to_owned(),
                model: "nested".to_owned(),
            },
        );
        output.create_global::<Self>(display);
        let output_state = OutputModel::new(
            OutputId::new(0),
            "crab-0",
            Rect::default(),
            1.0,
            Self::initial_space_state(config),
        );
        state.add_output(output, output_state, render_transform);
        state
    }

    /// Builds the compositor state without registering an output.
    ///
    /// The winit backend adds its nested output immediately. The DRM backend
    /// will create one `Output` per connector and add each as it is discovered.
    pub(crate) fn new_without_outputs(display: &DisplayHandle, config: &Config) -> Self {
        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(display, "crab");
        // A bad configured layout falls back to xkbcommon's defaults instead of
        // stopping the session; the error is logged and the config stays
        // otherwise usable.
        let xkb_config = XkbConfig {
            layout: &config.input.xkb_rules_layout,
            ..Default::default()
        };
        let keyboard = match seat.add_keyboard(
            xkb_config,
            config.input.repeat_delay.max(0),
            config.input.repeat_rate.max(0),
        ) {
            Ok(keyboard) => keyboard,
            Err(err) => {
                tracing::warn!(
                    ?err,
                    layout = %config.input.xkb_rules_layout,
                    "failed to load the configured keyboard layout; using xkb defaults"
                );
                seat.add_keyboard(
                    Default::default(),
                    config.input.repeat_delay.max(0),
                    config.input.repeat_rate.max(0),
                )
                .expect("failed to add the default keyboard to the seat")
            }
        };
        let pointer = seat.add_pointer();

        // Physical size comes from the backend once it exists; M2 starts at
        // zero and lets `set_size` correct it on the first resize.
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(display);
        let decoration_state = XdgDecorationState::new::<Self>(display);
        let fractional_scale_state = FractionalScaleManagerState::new::<Self>(display);
        let viewporter_state = ViewporterState::new::<Self>(display);
        let xdg_activation_state = XdgActivationState::new::<Self>(display);

        let (compiled, issues) = CompiledBinds::compile(config);
        for (kind, issue) in issues {
            log_bind_issue(&kind, &issue);
        }
        tracing::debug!(
            key = compiled.keys.len(),
            mouse = compiled.mouse.len(),
            axis = compiled.axis.len(),
            gesture = compiled.gestures.len(),
            modes = compiled.modes.len(),
            "compiled bindings"
        );

        let mut state = Self {
            compositor_state: CompositorState::new::<Self>(display),
            xdg_shell_state: XdgShellState::new::<Self>(display),
            layer_shell_state: WlrLayerShellState::new::<Self>(display),
            _decoration_state: decoration_state,
            _fractional_scale_state: fractional_scale_state,
            _viewporter_state: viewporter_state,
            xdg_activation_state,
            _output_manager_state: output_manager_state,
            shm_state: ShmState::new::<Self>(display, vec![]),
            data_device_state: DataDeviceState::new::<Self>(display),
            seat_state,
            keyboard,
            keyboard_layout: None,
            pointer,
            outputs: HashMap::new(),
            primary_output: OutputId::new(0),
            config: config.clone(),
            binds: compiled.keys,
            mouse_binds: compiled.mouse,
            axis_binds: compiled.axis,
            gesture_binds: compiled.gestures,
            mode_binds: compiled.modes,
            modifiers: ModifiersState::default(),
            mode_stack: vec!["default".to_owned()],
            wayland_display: None,
            ipc_socket: None,
            config_path: None,
            quit_requested: false,
            config_epoch: 0,
            config_error: None,
            config_error_since: None,
            windows: Vec::new(),
            layers: Vec::new(),
            closings: Vec::new(),
            popup_manager: PopupManager::default(),
            focus: FocusTarget::None,
            focus_stack: FocusStack::new(),
            bound_keys: HashMap::new(),
            bound_buttons: HashSet::new(),
            interaction: None,
            last_pointer_press: None,
            start_time: Instant::now(),
            frame_clock: Clocks::new(),
            pointer_location: (0.0, 0.0).into(),
            pointer_seen: false,
            cursor_image: CursorImageStatus::default_named(),
            next_window_id: 1,
            next_layer_id: 1,
        };
        state.refresh_keyboard_layout();
        state
    }

    /// The initial per-space state for a new output.
    ///
    /// Every space starts with the configured layout and tunables; a space then
    /// remembers its own as the user cycles it (per-space layout memory).
    fn initial_space_state(config: &Config) -> SpaceState {
        let params = LayoutParams {
            master_count: u32::try_from(config.master_count.max(0)).unwrap_or(0),
            master_ratio: config.master_ratio,
            gaps: f64::from(config.gaps),
            border_width: f64::from(config.border_width),
            position: TilePosition::Left,
            column_width: ColumnWidth::Proportion(config.scroll.default_column_ratio),
            center: config.scroll.center_focused_column,
            ..LayoutParams::default()
        };
        SpaceState::new(
            LayoutKind::from_name(&config.layout).unwrap_or_default(),
            params,
        )
    }

    /// The output that currently owns focus and input.
    pub(crate) fn output(&self) -> &OutputRuntime {
        self.outputs
            .get(&self.primary_output)
            .expect("the primary output is always registered")
    }

    /// The output that currently owns focus and input, mutably.
    pub(crate) fn output_mut(&mut self) -> &mut OutputRuntime {
        self.outputs
            .get_mut(&self.primary_output)
            .expect("the primary output is always registered")
    }

    /// The output under `position`, or the nearest one when the point falls in
    /// a gap between outputs.
    pub(crate) fn output_at(&self, position: Point<f64, Logical>) -> Option<OutputId> {
        output_at_position(&output_rects(&self.outputs), position)
    }

    /// The top-left corner of `output` in global logical coordinates.
    fn output_origin(&self, output: OutputId) -> Option<Point<i32, Logical>> {
        self.outputs.get(&output).map(OutputRuntime::origin)
    }

    /// The crab output handle for a Smithay `Output` object.
    fn output_id_for_wayland(&self, output: &Output) -> Option<OutputId> {
        self.outputs
            .iter()
            .find_map(|(id, runtime)| (runtime.wayland == *output).then_some(*id))
    }

    /// Registers an output and makes it primary when it is the first one.
    pub(crate) fn add_output(
        &mut self,
        output: Output,
        mut model: OutputModel,
        render_transform: Transform,
    ) -> OutputId {
        let id = model.id;
        if self.outputs.contains_key(&id) {
            tracing::warn!(output = %id, "ignoring a duplicate output id");
            return id;
        }
        // A connector's configured position is known before its mode, so the
        // backend can apply it here and let `set_output_size` preserve the
        // origin. Mode, scale, and transform wait for the DRM backend.
        apply_configured_position(&self.config, &mut model);
        tracing::debug!(%id, connector = %model.name, "registered an output");
        let first = self.outputs.is_empty();
        let location = Point::<i32, Logical>::from((
            model.geometry.x.round() as i32,
            model.geometry.y.round() as i32,
        ));
        let runtime = OutputRuntime::new(output, model, render_transform);
        runtime
            .wayland
            .change_current_state(None, None, None, Some(location));
        self.outputs.insert(id, runtime);
        if first {
            self.primary_output = id;
        }
        self.relayout();
        id
    }

    /// Removes an output and migrates its windows to a remaining one.
    ///
    /// The last output cannot be removed: there would be nowhere to put its
    /// windows. The DRM hotplug path (M4) calls this when a connector goes
    /// away; the fallback is the first remaining output for now.
    #[allow(dead_code)]
    pub(crate) fn remove_output(&mut self, id: OutputId) -> bool {
        if self.outputs.len() <= 1 || !self.outputs.contains_key(&id) {
            return false;
        }
        self.outputs.remove(&id);
        let fallback = self
            .outputs
            .keys()
            .copied()
            .min()
            .expect("a remaining output exists");
        for window in &mut self.windows {
            if window.window.output == id {
                window.window.output = fallback;
            }
        }
        let removed_layers: Vec<LayerId> = self
            .layers
            .iter()
            .filter(|layer| layer.output == id)
            .map(|layer| layer.id)
            .collect();
        self.layers.retain(|layer| layer.output != id);
        self.closings.retain(|closing| closing.output != id);
        let mut lost_layer_focus = false;
        for layer in removed_layers {
            self.focus_stack.remove(FocusTarget::LayerSurface(layer));
            if self.focus == FocusTarget::LayerSurface(layer) {
                self.focus = FocusTarget::None;
                lost_layer_focus = true;
            }
        }
        if self.primary_output == id {
            self.primary_output = fallback;
        }
        if lost_layer_focus {
            self.refocus_after_view_change();
        }
        self.relayout();
        true
    }

    /// Sets crab's own Wayland socket name; spawned and autostarted clients
    /// receive it as `WAYLAND_DISPLAY`.
    pub(crate) fn set_wayland_display(&mut self, name: String) {
        self.wayland_display = Some(name);
    }

    /// Records the IPC socket path; spawned clients receive it as
    /// `CRAB_SOCKET`.
    pub(crate) fn set_ipc_socket(&mut self, path: PathBuf) {
        self.ipc_socket = Some(path);
    }

    /// Records the path of the config file used to start the compositor.
    pub(crate) fn set_config_path(&mut self, path: Option<PathBuf>) {
        self.config_path = path;
    }

    /// Reads the active xkb layout name back into the compositor state.
    pub(crate) fn refresh_keyboard_layout(&mut self) {
        let keyboard = self.keyboard.clone();
        let layout = keyboard.with_xkb_state(self, |ctx| {
            let xkb = ctx.xkb().lock().unwrap();
            xkb.layout_name(xkb.active_layout()).to_owned()
        });
        self.keyboard_layout = Some(layout);
    }

    /// Cycles the active xkb layout, returning whether it actually changed.
    ///
    /// Cycling a keymap with a single layout is a no-op, so an event is only
    /// emitted when the name changes.
    pub(crate) fn switch_keyboard_layout(&mut self, forward: bool) -> bool {
        let keyboard = self.keyboard.clone();
        let previous = self.keyboard_layout.clone();
        let current = keyboard.with_xkb_state(self, |mut ctx| {
            if forward {
                ctx.cycle_next_layout();
            } else {
                ctx.cycle_prev_layout();
            }
            let xkb = ctx.xkb().lock().unwrap();
            xkb.layout_name(xkb.active_layout()).to_owned()
        });
        self.keyboard_layout = Some(current);
        self.keyboard_layout != previous
    }

    /// Replaces the keymap with one built from `layout`.
    ///
    /// Smithay compiles the new keymap before touching the live state, so an
    /// invalid layout leaves the previous keymap in place.
    pub(crate) fn apply_keyboard_layout(&mut self, layout: &str) -> Result<(), String> {
        let keyboard = self.keyboard.clone();
        keyboard
            .set_xkb_config(
                self,
                XkbConfig {
                    layout,
                    ..Default::default()
                },
            )
            .map_err(|err| format!("failed to load keyboard layout `{layout}`: {err:?}"))?;
        self.refresh_keyboard_layout();
        Ok(())
    }

    /// Asks the backend to stop the event loop after the current dispatch.
    pub(crate) fn request_quit(&mut self) {
        self.quit_requested = true;
    }

    /// Runs the `[autostart] exec_once` commands.
    ///
    /// This is called once the socket exists, so `WAYLAND_DISPLAY` can point at
    /// crab. A command that fails to parse or spawn is logged and skipped: a
    /// bad autostart entry must not stop the session.
    pub(crate) fn run_autostart(&mut self) {
        for line in &self.config.autostart.exec_once {
            match spawn::parse_command(line) {
                Ok(argv) => {
                    tracing::info!(command = %line, "starting an autostart command");
                    spawn::spawn(
                        &argv,
                        self.wayland_display.as_deref(),
                        self.ipc_socket.as_deref(),
                    );
                }
                Err(reason) => {
                    tracing::warn!(command = %line, %reason, "invalid autostart command");
                }
            }
        }
    }

    /// Advertises the backend's current size to clients and re-lays out.
    ///
    /// M2 keeps the logical size equal to the physical size (the output is
    /// `Scale::Integer(1)`); fractional scale arrives with HDR and the render
    /// rework (post-0.1).
    pub(crate) fn set_size(&mut self, physical: Size<i32, Physical>) {
        self.set_output_size(self.primary_output, physical);
    }

    /// Updates one output's mode, damage tracker, and logical geometry.
    ///
    /// The winit backend drives this through [`Self::set_size`]; the DRM
    /// backend will call it per connector when a mode is selected.
    pub(crate) fn set_output_size(&mut self, output: OutputId, physical: Size<i32, Physical>) {
        let Some(runtime) = self.outputs.get_mut(&output) else {
            return;
        };
        runtime.size = (physical.w, physical.h).into();
        let mode = Mode {
            size: physical,
            refresh: 60_000,
        };
        runtime.wayland.change_current_state(
            Some(mode),
            Some(Transform::Normal),
            Some(Scale::Integer(1)),
            None,
        );
        runtime.wayland.set_preferred(mode);
        // The tracker's static mode must follow the new size. Rebuilding it
        // deliberately loses the previous damage state, so the first frame
        // after a resize repaints everything. The transform is the backend's
        // framebuffer correction, not the output's Wayland transform.
        runtime.damage_tracker = OutputDamageTracker::new(physical, 1.0, runtime.render_transform);
        let origin = runtime.model.geometry;
        runtime.model.set_geometry(Rect::new(
            origin.x,
            origin.y,
            f64::from(physical.w),
            f64::from(physical.h),
        ));
        self.sync_layer_zone(output);
        self.relayout();
    }

    /// Re-arranges one output's layer map after a size or exclusive-zone
    /// change and updates the area that output leaves for ordinary windows.
    fn sync_layer_zone(&mut self, output: OutputId) {
        let Some(runtime) = self.outputs.get(&output) else {
            return;
        };
        let origin = runtime.model.geometry;
        let zone = {
            let mut map = layer_map_for_output(&runtime.wayland);
            map.arrange();
            map.non_exclusive_zone()
        };
        // Layer maps arrange in output-local coordinates; the layout works in
        // the compositor's global space, so put the usable area back at the
        // output's origin before any window is placed in it.
        let area = Rect::new(
            origin.x + f64::from(zone.loc.x),
            origin.y + f64::from(zone.loc.y),
            f64::from(zone.size.w),
            f64::from(zone.size.h),
        );
        let Some(runtime) = self.outputs.get_mut(&output) else {
            return;
        };
        let changed = area != runtime.model.usable_area;
        tracing::trace!(?area, changed, %output, "synced the area left by layer surfaces");
        if changed {
            runtime.model.usable_area = area;
        }
    }

    /// Registers a new layer-shell surface and maps it onto `requested_output`
    /// (or the primary output when none was named).
    pub(crate) fn add_layer_surface(
        &mut self,
        surface: WlrLayerSurface,
        namespace: String,
        requested_output: Option<Output>,
    ) {
        let output = requested_output
            .as_ref()
            .and_then(|output| self.output_id_for_wayland(output))
            .unwrap_or(self.primary_output);
        let id = LayerId::new(self.next_layer_id);
        self.next_layer_id += 1;

        // A layer rule selects the animation by namespace. No rule (or no
        // animation named by it) means the surface appears immediately.
        let style = self
            .config
            .layer_rules
            .iter()
            .find(|rule| {
                rule.layer_name.is_empty() || rule.layer_name.iter().any(|name| name == &namespace)
            })
            .and_then(|rule| rule.animation_type_open.as_deref())
            .and_then(AnimationStyle::from_name);

        let layer = LayerSurface::new(surface, namespace.clone());
        let Some(runtime) = self.outputs.get(&output) else {
            tracing::warn!(%namespace, %output, "cannot map a layer surface on an unknown output");
            return;
        };
        let mapped = {
            let mut map = layer_map_for_output(&runtime.wayland);
            map.map_layer(&layer)
        };
        if let Err(err) = mapped {
            tracing::warn!(?err, %namespace, "failed to map a layer surface");
            return;
        }

        tracing::debug!(%namespace, %output, "mapped a layer surface");
        self.layers.push(LayerEntry {
            id,
            surface: layer,
            output,
            style,
            open: Animated::new(1.0, OPEN_CURVE, Kind::WindowOpen),
            mapped: false,
        });
        self.sync_layer_zone(output);
        self.relayout();
    }

    /// Handles a commit on a layer-shell surface: arranges the map, sends the
    /// initial configure when needed, and starts the layer's open animation on
    /// its first buffer.
    pub(crate) fn on_layer_commit(&mut self, surface: &WlSurface) {
        let Some(index) = self
            .layers
            .iter()
            .position(|entry| entry.surface.wl_surface() == surface)
        else {
            return;
        };

        let output = self.layers[index].output;
        let committed = committed_frame(surface).is_some();
        let Some(runtime) = self.outputs.get(&output) else {
            return;
        };
        let (layer, needs_configure, zone) = {
            let mut map = layer_map_for_output(&runtime.wayland);
            // The initial configure must follow the first commit; arranging
            // first respects the size and anchors the client has just sent.
            map.arrange();
            let Some(layer) = map
                .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
                .cloned()
            else {
                return;
            };
            let needs_configure = layer.layer_surface().has_pending_changes();
            (layer, needs_configure, map.non_exclusive_zone())
        };
        if needs_configure {
            layer.layer_surface().send_pending_configure();
        }

        let area = Rect::new(
            f64::from(zone.loc.x),
            f64::from(zone.loc.y),
            f64::from(zone.size.w),
            f64::from(zone.size.h),
        );
        let mut relayout = false;
        if let Some(runtime) = self.outputs.get_mut(&output) {
            if area != runtime.model.usable_area {
                runtime.model.usable_area = area;
                relayout = true;
            }
        }

        let animate = self.config.animations.enabled;
        let clocks = self.frame_clock;
        let entry = &mut self.layers[index];
        if !entry.mapped && committed {
            entry.mapped = true;
            if animate {
                entry.start_opening(&clocks);
            }
        }

        // An exclusive surface on the top or overlay layer owns the keyboard,
        // as the protocol requires. A bar normally asks for no keyboard and
        // leaves focus alone.
        let exclusive = self.exclusive_layer();
        if let Some(id) = exclusive {
            if self.focus != FocusTarget::LayerSurface(id) {
                self.focus_layer(id);
            }
        } else if let FocusTarget::LayerSurface(id) = self.focus {
            let still_focusable = self
                .layer(id)
                .is_some_and(|entry| entry.surface.can_receive_keyboard_focus());
            if !still_focusable {
                self.focus = FocusTarget::None;
                self.focus_stack.remove(FocusTarget::LayerSurface(id));
                self.refocus_after_view_change();
            }
        }

        if relayout {
            self.relayout();
        }
    }

    /// Removes a layer-shell surface from the map and focus bookkeeping.
    pub(crate) fn remove_layer_surface(&mut self, surface: &WlrLayerSurface) {
        let wl_surface = surface.wl_surface().clone();
        let Some(index) = self
            .layers
            .iter()
            .position(|entry| entry.surface.wl_surface() == &wl_surface)
        else {
            return;
        };
        let output = self.layers[index].output;
        if let Some(runtime) = self.outputs.get(&output) {
            let mut map = layer_map_for_output(&runtime.wayland);
            let found = map
                .layers()
                .find(|layer| layer.layer_surface() == surface)
                .cloned();
            if let Some(layer) = found {
                map.unmap_layer(&layer);
            }
        }

        let id = self.layers.remove(index).id;
        tracing::debug!(?id, %output, "removed a layer surface");
        self.focus_stack.remove(FocusTarget::LayerSurface(id));
        let was_focused = self.focus == FocusTarget::LayerSurface(id);
        if was_focused {
            self.focus = FocusTarget::None;
        }
        self.sync_layer_zone(output);
        if was_focused {
            self.refocus_after_view_change();
        }
        self.relayout();
    }

    /// The layer surface with this id.
    pub(crate) fn layer(&self, id: LayerId) -> Option<&LayerEntry> {
        self.layers.iter().find(|entry| entry.id == id)
    }

    /// The id of the layer surface whose toplevel is `surface`.
    pub(crate) fn layer_id_for(&self, surface: &WlSurface) -> Option<LayerId> {
        self.layers
            .iter()
            .find(|entry| entry.surface.wl_surface() == surface)
            .map(|entry| entry.id)
    }

    /// The top-most `Top`/`Overlay` surface that asked for exclusive keyboard
    /// focus, if any.
    fn exclusive_layer(&self) -> Option<LayerId> {
        self.layers
            .iter()
            .rev()
            .find(|entry| {
                entry.output == self.primary_output
                    && matches!(entry.surface.layer(), Layer::Top | Layer::Overlay)
                    && entry.surface.cached_state().keyboard_interactivity
                        == KeyboardInteractivity::Exclusive
            })
            .map(|entry| entry.id)
    }

    /// The top-most layer surface under `position`, if any.
    ///
    /// Background surfaces are wallpaper and never take pointer input; bottom,
    /// top, and overlay surfaces do.
    pub(crate) fn layer_at(&self, position: Point<f64, Logical>) -> Option<LayerId> {
        let output = self.output_at(position)?;
        let origin = self.output_origin(output)?;
        let local = position - origin.to_f64();
        let runtime = self.outputs.get(&output)?;
        let map = layer_map_for_output(&runtime.wayland);
        for kind in [Layer::Overlay, Layer::Top, Layer::Bottom] {
            if let Some(layer) = map.layer_under(kind, local) {
                return self.layer_id_for(layer.wl_surface());
            }
        }
        None
    }

    /// The surface under `position` within the given layer kinds, with its
    /// logical origin.
    fn layer_surface_under(
        &self,
        position: Point<f64, Logical>,
        layers: &[Layer],
        output: OutputId,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        let origin = self.output_origin(output)?;
        let local = position - origin.to_f64();
        let runtime = self.outputs.get(&output)?;
        let map = layer_map_for_output(&runtime.wayland);
        for &kind in layers {
            let Some(layer) = map.layer_under(kind, local) else {
                continue;
            };
            let Some(geometry) = map.layer_geometry(layer) else {
                continue;
            };
            // Layer geometry is output-local; report the surface origin in
            // global coordinates because that is what the seat subtracts from
            // the pointer position.
            let surface_origin = origin + geometry.loc + self.layer_offset(layer.wl_surface());
            let Some((surface, location)) =
                layer.surface_under(position - surface_origin.to_f64(), WindowSurfaceType::ALL)
            else {
                continue;
            };
            return Some((surface, (surface_origin + location).to_f64()));
        }
        None
    }

    /// How opaque a layer surface is while its open animation runs.
    pub(crate) fn layer_alpha(&self, surface: &WlSurface) -> f32 {
        self.layers
            .iter()
            .find(|entry| entry.surface.wl_surface() == surface)
            .map_or(1.0, |entry| entry.alpha(&self.frame_clock))
    }

    /// Where a layer surface is drawn, including the slide component of its
    /// open animation.
    pub(crate) fn layer_offset(&self, surface: &WlSurface) -> Point<i32, Logical> {
        let Some(entry) = self
            .layers
            .iter()
            .find(|entry| entry.surface.wl_surface() == surface)
        else {
            return (0, 0).into();
        };
        if entry.style != Some(AnimationStyle::Slide) {
            return (0, 0).into();
        }
        let progress = entry.progress(&self.frame_clock);
        let dy = ((1.0 - progress) * OPEN_SLIDE).round() as i32;
        (0, dy).into()
    }

    /// Recomputes every window's rectangle for the current view and layout.
    pub(crate) fn relayout(&mut self) {
        let outputs: Vec<OutputId> = self.outputs.keys().copied().collect();
        for output in outputs {
            layout::arrange(self, output);
        }
    }

    /// Re-times the animation clock for the frame about to be drawn.
    ///
    /// `frame_time` is the presentation time of that frame, in seconds since
    /// [`Crab::start_time`]. The winit backend has no vertical-blank timestamp
    /// and reads its own loop clock at the frame boundary; the tty backend will
    /// pass the real one (TODO.md §3.2).
    pub(crate) fn advance_frame(&mut self, frame_time: f64) {
        self.frame_clock.advance(frame_time);
        // A close whose fade has finished has nothing left to draw; retire it
        // before the frame that would sample it at zero opacity.
        self.closings
            .retain(|closing| closing.progress.is_animating(&self.frame_clock));
    }

    /// Whether anything on screen is still moving.
    ///
    /// The backends ask this after each frame: while it holds, the redraw
    /// machine keeps requesting frames; once it stops, the compositor goes back
    /// to drawing nothing at all (ROADMAP.md §9, COMPOSITOR.md §9).
    pub(crate) fn is_animating(&self) -> bool {
        self.windows.iter().any(|window| {
            window.visual.is_animating(&self.frame_clock)
                || window.open.is_animating(&self.frame_clock)
        }) || self
            .layers
            .iter()
            .any(|layer| layer.open.is_animating(&self.frame_clock))
            || self
                .closings
                .iter()
                .any(|closing| closing.progress.is_animating(&self.frame_clock))
    }

    /// Registers a newly created toplevel with the space/layout model.
    ///
    /// A toplevel with a parent is a transient dialog: it floats immediately
    /// (`COMPOSITOR.md` §7.5). Everything else enters the tiled layout.
    pub(crate) fn add_toplevel(&mut self, surface: ToplevelSurface) {
        let id = WindowId::new(self.next_window_id);
        self.next_window_id += 1;

        let output = self.primary_output;
        let spaces = self.output().model.view.visible;
        let mut window = Window::new(id, output, spaces);
        let (title, app_id) = read_toplevel_info(&surface);
        window.title = title;
        window.app_id = app_id;
        window.floating = surface.parent().is_some();

        // Tiled windows get a starting size; floating ones choose their own.
        surface.with_pending_state(|state| {
            if !window.floating {
                state.size = Some(DEFAULT_WINDOW_SIZE.into());
            }
        });
        surface.send_configure();

        // Only tiled windows were given a size to be; a floating one chose
        // its own and gets an empty configure (xdg-shell's initial one).
        let mut configure = Configure::default();
        if !window.floating {
            configure.ask(DEFAULT_WINDOW_SIZE.into());
        }

        self.windows.push(ManagedWindow {
            surface,
            window,
            location: (0, 0).into(),
            size: DEFAULT_WINDOW_SIZE.into(),
            geometry: Rectangle::default(),
            visible: false,
            mapped: false,
            visual: Animated::new(
                Rect::new(
                    0.0,
                    0.0,
                    f64::from(DEFAULT_WINDOW_SIZE.0),
                    f64::from(DEFAULT_WINDOW_SIZE.1),
                ),
                WINDOW_CURVE,
                Kind::WindowMove,
            ),
            open: Animated::new(1.0, OPEN_CURVE, Kind::WindowOpen),
            visual_placed: false,
            last_frame: None,
            rule_applied: false,
            configure,
        });

        // A rule that has all the information it needs can be applied right
        // away; one that names a title or app id waits for `update_toplevel_info`.
        self.apply_window_rules(id);

        // A first window on the empty space past the end turns that space into
        // space 1 (the view moves with it). A rule may also have sent the window
        // somewhere else, in which case focusing it would put the keyboard on a
        // hidden surface.
        self.compact_spaces();
        let on_view = self
            .window(id)
            .is_some_and(|window| window.window.is_on(self.output().model.view.visible));
        if on_view {
            self.focus_window(Some(id));
        }
        if self.focused_id().is_none() {
            self.refocus_after_view_change();
        }
        self.relayout();
    }

    /// Places a new popup: computes the geometry its positioner asks for
    /// (clamped to the output), hands it to the [`PopupManager`] so its tree
    /// and grabs are tracked, and sends the initial configure so it can map.
    pub(crate) fn add_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        let geometry = self.popup_geometry(&surface, positioner);
        tracing::trace!(?geometry, "placed a new popup");
        surface.with_pending_state(|state| {
            state.positioner = positioner;
            state.geometry = geometry;
        });

        if let Err(err) = self
            .popup_manager
            .track_popup(PopupKind::Xdg(surface.clone()))
        {
            tracing::debug!(?err, "failed to track a new popup");
        }
        if let Err(err) = surface.send_configure() {
            tracing::debug!(?err, "failed to configure a new popup");
        }
    }

    /// Replaces the geometry a popup was placed with, from a fresh positioner
    /// (an `xdg_popup.reposition` request), and acknowledges the token.
    pub(crate) fn reposition_popup(
        &mut self,
        surface: &PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        let geometry = self.popup_geometry(surface, positioner);
        tracing::trace!(?geometry, "re-placed a popup");
        surface.with_pending_state(|state| {
            state.positioner = positioner;
            state.geometry = geometry;
        });
        surface.send_repositioned(token);
    }

    /// The geometry a positioner asks for, after constraint adjustment against
    /// the output.
    ///
    /// [`PositionerState::get_unconstrained_geometry`] works in the parent's
    /// *window geometry* space, so the output is translated into that space
    /// first; a parent we do not track (or an output with no size yet) falls
    /// back to the unconstrained placement.
    fn popup_geometry(
        &self,
        surface: &PopupSurface,
        positioner: PositionerState,
    ) -> Rectangle<i32, Logical> {
        let origin = surface
            .get_parent_surface()
            .and_then(|parent| self.window_geometry_origin(&parent));
        match origin {
            Some(origin) if self.output().size.w > 0 && self.output().size.h > 0 => positioner
                .get_unconstrained_geometry(Rectangle::new(
                    (-origin.x, -origin.y).into(),
                    self.output().size,
                )),
            _ => positioner.get_geometry(),
        }
    }

    /// Handles a surface commit: refresh hints, mark it mapped, and re-lay out
    /// when something the layout cares about changed.
    pub(crate) fn on_commit(&mut self, surface: &WlSurface) {
        let mut relayout = false;
        let mut newly_mapped = Vec::new();

        let (min_size, max_size) = read_size_hints(surface);
        let bbox = smithay::desktop::utils::bbox_from_surface_tree(surface, (0, 0));
        let geometry = window_geometry(surface, bbox);
        let committed = committed_frame(surface);
        for window in &mut self.windows {
            if window.wl_surface() != surface {
                continue;
            }
            if window.geometry != geometry {
                tracing::trace!(
                    ?geometry,
                    title = ?window.window.title,
                    "window geometry changed"
                );
                window.geometry = geometry;
                relayout = true;
            }
            if geometry.size.w > 0 && geometry.size.h > 0 {
                if let Some((buffer, buffer_scale, buffer_transform)) = &committed {
                    window.last_frame = Some(FrameSnapshot {
                        buffer: buffer.clone(),
                        buffer_scale: *buffer_scale,
                        buffer_transform: *buffer_transform,
                        geometry,
                    });
                }
            }
            if window.window.min_size != min_size || window.window.max_size != max_size {
                window.window.min_size = min_size;
                window.window.max_size = max_size;
                relayout = true;
            }
            // A commit without a buffer is only metadata (initial state, size
            // hints, decoration); the window becomes visible on the first
            // buffer, which is also when the open animation should start.
            if !window.mapped && committed.is_some() && geometry.size.w > 0 && geometry.size.h > 0 {
                window.mapped = true;
                newly_mapped.push(window.id());
                relayout = true;
            }
            // A floating window owns its size; adopt whatever it committed,
            // measured by its window geometry rather than by its buffer (which
            // carries client-side decoration margins the layout must not see).
            // If a rule asked for a size, wait until the client answers that
            // configure (or refuses it) before letting the commit take over.
            if window.window.floating && geometry.size.w > 0 && geometry.size.h > 0 {
                let asked = window.configure.asked();
                let complied = asked == Some(geometry.size);
                if asked.is_some() {
                    window.on_committed_size(geometry.size);
                }
                if complied || asked.is_none() || window.is_size_refusing() {
                    if window.size != geometry.size {
                        window.size = geometry.size;
                        relayout = true;
                    }
                    let rect = window.rect();
                    window.window.float_geometry = Some(rect);
                }
            } else if geometry.size.w > 0 && geometry.size.h > 0 {
                // A tiled window is the size we configured it to, or it is
                // behind, or it is refusing; this is where the three are told
                // apart (TODO.md §1.5).
                window.on_committed_size(geometry.size);
            }
        }

        // Popups change trees and map through the manager (it needs every
        // commit to move an unmapped popup into its tree).
        self.popup_manager.commit(surface);

        // Layer-shell surfaces own their placement through Smithay's layer
        // map; the commit is also when the initial configure may be sent.
        if self.layer_id_for(surface).is_some() {
            self.on_layer_commit(surface);
        }

        if relayout {
            self.relayout();
        }

        // The first buffer is where the window becomes visible: start its open
        // animation after the layout has settled on the rectangle it is heading
        // for, so the animation does not immediately retarget.
        if self.config.animations.enabled {
            let style = self.config.animations.open;
            let clocks = self.frame_clock;
            for id in newly_mapped {
                if let Some(window) = self.window_mut(id) {
                    window.start_opening(style, &clocks);
                }
            }
        }
    }

    /// Drops entries whose client object is gone.
    pub(crate) fn prune_dead(&mut self) {
        if self.windows.iter().any(|window| !window.surface.alive()) {
            let dead: Vec<WindowId> = self
                .windows
                .iter()
                .filter(|window| !window.surface.alive())
                .map(ManagedWindow::id)
                .collect();
            self.windows.retain(|window| window.surface.alive());
            self.forget_windows(&dead);
        }
        self.prune_dead_layers();
        self.popup_manager.cleanup();
    }

    /// Drops layer entries whose client object is gone and re-arranges the map.
    fn prune_dead_layers(&mut self) {
        let dead: Vec<LayerId> = self
            .layers
            .iter()
            .filter(|entry| !entry.surface.alive())
            .map(|entry| entry.id)
            .collect();
        if dead.is_empty() {
            return;
        }
        self.layers.retain(|entry| entry.surface.alive());
        let mut lost_focus = false;
        for id in dead {
            self.focus_stack.remove(FocusTarget::LayerSurface(id));
            if self.focus == FocusTarget::LayerSurface(id) {
                self.focus = FocusTarget::None;
                lost_focus = true;
            }
        }
        let outputs: Vec<OutputId> = self.outputs.keys().copied().collect();
        for output in outputs {
            if let Some(runtime) = self.outputs.get(&output) {
                let mut map = layer_map_for_output(&runtime.wayland);
                map.cleanup();
            }
            self.sync_layer_zone(output);
        }
        if lost_focus {
            self.refocus_after_view_change();
        }
        self.relayout();
    }

    /// Removes the window owning `surface` (from `toplevel_destroyed`).
    ///
    /// If it was on screen and animations are on, its last buffer is kept and
    /// the window becomes a [`ClosingWindow`] that fades out over the scene.
    pub(crate) fn remove_window(&mut self, surface: &WlSurface) {
        let Some(index) = self
            .windows
            .iter()
            .position(|window| window.wl_surface() == surface)
        else {
            return;
        };
        let id = self.windows[index].window.id;
        // Remember where in the scene it was, before removing it shifts the
        // surviving windows down. The snapshot fades out in that group and near
        // that index instead of always covering the live scene.
        let layer = self.windows[index].stack_layer();
        let order = self.stacked_windows();
        let stack_index = stack_index_in_layer(&order, id, layer, |other| {
            self.window(other).map(ManagedWindow::stack_layer)
        });
        let closing = if self.config.animations.enabled
            && self.windows[index].mapped
            && self.windows[index].visible
        {
            let window = &self.windows[index];
            let style = self.config.animations.close;
            let alpha = window.open_alpha(&self.frame_clock);
            let visual = window.visual_rect(&self.frame_clock);
            let origin = window.visual_origin(&self.frame_clock);
            let size = window.visual_size(&self.frame_clock);
            window.last_frame.as_ref().map(|frame| {
                // Map the kept buffer through the fit it had when it was last
                // committed, even though the client detached it before closing.
                let fit = WindowFit::new(
                    Rectangle::new(origin, size),
                    origin - frame.geometry.loc,
                    frame.geometry.size,
                    !window.is_size_refusing(),
                );
                let mut progress = Animated::new(0.0, CLOSE_CURVE, Kind::WindowClose);
                progress.animate(1.0, Kind::WindowClose, &self.frame_clock);
                ClosingWindow {
                    output: window.window.output,
                    buffer: frame.buffer.clone(),
                    buffer_scale: frame.buffer_scale,
                    buffer_transform: frame.buffer_transform,
                    fit,
                    visual,
                    style,
                    alpha,
                    layer,
                    stack_index,
                    progress,
                }
            })
        } else {
            None
        };
        self.windows.remove(index);
        if let Some(closing) = closing {
            tracing::trace!("fading out a window whose toplevel is gone");
            self.closings.push(closing);
        }
        self.forget_windows(&[id]);
    }

    /// The active key mode: the top of the stack.
    #[must_use]
    pub(crate) fn active_mode(&self) -> &str {
        self.mode_stack
            .last()
            .map_or("default", std::string::String::as_str)
    }

    /// Enter a key mode/submap.
    ///
    /// Unknown modes and the reserved `default` name are ignored with a
    /// warning: a bad IPC request or stale bind must not break input. Entering
    /// the mode already on top is a no-op so a held key cannot deepen the stack.
    pub(crate) fn enter_mode(&mut self, name: &str) {
        if name == "default" || !self.mode_binds.contains_key(name) {
            tracing::warn!(mode = name, "ignoring request to enter an unknown key mode");
            return;
        }
        if self.active_mode() == name {
            return;
        }
        if self.mode_stack.len() >= MAX_MODE_DEPTH {
            tracing::warn!(mode = name, "key mode stack is full; ignoring enter_mode");
            return;
        }
        self.mode_stack.push(name.to_owned());
    }

    /// Leave every active mode, returning to the default mode.
    pub(crate) fn leave_mode(&mut self) {
        self.mode_stack.truncate(1);
    }

    /// Pop one mode from the mode stack, staying in default if it is the only
    /// entry.
    pub(crate) fn pop_mode(&mut self) {
        if self.mode_stack.len() > 1 {
            self.mode_stack.pop();
        }
    }

    /// Drops modes that no longer exist after a reload.
    ///
    /// The stack keeps its order but loses unknown names, so a reload that
    /// deletes the active mode falls back to the mode below it instead of
    /// leaving input stuck in a mode with no binds.
    pub(crate) fn prune_modes(&mut self) {
        self.mode_stack
            .retain(|mode| mode == "default" || self.mode_binds.contains_key(mode));
        if self.mode_stack.first().map(String::as_str) != Some("default") {
            self.mode_stack.insert(0, "default".to_owned());
        }
    }

    /// The keyboard binding for `keysym`, walking the mode stack top-down and
    /// falling back to the default tables.
    #[must_use]
    pub(crate) fn key_bind(&self, modifiers: &ModifiersState, keysym: u32) -> Option<BoundAction> {
        for mode in self.mode_stack.iter().skip(1).rev() {
            if let Some(compiled) = self.mode_binds.get(mode) {
                if let Some(bound) = compiled.keys.bind(modifiers, keysym) {
                    return Some(bound);
                }
            }
        }
        self.binds.bind(modifiers, keysym)
    }

    /// The pointer-button binding for `button`, using the same mode stack.
    #[must_use]
    pub(crate) fn mouse_bind(
        &self,
        modifiers: &ModifiersState,
        button: u32,
    ) -> Option<crate::compositor::binds::MouseAction> {
        for mode in self.mode_stack.iter().skip(1).rev() {
            if let Some(compiled) = self.mode_binds.get(mode) {
                if let Some(action) = compiled.mouse.action(modifiers, button) {
                    return Some(action);
                }
            }
        }
        self.mouse_binds.action(modifiers, button)
    }

    /// The scroll binding for `axis`/`positive`, using the same mode stack.
    #[must_use]
    pub(crate) fn axis_bind(
        &self,
        modifiers: &ModifiersState,
        axis: crate::compositor::binds::ScrollAxis,
        positive: bool,
    ) -> Option<Action> {
        for mode in self.mode_stack.iter().skip(1).rev() {
            if let Some(compiled) = self.mode_binds.get(mode) {
                if let Some(action) = compiled.axis.action(modifiers, axis, positive) {
                    return Some(action);
                }
            }
        }
        self.axis_binds.action(modifiers, axis, positive)
    }

    /// The gesture binding for a direction and finger count, using the same
    /// mode stack.
    #[must_use]
    pub(crate) fn gesture_bind(
        &self,
        modifiers: &ModifiersState,
        dir: crate::compositor::binds::GestureDir,
        fingers: u32,
    ) -> Option<Action> {
        for mode in self.mode_stack.iter().skip(1).rev() {
            if let Some(compiled) = self.mode_binds.get(mode) {
                if let Some(action) = compiled.gestures.action(modifiers, dir, fingers) {
                    return Some(action);
                }
            }
        }
        self.gesture_binds.action(modifiers, dir, fingers)
    }

    /// The id of the window whose toplevel is `surface`.
    pub(crate) fn window_id_for(&self, surface: &WlSurface) -> Option<WindowId> {
        self.windows
            .iter()
            .find(|window| window.wl_surface() == surface)
            .map(ManagedWindow::id)
    }

    /// Re-reads the `xdg_toplevel` title and app id into the window model.
    pub(crate) fn update_toplevel_info(&mut self, surface: &ToplevelSurface) {
        let (title, app_id) = read_toplevel_info(surface);
        let Some(id) = self.window_id_for(surface.wl_surface()) else {
            return;
        };
        if let Some(window) = self.window_mut(id) {
            window.window.title = title;
            window.window.app_id = app_id;
        }
        // `set_app_id` is not double buffered and arrives before the first
        // commit, so this is where an `app_id` rule gets its chance. A title
        // usually arrives through the same path, and either may be the last
        // matcher a rule was waiting for.
        if self.apply_window_rules(id) {
            self.compact_spaces();
            self.relayout();
        }
    }

    /// Applies the first matching `[window_rule]` to a window, once.
    ///
    /// A rule waits until every matcher it names can be decided: an app-id rule
    /// waits for `app_id`, a title rule for `title`. The first rule in config
    /// order wins, even when a later catch-all could already match, so the
    /// outcome does not depend on protocol event order. The size and offset
    /// place a floating window at the centre of the usable area plus the rule's
    /// offset (the mango convention), and the size is asked for with a configure
    /// so the client can honour it.
    ///
    /// Returns whether a rule matched, so a late title/app-id can trigger the
    /// compact/relayout that its action may need.
    fn apply_window_rules(&mut self, id: WindowId) -> bool {
        let Some(index) = self
            .windows
            .iter()
            .position(|window| window.window.id == id)
        else {
            return false;
        };
        let was_applied = self.windows[index].rule_applied;
        let app_id = self.windows[index].window.app_id.clone();
        let title = self.windows[index].window.title.clone();
        let output_name = self
            .outputs
            .get(&self.windows[index].window.output)
            .map(|o| o.model.name.as_str());
        // X11/Xwayland is not supported yet, so every surface is Wayland. This
        // is a known value, not a matcher that can still arrive: an `x11 = true`
        // rule must be skipped rather than stalling the rest of the chain.
        let x11 = false;
        let floating = self.windows[index].window.floating;
        let fullscreen = self.windows[index].window.fullscreen;

        if !was_applied {
            let rule = match find_window_rule(
                &self.config.window_rules,
                app_id.as_deref(),
                title.as_deref(),
                output_name,
                x11,
                floating,
                fullscreen,
            ) {
                RuleSearch::Pending => return false,
                RuleSearch::NoMatch => {
                    self.windows[index].rule_applied = true;
                    return false;
                }
                RuleSearch::Matched(rule) => rule.clone(),
            };
            self.windows[index].rule_applied = true;

            let occupied = self.occupied_spaces();
            let area = self.output().model.usable_area;
            let window = &mut self.windows[index];
            window.window.pinned |= rule.pinned;
            window.window.always_on_top |= rule.always_on_top;
            window.window.rules.pinned |= rule.pinned;
            window.window.rules.always_on_top |= rule.always_on_top;
            window.window.rules.no_focus |= rule.no_focus;
            if let Some(space) = rule.space {
                if let Some(mask) = SpaceSet::from_dynamic_index(space, occupied) {
                    window.window.move_to_space(mask);
                    window.window.rules.spaces = Some(mask);
                }
            }

            if rule.open_floating {
                window.window.rules.floating = Some(true);
                let requested = rule
                    .size
                    .map_or(window.size, |(w, h)| Size::new(w.max(1), h.max(1)));
                let size =
                    clamp_to_hints(requested, window.window.min_size, window.window.max_size);
                let rect = rule_rect(&rule, size, area);
                window.size = size;
                window.location = (rect.x as i32, rect.y as i32).into();
                window.float_at(rect);
                window.snap_visual_to(rect);
                window.configure_to(size);
                tracing::debug!(
                    app_id = app_id.as_deref().unwrap_or(""),
                    title = title.as_deref().unwrap_or(""),
                    ?size,
                    x = window.location.x,
                    y = window.location.y,
                    "applied a floating window rule"
                );
            } else if rule.space.is_some() {
                tracing::debug!(
                    app_id = app_id.as_deref().unwrap_or(""),
                    title = title.as_deref().unwrap_or(""),
                    spaces = ?window.window.spaces,
                    "applied a window rule's initial space"
                );
            }

            if self.focus.window() == Some(id) {
                self.refocus_after_view_change();
            }
            true
        } else {
            // Re-evaluate dynamic rules when title/app_id change
            let rule = match find_window_rule(
                &self.config.window_rules,
                app_id.as_deref(),
                title.as_deref(),
                output_name,
                x11,
                floating,
                fullscreen,
            ) {
                RuleSearch::Pending | RuleSearch::NoMatch => return false,
                RuleSearch::Matched(rule) => rule.clone(),
            };
            if !rule.dynamic {
                return false;
            }
            let mut changed = false;
            let occupied = self.occupied_spaces();
            let area = self.output().model.usable_area;
            if let Some(window) = self.windows.get_mut(index) {
                if rule.pinned != window.window.pinned {
                    window.window.pinned = rule.pinned;
                    window.window.rules.pinned = rule.pinned;
                    changed = true;
                }
                if rule.always_on_top != window.window.always_on_top {
                    window.window.always_on_top = rule.always_on_top;
                    window.window.rules.always_on_top = rule.always_on_top;
                    changed = true;
                }
                if rule.no_focus != window.window.rules.no_focus {
                    window.window.rules.no_focus = rule.no_focus;
                    changed = true;
                }
                if let Some(space) = rule.space {
                    if let Some(mask) = SpaceSet::from_dynamic_index(space, occupied) {
                        if window.window.rules.spaces != Some(mask) || window.window.spaces != mask
                        {
                            window.window.move_to_space(mask);
                            window.window.rules.spaces = Some(mask);
                            changed = true;
                        }
                    }
                }
                if rule.open_floating
                    && (!window.window.floating || window.window.rules.floating != Some(true))
                {
                    window.window.rules.floating = Some(true);
                    let requested = rule
                        .size
                        .map_or(window.size, |(w, h)| Size::new(w.max(1), h.max(1)));
                    let size =
                        clamp_to_hints(requested, window.window.min_size, window.window.max_size);
                    let rect = rule_rect(&rule, size, area);
                    window.size = size;
                    window.location = (rect.x as i32, rect.y as i32).into();
                    window.float_at(rect);
                    window.snap_visual_to(rect);
                    window.configure_to(size);
                    changed = true;
                }
            }
            if changed {
                self.compact_spaces();
                self.relayout();
                tracing::debug!("re-applied dynamic window rule actions");
            }
            if self.focus.window() == Some(id) {
                self.refocus_after_view_change();
            }
            changed
        }
    }

    /// Drops `ids` from the focus stack and per-space history and refocuses.
    fn forget_windows(&mut self, ids: &[WindowId]) {
        // A space whose last window just went away ceases to exist, and the
        // spaces after it move down (niri-style dynamic spaces).
        self.compact_spaces();
        for &id in ids {
            self.focus_stack.remove(FocusTarget::Window(id));
            for space in &mut self.output_mut().model.space_states {
                space.forget(id);
            }
        }
        self.refocus_after_view_change();
        self.relayout();
    }

    /// Every space that currently holds a window.
    fn occupied_spaces(&self) -> SpaceSet {
        self.windows
            .iter()
            .filter(|window| window.window.output == self.primary_output)
            .fold(SpaceSet::empty(), |spaces, window| {
                spaces | window.window.spaces
            })
    }

    /// Resolves a 1-based dynamic index to the fixed slot it names.
    ///
    /// The spaces that hold windows are dense from 1. An index past the last one
    /// names the empty space right after them, so `MOD+9` on a two-space desktop
    /// means "the new space" instead of leaving six holes.
    fn dynamic_space(&self, index: u8) -> Option<SpaceSet> {
        SpaceSet::from_dynamic_index(index, self.occupied_spaces())
    }

    /// Removes spaces that just became empty and renumbers the rest.
    ///
    /// Window membership is renumbered to match, and each space's layout state
    /// moves with it (see [`crab::core::Output::compact`]).
    fn compact_spaces(&mut self) {
        let occupied = self.occupied_spaces();
        let remap = self.output_mut().model.compact(occupied);
        for window in &mut self.windows {
            if window.window.output == self.primary_output {
                window.window.spaces = remap.map_mask(window.window.spaces);
            }
        }
    }

    /// The origin of the *window geometry* of the toplevel or popup owning
    /// `surface`, in output coordinates, if we track it.
    ///
    /// The layout places windows by their window geometry and popups are
    /// positioned against their parent's, so this is the reference frame every
    /// placed rectangle is expressed in.
    fn window_geometry_origin(&self, surface: &WlSurface) -> Option<Point<i32, Logical>> {
        if let Some(window) = self
            .windows
            .iter()
            .find(|window| window.wl_surface() == surface)
        {
            return Some(window.visual_origin(&self.frame_clock));
        }
        for window in &self.windows {
            let origin = window.visual_origin(&self.frame_clock);
            for (popup, offset) in popup_offsets(window) {
                if popup.wl_surface() == surface {
                    // `offset` is the popup's surface origin; its geometry
                    // origin is that plus whatever the popup itself reports.
                    return Some(origin + offset + popup.geometry().loc);
                }
            }
        }
        // A popup may belong to a layer-shell surface (a launcher's menu).
        for entry in &self.layers {
            let Some(origin) = self.layer_origin(&entry.surface) else {
                continue;
            };
            if entry.surface.wl_surface() == surface {
                return Some(origin);
            }
            for (popup, location) in PopupManager::popups_for_surface(entry.surface.wl_surface()) {
                if popup.wl_surface() == surface {
                    let offset = location - popup.geometry().loc;
                    return Some(origin + offset + popup.geometry().loc);
                }
            }
        }
        None
    }

    /// The drawn origin of a layer surface, in output coordinates.
    fn layer_origin(&self, layer: &LayerSurface) -> Option<Point<i32, Logical>> {
        let output = self
            .layers
            .iter()
            .find(|entry| entry.surface.wl_surface() == layer.wl_surface())
            .map_or(self.primary_output, |entry| entry.output);
        let runtime = self.outputs.get(&output)?;
        let origin = runtime.origin();
        let map = layer_map_for_output(&runtime.wayland);
        // Layer maps place surfaces relative to their output; popups are
        // tracked in global compositor coordinates, like window popups.
        map.layer_geometry(layer)
            .map(|geometry| origin + geometry.loc + self.layer_offset(layer.wl_surface()))
    }

    /// The managed window with this id.
    pub(crate) fn window(&self, id: WindowId) -> Option<&ManagedWindow> {
        self.windows.iter().find(|window| window.window.id == id)
    }

    /// The managed window with this id, mutably.
    pub(crate) fn window_mut(&mut self, id: WindowId) -> Option<&mut ManagedWindow> {
        self.windows
            .iter_mut()
            .find(|window| window.window.id == id)
    }

    /// The focused window id, if the focused window still exists.
    pub(crate) fn focused_id(&self) -> Option<WindowId> {
        self.focus.window().filter(|id| self.window(*id).is_some())
    }

    /// Every visible window in bottom-to-top painting order.
    ///
    /// Tiled and covering windows keep their window-list order. Floating windows
    /// are ordered by the focus stack instead, least recently focused first: an
    /// unfocused floating window sits below every focused one, and focusing a
    /// peer raises it above the rest. Rendering and hit-testing share this order
    /// so a click lands on the window the user can see.
    pub(crate) fn stacked_windows(&self) -> Vec<WindowId> {
        self.stacked_windows_on(self.primary_output)
    }

    /// Every visible window on `output` in bottom-to-top painting order.
    pub(crate) fn stacked_windows_on(&self, output: OutputId) -> Vec<WindowId> {
        let entries: Vec<StackEntry> = self
            .windows
            .iter()
            .filter(|window| window.window.output == output)
            .map(|window| StackEntry {
                id: window.id(),
                layer: window.stack_layer(),
                visible: window.mapped && window.visible,
            })
            .collect();
        stacked_order(&entries, &self.focus_stack)
    }

    /// Focuses `id` (or nothing), updating activation and keyboard focus.
    ///
    /// Every focus change goes through here (`COMPOSITOR.md` §6) so the
    /// protocol state, the focus stack, and the space history stay in step.
    pub(crate) fn focus_window(&mut self, id: Option<WindowId>) {
        if id
            .and_then(|id| self.window(id))
            .is_some_and(|window| !window.is_focusable())
        {
            return;
        }
        // Keyboard focus follows the window's output, so space actions and
        // rendering target the monitor the user just interacted with. Do this
        // before looking for an exclusive layer: that layer belongs to the
        // target output, not the one focus is leaving.
        if let Some(window) = id.and_then(|id| self.window(id)) {
            self.primary_output = window.window.output;
        }
        // A keyboard-exclusive layer (a launcher, most often) keeps the
        // keyboard until it goes away, whatever tried to focus a window.
        if let Some(layer) = self.exclusive_layer() {
            self.focus_layer(layer);
            return;
        }
        let previous = self.focus;
        let target = id.map_or(FocusTarget::None, FocusTarget::Window);
        self.focus = target;

        if let Some(old) = previous.window() {
            if Some(old) != id {
                self.set_activated(old, false);
            }
        }
        if let Some(new) = id {
            self.set_activated(new, true);
            self.focus_stack.focus(target);
            if let Some(space) = self.output_mut().model.current_space_state_mut() {
                space.note_focus(new);
            }
        }

        let surface = id.and_then(|id| self.window(id).map(|w| w.surface.wl_surface().clone()));
        self.keyboard.clone().set_focus(self, surface, 0.into());
    }

    /// Focuses a keyboard-interactive layer surface.
    ///
    /// Layer surfaces have no `xdg_toplevel` activation state, so this only
    /// updates the focus stack and the seat's keyboard focus.
    pub(crate) fn focus_layer(&mut self, id: LayerId) -> bool {
        let Some((surface, output)) = self.layer(id).and_then(|entry| {
            entry
                .surface
                .can_receive_keyboard_focus()
                .then(|| (entry.surface.wl_surface().clone(), entry.output))
        }) else {
            return false;
        };

        if self.focus == FocusTarget::LayerSurface(id) {
            self.primary_output = output;
            return true;
        }
        self.primary_output = output;
        if let Some(old) = self.focus.window() {
            self.set_activated(old, false);
        }
        self.focus = FocusTarget::LayerSurface(id);
        self.focus_stack.focus(self.focus);
        self.keyboard
            .clone()
            .set_focus(self, Some(surface), 0.into());
        true
    }

    /// Sets the xdg `activated` state for a window.
    fn set_activated(&mut self, id: WindowId, activated: bool) {
        let Some(window) = self.window_mut(id) else {
            return;
        };
        window.surface.with_pending_state(|state| {
            if activated {
                state.states.set(xdg_toplevel::State::Activated);
            } else {
                state.states.unset(xdg_toplevel::State::Activated);
            }
        });
        window.surface.send_configure();
    }

    /// The top-most visible window covering `position`.
    pub(crate) fn window_at(&self, position: Point<f64, Logical>) -> Option<WindowId> {
        let point = crab::core::Point::new(position.x, position.y);
        let output = self.output_at(position)?;
        // Test in the exact reverse of the painting order. Sharing
        // [`Self::stacked_windows_on`] with the renderer is what guarantees a
        // click lands on the window the user can see, including the floating
        // focus stack and the always-on-top group.
        self.stacked_windows_on(output)
            .into_iter()
            .rev()
            .find(|id| {
                self.window(*id)
                    .is_some_and(|window| window.visual_rect(&self.frame_clock).contains(point))
            })
    }

    /// Returns the top-most mapped surface covering `position`, with the
    /// logical coordinates of its top-left corner: the origin to subtract
    /// `position` from to get the surface-local point under it.
    ///
    /// For a toplevel that is the fit's origin rather than the surface's
    /// placement. While the window is stretched, the surface-local point drawn
    /// at `position` is not the one the placement puts there, and reporting
    /// the placement would hand the client a position where it drew nothing
    /// (TODO.md §3.5). Popups are never stretched — they are drawn where they
    /// are placed — so theirs is their placement straight.
    pub(crate) fn surface_under(
        &self,
        position: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        let output = self.output_at(position)?;
        // Top and overlay layers sit above ordinary windows and their popups.
        if let Some(hit) = self.layer_surface_under(position, &[Layer::Overlay, Layer::Top], output)
        {
            return Some(hit);
        }
        // A popup floats above every toplevel, so its tree gets first refusal:
        // this is what keeps the pointer on an open menu (and lets its grab see
        // that the pointer is still on the right client). Test them in reverse
        // painting order, like the toplevels below.
        for id in self.stacked_windows_on(output).into_iter().rev() {
            let Some(window) = self.window(id) else {
                continue;
            };
            let origin = window.visual_origin(&self.frame_clock);
            for (popup, offset) in popup_offsets(window) {
                let origin = origin + offset;
                if let Some((surface, location)) = under_from_surface_tree(
                    popup.wl_surface(),
                    position - origin.to_f64(),
                    (0, 0),
                    WindowSurfaceType::ALL,
                ) {
                    return Some((surface, (origin + location).to_f64()));
                }
            }
        }
        if let Some(id) = self.window_at(position) {
            let window = self.window(id)?;
            return Some((
                window.wl_surface().clone(),
                window.fit(&self.frame_clock).input_origin(position),
            ));
        }
        // Bottom layers (panels that do take input) sit below windows.
        self.layer_surface_under(position, &[Layer::Bottom], output)
    }

    /// Focuses the window under `position` (click-to-focus).
    pub(crate) fn focus_at(&mut self, position: Point<f64, Logical>) {
        if let Some(id) = self.exclusive_layer() {
            self.focus_layer(id);
            return;
        }
        if let Some(id) = self.layer_at(position) {
            // A bar or wallpaper that asked for no keyboard still owns the
            // click: focus must not fall through to a window behind it.
            self.focus_layer(id);
            return;
        }
        if let Some(id) = self.window_at(position) {
            self.focus_window(Some(id));
        } else if self.surface_under(position).is_none() {
            // Clicking empty space clears focus; clicking a popup does not.
            self.focus_window(None);
        }
    }

    /// Focuses the next/previous visible window in list order.
    pub(crate) fn focus_cycle(&mut self, forward: bool) {
        let view = self.output().model.view.visible;
        let ids: Vec<WindowId> = self
            .windows
            .iter()
            .filter(|window| {
                window.window.output == self.primary_output
                    && window.mapped
                    && window.window.is_on(view)
                    && window.is_focusable()
            })
            .map(ManagedWindow::id)
            .collect();
        if ids.is_empty() {
            self.focus_window(None);
            return;
        }
        let index = self
            .focused_id()
            .and_then(|id| ids.iter().position(|&x| x == id));
        let next = match index {
            Some(i) if forward => ids[(i + 1) % ids.len()],
            Some(i) => ids[(i + ids.len() - 1) % ids.len()],
            None => ids[0],
        };
        self.focus_window(Some(next));
        self.relayout();
    }

    /// Directional focus within the current layout (`MOD+arrow`).
    pub(crate) fn focus_direction(&mut self, direction: Direction) {
        let Some(current) = self.focused_id() else {
            return;
        };
        if let Some(next) = layout::directional_target(self, current, direction) {
            if self.window(next).is_some_and(ManagedWindow::is_focusable) {
                self.focus_window(Some(next));
                self.relayout();
            }
        }
    }

    /// Views a single space by 1-based index.
    ///
    /// Spaces are dynamic: an index past the last space lands on the empty space
    /// after it, so `MOD+5` on a two-space desktop shows space 3 rather than a
    /// gap of empty spaces.
    pub(crate) fn view_space(&mut self, index: u8) {
        let Some(mask) = self.dynamic_space(index) else {
            return;
        };
        self.output_mut().model.view_space(mask);
        self.compact_spaces();
        self.refocus_after_view_change();
        self.relayout();
    }

    /// Toggles a space in the view (`MOD+ctrl+N`).
    ///
    /// The index is dynamic like [`view_space`](Self::view_space), so toggling
    /// the same bind twice adds and then removes the same space.
    pub(crate) fn toggle_view_space(&mut self, index: u8) {
        let Some(mask) = self.dynamic_space(index) else {
            return;
        };
        self.output_mut().model.toggle_view(mask);
        self.compact_spaces();
        self.refocus_after_view_change();
        self.relayout();
    }

    /// Steps the view one dynamic space left/normal or right/next.
    ///
    /// The step stops at space 1 and at the empty space past the last occupied
    /// one, so it can always reach a fresh space but never a hole beyond it.
    pub(crate) fn view_space_step(&mut self, step: i8) {
        let Some(current) = self.output().model.current_space_index() else {
            return;
        };
        let occupied = self.occupied_spaces().iter_indices().count();
        let last = u8::try_from(occupied)
            .unwrap_or(SpaceSet::COUNT)
            .saturating_add(1)
            .min(SpaceSet::COUNT);
        let index = u8::try_from(current).unwrap_or(0).saturating_add(1);
        let target = if step < 0 {
            index.saturating_sub(step.unsigned_abs()).max(1)
        } else {
            index.saturating_add(step.unsigned_abs()).min(last)
        };
        if target != index {
            self.view_space(target);
        }
    }

    /// Steps the view to the next occupied space in a direction.
    ///
    /// This powers `viewtoleft_have_client`/`viewtoright_have_client` (and the
    /// `viewprev`/`viewnext` aliases): empty spaces in between are skipped and
    /// the view stops at the outermost occupied space.
    pub(crate) fn view_space_have_client(&mut self, step: i8) {
        let Some(current) = self.output().model.current_space_index() else {
            return;
        };
        let index = u8::try_from(current).unwrap_or(0).saturating_add(1);
        if let Some(target) = occupied_target(self.occupied_spaces(), index, step) {
            self.view_space(target);
        }
    }

    /// Swaps to the previously-viewed space set.
    pub(crate) fn toggle_back(&mut self) {
        self.output_mut().model.toggle_back();
        self.compact_spaces();
        self.refocus_after_view_change();
        self.relayout();
    }

    /// Moves the focused window to a space by 1-based index and shows it there.
    ///
    /// The index is dynamic like [`view_space`](Self::view_space): moving past
    /// the last space sends the window to the empty space after it, and because
    /// the space it left may become empty, the spaces after that one move down.
    pub(crate) fn move_to_space(&mut self, index: u8) {
        let (Some(mask), Some(id)) = (self.dynamic_space(index), self.focused_id()) else {
            return;
        };
        if let Some(window) = self.window_mut(id) {
            window.window.move_to_space(mask);
        }
        self.output_mut().model.view_space(mask);
        self.compact_spaces();
        self.refocus_after_view_change();
        self.relayout();
    }

    /// Toggles a space on the focused window (`MOD+shift+ctrl+N`).
    pub(crate) fn toggle_space(&mut self, index: u8) {
        let (Some(mask), Some(id)) = (self.dynamic_space(index), self.focused_id()) else {
            return;
        };
        if let Some(window) = self.window_mut(id) {
            window.window.toggle_space(mask);
        }
        self.compact_spaces();
        self.refocus_after_view_change();
        self.relayout();
    }

    /// Re-focuses something sensible after the visible set changed.
    ///
    /// Prefers the current keyboard-interactive layer, the space's own focus
    /// history, then any mapped window on the view; clears focus if the new
    /// view is empty.
    fn refocus_after_view_change(&mut self) {
        if let FocusTarget::LayerSurface(id) = self.focus {
            if self
                .layer(id)
                .is_some_and(|entry| entry.surface.can_receive_keyboard_focus())
            {
                return;
            }
        }
        let view = self.output().model.view.visible;
        let on_screen = |window: &ManagedWindow| {
            window.window.output == self.primary_output
                && window.mapped
                && window.window.is_on(view)
                && window.is_focusable()
        };
        if self
            .focused_id()
            .and_then(|id| self.window(id))
            .is_some_and(on_screen)
        {
            return;
        }
        let next = self
            .output()
            .model
            .current_space_state()
            .and_then(|space| {
                space
                    .focus_history
                    .iter()
                    .copied()
                    .find(|id| self.window(*id).is_some_and(on_screen))
            })
            .or_else(|| {
                self.windows
                    .iter()
                    .find(|w| on_screen(w))
                    .map(ManagedWindow::id)
            });
        self.focus_window(next);
    }

    /// Cycles the current space to the next mode within its layout family.
    pub(crate) fn cycle_layout_mode(&mut self) {
        if let Some(space) = self.output_mut().model.current_space_state_mut() {
            space.layout = space.layout.next_mode();
        }
        self.relayout();
    }

    /// Cycles the current space to the next layout family's default mode.
    pub(crate) fn cycle_layout_family(&mut self) {
        if let Some(space) = self.output_mut().model.current_space_state_mut() {
            space.layout = space.layout.next_family();
        }
        self.relayout();
    }

    /// Sets the current space's layout by mode name.
    ///
    /// A family name is accepted as an alias for that family's default mode
    /// (`LayoutKind::from_name`), which keeps `set-layout` and the `layouts`
    /// reply aligned with the config.
    ///
    /// # Errors
    /// Returns a reason when the name is unknown or no space is visible.
    pub(crate) fn set_layout(&mut self, name: &str) -> Result<(), String> {
        let layout =
            LayoutKind::from_name(name).ok_or_else(|| format!("unknown layout `{name}`"))?;
        {
            let Some(space) = self.output_mut().model.current_space_state_mut() else {
                return Err("no space is visible on the focused output".to_owned());
            };
            space.layout = layout;
        }
        self.relayout();
        Ok(())
    }

    /// Applies a layout command (master ratio/count, column width, centring)
    /// to the current space.
    ///
    /// The command goes through the layout rather than straight to the
    /// parameters, so a family that owns its state — the scroller and its
    /// columns — gets to handle it (`crab::layout::apply_command`).
    pub(crate) fn layout_command(&mut self, command: LayoutCommand) {
        if let Some(space) = self.output_mut().model.current_space_state_mut() {
            crab::layout::apply_command(
                space.layout,
                command,
                &mut space.params,
                &mut space.scroll,
            );
        }
        self.relayout();
    }

    /// Flips the focused window between tiled and floating.
    pub(crate) fn toggle_floating(&mut self) {
        let Some(id) = self.focused_id() else {
            return;
        };
        let Some(rect) = self
            .window(id)
            .map(|window| window.visual_rect(&self.frame_clock))
        else {
            return;
        };
        let now_floating = self
            .window_mut(id)
            .is_some_and(|window| window.window.toggle_floating(rect));
        if !now_floating {
            // Re-enter the layout right after the previously focused tile.
            if let Some(index) = self
                .windows
                .iter()
                .position(|window| window.window.id == id)
            {
                let mut window = self.windows.remove(index);
                let insert = self.tiled_insert_index(id).min(self.windows.len());
                window.window.floating = false;
                self.windows.insert(insert, window);
            }
        }
        self.relayout();
    }

    /// Moves the focused window toward `direction` in the layout order.
    ///
    /// The neighbor is the one directional focus would reach, so `MOVE` follows
    /// the same geometry focus does; swapping their places reorders the tile or
    /// scroll column order without changing which space the window is on. On a
    /// free space the geometry does not come from the order, so this is a no-op.
    pub(crate) fn move_window(&mut self, direction: Direction) {
        let Some(id) = self.focused_id() else {
            return;
        };
        let Some(neighbor) = layout::directional_target(self, id, direction) else {
            return;
        };
        let (Some(from), Some(to)) = (
            self.windows
                .iter()
                .position(|window| window.window.id == id),
            self.windows
                .iter()
                .position(|window| window.window.id == neighbor),
        ) else {
            return;
        };
        self.windows.swap(from, to);
        self.relayout();
    }

    /// Where an un-floated window should re-enter the tiling order.
    fn tiled_insert_index(&self, id: WindowId) -> usize {
        let Some(output) = self.window(id).map(|window| window.window.output) else {
            return self.windows.len();
        };
        for target in self.focus_stack.iter() {
            if let Some(other) = target.window() {
                if other != id {
                    if let Some(position) = self.windows.iter().position(|w| {
                        w.window.id == other && w.window.output == output && w.window.is_tiled()
                    }) {
                        return position + 1;
                    }
                }
            }
        }
        self.windows.len()
    }

    /// Flips fullscreen for the focused window.
    pub(crate) fn toggle_fullscreen(&mut self) {
        let Some(id) = self.focused_id() else {
            return;
        };
        let to = self.window(id).is_some_and(|w| !w.window.fullscreen);
        self.set_fullscreen(id, to);
        self.relayout();
    }

    /// Sets the fullscreen state of a window (also used by the client request).
    pub(crate) fn set_fullscreen(&mut self, id: WindowId, fullscreen: bool) {
        let Some(window) = self.window_mut(id) else {
            return;
        };
        if window.window.fullscreen == fullscreen {
            return;
        }
        window.window.fullscreen = fullscreen;
        window.surface.with_pending_state(|state| {
            if fullscreen {
                state.states.set(xdg_toplevel::State::Fullscreen);
            } else {
                state.states.unset(xdg_toplevel::State::Fullscreen);
            }
        });
        window.surface.send_configure();
    }

    /// Sets the maximized state of a window.
    pub(crate) fn set_maximized(&mut self, id: WindowId, maximized: bool) {
        let Some(window) = self.window_mut(id) else {
            return;
        };
        if window.window.maximized == maximized {
            return;
        }
        window.window.maximized = maximized;
        window.surface.with_pending_state(|state| {
            if maximized {
                state.states.set(xdg_toplevel::State::Maximized);
            } else {
                state.states.unset(xdg_toplevel::State::Maximized);
            }
        });
        window.surface.send_configure();
    }

    /// Flips maximized for the focused window.
    pub(crate) fn toggle_maximized(&mut self) {
        let Some(id) = self.focused_id() else {
            return;
        };
        let to = self.window(id).is_some_and(|w| !w.window.maximized);
        self.set_maximized(id, to);
        self.relayout();
    }

    /// Asks the focused window or layer surface to close.
    pub(crate) fn close_focused(&mut self) {
        match self.focus {
            FocusTarget::Window(id) => {
                if let Some(window) = self.window(id) {
                    window.surface.send_close();
                }
            }
            FocusTarget::LayerSurface(id) => {
                if let Some(layer) = self.layer(id) {
                    layer.surface.layer_surface().send_close();
                }
            }
            _ => {}
        }
    }

    /// Starts an interactive move/resize of the window under the pointer.
    ///
    /// Returns whether the grab was taken (so the caller can swallow the button
    /// that triggered it). A tiled window is floated first, which is what makes
    /// `MOD+drag` work on tiled windows.
    pub(crate) fn begin_interaction(
        &mut self,
        mode: GrabMode,
        position: Point<f64, Logical>,
        button: u32,
    ) -> bool {
        let Some(id) = self.window_at(position) else {
            return false;
        };
        self.begin_interaction_for(id, mode, position, Some(button), false)
    }

    /// Starts a move/resize because `id`'s client asked for one.
    ///
    /// xdg-shell lets the compositor ignore interactive requests, and a stale
    /// serial is exactly what a request from a window the user did not just
    /// press looks like. Require the last press we sent and the same window
    /// still under the pointer. Fullscreen and maximized windows keep their
    /// state: moving them is what leaving those states is for.
    pub(crate) fn begin_client_interaction(
        &mut self,
        id: WindowId,
        mode: GrabMode,
        serial: Serial,
    ) -> bool {
        if self.last_pointer_press != Some(serial) {
            tracing::debug!(
                ?serial,
                "ignored an interactive request with a stale serial"
            );
            return false;
        }
        let Some(window) = self.window(id) else {
            return false;
        };
        if window.window.fullscreen || window.window.maximized {
            tracing::debug!("ignored an interactive request on a fullscreen or maximized window");
            return false;
        }
        let position = self.pointer_location;
        if self.window_at(position) != Some(id) {
            tracing::debug!("ignored an interactive request on a window not under the pointer");
            return false;
        }
        self.begin_interaction_for(id, mode, position, None, true)
    }

    /// Puts `id` into an interactive grab, floating it first if it was tiled.
    fn begin_interaction_for(
        &mut self,
        id: WindowId,
        mode: GrabMode,
        position: Point<f64, Logical>,
        button: Option<u32>,
        from_client: bool,
    ) -> bool {
        let Some(rect) = self
            .window(id)
            .map(|window| window.visual_rect(&self.frame_clock))
        else {
            return false;
        };
        if let Some(window) = self.window_mut(id) {
            if window.window.is_tiled() {
                window.float_at(rect);
            }
        }
        self.focus_window(Some(id));
        self.interaction = Some(Interaction {
            id,
            mode,
            grab: position,
            origin: rect,
            button,
            from_client,
        });
        self.relayout();
        true
    }

    /// Updates an in-progress interaction for a new pointer position.
    ///
    /// Returns whether a grab is active (so motion is not forwarded).
    pub(crate) fn update_interaction(&mut self, position: Point<f64, Logical>) -> bool {
        let Some(interaction) = self.interaction else {
            return false;
        };
        let dx = position.x - interaction.grab.x;
        let dy = position.y - interaction.grab.y;
        let mode = interaction.mode;
        let rect = self.clamp_rect(mode.rect_after(interaction.origin, dx, dy));
        // A move feels magnetic at the usable-area edges; a resize keeps the
        // exact edge the pointer requested so the size does not jump.
        let rect = match mode {
            GrabMode::Move => {
                snap_move_to_edges(rect, self.output().model.usable_area, SNAP_DISTANCE)
            }
            GrabMode::Resize { .. } => rect,
        };
        let resize = matches!(mode, GrabMode::Resize { .. });

        if let Some(window) = self.window_mut(interaction.id) {
            window.window.float_geometry = Some(rect);
            window.location = point_i32(rect.x, rect.y);
            // The window tracks the pointer 1:1, so the drawn rectangle is
            // pinned rather than animated toward the new geometry.
            window.snap_visual_to(rect);
            if resize {
                let size = size_i32(rect.w, rect.h);
                if window.size != size {
                    window.size = size;
                    // The client may not be able to follow — its size hints
                    // may not reach this far — which `configure_to` clamps
                    // to; the window is drawn at the dragged size either way.
                    window.configure_to(size);
                }
            }
        }
        self.relayout();
        true
    }

    /// Ends the interaction if `button`'s release should end it.
    ///
    /// A compositor grab belongs to the button that started it, so releasing a
    /// different one leaves the drag running and reaches the client, which saw
    /// that press. A client-requested grab ends on any release.
    pub(crate) fn end_interaction(&mut self, button: u32) -> Option<Interaction> {
        let interaction = self.interaction?;
        if interaction.button.is_some_and(|grabbed| grabbed != button) {
            return None;
        }
        self.interaction = None;
        self.relayout();
        Some(interaction)
    }

    /// Keeps a dragged/resized window partly on screen.
    fn clamp_rect(&self, rect: Rect) -> Rect {
        let area = self.output().model.usable_area;
        let x = rect
            .x
            .clamp(area.x, (area.right() - MIN_VISIBLE).max(area.x));
        let y = rect
            .y
            .clamp(area.y, (area.bottom() - MIN_VISIBLE).max(area.y));
        Rect::new(x, y, rect.w, rect.h)
    }

    /// Clamps `position` onto the output layout.
    ///
    /// Unlike [`Self::output_at`], this never reports a point in dead space:
    /// the pointer is pulled to the nearest monitor edge instead.
    pub(crate) fn clamp_to_output(&self, position: Point<f64, Logical>) -> Point<f64, Logical> {
        clamp_to_outputs(&output_rects(&self.outputs), position)
    }
}

/// Rounds a logical x/y pair to a compositor-space point.
pub(crate) fn point_i32(x: f64, y: f64) -> Point<i32, Logical> {
    let point: Point<i32, Logical> = (x.round() as i32, y.round() as i32).into();
    point
}

/// Rounds a logical size to compositor-space, keeping at least one pixel.
pub(crate) fn size_i32(w: f64, h: f64) -> Size<i32, Logical> {
    let size: Size<i32, Logical> = ((w.round() as i32).max(1), (h.round() as i32).max(1)).into();
    size
}

/// Clamps a size into the hints a client set for itself.
///
/// A hint of zero on an axis means the client set no limit on it, which is how
/// `xdg_toplevel.set_min_size`/`set_max_size` spell "unbounded". Asking a
/// client for a size outside its hints gets a refusal, so the layout sizes it
/// to what it accepts instead.
pub(crate) fn clamp_to_hints(
    size: Size<i32, Logical>,
    min: Option<CoreSize>,
    max: Option<CoreSize>,
) -> Size<i32, Logical> {
    let (mut w, mut h) = (f64::from(size.w), f64::from(size.h));
    if let Some(min) = min {
        if min.w > 0.0 {
            w = w.max(min.w);
        }
        if min.h > 0.0 {
            h = h.max(min.h);
        }
    }
    if let Some(max) = max {
        if max.w > 0.0 {
            w = w.min(max.w);
        }
        if max.h > 0.0 {
            h = h.min(max.h);
        }
    }
    size_i32(w, h)
}

/// Every popup hanging off `window`, with the popup's *surface* origin relative
/// to the top-left of the window's window geometry.
///
/// Popups are placed against their parent's window geometry, so the manager's
/// location needs only the popup's own geometry subtracted to get back out to
/// its surface origin — the popup's buffer carries its own decoration margins,
/// exactly like a toplevel's does.
pub(crate) fn popup_offsets(
    window: &ManagedWindow,
) -> impl Iterator<Item = (PopupKind, Point<i32, Logical>)> {
    PopupManager::popups_for_surface(window.wl_surface()).map(|(popup, location)| {
        let offset = location - popup.geometry().loc;
        (popup, offset)
    })
}

/// The client's window geometry, clamped to the surface tree's bounding box.
///
/// `xdg_surface.set_window_geometry` is what the client considers its window:
/// for client-side decorations it excludes the shadow margins drawn around the
/// content. The bounding box is the fallback for clients that never set one.
fn window_geometry(surface: &WlSurface, bbox: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
    with_states(surface, |states| {
        states
            .cached_state
            .get::<SurfaceCachedState>()
            .current()
            .geometry
    })
    .and_then(|geometry| geometry.intersection(bbox))
    .unwrap_or(bbox)
}

/// The `xdg_toplevel` title and app id, once the client has set them.
fn read_toplevel_info(surface: &ToplevelSurface) -> (Option<String>, Option<String>) {
    with_states(surface.wl_surface(), |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .map(|data| {
                let attributes = data.lock().unwrap();
                (attributes.title.clone(), attributes.app_id.clone())
            })
            .unwrap_or((None, None))
    })
}

/// The result of scanning the rule list for a window.
enum RuleSearch<'a> {
    /// An earlier rule needs a title, app id, or output the client has not
    /// reported yet.
    Pending,
    /// Every rule was decided and none matched.
    NoMatch,
    /// The first matching rule.
    Matched(&'a WindowRule),
}

/// Finds the first rule that matches, waiting for matchers that are still unknown.
///
/// Rules are ordered: a rule whose app id or title has not arrived blocks the
/// scan, because it might match and would precede anything after it. The window
/// protocol (`x11`) is known when the surface is created, so it never blocks.
fn find_window_rule<'a>(
    rules: &'a [WindowRule],
    app_id: Option<&str>,
    title: Option<&str>,
    output: Option<&str>,
    x11: bool,
    floating: bool,
    fullscreen: bool,
) -> RuleSearch<'a> {
    for rule in rules {
        if !rule.app_id.is_empty() && app_id.is_none() {
            return RuleSearch::Pending;
        }
        if !rule.title.is_empty() && title.is_none() {
            return RuleSearch::Pending;
        }
        if !rule.output.is_empty() && output.is_none() {
            return RuleSearch::Pending;
        }
        if rule
            .is_floating
            .is_some_and(|expected| expected != floating)
        {
            continue;
        }
        if rule
            .is_fullscreen
            .is_some_and(|expected| expected != fullscreen)
        {
            continue;
        }
        if let Some(expected) = rule.x11 {
            if x11 != expected {
                continue;
            }
        }
        if rule_matches(rule, app_id, title, output) {
            return RuleSearch::Matched(rule);
        }
    }
    RuleSearch::NoMatch
}

/// Whether a `[window_rule]`'s text matchers apply.
///
/// An empty rule `app_id` or `title` or `output` list is a wildcard. app ids match exactly;
/// title fragments match case-insensitively anywhere in the title. output matches by connector name exactly.
fn rule_matches(
    rule: &WindowRule,
    app_id: Option<&str>,
    title: Option<&str>,
    output: Option<&str>,
) -> bool {
    let app_matches = rule.app_id.is_empty()
        || app_id.is_some_and(|app_id| rule.app_id.iter().any(|expected| expected == app_id));
    let title_matches = rule.title.is_empty()
        || title.is_some_and(|title| {
            let title = title.to_lowercase();
            rule.title
                .iter()
                .any(|fragment| title.contains(&fragment.to_lowercase()))
        });
    let output_matches =
        rule.output.is_empty() || output.is_some_and(|out| rule.output.iter().any(|o| o == out));
    app_matches && title_matches && output_matches
}

/// Where a rule's floating window goes: centred in the usable area, then moved
/// by the rule's offsets (so `-90,-90` is 90 px up and left of centre).
fn rule_rect(rule: &WindowRule, size: Size<i32, Logical>, area: Rect) -> Rect {
    let x = area.x + (area.w - f64::from(size.w)) / 2.0 + f64::from(rule.offset_x.unwrap_or(0));
    let y = area.y + (area.h - f64::from(size.h)) / 2.0 + f64::from(rule.offset_y.unwrap_or(0));
    Rect::new(x.round(), y.round(), f64::from(size.w), f64::from(size.h))
}

/// The client's min/max size hints, as logical sizes (`None` = unconstrained).
fn read_size_hints(surface: &WlSurface) -> (Option<CoreSize>, Option<CoreSize>) {
    with_states(surface, |states| {
        let mut lookup = states.cached_state.get::<SurfaceCachedState>();
        let cached = lookup.current();
        let convert = |size: Size<i32, Logical>| {
            (size.w > 0 || size.h > 0)
                .then(|| CoreSize::new(f64::from(size.w.max(0)), f64::from(size.h.max(0))))
        };
        (convert(cached.min_size), convert(cached.max_size))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge;

    /// A size in logical pixels, the way the protocol spells it.
    fn size(w: i32, h: i32) -> Size<i32, Logical> {
        (w, h).into()
    }

    fn id(raw: u64) -> WindowId {
        WindowId::new(raw)
    }

    fn entry(raw: u64, layer: WindowLayer, visible: bool) -> StackEntry {
        StackEntry {
            id: id(raw),
            layer,
            visible,
        }
    }

    fn output(id: u64, x: f64, y: f64, w: f64, h: f64) -> (OutputId, Rect) {
        (OutputId::new(id), rect(x, y, w, h))
    }

    #[test]
    fn a_configured_position_moves_an_output_and_keeps_its_size() {
        let mut config = Config::default();
        config.outputs.insert(
            "DP-2".to_owned(),
            crab::config::Output {
                position: Some((1920, 120)),
                ..crab::config::Output::default()
            },
        );
        let mut model = OutputModel::new(
            OutputId::new(1),
            "DP-2",
            Rect::new(0.0, 0.0, 2560.0, 1440.0),
            1.0,
            SpaceState::default(),
        );
        apply_configured_position(&config, &mut model);
        assert_eq!(model.geometry, Rect::new(1920.0, 120.0, 2560.0, 1440.0));
        assert_eq!(model.usable_area, model.geometry);
    }

    #[test]
    fn the_output_under_a_point_wins_over_the_nearest_one() {
        let outputs = [
            output(1, 0.0, 0.0, 1920.0, 1080.0),
            output(2, 1920.0, 0.0, 2560.0, 1440.0),
        ];
        assert_eq!(
            output_at_position(&outputs, (100.0, 100.0).into()),
            Some(OutputId::new(1))
        );
        assert_eq!(
            output_at_position(&outputs, (2000.0, 500.0).into()),
            Some(OutputId::new(2))
        );
        // The shared edge belongs to the output that starts there, not both.
        assert_eq!(
            output_at_position(&outputs, (1920.0, 500.0).into()),
            Some(OutputId::new(2))
        );
    }

    #[test]
    fn a_point_in_a_gap_falls_back_to_the_nearest_output() {
        // A vertical arrangement with a gap between the monitors.
        let outputs = [
            output(1, 0.0, 0.0, 100.0, 100.0),
            output(2, 0.0, 200.0, 100.0, 100.0),
        ];
        assert_eq!(
            output_at_position(&outputs, (50.0, 150.0).into()),
            Some(OutputId::new(1)),
            "the point is closer to the bottom edge of the upper output"
        );
        assert_eq!(
            clamp_to_outputs(&outputs, (50.0, 150.0).into()),
            (50.0, 99.0).into(),
            "the pointer is pulled onto the nearest monitor instead of staying in the gap"
        );
    }

    #[test]
    fn a_point_on_an_output_is_left_where_it_is() {
        let outputs = [output(1, 100.0, 200.0, 300.0, 400.0)];
        assert_eq!(
            clamp_to_outputs(&outputs, (250.0, 350.0).into()),
            (250.0, 350.0).into()
        );
        assert_eq!(
            clamp_to_outputs(&outputs, (0.0, 0.0).into()),
            (100.0, 200.0).into(),
            "a point outside the layout is clamped to the output's top-left corner"
        );
    }

    #[test]
    fn floating_windows_stack_by_focus_and_always_on_top_stays_above() {
        let entries = [
            entry(1, WindowLayer::Tiled, true),
            entry(2, WindowLayer::Floating, true),
            entry(3, WindowLayer::Floating, true),
            entry(4, WindowLayer::FloatingOnTop, true),
            entry(5, WindowLayer::Covering, true),
        ];
        let mut focus = FocusStack::new();
        focus.focus(FocusTarget::Window(id(2)));

        assert_eq!(
            stacked_order(&entries, &focus),
            vec![id(1), id(3), id(2), id(4), id(5)],
            "tiled below floating, the focused peer above the unfocused one, \
             and always-on-top above both"
        );

        // Focusing the other ordinary floating window raises it in turn; the
        // always-on-top window stays above it because its layer is higher.
        focus.focus(FocusTarget::Window(id(3)));
        assert_eq!(
            stacked_order(&entries, &focus),
            vec![id(1), id(2), id(3), id(4), id(5)]
        );
    }

    #[test]
    fn hidden_and_closed_windows_never_enter_the_stack() {
        let entries = [
            entry(1, WindowLayer::Floating, false),
            entry(2, WindowLayer::Floating, true),
            entry(3, WindowLayer::Tiled, true),
        ];
        let mut focus = FocusStack::new();
        // The closing window 1 was focused, and 9 is a stale stack entry.
        focus.focus(FocusTarget::Window(id(1)));
        focus.focus(FocusTarget::Window(id(9)));
        focus.focus(FocusTarget::Window(id(2)));

        assert_eq!(
            stacked_order(&entries, &focus),
            vec![id(3), id(2)],
            "a hidden window's focus entry must not draw or reserve a slot"
        );
    }

    #[test]
    fn a_closing_window_keeps_its_index_within_its_layer() {
        let order = [id(1), id(2), id(3), id(4), id(5)];
        let layer_of = |window: WindowId| match window.raw() {
            1 | 3 => Some(WindowLayer::Tiled),
            2 | 4 | 5 => Some(WindowLayer::Floating),
            _ => None,
        };

        // The floating group is [2, 4, 5], so window 4's index is 1 there,
        // not 3 in the full scene.
        assert_eq!(
            stack_index_in_layer(&order, id(4), WindowLayer::Floating, layer_of),
            1
        );
        // A window that is already gone falls back to the bottom of its group.
        assert_eq!(
            stack_index_in_layer(&order, id(9), WindowLayer::Floating, layer_of),
            0
        );
    }

    #[test]
    fn have_client_steps_skip_empty_spaces_and_stop_at_the_edges() {
        let occupied = SpaceSet::SPACE_1 | SpaceSet::SPACE_3 | SpaceSet::SPACE_4;
        assert_eq!(occupied_target(occupied, 1, 1), Some(3));
        assert_eq!(occupied_target(occupied, 2, 1), Some(3));
        assert_eq!(occupied_target(occupied, 4, 1), None);
        assert_eq!(occupied_target(occupied, 4, -1), Some(3));
        assert_eq!(occupied_target(occupied, 2, -1), Some(1));
        assert_eq!(occupied_target(occupied, 1, -1), None);
        assert_eq!(occupied_target(occupied, 3, 0), None);
    }

    #[test]
    fn opening_slides_up_into_the_rect() {
        let target = Rect::new(100.0, 100.0, 400.0, 300.0);
        let start = open_start_rect(target, AnimationStyle::Slide);
        assert_eq!(start.x, target.x);
        assert_eq!(start.y, target.y + OPEN_SLIDE);
        assert_eq!((start.w, start.h), (target.w, target.h));
    }

    #[test]
    fn opening_zoom_starts_smaller_around_the_centre() {
        let target = Rect::new(100.0, 100.0, 400.0, 300.0);
        let start = open_start_rect(target, AnimationStyle::Zoom);
        assert_eq!(start.w, target.w * OPEN_ZOOM);
        assert_eq!(start.h, target.h * OPEN_ZOOM);
        assert_eq!(start.x + start.w / 2.0, target.x + target.w / 2.0);
        assert_eq!(start.y + start.h / 2.0, target.y + target.h / 2.0);
    }

    #[test]
    fn a_client_that_complies_is_asked_once() {
        let mut configure = Configure::default();
        assert!(configure.ask(size(940, 1030)));
        // Asking again for the same rectangle — which every re-layout of an
        // unchanged window does — configures nothing.
        assert!(!configure.ask(size(940, 1030)));
        assert_eq!(configure.commit(size(940, 1030)), Reply::Complied);
        assert!(!configure.gave_up());
    }

    #[test]
    fn the_same_wrong_size_is_not_a_new_refusal() {
        let mut configure = Configure::default();
        assert!(configure.ask(size(940, 1030)));
        // A new size it was not asked for: worth asking once more.
        assert_eq!(configure.commit(size(469, 600)), Reply::TryAgain);
        // The same answer over and over is not new, so it buys no more
        // configures however often the client commits. This is the shape of a
        // terminal that rounds its window to whole character cells, which will
        // never be exactly what was asked for and must never loop on that.
        assert_eq!(configure.commit(size(469, 600)), Reply::SameAsBefore);
        assert_eq!(configure.commit(size(469, 600)), Reply::SameAsBefore);
        assert!(!configure.gave_up());
    }

    #[test]
    fn enough_different_sizes_end_the_asking() {
        let mut configure = Configure::default();
        assert!(configure.ask(size(940, 1030)));
        assert_eq!(configure.commit(size(469, 600)), Reply::TryAgain);
        assert_eq!(configure.commit(size(500, 620)), Reply::TryAgain);
        // The third distinct answer is one too many: stop configuring.
        assert_eq!(configure.commit(size(520, 640)), Reply::GaveUp);
        assert!(configure.gave_up());
        // And stay stopped, for this rectangle.
        assert_eq!(configure.commit(size(540, 660)), Reply::SameAsBefore);
        assert_eq!(configure.commit(size(469, 600)), Reply::SameAsBefore);
    }

    #[test]
    fn a_client_that_refused_a_size_gets_a_new_chance_at_another() {
        let mut configure = Configure::default();
        assert!(configure.ask(size(940, 1030)));
        assert_eq!(configure.commit(size(469, 600)), Reply::TryAgain);
        assert_eq!(configure.commit(size(500, 620)), Reply::TryAgain);
        assert_eq!(configure.commit(size(520, 640)), Reply::GaveUp);
        assert!(configure.gave_up());

        // A different rectangle is a different question, and the allowance of
        // refusals goes with it.
        assert!(configure.ask(size(600, 800)));
        assert!(!configure.gave_up());
        assert_eq!(configure.commit(size(469, 600)), Reply::TryAgain);
    }

    #[test]
    fn a_client_that_complies_starts_over() {
        let mut configure = Configure::default();
        assert!(configure.ask(size(940, 1030)));
        assert_eq!(configure.commit(size(469, 600)), Reply::TryAgain);
        assert_eq!(configure.commit(size(940, 1030)), Reply::Complied);

        // Having been right once does not buy the client forever: a later
        // answer of its own counts from the top again.
        assert_eq!(configure.commit(size(469, 600)), Reply::TryAgain);
        assert!(!configure.gave_up());
    }

    #[test]
    fn hints_clamp_a_request_to_what_the_client_can_be() {
        let max = Some(CoreSize::new(469.0, 0.0));
        assert_eq!(clamp_to_hints(size(940, 1030), None, max), size(469, 1030));

        let min = Some(CoreSize::new(300.0, 400.0));
        assert_eq!(clamp_to_hints(size(100, 100), min, None), size(300, 400));

        // A zero on an axis means the client set no limit on it.
        assert_eq!(
            clamp_to_hints(
                size(940, 1030),
                Some(CoreSize::new(0.0, 0.0)),
                Some(CoreSize::new(0.0, 0.0))
            ),
            size(940, 1030)
        );

        // Nothing is ever asked for a degenerate size.
        assert_eq!(clamp_to_hints(size(0, 0), min, None), size(300, 400));
    }

    /// A window rectangle to drag, in logical pixels.
    fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect::new(x, y, w, h)
    }

    #[test]
    fn a_move_translates_the_rectangle() {
        let origin = rect(100.0, 100.0, 400.0, 300.0);
        assert_eq!(
            GrabMode::Move.rect_after(origin, 25.0, -40.0),
            rect(125.0, 60.0, 400.0, 300.0)
        );
    }

    #[test]
    fn a_moved_window_snaps_flush_to_an_output_edge() {
        let area = rect(0.0, 0.0, 1920.0, 1080.0);
        let snap = |window| snap_move_to_edges(window, area, SNAP_DISTANCE);

        // Near edge, far edge, top, and bottom all snap flush without resizing.
        assert_eq!(
            snap(rect(10.0, 120.0, 400.0, 300.0)),
            rect(0.0, 120.0, 400.0, 300.0)
        );
        assert_eq!(
            snap(rect(1510.0, 120.0, 400.0, 300.0)),
            rect(1520.0, 120.0, 400.0, 300.0)
        );
        assert_eq!(
            snap(rect(100.0, 10.0, 400.0, 300.0)),
            rect(100.0, 0.0, 400.0, 300.0)
        );
        assert_eq!(
            snap(rect(100.0, 770.0, 400.0, 300.0)),
            rect(100.0, 780.0, 400.0, 300.0)
        );

        // Just outside the threshold is left alone, and the threshold itself
        // still snaps.
        assert_eq!(
            snap(rect(16.1, 120.0, 400.0, 300.0)),
            rect(16.1, 120.0, 400.0, 300.0)
        );
        assert_eq!(
            snap(rect(16.0, 120.0, 400.0, 300.0)),
            rect(0.0, 120.0, 400.0, 300.0)
        );
    }

    #[test]
    fn a_window_wider_than_the_area_snaps_its_nearest_edge() {
        let area = rect(0.0, 0.0, 100.0, 100.0);
        // Both edges are in range; the far edge is closer, so it wins and the
        // near edge moves out of the area rather than the size changing.
        assert_eq!(
            snap_move_to_edges(rect(-15.0, 0.0, 120.0, 50.0), area, SNAP_DISTANCE),
            rect(-20.0, 0.0, 120.0, 50.0)
        );
    }

    #[test]
    fn a_resize_pins_the_edges_that_are_not_dragged() {
        let origin = rect(100.0, 100.0, 400.0, 300.0);
        let resize = |edges| GrabMode::Resize { edges };

        // Bottom-right, the compositor's own drag: the origin stays put.
        assert_eq!(
            resize(ResizeEdge::BottomRight).rect_after(origin, 30.0, 20.0),
            rect(100.0, 100.0, 430.0, 320.0)
        );
        // Left: the right edge stays where it was, so the origin moves too.
        assert_eq!(
            resize(ResizeEdge::Left).rect_after(origin, 40.0, 99.0),
            rect(140.0, 100.0, 360.0, 300.0)
        );
        // Top-left: both far edges are pinned.
        assert_eq!(
            resize(ResizeEdge::TopLeft).rect_after(origin, -10.0, -10.0),
            rect(90.0, 90.0, 410.0, 310.0)
        );
        // Bottom: only the height moves.
        assert_eq!(
            resize(ResizeEdge::Bottom).rect_after(origin, 30.0, 20.0),
            rect(100.0, 100.0, 400.0, 320.0)
        );
    }

    #[test]
    fn a_dragged_edge_cannot_cross_the_fixed_one() {
        let origin = rect(100.0, 100.0, 400.0, 300.0);
        let resize = |edges| GrabMode::Resize { edges };

        // Pulling the left edge right past the right edge stops just short.
        assert_eq!(
            resize(ResizeEdge::Left).rect_after(origin, 500.0, 0.0),
            rect(499.0, 100.0, 1.0, 300.0)
        );
        // The same for the top edge, leaving the width alone.
        assert_eq!(
            resize(ResizeEdge::Top).rect_after(origin, 0.0, 400.0),
            rect(100.0, 399.0, 400.0, 1.0)
        );
        // Dragging the right edge far left stops at the left edge, which is
        // the one that stays put.
        assert_eq!(
            resize(ResizeEdge::Right).rect_after(origin, -500.0, 0.0),
            rect(100.0, 100.0, 1.0, 300.0)
        );
        assert_eq!(
            resize(ResizeEdge::BottomRight).rect_after(origin, -500.0, -500.0),
            rect(100.0, 100.0, 1.0, 1.0)
        );
    }

    #[test]
    fn window_rules_match_by_app_id() {
        let mut rule = WindowRule {
            app_id: vec!["brave".to_owned(), "obsidian".to_owned()],
            ..WindowRule::default()
        };
        assert!(rule_matches(&rule, Some("brave"), None, None));
        assert!(rule_matches(&rule, Some("obsidian"), None, None));
        assert!(!rule_matches(&rule, Some("Nautilus"), None, None));
        // An empty list is a catch-all rule.
        rule.app_id.clear();
        assert!(rule_matches(&rule, Some("anything"), None, None));
    }

    #[test]
    fn window_rules_match_title_fragments_case_insensitively() {
        let rule = WindowRule {
            title: vec!["picture".to_owned(), "settings".to_owned()],
            ..WindowRule::default()
        };
        assert!(rule_matches(&rule, None, Some("Save Picture As…"), None));
        assert!(rule_matches(&rule, None, Some("SETTINGS"), None));
        assert!(!rule_matches(&rule, None, Some("Terminal"), None));
        // Both matchers must agree.
        let both = WindowRule {
            app_id: vec!["foot".to_owned()],
            title: vec!["vim".to_owned()],
            ..WindowRule::default()
        };
        assert!(rule_matches(&both, Some("foot"), Some("nvim"), None));
        assert!(!rule_matches(&both, Some("foot"), Some("shell"), None));
    }

    #[test]
    fn window_rules_wait_for_unknown_matchers() {
        let rules = [
            WindowRule {
                title: vec!["save".to_owned()],
                no_focus: true,
                ..WindowRule::default()
            },
            WindowRule {
                open_floating: true,
                ..WindowRule::default()
            },
        ];
        // The first rule could still match, so the catch-all must not jump the
        // queue before the title arrives.
        assert!(matches!(
            find_window_rule(&rules, Some("app"), None, None, false, false, false),
            RuleSearch::Pending
        ));
        match find_window_rule(
            &rules,
            Some("app"),
            Some("Save As"),
            None,
            false,
            false,
            false,
        ) {
            RuleSearch::Matched(rule) => assert!(rule.no_focus),
            _ => panic!("the title rule should match"),
        }
        assert!(matches!(
            find_window_rule(
                &rules,
                Some("app"),
                Some("Terminal"),
                None,
                false,
                false,
                false,
            ),
            RuleSearch::Matched(rule) if rule.open_floating
        ));
    }

    #[test]
    fn an_unmatchable_x11_rule_does_not_block_later_rules() {
        let rules = [
            WindowRule {
                x11: Some(true),
                open_floating: true,
                ..WindowRule::default()
            },
            WindowRule {
                no_focus: true,
                ..WindowRule::default()
            },
        ];
        // Wayland windows cannot satisfy `x11 = true`, so that rule is skipped
        // and the later catch-all still applies.
        match find_window_rule(
            &rules,
            Some("foot"),
            Some("Terminal"),
            None,
            false,
            false,
            false,
        ) {
            RuleSearch::Matched(rule) => {
                assert!(rule.no_focus);
                assert!(!rule.open_floating);
            }
            _ => panic!("the catch-all after the x11 rule should match"),
        }
        // On an X11 surface the first rule still wins.
        assert!(matches!(
            find_window_rule(
                &rules,
                Some("foot"),
                Some("Terminal"),
                None,
                true,
                false,
                false,
            ),
            RuleSearch::Matched(rule) if rule.open_floating
        ));
    }

    #[test]
    fn window_rules_can_match_the_current_state() {
        let rules = [WindowRule {
            is_floating: Some(true),
            open_floating: true,
            ..WindowRule::default()
        }];
        assert!(matches!(
            find_window_rule(&rules, None, None, None, false, true, false),
            RuleSearch::Matched(_)
        ));
        assert!(matches!(
            find_window_rule(&rules, None, None, None, false, false, false),
            RuleSearch::NoMatch
        ));
    }

    #[test]
    fn a_rule_centres_and_offsets_its_window() {
        let rule = WindowRule {
            open_floating: true,
            size: Some((800, 500)),
            offset_x: Some(-90),
            offset_y: Some(-90),
            ..WindowRule::default()
        };
        // 1920x1080: (1920-800)/2 - 90 = 470, (1080-500)/2 - 90 = 200.
        assert_eq!(
            rule_rect(&rule, size(800, 500), rect(0.0, 0.0, 1920.0, 1080.0)),
            rect(470.0, 200.0, 800.0, 500.0)
        );
    }
}

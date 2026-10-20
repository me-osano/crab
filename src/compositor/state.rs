//! The single-owner compositor state — `calloop`'s `State` for crawm.
//!
//! [`Crawm`] owns every piece of Smithay state the compositor touches and is
//! owned by the main loop on one thread (ROADMAP.md §6 M1: *one
//! `calloop::EventLoop<State>` on the main thread; `State` is single-owner*).
//! Protocol handling is split per protocol under
//! [`crate::compositor::handlers`]; input lives in [`crate::compositor::input`]
//! and drawing in [`crate::compositor::render`].
//!
//! The pure model — windows, spaces, layouts, focus — lives in the `crawm`
//! library ([`crawm::core`], [`crawm::layout`]); this file is the glue that
//! keeps a Smithay `ToplevelSurface` next to its [`Window`] and applies the
//! layout. That glue is binary-only, so it may depend on Smithay freely; the
//! library's four modules stay pure (`scripts/check-boundaries.sh`).

use std::collections::HashSet;
use std::time::Instant;

use crawm::animation::{Animated, Clocks, Curve, Kind, SpringParams};
use crawm::config::Config;
use crawm::core::{
    Direction, FocusStack, FocusTarget, LayoutCommand, LayoutKind, LayoutParams,
    Output as OutputModel, OutputId, Rect, Size as CoreSize, SpaceSet, SpaceState, TilePosition,
    Window, WindowId,
};
use smithay::desktop::utils::under_from_surface_tree;
use smithay::desktop::{PopupKind, PopupManager, WindowSurfaceType};
use smithay::input::SeatState;
use smithay::input::keyboard::KeyboardHandle;
use smithay::input::pointer::{CursorImageStatus, PointerHandle};
use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Physical, Point, Rectangle, Serial, Size, Transform};
use smithay::wayland::compositor::{CompositorClientState, CompositorState, with_states};
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, SurfaceCachedState, ToplevelSurface, XdgShellState,
    XdgToplevelSurfaceData,
};
use smithay::wayland::shm::ShmState;

use crate::compositor::fit::WindowFit;
use crate::compositor::layout;

/// Size a new toplevel is configured to before the layout first sizes it, in
/// logical pixels. Floating windows pick their own size instead.
pub(crate) const DEFAULT_WINDOW_SIZE: (i32, i32) = (800, 600);

/// How much of a window must stay on screen when it is dragged, in pixels.
const MIN_VISIBLE: f64 = 32.0;

/// How many different sizes a client may commit instead of the one it was
/// configured to before we stop asking it to resize.
///
/// A client that is merely behind commits its old size once (which is already
/// booked, so it does not count again) and then the new one; a client that
/// needs a second configure answers the first retry. Three is room for both,
/// and a client that keeps answering with fresh sizes of its own still only
/// buys itself two retries.
const MAX_SIZE_REFUSALS: u8 = 3;

/// The curve a window's rectangle moves along when the layout moves it.
///
/// One curve for every window rectangle for now; M5 makes it per-event config
/// (`COMPOSITOR.md` §9). Springs are the right default because retargeting
/// mid-flight then keeps the velocity, so a second layout switch blends out of
/// the first instead of restarting.
const WINDOW_CURVE: Curve = Curve::Spring(SpringParams::DEFAULT);

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
    /// Whether [`Self::visual`] has had a real placement yet. The first one
    /// snaps, so a new window appears where the layout put it instead of flying
    /// in from its default rectangle.
    visual_placed: bool,
    /// What the client has been asked to be, and how it has answered.
    configure: Configure,
}

impl ManagedWindow {
    /// The window's model id.
    pub(crate) fn id(&self) -> WindowId {
        self.window.id
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
            self.visual.animate(rect, kind, clocks);
        } else {
            self.visual.snap_to(rect);
            self.visual_placed = true;
        }
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

pub(crate) struct Crawm {
    pub(crate) compositor_state: CompositorState,
    pub(crate) xdg_shell_state: XdgShellState,
    pub(crate) shm_state: ShmState,
    pub(crate) data_device_state: DataDeviceState,
    pub(crate) seat_state: SeatState<Self>,
    pub(crate) keyboard: KeyboardHandle<Self>,
    pub(crate) pointer: PointerHandle<Self>,
    pub(crate) output: Output,
    /// The loaded configuration.
    pub(crate) config: Config,
    /// The pure output/space model for the single nested output.
    pub(crate) output_state: OutputModel,
    /// Managed toplevels, in layout order (first = first tile).
    pub(crate) windows: Vec<ManagedWindow>,
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
    /// Keycodes swallowed by a keybind, so the matching release is swallowed
    /// too (the M2 hardcoded binds; config binds land in M5).
    pub(crate) bound_keys: HashSet<u32>,
    /// An in-progress interactive move/resize, if any.
    pub(crate) interaction: Option<Interaction>,
    /// Serial of the last pointer press sent to a client.
    ///
    /// `xdg_toplevel.move` and `resize` name the press that triggered them; a
    /// request whose serial is not this one is stale (or not ours) and is
    /// ignored.
    pub(crate) last_pointer_press: Option<Serial>,
    /// Whether Super is currently held (drives pointer grabs).
    pub(crate) super_held: bool,
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
    /// Logical size of the visible area; kept in sync by the backends.
    pub(crate) size: Size<i32, Logical>,
    /// Next window id to hand out (never reused, even after closes).
    next_window_id: u64,
}

impl Crawm {
    /// Builds every Wayland global on `display` and returns the initial state.
    ///
    /// The globals registered here (compositor, xdg-shell, shm, data-device,
    /// wl_seat, wl_output) are M1's core set; M3 adds the layer shell and the
    /// protocol breadth from `crawm-protocols`.
    pub(crate) fn new(display: &DisplayHandle, config: &Config) -> Self {
        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(display, "crawm");
        // Focus is pointless without a keyboard, and adding it can only fail if
        // keymap creation fails, which is not recoverable at startup.
        let keyboard = seat
            .add_keyboard(Default::default(), 200, 25)
            .expect("failed to add the default keyboard to the seat");
        let pointer = seat.add_pointer();

        // Physical size comes from the backend once it exists; M2 starts at
        // zero and lets `set_size` correct it on the first resize.
        let output = Output::new(
            "crawm-0".to_owned(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "crawm".to_owned(),
                model: "nested".to_owned(),
            },
        );
        output.create_global::<Self>(display);

        // Every space starts with the configured layout and tunables; a space then
        // remembers its own as the user cycles it (per-space layout memory).
        let params = LayoutParams {
            master_count: u32::try_from(config.master_count.max(0)).unwrap_or(0),
            master_ratio: config.master_ratio,
            gaps: f64::from(config.gaps),
            border_width: f64::from(config.border_width),
            position: TilePosition::Left,
            ..LayoutParams::default()
        };
        let space_state = SpaceState::new(
            LayoutKind::from_name(&config.layout).unwrap_or_default(),
            params,
        );
        let output_state = OutputModel::new(
            OutputId::new(0),
            "crawm-0",
            Rect::default(),
            1.0,
            space_state,
        );

        Self {
            compositor_state: CompositorState::new::<Self>(display),
            xdg_shell_state: XdgShellState::new::<Self>(display),
            shm_state: ShmState::new::<Self>(display, vec![]),
            data_device_state: DataDeviceState::new::<Self>(display),
            seat_state,
            keyboard,
            pointer,
            output,
            config: config.clone(),
            output_state,
            windows: Vec::new(),
            popup_manager: PopupManager::default(),
            focus: FocusTarget::None,
            focus_stack: FocusStack::new(),
            bound_keys: HashSet::new(),
            interaction: None,
            last_pointer_press: None,
            super_held: false,
            start_time: Instant::now(),
            frame_clock: Clocks::new(),
            pointer_location: (0.0, 0.0).into(),
            pointer_seen: false,
            cursor_image: CursorImageStatus::default_named(),
            size: (0, 0).into(),
            next_window_id: 1,
        }
    }

    /// Advertises the backend's current size to clients and re-lays out.
    ///
    /// M2 keeps the logical size equal to the physical size (the output is
    /// `Scale::Integer(1)`); fractional scale arrives with HDR and the render
    /// rework (post-0.1).
    pub(crate) fn set_size(&mut self, physical: Size<i32, Physical>) {
        self.size = (physical.w, physical.h).into();
        let mode = Mode {
            size: physical,
            refresh: 60_000,
        };
        self.output.change_current_state(
            Some(mode),
            Some(Transform::Normal),
            Some(Scale::Integer(1)),
            None,
        );
        self.output.set_preferred(mode);
        self.output_state.set_geometry(Rect::new(
            0.0,
            0.0,
            f64::from(physical.w),
            f64::from(physical.h),
        ));
        self.relayout();
    }

    /// Recomputes every window's rectangle for the current view and layout.
    pub(crate) fn relayout(&mut self) {
        layout::arrange(self);
    }

    /// Re-times the animation clock for the frame about to be drawn.
    ///
    /// `frame_time` is the presentation time of that frame, in seconds since
    /// [`Crawm::start_time`]. The winit backend has no vertical-blank timestamp
    /// and reads its own loop clock at the frame boundary; the tty backend will
    /// pass the real one (TODO.md §3.2).
    pub(crate) fn advance_frame(&mut self, frame_time: f64) {
        self.frame_clock.advance(frame_time);
    }

    /// Whether anything on screen is still moving.
    ///
    /// The backends ask this after each frame: while it holds, the redraw
    /// machine keeps requesting frames; once it stops, the compositor goes back
    /// to drawing nothing at all (ROADMAP.md §9, COMPOSITOR.md §9).
    pub(crate) fn is_animating(&self) -> bool {
        self.windows
            .iter()
            .any(|window| window.visual.is_animating(&self.frame_clock))
    }

    /// Registers a newly created toplevel with the space/layout model.
    ///
    /// A toplevel with a parent is a transient dialog: it floats immediately
    /// (`COMPOSITOR.md` §7.5). Everything else enters the tiled layout.
    pub(crate) fn add_toplevel(&mut self, surface: ToplevelSurface) {
        let id = WindowId::new(self.next_window_id);
        self.next_window_id += 1;

        let spaces = self.output_state.view.visible;
        let mut window = Window::new(id, spaces);
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
            visual_placed: false,
            configure,
        });

        self.focus_window(Some(id));
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
            Some(origin) if self.size.w > 0 && self.size.h > 0 => positioner
                .get_unconstrained_geometry(Rectangle::new(
                    (-origin.x, -origin.y).into(),
                    self.size,
                )),
            _ => positioner.get_geometry(),
        }
    }

    /// Handles a surface commit: refresh hints, mark it mapped, and re-lay out
    /// when something the layout cares about changed.
    pub(crate) fn on_commit(&mut self, surface: &WlSurface) {
        let mut relayout = false;

        let (min_size, max_size) = read_size_hints(surface);
        let bbox = smithay::desktop::utils::bbox_from_surface_tree(surface, (0, 0));
        let geometry = window_geometry(surface, bbox);
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
            if window.window.min_size != min_size || window.window.max_size != max_size {
                window.window.min_size = min_size;
                window.window.max_size = max_size;
                relayout = true;
            }
            if !window.mapped {
                window.mapped = true;
                relayout = true;
            }
            // A floating window owns its size; adopt whatever it committed,
            // measured by its window geometry rather than by its buffer (which
            // carries client-side decoration margins the layout must not see).
            if window.window.floating && geometry.size.w > 0 && geometry.size.h > 0 {
                if window.size != geometry.size {
                    window.size = geometry.size;
                    relayout = true;
                }
                let rect = window.rect();
                window.window.float_geometry = Some(rect);
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

        if relayout {
            self.relayout();
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
        self.popup_manager.cleanup();
    }

    /// Removes the window owning `surface` (from `toplevel_destroyed`).
    pub(crate) fn remove_window(&mut self, surface: &WlSurface) {
        let Some(index) = self
            .windows
            .iter()
            .position(|window| window.wl_surface() == surface)
        else {
            return;
        };
        let id = self.windows[index].window.id;
        self.windows.remove(index);
        self.forget_windows(&[id]);
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
        if let Some(window) = self
            .windows
            .iter_mut()
            .find(|window| window.wl_surface() == surface.wl_surface())
        {
            window.window.title = title;
            window.window.app_id = app_id;
        }
    }

    /// Drops `ids` from the focus stack and per-space history and refocuses.
    fn forget_windows(&mut self, ids: &[WindowId]) {
        for &id in ids {
            self.focus_stack.remove(FocusTarget::Window(id));
            for space in &mut self.output_state.space_states {
                space.forget(id);
            }
        }
        self.refocus_after_view_change();
        self.relayout();
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
        None
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

    /// Focuses `id` (or nothing), updating activation and keyboard focus.
    ///
    /// Every focus change goes through here (`COMPOSITOR.md` §6) so the
    /// protocol state, the focus stack, and the space history stay in step.
    pub(crate) fn focus_window(&mut self, id: Option<WindowId>) {
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
            if let Some(space) = self.output_state.current_space_state_mut() {
                space.note_focus(new);
            }
        }

        let surface = id.and_then(|id| self.window(id).map(|w| w.surface.wl_surface().clone()));
        self.keyboard.clone().set_focus(self, surface, 0.into());
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
        let point = crawm::core::Point::new(position.x, position.y);
        let hits = |window: &ManagedWindow| {
            window.mapped && window.visible && window.visual_rect(&self.frame_clock).contains(point)
        };
        // Floating windows stack by focus, most recent on top.
        for target in self.focus_stack.iter() {
            if let Some(id) = target.window() {
                if let Some(window) = self.window(id) {
                    if window.window.floating && hits(window) {
                        return Some(id);
                    }
                }
            }
        }
        self.windows
            .iter()
            .rev()
            .find(|window| hits(window))
            .map(ManagedWindow::id)
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
        // A popup floats above every toplevel, so its tree gets first refusal:
        // this is what keeps the pointer on an open menu (and lets its grab see
        // that the pointer is still on the right client).
        for window in self.windows.iter().rev().filter(|w| w.mapped && w.visible) {
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
        let id = self.window_at(position)?;
        let window = self.window(id)?;
        Some((
            window.wl_surface().clone(),
            window.fit(&self.frame_clock).input_origin(position),
        ))
    }

    /// Focuses the window under `position` (click-to-focus).
    pub(crate) fn focus_at(&mut self, position: Point<f64, Logical>) {
        if let Some(id) = self.window_at(position) {
            self.focus_window(Some(id));
        } else if self.surface_under(position).is_none() {
            // Clicking empty space clears focus; clicking a popup does not.
            self.focus_window(None);
        }
    }

    /// Focuses the next/previous visible window in list order.
    pub(crate) fn focus_cycle(&mut self, forward: bool) {
        let view = self.output_state.view.visible;
        let ids: Vec<WindowId> = self
            .windows
            .iter()
            .filter(|window| window.mapped && window.window.is_on(view))
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
            self.focus_window(Some(next));
            self.relayout();
        }
    }

    /// Views a single space by 1-based index.
    pub(crate) fn view_space(&mut self, index: u8) {
        let Some(mask) = SpaceSet::from_index(index) else {
            return;
        };
        self.output_state.view_space(mask);
        self.refocus_after_view_change();
        self.relayout();
    }

    /// Toggles a space in the view (`MOD+ctrl+N`).
    pub(crate) fn toggle_view_space(&mut self, index: u8) {
        let Some(mask) = SpaceSet::from_index(index) else {
            return;
        };
        self.output_state.toggle_view(mask);
        self.refocus_after_view_change();
        self.relayout();
    }

    /// Swaps to the previously-viewed space set.
    pub(crate) fn toggle_back(&mut self) {
        self.output_state.toggle_back();
        self.refocus_after_view_change();
        self.relayout();
    }

    /// Moves the focused window to a space by 1-based index.
    pub(crate) fn move_to_space(&mut self, index: u8) {
        let (Some(mask), Some(id)) = (SpaceSet::from_index(index), self.focused_id()) else {
            return;
        };
        if let Some(window) = self.window_mut(id) {
            window.window.move_to_space(mask);
        }
        self.refocus_after_view_change();
        self.relayout();
    }

    /// Toggles a space on the focused window (`MOD+shift+ctrl+N`).
    pub(crate) fn toggle_space(&mut self, index: u8) {
        let (Some(mask), Some(id)) = (SpaceSet::from_index(index), self.focused_id()) else {
            return;
        };
        if let Some(window) = self.window_mut(id) {
            window.window.toggle_space(mask);
        }
        self.refocus_after_view_change();
        self.relayout();
    }

    /// Re-focuses something sensible after the visible set changed.
    ///
    /// Prefers the space's own focus history, then any mapped window on the view;
    /// clears focus if the new view is empty.
    fn refocus_after_view_change(&mut self) {
        let view = self.output_state.view.visible;
        let on_screen = |window: &ManagedWindow| window.mapped && window.window.is_on(view);
        if self
            .focused_id()
            .and_then(|id| self.window(id))
            .is_some_and(on_screen)
        {
            return;
        }
        let next = self
            .output_state
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
        if let Some(space) = self.output_state.current_space_state_mut() {
            space.layout = space.layout.next_mode();
        }
        self.relayout();
    }

    /// Cycles the current space to the next layout family's default mode.
    pub(crate) fn cycle_layout_family(&mut self) {
        if let Some(space) = self.output_state.current_space_state_mut() {
            space.layout = space.layout.next_family();
        }
        self.relayout();
    }

    /// Applies a layout command (master ratio/count, column width, centring)
    /// to the current space.
    ///
    /// The command goes through the layout rather than straight to the
    /// parameters, so a family that owns its state — the scroller and its
    /// columns — gets to handle it (`crawm::layout::apply_command`).
    pub(crate) fn layout_command(&mut self, command: LayoutCommand) {
        if let Some(space) = self.output_state.current_space_state_mut() {
            crawm::layout::apply_command(
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

    /// Where an un-floated window should re-enter the tiling order.
    fn tiled_insert_index(&self, id: WindowId) -> usize {
        for target in self.focus_stack.iter() {
            if let Some(other) = target.window() {
                if other != id {
                    if let Some(position) = self
                        .windows
                        .iter()
                        .position(|w| w.window.id == other && w.window.is_tiled())
                    {
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

    /// Asks the focused window to close.
    pub(crate) fn close_focused(&mut self) {
        if let Some(id) = self.focused_id() {
            if let Some(window) = self.window(id) {
                window.surface.send_close();
            }
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
        let area = self.output_state.usable_area;
        let x = rect
            .x
            .clamp(area.x, (area.right() - MIN_VISIBLE).max(area.x));
        let y = rect
            .y
            .clamp(area.y, (area.bottom() - MIN_VISIBLE).max(area.y));
        Rect::new(x, y, rect.w, rect.h)
    }

    /// Clamps `position` into the visible area.
    pub(crate) fn clamp_to_output(&self, position: Point<f64, Logical>) -> Point<f64, Logical> {
        let max_x = f64::from(self.size.w.max(1) - 1);
        let max_y = f64::from(self.size.h.max(1) - 1);
        (position.x.clamp(0.0, max_x), position.y.clamp(0.0, max_y)).into()
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
}

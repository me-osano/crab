//! The single-owner compositor state — `calloop`'s `State` for crawm.
//!
//! [`Crawm`] owns every piece of Smithay state the compositor touches and is
//! owned by the main loop on one thread (ROADMAP.md §6 M1: *one
//! `calloop::EventLoop<State>` on the main thread; `State` is single-owner*).
//! Protocol handling is split per protocol under [`crate::handlers`]; input
//! lives in [`crate::input`] and drawing in [`crate::render`].
//!
//! The pure model — windows, spaces, layouts, focus — lives in the `crawm`
//! library ([`crawm::core`], [`crawm::layout`]); this file is the glue that
//! keeps a Smithay `ToplevelSurface` next to its [`Window`] and applies the
//! layout. That glue is binary-only, so it may depend on Smithay freely; the
//! library's four modules stay pure (`scripts/check-boundaries.sh`).

use std::collections::HashSet;
use std::time::Instant;

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
use smithay::utils::{Logical, Physical, Point, Rectangle, Size, Transform};
use smithay::wayland::compositor::{CompositorClientState, CompositorState, with_states};
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, SurfaceCachedState, ToplevelSurface, XdgShellState,
    XdgToplevelSurfaceData,
};
use smithay::wayland::shm::ShmState;

use crate::layout_glue;

/// Size a new toplevel is configured to before the layout first sizes it, in
/// logical pixels. Floating windows pick their own size instead.
pub(crate) const DEFAULT_WINDOW_SIZE: (i32, i32) = (800, 600);

/// How much of a window must stay on screen when it is dragged, in pixels.
const MIN_VISIBLE: f64 = 32.0;

/// One managed toplevel: the protocol object and its pure model.
pub(crate) struct ManagedWindow {
    /// The xdg toplevel this describes.
    pub(crate) surface: ToplevelSurface,
    /// Its place in the space/layout model.
    pub(crate) window: Window,
    /// Top-left corner in logical compositor coordinates.
    pub(crate) location: Point<i32, Logical>,
    /// Logical size the window is drawn at.
    pub(crate) size: Size<i32, Logical>,
    /// Whether the layout currently shows this window.
    pub(crate) visible: bool,
    /// Set once the client commits a buffer; unmapped windows are not drawn.
    pub(crate) mapped: bool,
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

    /// Current rectangle in logical coordinates.
    pub(crate) fn rect(&self) -> Rect {
        Rect::new(
            f64::from(self.location.x),
            f64::from(self.location.y),
            f64::from(self.size.w),
            f64::from(self.size.h),
        )
    }
}

/// What a pointer grab is doing to a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GrabMode {
    /// Moving the window around.
    Move,
    /// Resizing it from its bottom-right corner.
    Resize,
}

/// An in-progress interactive move/resize started from a keybind.
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
    /// Whether Super is currently held (drives pointer grabs).
    pub(crate) super_held: bool,
    /// Time of the first frame, used for `wl_surface.frame` timestamps.
    pub(crate) start_time: Instant,
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
            super_held: false,
            start_time: Instant::now(),
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
        layout_glue::arrange(self);
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

        self.windows.push(ManagedWindow {
            surface,
            window,
            location: (0, 0).into(),
            size: DEFAULT_WINDOW_SIZE.into(),
            visible: false,
            mapped: false,
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
        for window in &mut self.windows {
            if window.wl_surface() != surface {
                continue;
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
            // A floating window owns its size; adopt whatever it committed.
            if window.window.floating && bbox.size.w > 0 && bbox.size.h > 0 {
                if window.size != bbox.size {
                    window.size = bbox.size;
                    relayout = true;
                }
                let rect = window.rect();
                window.window.float_geometry = Some(rect);
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
    /// Popups are positioned against their parent's window geometry (the
    /// rectangle set with `xdg_surface.set_window_geometry`), while the layout
    /// places windows by their surface origin, so the two differ by whatever
    /// client-side decorations the client reports.
    fn window_geometry_origin(&self, surface: &WlSurface) -> Option<Point<i32, Logical>> {
        if let Some(window) = self
            .windows
            .iter()
            .find(|window| window.wl_surface() == surface)
        {
            return Some(window.location + surface_geometry_loc(surface));
        }
        for window in &self.windows {
            for (popup, offset) in popup_offsets(window) {
                if popup.wl_surface() == surface {
                    // `offset` is the popup's surface origin; its geometry
                    // origin is that plus whatever the popup itself reports.
                    return Some(window.location + offset + popup.geometry().loc);
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
            window.mapped && window.visible && window.rect().contains(point)
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
    /// logical coordinates of its top-left corner.
    pub(crate) fn surface_under(
        &self,
        position: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<i32, Logical>)> {
        // A popup floats above every toplevel, so its tree gets first refusal:
        // this is what keeps the pointer on an open menu (and lets its grab see
        // that the pointer is still on the right client).
        for window in self.windows.iter().rev().filter(|w| w.mapped && w.visible) {
            for (popup, offset) in popup_offsets(window) {
                let origin = window.location + offset;
                if let Some((surface, location)) = under_from_surface_tree(
                    popup.wl_surface(),
                    position - origin.to_f64(),
                    (0, 0),
                    WindowSurfaceType::ALL,
                ) {
                    return Some((surface, origin + location));
                }
            }
        }
        let id = self.window_at(position)?;
        let window = self.window(id)?;
        Some((window.wl_surface().clone(), window.location))
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
        if let Some(next) = layout_glue::directional_target(self, current, direction) {
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

    /// Applies a layout command (master ratio/count) to the current space.
    pub(crate) fn layout_command(&mut self, command: LayoutCommand) {
        if let Some(space) = self.output_state.current_space_state_mut() {
            command.apply(&mut space.params);
        }
        self.relayout();
    }

    /// Flips the focused window between tiled and floating.
    pub(crate) fn toggle_floating(&mut self) {
        let Some(id) = self.focused_id() else {
            return;
        };
        let Some(rect) = self.window(id).map(ManagedWindow::rect) else {
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
    ) -> bool {
        let Some(id) = self.window_at(position) else {
            return false;
        };
        let rect = self.window(id).map(ManagedWindow::rect).unwrap_or_default();
        if let Some(window) = self.window_mut(id) {
            if window.window.is_tiled() {
                window.window.set_floating(true, rect);
            }
        }
        self.focus_window(Some(id));
        self.interaction = Some(Interaction {
            id,
            mode,
            grab: position,
            origin: rect,
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
        let origin = interaction.origin;
        let (rect, resize) = match interaction.mode {
            GrabMode::Move => (origin.translated(dx, dy), false),
            GrabMode::Resize => (
                Rect::new(
                    origin.x,
                    origin.y,
                    (origin.w + dx).max(1.0),
                    (origin.h + dy).max(1.0),
                ),
                true,
            ),
        };
        let rect = self.clamp_rect(rect);

        if let Some(window) = self.window_mut(interaction.id) {
            window.window.float_geometry = Some(rect);
            window.location = point_i32(rect.x, rect.y);
            if resize {
                let size = size_i32(rect.w, rect.h);
                if window.size != size {
                    window.size = size;
                    window
                        .surface
                        .with_pending_state(|state| state.size = Some(size));
                    window.surface.send_configure();
                }
            }
        }
        self.relayout();
        true
    }

    /// Ends an in-progress interaction, if any. Returns whether one was active.
    pub(crate) fn end_interaction(&mut self) -> bool {
        let active = self.interaction.is_some();
        if active {
            self.interaction = None;
            self.relayout();
        }
        active
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

/// Every popup hanging off `window`, with the popup's *surface* origin relative
/// to the top-left of the window's surface.
///
/// Popups are placed against their parent's window geometry, which clients inset
/// from the surface by their decorations (and their own geometry, for a popup),
/// so the offset has to add the parent's inset and subtract the popup's:
/// `parent_geometry + location - popup_geometry`, exactly how Smithay's own
/// `Window` element computes it.
pub(crate) fn popup_offsets(
    window: &ManagedWindow,
) -> impl Iterator<Item = (PopupKind, Point<i32, Logical>)> {
    let parent = surface_geometry_loc(window.wl_surface());
    PopupManager::popups_for_surface(window.wl_surface()).map(move |(popup, location)| {
        let offset = parent + location - popup.geometry().loc;
        (popup, offset)
    })
}

/// `xdg_surface.set_window_geometry`'s origin, or the surface origin if unset.
fn surface_geometry_loc(surface: &WlSurface) -> Point<i32, Logical> {
    with_states(surface, |states| {
        states
            .cached_state
            .get::<SurfaceCachedState>()
            .current()
            .geometry
            .map_or((0, 0).into(), |geometry| geometry.loc)
    })
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

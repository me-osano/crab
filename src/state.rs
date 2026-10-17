//! The single-owner compositor state — `calloop`'s `State` for crawm.
//!
//! [`Crawm`] owns every piece of Smithay state the compositor touches and is
//! owned by the main loop on one thread (ROADMAP.md §6 M1: *one
//! `calloop::EventLoop<State>` on the main thread; `State` is single-owner*).
//! Protocol handling is split per protocol under [`crate::handlers`]; input
//! lives in [`crate::input`] and drawing in [`crate::render`].
//!
//! This is binary-only glue, so it may depend on Smithay freely. The
//! Smithay-free rule applies to `src/lib.rs`'s four modules (`core`, `config`,
//! `layout`, `animation`), which `scripts/check-boundaries.sh` keeps pure.

use std::time::Instant;

use smithay::input::SeatState;
use smithay::input::keyboard::KeyboardHandle;
use smithay::input::pointer::PointerHandle;
use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Physical, Point, Rectangle, Size, Transform};
use smithay::wayland::compositor::{CompositorClientState, CompositorState};
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::shell::xdg::{PopupSurface, PositionerState, ToplevelSurface, XdgShellState};
use smithay::wayland::shm::ShmState;

/// Size every new toplevel is configured to, in logical pixels (M1 only — real
/// placement asks the client and lives with the layout engine in M2).
pub(crate) const TOPLEVEL_SIZE: (i32, i32) = (800, 600);

/// Gap between two cascaded windows, in logical pixels (M1 fixed placement).
pub(crate) const CASCADE_STEP: i32 = 32;

/// One managed toplevel and where it sits on screen.
pub(crate) struct Toplevel {
    pub(crate) surface: ToplevelSurface,
    /// Top-left corner in logical compositor coordinates.
    pub(crate) location: Point<i32, Logical>,
    /// Logical size this window was configured to; M1 placement and hit-testing
    /// both rely on it until the layout engine lands (M2).
    pub(crate) size: Size<i32, Logical>,
    /// Set once the client commits a buffer; unmapped windows are not drawn.
    pub(crate) mapped: bool,
}

/// One xdg popup (menus, tooltips), resolved to a global position at creation.
pub(crate) struct Popup {
    pub(crate) surface: PopupSurface,
    pub(crate) location: Point<i32, Logical>,
    pub(crate) size: Size<i32, Logical>,
    pub(crate) mapped: bool,
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
    /// Time of the first frame, used for `wl_surface.frame` timestamps.
    pub(crate) start_time: Instant,
    /// Pointer position in logical compositor coordinates.
    pub(crate) pointer_location: Point<f64, Logical>,
    /// Logical size of the visible area; kept in sync by the backends.
    pub(crate) size: Size<i32, Logical>,
    /// Managed toplevels, in stacking order (first = bottom).
    pub(crate) toplevels: Vec<Toplevel>,
    /// Popups always draw above every toplevel (M1 has no z-ordering beyond
    /// this).
    pub(crate) popups: Vec<Popup>,
}

impl Crawm {
    /// Builds every Wayland global on `display` and returns the initial state.
    ///
    /// The globals registered here (compositor, xdg-shell, shm, data-device,
    /// wl_seat, wl_output) are M1's core set; M3 adds the layer shell and the
    /// protocol breadth from `crawm-protocols`.
    pub(crate) fn new(display: &DisplayHandle) -> Self {
        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(display, "crawm");
        // Focus is pointless without a keyboard, and adding it can only fail if
        // keymap creation fails, which is not recoverable at startup.
        let keyboard = seat
            .add_keyboard(Default::default(), 200, 25)
            .expect("failed to add the default keyboard to the seat");
        let pointer = seat.add_pointer();

        // Physical size comes from the backend once it exists; M1 starts at
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

        Self {
            compositor_state: CompositorState::new::<Self>(display),
            xdg_shell_state: XdgShellState::new::<Self>(display),
            shm_state: ShmState::new::<Self>(display, vec![]),
            data_device_state: DataDeviceState::new::<Self>(display),
            seat_state,
            keyboard,
            pointer,
            output,
            start_time: Instant::now(),
            pointer_location: (0.0, 0.0).into(),
            size: (0, 0).into(),
            toplevels: Vec::new(),
            popups: Vec::new(),
        }
    }

    /// Advertises the backend's current size to clients.
    ///
    /// M1 keeps the logical size equal to the physical size (the output is
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
    }

    /// Adds a newly configured toplevel at its cascaded position.
    pub(crate) fn add_toplevel(&mut self, surface: ToplevelSurface) {
        let offset = CASCADE_STEP * self.toplevels.len() as i32;
        self.toplevels.push(Toplevel {
            surface,
            location: (offset, offset).into(),
            size: TOPLEVEL_SIZE.into(),
            mapped: false,
        });
    }

    /// Adds a popup at the position its positioner asks for, clamped to the
    /// output, and sends the initial configure so it can map.
    pub(crate) fn add_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        let parent_location = surface
            .get_parent_surface()
            .and_then(|parent| self.location_of(&parent))
            .unwrap_or_else(|| (0, 0).into());

        // `get_geometry` is relative to the parent's window geometry; the
        // output expressed in that same space is the constraint target.
        let geometry = if self.size.w > 0 && self.size.h > 0 {
            let target = Rectangle::new((-parent_location.x, -parent_location.y).into(), self.size);
            positioner.get_unconstrained_geometry(target)
        } else {
            positioner.get_geometry()
        };

        if let Err(err) = surface.send_configure() {
            tracing::debug!(?err, "failed to configure a new popup");
        }

        self.popups.push(Popup {
            surface,
            location: parent_location + geometry.loc,
            size: geometry.size,
            mapped: false,
        });
    }

    /// Marks `surface` mapped once it has committed a real buffer.
    pub(crate) fn map_surface(&mut self, surface: &WlSurface) {
        for toplevel in &mut self.toplevels {
            if !toplevel.mapped && toplevel.surface.wl_surface() == surface {
                toplevel.mapped = true;
            }
        }
        for popup in &mut self.popups {
            if !popup.mapped && popup.surface.wl_surface() == surface {
                popup.mapped = true;
            }
        }
    }

    /// Drops entries whose client object is gone.
    pub(crate) fn prune_dead(&mut self) {
        self.toplevels.retain(|toplevel| toplevel.surface.alive());
        self.popups.retain(|popup| popup.surface.alive());
    }

    /// Top-left corner of the toplevel or popup owning `surface`, if any.
    fn location_of(&self, surface: &WlSurface) -> Option<Point<i32, Logical>> {
        if let Some(toplevel) = self
            .toplevels
            .iter()
            .find(|toplevel| toplevel.surface.wl_surface() == surface)
        {
            return Some(toplevel.location);
        }
        self.popups
            .iter()
            .find(|popup| popup.surface.wl_surface() == surface)
            .map(|popup| popup.location)
    }

    /// Returns the top-most mapped surface covering `position`, with the
    /// logical coordinates of its top-left corner.
    pub(crate) fn surface_under(
        &self,
        position: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<i32, Logical>)> {
        let (x, y) = (position.x.floor(), position.y.floor());
        let hit = |location: Point<i32, Logical>, size: Size<i32, Logical>| {
            x >= f64::from(location.x)
                && y >= f64::from(location.y)
                && x < f64::from(location.x + size.w)
                && y < f64::from(location.y + size.h)
        };

        // Popups draw above toplevels (see `Crawm::popups`).
        for popup in self.popups.iter().rev() {
            if popup.mapped && hit(popup.location, popup.size) {
                return Some((popup.surface.wl_surface().clone(), popup.location));
            }
        }
        for toplevel in self.toplevels.iter().rev() {
            if toplevel.mapped && hit(toplevel.location, toplevel.size) {
                return Some((toplevel.surface.wl_surface().clone(), toplevel.location));
            }
        }
        None
    }

    /// Gives keyboard focus to the surface under `position` (click-to-focus).
    pub(crate) fn focus_at(&mut self, position: Point<f64, Logical>) {
        let focus = self.surface_under(position).map(|(surface, _)| surface);
        self.keyboard.clone().set_focus(self, focus, 0.into());
    }

    /// Clamps `position` into the visible area.
    pub(crate) fn clamp_to_output(&self, position: Point<f64, Logical>) -> Point<f64, Logical> {
        let max_x = f64::from(self.size.w.max(1) - 1);
        let max_y = f64::from(self.size.h.max(1) - 1);
        (position.x.clamp(0.0, max_x), position.y.clamp(0.0, max_y)).into()
    }
}

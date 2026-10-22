//! The bridge between the pure layout engine (`crab::layout`) and Smithay state.
//!
//! `crab::layout` is a pure function over logical rectangles; [`Crab`] holds
//! `ToplevelSurface`s that must be told about those rectangles through
//! `configure`. This module is the one place that knows both: it reads the
//! current space's layout, builds the [`LayoutInput`], and applies the resulting
//! [`Placement`]s (and the floating/fullscreen rules the layouts deliberately do
//! not know about, `COMPOSITOR.md` §7.1). Every layout *decision* is made in the
//! pure module; this one only carries it out.
//!
//! It is part of the binary (see [`crate::compositor`]), so it may depend on
//! Smithay freely.

use crab::animation::Kind;
use crab::core::{
    Direction, LayoutFamily, LayoutKind, LayoutParams, Rect, ScrollState, SpaceSet, WindowId,
};
use crab::layout::{self, LayoutInput, LayoutWindow, Placement};

use crate::compositor::state::{Crab, ManagedWindow, point_i32, size_i32};

/// Gap between two cascaded floating windows, in logical pixels.
const CASCADE_STEP: f64 = 32.0;

/// Recomputes and applies every window's rectangle for the current view.
///
/// The order the passes run in is the stacking order: fullscreen/maximized
/// windows fill the output, then the tiled windows are arranged, then floating
/// windows sit on top with their remembered geometry.
pub(crate) fn arrange(state: &mut Crab) {
    let view = state.output_state.view.visible;
    let area = state.output_state.usable_area;

    // Everything not re-placed below ends up hidden.
    for window in &mut state.windows {
        window.visible = false;
    }

    // 1. Fullscreen and maximized windows cover the usable area.
    let covering: Vec<WindowId> = state
        .windows
        .iter()
        .filter(|window| {
            window.mapped
                && (window.window.fullscreen || window.window.maximized)
                && window.window.is_on(view)
        })
        .map(ManagedWindow::id)
        .collect();
    let covering_count = covering.len();
    for id in covering {
        place(state, id, area, true, true);
    }

    // 2. Tiled windows, arranged by the current space's layout.
    //
    // A stack-family space in its `floating` mode places nothing: every window
    // on it is handled by the floating pass below. This is what makes
    // `layout = "floating"` behave like floating even for windows that are not
    // individually floated (`COMPOSITOR.md` §7.3).
    let (kind, params, mut scroll) = current_layout(state);
    let free = kind.is_free();
    let tiled = if free {
        Vec::new()
    } else {
        tiled_windows(state, view)
    };
    let tiled_count = tiled.len();
    let focused = state
        .focus
        .window()
        .and_then(|id| tiled.iter().position(|window| window.id == id));
    let placements = layout::arrange(
        kind,
        LayoutInput {
            area,
            windows: &tiled,
            focused,
            params: &params,
            scroll: &mut scroll,
        },
    );
    for placement in &placements {
        if placement.visible {
            place(state, placement.window, placement.rect, true, true);
        }
    }
    // The scroller's model is worth seeing when a strip does not look right; it
    // is tallied at `trace`, so a normal run stays quiet.
    if kind.family() == LayoutFamily::Scroll {
        let placed: Vec<_> = placements
            .iter()
            .map(|placement| (placement.window, placement.rect))
            .collect();
        tracing::trace!(
            columns = scroll.columns.len(),
            offset = scroll.offset,
            ?focused,
            placed = ?placed,
            "scrolled the view"
        );
    }

    // A scrolling layout may have updated its offset.
    if let Some(space) = state.output_state.current_space_state_mut() {
        space.scroll = scroll;
    }

    // 3. Floating windows keep (or, on first sight, get) their own geometry.
    // On a free space every window belongs here, floated or not.
    let floating: Vec<WindowId> = state
        .windows
        .iter()
        .filter(|window| {
            window.mapped
                && (free || window.window.floating)
                && !window.window.fullscreen
                && !window.window.maximized
                && window.window.is_on(view)
        })
        .map(ManagedWindow::id)
        .collect();
    let floating_count = floating.len();
    for (index, id) in floating.into_iter().enumerate() {
        place_floating(state, id, index, area);
    }

    tracing::debug!(
        layout = kind.name(),
        tiled = tiled_count,
        floating = floating_count,
        covering = covering_count,
        "arranged the view"
    );
}

/// The window to focus when moving `dir` from `current`, if any.
pub(crate) fn directional_target(
    state: &Crab,
    current: WindowId,
    dir: Direction,
) -> Option<WindowId> {
    let view = state.output_state.view.visible;
    let (kind, params, mut scroll) = current_layout(state);
    let tiled = if kind.is_free() {
        Vec::new()
    } else {
        tiled_windows(state, view)
    };
    if let Some(from) = tiled.iter().position(|window| window.id == current) {
        let input = LayoutInput {
            area: state.output_state.usable_area,
            windows: &tiled,
            focused: Some(from),
            params: &params,
            scroll: &mut scroll,
        };
        return layout::neighbor(kind, &input, from, dir)
            .and_then(|index| tiled.get(index).map(|window| window.id));
    }

    // Floating (or hidden) windows: fall back to a geometric search.
    let placements: Vec<Placement> = state
        .windows
        .iter()
        .filter(|window| window.visible)
        .map(|window| Placement::visible(window.id(), window.rect()))
        .collect();
    let from = placements.iter().position(|p| p.window == current)?;
    layout::directional_neighbor(&placements, from, dir).map(|index| placements[index].window)
}

/// The tiled windows on `view`, in layout order.
fn tiled_windows(state: &Crab, view: SpaceSet) -> Vec<LayoutWindow> {
    state
        .windows
        .iter()
        .filter(|window| {
            window.mapped
                && window.window.is_tiled()
                && !window.window.maximized
                && window.window.is_on(view)
        })
        .map(|window| LayoutWindow {
            id: window.id(),
            min_size: window.window.min_size,
            max_size: window.window.max_size,
        })
        .collect()
}

/// The current space's layout, parameters, and scroll state (copied out so the
/// layout call does not borrow `state`).
///
/// The scroll state is cloned rather than copied — it owns the scroller's
/// columns — and written back after the arrange.
fn current_layout(state: &Crab) -> (LayoutKind, LayoutParams, ScrollState) {
    state.output_state.current_space_state().map_or_else(
        || {
            (
                LayoutKind::default(),
                LayoutParams::default(),
                ScrollState::default(),
            )
        },
        |space| (space.layout, space.params, space.scroll.clone()),
    )
}

/// Places a tiled/fullscreen window, configuring the client when the size
/// changes.
///
/// The layout's rectangle is the *target*: the client is configured with it —
/// clamped to the size hints it set, since a request it cannot honour only
/// earns a refusal — and the drawn rectangle is animated toward it. The kind
/// records what moved the window — a change of size or a change of place —
/// which is the granularity animations are configured at (M5).
fn place(state: &mut Crab, id: WindowId, rect: Rect, configure_size: bool, visible: bool) {
    let location = point_i32(rect.x, rect.y);
    let size = size_i32(rect.w, rect.h);
    let clocks = state.frame_clock;
    // `[animations] enabled = 0` makes every placement instant; the first
    // placement snaps either way.
    let animate = state.config.animations.enabled;
    let Some(window) = state.window_mut(id) else {
        return;
    };
    window.visible = visible;
    window.location = location;
    let kind = if window.size == size {
        Kind::WindowMove
    } else {
        Kind::WindowResize
    };
    if configure_size {
        window.size = size;
        // Asks the client for this rectangle. A size already asked for sends
        // nothing, so a re-layout that changes no rectangle configures no
        // client; a rectangle the client cannot be (its hints) asks for the
        // size it can.
        window.configure_to(size);
    }
    if animate {
        window.animate_to(rect, kind, &clocks);
    } else {
        window.snap_visual_to(rect);
    }
}

/// Places a floating window at its remembered geometry (cascading a new one).
///
/// Floating windows own their size, so none is configured here: the client's
/// committed size is adopted in [`Crab::on_commit`].
fn place_floating(state: &mut Crab, id: WindowId, index: usize, area: Rect) {
    let Some(window) = state.window(id) else {
        return;
    };
    let rect = match window.window.float_geometry {
        Some(rect) => rect,
        None => {
            let size = crab::core::Size::new(
                f64::from(window.size.w.max(1)),
                f64::from(window.size.h.max(1)),
            );
            layout::cascade_rect(index, area, size, CASCADE_STEP)
        }
    };
    if let Some(window) = state.window_mut(id) {
        window.window.float_geometry = Some(rect);
    }
    place(state, id, rect, false, true);
}

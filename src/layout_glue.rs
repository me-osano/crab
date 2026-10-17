//! The bridge between the pure layout engine and Smithay state.
//!
//! `crawm::layout` is a pure function over logical rectangles; [`Crawm`] holds
//! `ToplevelSurface`s that must be told about those rectangles through
//! `configure`. This module is the one place that knows both: it reads the
//! current space's layout, builds the [`LayoutInput`], and applies the resulting
//! [`Placement`]s (and the floating/fullscreen rules the layouts deliberately do
//! not know about, `COMPOSITOR.md` §7.1).
//!
//! It is binary-only glue, so it may depend on Smithay freely.

use crawm::core::{Direction, LayoutKind, LayoutParams, Rect, ScrollState, SpaceSet, WindowId};
use crawm::layout::{self, LayoutInput, LayoutWindow, Placement};

use crate::state::{Crawm, ManagedWindow, point_i32, size_i32};

/// Gap between two cascaded floating windows, in logical pixels.
const CASCADE_STEP: f64 = 32.0;

/// Recomputes and applies every window's rectangle for the current view.
///
/// The order the passes run in is the stacking order: fullscreen/maximized
/// windows fill the output, then the tiled windows are arranged, then floating
/// windows sit on top with their remembered geometry.
pub(crate) fn arrange(state: &mut Crawm) {
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
    state: &Crawm,
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
fn tiled_windows(state: &Crawm, view: SpaceSet) -> Vec<LayoutWindow> {
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
fn current_layout(state: &Crawm) -> (LayoutKind, LayoutParams, ScrollState) {
    state.output_state.current_space_state().map_or_else(
        || {
            (
                LayoutKind::default(),
                LayoutParams::default(),
                ScrollState::default(),
            )
        },
        |space| (space.layout, space.params, space.scroll),
    )
}

/// Places a tiled/fullscreen window, configuring the client when the size
/// changes.
fn place(state: &mut Crawm, id: WindowId, rect: Rect, configure_size: bool, visible: bool) {
    let location = point_i32(rect.x, rect.y);
    let size = size_i32(rect.w, rect.h);
    let Some(window) = state.window_mut(id) else {
        return;
    };
    window.visible = visible;
    window.location = location;
    if configure_size && window.size != size {
        window.size = size;
        window
            .surface
            .with_pending_state(|pending| pending.size = Some(size));
        window.surface.send_configure();
    }
}

/// Places a floating window at its remembered geometry (cascading a new one).
///
/// Floating windows own their size, so none is configured here: the client's
/// committed size is adopted in [`Crawm::on_commit`].
fn place_floating(state: &mut Crawm, id: WindowId, index: usize, area: Rect) {
    let Some(window) = state.window(id) else {
        return;
    };
    let rect = match window.window.float_geometry {
        Some(rect) => rect,
        None => {
            let size = crawm::core::Size::new(
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

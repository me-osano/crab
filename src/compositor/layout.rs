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
    Direction, LayoutFamily, LayoutKind, LayoutParams, OutputId, Rect, ScrollState, SpaceSet,
    WindowId,
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
///
/// Returns the windows that were configured and have not committed the size
/// they were asked for. [`Crab::relayout`] turns them into a configure
/// transaction so the frame is not presented with a mix of generations.
pub(crate) fn arrange(state: &mut Crab, output: OutputId) -> Vec<WindowId> {
    let Some(runtime) = state.outputs.get(&output) else {
        return Vec::new();
    };
    let view = runtime.model.view.visible;
    let area = runtime.model.usable_area;

    // Everything on this output that is not re-placed below ends up hidden.
    // Windows on other outputs keep the visibility their own arrange set.
    for window in &mut state.windows {
        if window.window.output == output {
            window.visible = false;
        }
    }

    // 1. Fullscreen and maximized windows cover the usable area.
    let covering: Vec<WindowId> = state
        .windows
        .iter()
        .filter(|window| {
            window.window.output == output
                && window.mapped
                && (window.window.fullscreen || window.window.maximized)
                && window.window.is_on(view)
        })
        .map(ManagedWindow::id)
        .collect();
    let covering_count = covering.len();
    let mut blockers = Vec::new();
    for id in covering {
        if place(state, id, area, true, true) {
            blockers.push(id);
        }
    }

    // 2. Tiled windows, arranged by the current space's layout.
    //
    // A stack-family space in its `floating` mode places nothing: every window
    // on it is handled by the floating pass below. This is what makes
    // `layout = "floating"` behave like floating even for windows that are not
    // individually floated (`COMPOSITOR.md` §7.3).
    let (kind, params, mut scroll) = take_layout(state, output);
    let free = kind.is_free();
    let tiled = if free {
        Vec::new()
    } else {
        tiled_windows(state, output, view)
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
        if placement.visible && place(state, placement.window, placement.rect, true, true) {
            blockers.push(placement.window);
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
    if let Some(space) = state
        .outputs
        .get_mut(&output)
        .and_then(|runtime| runtime.model.current_space_state_mut())
    {
        space.scroll = scroll;
    }

    // 3. Floating windows keep (or, on first sight, get) their own geometry.
    // On a free space every window belongs here, floated or not.
    let floating: Vec<WindowId> = state
        .windows
        .iter()
        .filter(|window| {
            window.window.output == output
                && window.mapped
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

    blockers
}

/// The window to focus when moving `dir` from `current`, if any.
pub(crate) fn directional_target(
    state: &Crab,
    current: WindowId,
    dir: Direction,
) -> Option<WindowId> {
    let output = state
        .window(current)
        .map_or(state.focused_output, |window| window.window.output);
    let runtime = state.outputs.get(&output)?;
    let view = runtime.model.view.visible;
    let (kind, params, mut scroll) = current_layout(state, output);
    let tiled = if kind.is_free() {
        Vec::new()
    } else {
        tiled_windows(state, output, view)
    };
    if let Some(from) = tiled.iter().position(|window| window.id == current) {
        let input = LayoutInput {
            area: state.outputs.get(&output)?.model.usable_area,
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
        .filter(|window| window.visible && window.window.output == output)
        .map(|window| Placement::visible(window.id(), window.rect()))
        .collect();
    let from = placements.iter().position(|p| p.window == current)?;
    layout::directional_neighbor(&placements, from, dir).map(|index| placements[index].window)
}

/// The tiled windows on `view`, in layout order.
fn tiled_windows(state: &Crab, output: OutputId, view: SpaceSet) -> Vec<LayoutWindow> {
    state
        .windows
        .iter()
        .filter(|window| {
            window.window.output == output
                && window.mapped
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

/// The current space's layout, parameters, and scroll state, copied out for
/// read-only callers such as [`directional_target`].
///
/// The scroll state is cloned rather than copied — it owns the scroller's
/// columns. [`arrange`] instead uses [`take_layout`] so the per-frame path does
/// not allocate.
fn current_layout(state: &Crab, output: OutputId) -> (LayoutKind, LayoutParams, ScrollState) {
    state
        .outputs
        .get(&output)
        .and_then(|runtime| runtime.model.current_space_state())
        .map_or_else(
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

/// Moves the current space's layout, parameters, and scroll state out of
/// `state`, so [`arrange`] can borrow the state mutably without cloning the
/// scroller's columns.
///
/// The caller must write the returned scroll state back; [`arrange`] does so
/// after the layout has updated the offset.
fn take_layout(state: &mut Crab, output: OutputId) -> (LayoutKind, LayoutParams, ScrollState) {
    state
        .outputs
        .get_mut(&output)
        .and_then(|runtime| runtime.model.current_space_state_mut())
        .map_or_else(
            || {
                (
                    LayoutKind::default(),
                    LayoutParams::default(),
                    ScrollState::default(),
                )
            },
            |space| {
                (
                    space.layout,
                    space.params,
                    core::mem::take(&mut space.scroll),
                )
            },
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
fn place(state: &mut Crab, id: WindowId, rect: Rect, configure_size: bool, visible: bool) -> bool {
    let location = point_i32(rect.x, rect.y);
    let size = size_i32(rect.w, rect.h);
    let clocks = state.frame_clock;
    // `[animations] enabled = 0` makes every placement instant; the first
    // placement snaps either way.
    let animate = state.config.animations.enabled;
    let Some(window) = state.window_mut(id) else {
        return false;
    };
    window.visible = visible;
    window.location = location;
    let kind = if window.size == size {
        Kind::WindowMove
    } else {
        Kind::WindowResize
    };
    let mut awaits_configure = false;
    if configure_size {
        window.size = size;
        // Asks the client for this rectangle. A size already asked for sends
        // nothing, so a re-layout that changes no rectangle configures no
        // client; a rectangle the client cannot be (its hints) asks for the
        // size it can.
        window.configure_to(size);
        awaits_configure = window.awaits_configure();
    }
    if animate {
        window.animate_to(rect, kind, &clocks);
    } else {
        window.snap_visual_to(rect);
    }
    awaits_configure
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

//! The `Layout` trait and the types it consumes and produces.
//!
//! A [`Layout`] is a **pure function** from a [`LayoutInput`] to a list of
//! [`Placement`]s: no globals, no I/O, no Smithay (`ROADMAP.md` §3B,
//! `COMPOSITOR.md` §7.1). Floating and fullscreen windows are excluded from
//! [`Layout::arrange`] and drawn above tiled windows by the compositor, so a
//! layout never needs to know about them.

use crate::core::{
    Direction, LayoutCommand, LayoutKind, LayoutParams, Rect, ScrollState, Size, WindowId,
};

/// One tiled window, as a layout sees it.
///
/// Only what a layout needs to place it: identity plus its size hints, which
/// act as soft constraints (`COMPOSITOR.md` §7.5).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutWindow {
    /// The window this is.
    pub id: WindowId,
    /// Client-requested minimum size, if any.
    pub min_size: Option<Size>,
    /// Client-requested maximum size, if any (a zero axis is unconstrained).
    pub max_size: Option<Size>,
}

impl LayoutWindow {
    /// A window with no size hints.
    #[must_use]
    pub const fn new(id: WindowId) -> Self {
        Self {
            id,
            min_size: None,
            max_size: None,
        }
    }
}

/// Everything a [`Layout`] is given for one `arrange` call.
#[derive(Debug)]
pub struct LayoutInput<'a> {
    /// The area the layout may use, before outer gaps.
    pub area: Rect,
    /// Tiled windows on the visible space(s), in the layout order.
    pub windows: &'a [LayoutWindow],
    /// Index into `windows` of the focused window.
    pub focused: Option<usize>,
    /// The space's tunables (gaps, master ratio, …).
    pub params: &'a LayoutParams,
    /// Persistent state for scrolling layouts; M2's layouts ignore it.
    pub scroll: &'a mut ScrollState,
}

impl LayoutInput<'_> {
    /// The area left for windows after the outer gaps are removed.
    #[must_use]
    pub fn usable_area(&self) -> Rect {
        self.area.deflated(self.params.gaps)
    }

    /// The index of `window` in the input, if it is present.
    #[must_use]
    pub fn index_of(&self, window: WindowId) -> Option<usize> {
        self.windows.iter().position(|entry| entry.id == window)
    }

    /// The window at `index`, if any.
    #[must_use]
    pub fn window_at(&self, index: usize) -> Option<LayoutWindow> {
        self.windows.get(index).copied()
    }
}

/// Where a layout wants a window drawn (`COMPOSITOR.md` §7.1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// The window this placement is for.
    pub window: WindowId,
    /// The target rectangle in logical coordinates.
    pub rect: Rect,
    /// Whether the window should be shown; layouts such as `Scroll` hide all
    /// but one while keeping a rect for animation continuity.
    pub visible: bool,
    /// Relative stacking order within the layout; lower draws first.
    pub z_hint: i32,
}

impl Placement {
    /// A visible placement at `rect`.
    #[must_use]
    pub const fn visible(window: WindowId, rect: Rect) -> Self {
        Self {
            window,
            rect,
            visible: true,
            z_hint: 0,
        }
    }
}

/// A swappable arrangement strategy.
///
/// Implementations must be pure and deterministic: the same [`LayoutInput`]
/// must always yield the same [`Placement`] list.
pub trait Layout: core::fmt::Debug + Send + Sync {
    /// The kind this layout implements.
    fn kind(&self) -> LayoutKind;

    /// Computes placements for every window in `input`.
    fn arrange(&self, input: LayoutInput<'_>) -> Vec<Placement>;

    /// Directional focus: the window to focus when moving in `dir` from `from`.
    ///
    /// The default implementation arranges the layout and picks the nearest
    /// visible window in `dir`, which is right for every tiled layout; layouts
    /// with a better answer may override it.
    fn neighbor(&self, input: &LayoutInput<'_>, from: usize, dir: Direction) -> Option<usize> {
        let mut scratch = ScrollState::default();
        let placements = self.arrange(LayoutInput {
            area: input.area,
            windows: input.windows,
            focused: input.focused,
            params: input.params,
            scroll: &mut scratch,
        });
        directional_neighbor(&placements, from, dir)
    }

    /// A layout-specific command; the default ignores it.
    ///
    /// Commands that need persistent layout state — the scroller's column
    /// widths and centring — are handled by the layout that owns it, which is
    /// where the scroller keeps its columns (`COMPOSITOR.md` §7.1). `focused`
    /// is the window the user is on, so column actions have a target even when
    /// the stored anchor is stale.
    fn command(
        &self,
        _command: LayoutCommand,
        _focused: Option<WindowId>,
        _params: &mut LayoutParams,
        _scroll: &mut ScrollState,
    ) {
    }
}

/// Picks the visible window nearest `from` in `dir`.
///
/// "Nearest" prefers windows roughly along the axis of travel: the perpendicular
/// distance is weighted more heavily than the distance travelled, so moving
/// right from the top of a column lands on the row beside it rather than the
/// nearest corner. Returns `None` when `from` names no visible window or
/// nothing lies in `dir`.
#[must_use]
pub fn directional_neighbor(
    placements: &[Placement],
    from: usize,
    dir: Direction,
) -> Option<usize> {
    let source = placements.get(from)?;
    if !source.visible {
        return None;
    }
    let origin = source.rect.center();

    let mut best: Option<(usize, f64)> = None;
    for (index, candidate) in placements.iter().enumerate() {
        if index == from || !candidate.visible {
            continue;
        }
        let center = candidate.rect.center();
        let (along, across) = match dir {
            Direction::Left => (origin.x - center.x, (origin.y - center.y).abs()),
            Direction::Right => (center.x - origin.x, (origin.y - center.y).abs()),
            Direction::Up => (origin.y - center.y, (origin.x - center.x).abs()),
            Direction::Down => (center.y - origin.y, (origin.x - center.x).abs()),
        };
        if along <= f64::EPSILON {
            continue;
        }
        let score = along + across * 2.0;
        if best.is_none_or(|(_, best_score)| score < best_score) {
            best = Some((index, score));
        }
    }
    best.map(|(index, _)| index)
}

/// Applies a window's size hints to a placement rectangle.
///
/// Hints are soft constraints: a minimum larger than the cell wins, so a
/// client that demands a big window keeps its minimum rather than being
/// clipped (`COMPOSITOR.md` §7.5).
#[must_use]
pub fn apply_hints(rect: Rect, window: &LayoutWindow) -> Rect {
    let mut w = rect.w;
    let mut h = rect.h;
    if let Some(min) = window.min_size {
        w = w.max(min.w);
        h = h.max(min.h);
    }
    if let Some(max) = window.max_size {
        if max.w > 0.0 {
            w = w.min(max.w);
        }
        if max.h > 0.0 {
            h = h.min(max.h);
        }
    }
    Rect::new(rect.x, rect.y, w.max(1.0), h.max(1.0))
}

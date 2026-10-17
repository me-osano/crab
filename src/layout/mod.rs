//! Pure layout engine for crawm.
//!
//! A [`Layout`] is a **pure function** from a [`LayoutInput`] to a list of
//! [`Placement`]s: no globals, no I/O, no Smithay. That is what makes layouts
//! property-testable (ROADMAP §3B, COMPOSITOR.md §7.1).
//!
//! Floating and fullscreen windows are excluded from [`Layout::arrange`] and
//! drawn above tiled windows by the compositor, so layouts never need to know
//! about them (`COMPOSITOR.md` §7.1).
//!
//! M2 ships three layouts — [`Tile`], [`Monocle`], and [`Floating`] — selected
//! through [`LayoutKind`]. The compositor picks one with [`arrange`] /
//! [`neighbor`] rather than holding a trait object per space.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod placement;
mod tile;

pub use placement::{
    Layout, LayoutInput, LayoutWindow, Placement, apply_hints, directional_neighbor,
};
pub use tile::Tile;

use crate::core::{LayoutKind, Point, Rect, Size};

/// Shows only the focused window, filling the area (others are hidden).
#[derive(Clone, Copy, Debug, Default)]
pub struct Monocle;

impl Layout for Monocle {
    fn kind(&self) -> LayoutKind {
        LayoutKind::Monocle
    }

    fn arrange(&self, input: LayoutInput<'_>) -> Vec<Placement> {
        let area = input.usable_area();
        let focus = input.focused.or(if input.windows.is_empty() {
            None
        } else {
            Some(0)
        });
        input
            .windows
            .iter()
            .enumerate()
            .map(|(index, window)| Placement {
                window: window.id,
                // Every window keeps the same rect so switching back to the
                // focused window is instant; only visibility differs.
                rect: apply_hints(area, window),
                visible: Some(index) == focus,
                z_hint: i32::from(Some(index) == focus),
            })
            .collect()
    }
}

/// Places nothing automatically; windows keep their own floating geometry.
#[derive(Clone, Copy, Debug, Default)]
pub struct Floating;

impl Layout for Floating {
    fn kind(&self) -> LayoutKind {
        LayoutKind::Floating
    }

    fn arrange(&self, _input: LayoutInput<'_>) -> Vec<Placement> {
        Vec::new()
    }
}

/// The layout implementing `kind`.
#[must_use]
pub fn layout_for(kind: LayoutKind) -> Box<dyn Layout> {
    match kind {
        LayoutKind::Tile => Box::new(Tile),
        LayoutKind::Monocle => Box::new(Monocle),
        LayoutKind::Floating => Box::new(Floating),
    }
}

/// Convenience: the [`Layout`] for a config `layout` name, if known.
#[must_use]
pub fn by_name(name: &str) -> Option<Box<dyn Layout>> {
    LayoutKind::from_name(name).map(layout_for)
}

/// Computes placements for `kind` — the dispatch the compositor uses.
#[must_use]
pub fn arrange(kind: LayoutKind, input: LayoutInput<'_>) -> Vec<Placement> {
    layout_for(kind).arrange(input)
}

/// Directional focus for `kind`, using the layout's own geometry.
#[must_use]
pub fn neighbor(
    kind: LayoutKind,
    input: &LayoutInput<'_>,
    from: usize,
    direction: crate::core::Direction,
) -> Option<usize> {
    layout_for(kind).neighbor(input, from, direction)
}

/// A cascading starting position for a new floating window.
///
/// Each window is offset by `step` from the last, wrapping back to the top-left
/// once the cascade would leave `area`, so windows never start off-screen.
#[must_use]
pub fn cascade_rect(index: usize, area: Rect, size: Size, step: f64) -> Rect {
    let step = step.max(0.0);
    let x_max = (area.w - size.w).max(0.0);
    let y_max = (area.h - size.h).max(0.0);
    let columns = if step > 0.0 {
        (x_max / step).floor() as usize + 1
    } else {
        1
    };
    let rows = if step > 0.0 {
        (y_max / step).floor() as usize + 1
    } else {
        1
    };
    let column = index % columns.max(1);
    let row = (index / columns.max(1)) % rows.max(1);
    let origin = Point::new(
        area.x + (step * column as f64).min(x_max),
        area.y + (step * row as f64).min(y_max),
    );
    Rect::from_loc_and_size(origin, size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{LayoutParams, ScrollState, WindowId};

    fn input<'a>(
        windows: &'a [LayoutWindow],
        focused: Option<usize>,
        params: &'a LayoutParams,
        scroll: &'a mut ScrollState,
    ) -> LayoutInput<'a> {
        LayoutInput {
            area: Rect::new(0.0, 0.0, 1000.0, 1000.0),
            windows,
            focused,
            params,
            scroll,
        }
    }

    fn sample() -> Vec<LayoutWindow> {
        vec![
            LayoutWindow::new(WindowId::new(1)),
            LayoutWindow::new(WindowId::new(2)),
            LayoutWindow::new(WindowId::new(3)),
        ]
    }

    #[test]
    fn monocle_shows_only_the_focused_window() {
        let windows = sample();
        let params = LayoutParams {
            gaps: 10.0,
            ..Default::default()
        };
        let mut scroll = ScrollState::default();
        let placements = Monocle.arrange(input(&windows, Some(1), &params, &mut scroll));
        let visible: Vec<_> = placements.iter().filter(|p| p.visible).collect();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].window, WindowId::new(2));
        assert_eq!(visible[0].rect, Rect::new(10.0, 10.0, 980.0, 980.0));
    }

    #[test]
    fn monocle_is_deterministic() {
        let windows = sample();
        let params = LayoutParams::default();
        let mut a = ScrollState::default();
        let mut b = ScrollState::default();
        assert_eq!(
            Monocle.arrange(input(&windows, Some(2), &params, &mut a)),
            Monocle.arrange(input(&windows, Some(2), &params, &mut b))
        );
    }

    #[test]
    fn floating_places_nothing() {
        let windows = sample();
        let params = LayoutParams::default();
        let mut scroll = ScrollState::default();
        assert!(
            Floating
                .arrange(input(&windows, None, &params, &mut scroll))
                .is_empty()
        );
    }

    #[test]
    fn by_name_round_trips() {
        assert_eq!(by_name("tile").unwrap().kind(), LayoutKind::Tile);
        assert_eq!(by_name("monocle").unwrap().kind(), LayoutKind::Monocle);
        assert_eq!(by_name("floating").unwrap().kind(), LayoutKind::Floating);
        assert!(by_name("nope").is_none());
    }

    #[test]
    fn cascade_stays_inside_the_area() {
        let area = Rect::new(0.0, 0.0, 800.0, 600.0);
        let size = Size::new(400.0, 300.0);
        for index in 0..64 {
            let rect = cascade_rect(index, area, size, 32.0);
            assert!(
                area.contains_rect(&rect),
                "index {index}: {rect:?} escaped {area:?}"
            );
        }
    }
}

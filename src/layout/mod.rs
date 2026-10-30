//! Pure layout engine for crab.
//!
//! A [`Layout`] is a **pure function** from a [`LayoutInput`] to a list of
//! [`Placement`]s: no globals, no I/O, no Smithay. That is what makes layouts
//! property-testable (ROADMAP §3B, COMPOSITOR.md §7.1).
//!
//! Floating and fullscreen windows are excluded from [`Layout::arrange`] and
//! drawn above tiled windows by the compositor, so layouts never need to know
//! about them (`COMPOSITOR.md` §7.1).
//!
//! Crab ships exactly three modes: [`Fair`] (`tile(fair)`), [`Floating`]
//! (`stack(floating)`), and [`Scroll`] (`scroll(scroll)`). The compositor picks
//! one through [`LayoutKind`] with [`arrange`] / [`neighbor`] /
//! [`apply_command`] rather than holding a trait object per space.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod fair;
mod placement;
mod scroll;

pub use fair::Fair;
pub use placement::{
    Layout, LayoutInput, LayoutWindow, Placement, apply_hints, directional_neighbor,
};
pub use scroll::Scroll;

use crate::core::{
    Direction, LayoutCommand, LayoutKind, LayoutParams, Point, Rect, ScrollAxis, ScrollState, Size,
};

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
        LayoutKind::Fair => Box::new(Fair),
        LayoutKind::Floating => Box::new(Floating),
        LayoutKind::Scroll => Box::new(Scroll::new(ScrollAxis::Horizontal)),
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
    direction: Direction,
) -> Option<usize> {
    layout_for(kind).neighbor(input, from, direction)
}

/// Applies a [`LayoutCommand`] to `kind`'s parameters and persistent state.
///
/// The compositor calls this rather than [`LayoutCommand::apply`] directly, so
/// commands that a family owns itself — the scroller's width and centring,
/// which need its columns — reach the layout that understands them
/// (`COMPOSITOR.md` §7.1).
pub fn apply_command(
    kind: LayoutKind,
    command: LayoutCommand,
    params: &mut LayoutParams,
    scroll: &mut ScrollState,
) {
    layout_for(kind).command(command, params, scroll);
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
        assert_eq!(by_name("fair").unwrap().kind(), LayoutKind::Fair);
        assert_eq!(by_name("tile").unwrap().kind(), LayoutKind::Fair);
        assert_eq!(by_name("floating").unwrap().kind(), LayoutKind::Floating);
        assert_eq!(by_name("stack").unwrap().kind(), LayoutKind::Floating);
        assert_eq!(by_name("scroll").unwrap().kind(), LayoutKind::Scroll);
        assert!(by_name("monocle").is_none());
        assert!(by_name("vertical-scroll").is_none());
        assert!(by_name("nope").is_none());
    }

    #[test]
    fn every_kind_has_a_layout() {
        for kind in LayoutKind::ALL {
            assert_eq!(layout_for(kind).kind(), kind);
        }
    }

    #[test]
    fn apply_command_reaches_the_layout_that_owns_it() {
        let mut params = LayoutParams::default();
        let mut scroll = ScrollState::default();
        apply_command(
            LayoutKind::Scroll,
            LayoutCommand::CenterColumn,
            &mut params,
            &mut scroll,
        );
        assert!(
            scroll.center_request,
            "the scroller handled its own command"
        );

        // The tiling layout has no persistent state of its own to touch.
        let before = params;
        apply_command(
            LayoutKind::Fair,
            LayoutCommand::CenterColumn,
            &mut params,
            &mut scroll,
        );
        assert_eq!(params, before);
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

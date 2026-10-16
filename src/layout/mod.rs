//! Pure layout engine for crawm.
//!
//! A [`Layout`] is a **pure function** from a [`LayoutInput`] to a list of
//! [`Placement`]s: no globals, no I/O, no Smithay. That is what makes layouts
//! property-testable (ROADMAP §3B, COMPOSITOR.md §7.1).
//!
//! Floating windows are excluded from [`Layout::arrange`] and drawn above tiled
//! windows by the compositor, so layouts never need to know about them.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod placement;

pub use placement::{Layout, LayoutInput, Placement};

use crawm_core::WindowId;

/// Shows only the focused window, filling the area (others are hidden).
#[derive(Clone, Copy, Debug, Default)]
pub struct Monocle;

impl Layout for Monocle {
    fn name(&self) -> &'static str {
        "monocle"
    }

    fn arrange(&self, input: &LayoutInput) -> Vec<Placement> {
        let area = input.usable_area();
        let focus = input.focused.or_else(|| input.windows.first().copied());
        input
            .windows
            .iter()
            .map(|&window| Placement {
                window,
                rect: area,
                visible: Some(window) == focus,
            })
            .collect()
    }
}

/// Places nothing automatically; windows keep their own floating geometry.
#[derive(Clone, Copy, Debug, Default)]
pub struct Floating;

impl Layout for Floating {
    fn name(&self) -> &'static str {
        "floating"
    }

    fn arrange(&self, _input: &LayoutInput) -> Vec<Placement> {
        Vec::new()
    }
}

/// Convenience: the [`Layout`] for a config `layout` name, if known.
#[must_use]
pub fn by_name(name: &str) -> Option<Box<dyn Layout>> {
    match name {
        "monocle" => Some(Box::new(Monocle)),
        "floating" => Some(Box::new(Floating)),
        _ => None,
    }
}

/// Used by tests and the compositor to render a window that has no placement.
#[must_use]
pub fn hidden(window: WindowId, rect: crawm_core::Rect) -> Placement {
    Placement {
        window,
        rect,
        visible: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crawm_core::{Point, Rect, Size};

    fn input() -> LayoutInput {
        LayoutInput {
            area: Rect::new(0.0, 0.0, 1000.0, 1000.0),
            gaps: 10.0,
            windows: vec![WindowId::new(1), WindowId::new(2), WindowId::new(3)],
            focused: Some(WindowId::new(2)),
        }
    }

    #[test]
    fn monocle_shows_only_the_focused_window() {
        let placements = Monocle.arrange(&input());
        let visible: Vec<_> = placements.iter().filter(|p| p.visible).collect();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].window, WindowId::new(2));
        // The focused rect is inset by the gaps.
        assert_eq!(visible[0].rect, Rect::new(10.0, 10.0, 980.0, 980.0));
    }

    #[test]
    fn monocle_is_deterministic() {
        assert_eq!(Monocle.arrange(&input()), Monocle.arrange(&input()));
    }

    #[test]
    fn floating_places_nothing() {
        assert!(Floating.arrange(&input()).is_empty());
    }

    #[test]
    fn by_name_round_trips() {
        assert_eq!(by_name("monocle").unwrap().name(), "monocle");
        assert_eq!(by_name("floating").unwrap().name(), "floating");
        assert!(by_name("nope").is_none());
    }

    #[test]
    fn unused_point_size_constructors_compile() {
        // Keep the shared geometry types exercised from this crate.
        let _ = Point::new(0.0, 0.0);
        let _ = Size::new(1.0, 1.0);
    }
}

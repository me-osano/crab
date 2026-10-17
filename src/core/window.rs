//! The window model (`COMPOSITOR.md` §6).
//!
//! A [`Window`] is the pure, Smithay-free description of a managed toplevel:
//! identity, space membership, tiling state, size hints, and the result of
//! matching window rules. The compositor keeps one of these alongside the
//! Smithay `ToplevelSurface` that owns the actual protocol object.

use crate::core::{Rect, Size, SpaceSet, WindowId};

/// The outcome of matching window rules, remembered on the window so dynamic
/// re-evaluation (M5) can tell what a rule already decided.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AppliedRules {
    /// Forced floating state, if a rule set one.
    pub floating: Option<bool>,
    /// Forced space assignment, if a rule set one.
    pub spaces: Option<SpaceSet>,
    /// Whether a rule pinned the window to all spaces.
    pub pinned: bool,
    /// Whether a rule keeps the window above others.
    pub always_on_top: bool,
    /// Whether a rule excludes the window from focus.
    pub no_focus: bool,
}

impl AppliedRules {
    /// Whether any rule matched this window.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.floating.is_none()
            && self.spaces.is_none()
            && !self.pinned
            && !self.always_on_top
            && !self.no_focus
    }
}

/// A managed toplevel, in logical coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    /// Stable handle allocated by the compositor.
    pub id: WindowId,
    /// `xdg_toplevel` app id, once the client sets it.
    pub app_id: Option<String>,
    /// `xdg_toplevel` title, once the client sets it.
    pub title: Option<String>,
    /// Spaces the window belongs to (may be several).
    pub spaces: SpaceSet,
    /// Per-window floating override.
    pub floating: bool,
    /// Fullscreen state; a fullscreen window leaves the layout.
    pub fullscreen: bool,
    /// Maximized state (fills the usable area without leaving the space).
    pub maximized: bool,
    /// Set when a client requests attention.
    pub urgent: bool,
    /// Pinned to every space, whatever `spaces` says (`COMPOSITOR.md` §7.3).
    pub pinned: bool,
    /// Drawn above other floating windows.
    pub always_on_top: bool,
    /// Remembered geometry while floating.
    pub float_geometry: Option<Rect>,
    /// Client-requested minimum size.
    pub min_size: Option<Size>,
    /// Client-requested maximum size (a zero axis means unconstrained).
    pub max_size: Option<Size>,
    /// Rules that matched at map time.
    pub rules: AppliedRules,
}

impl Window {
    /// Creates a tiled window on `spaces`.
    #[must_use]
    pub fn new(id: WindowId, spaces: SpaceSet) -> Self {
        Self {
            id,
            app_id: None,
            title: None,
            spaces,
            floating: false,
            fullscreen: false,
            maximized: false,
            urgent: false,
            pinned: false,
            always_on_top: false,
            float_geometry: None,
            min_size: None,
            max_size: None,
            rules: AppliedRules::default(),
        }
    }

    /// Whether the window participates in a space's layout.
    ///
    /// Floating and fullscreen windows are excluded from [`crate::layout`]
    /// input and handled by the compositor (`COMPOSITOR.md` §7.1, §7.5).
    #[must_use]
    pub const fn is_tiled(&self) -> bool {
        !self.floating && !self.fullscreen
    }

    /// Whether the window should be shown on an output currently viewing
    /// `view`. Pinned windows show on every view.
    #[must_use]
    pub fn is_on(&self, view: SpaceSet) -> bool {
        self.pinned || !(self.spaces & view).is_empty()
    }

    /// Turns floating on or off, remembering `current` as the float geometry
    /// the first time the window floats.
    ///
    /// Returns the new floating state.
    pub fn set_floating(&mut self, floating: bool, current: Rect) -> bool {
        if floating {
            self.float_geometry.get_or_insert(current);
        }
        self.floating = floating;
        floating
    }

    /// Flips floating, returning the new state.
    pub fn toggle_floating(&mut self, current: Rect) -> bool {
        self.set_floating(!self.floating, current)
    }

    /// Reassigns the window to exactly `spaces`, ignoring an empty set.
    pub fn set_spaces(&mut self, spaces: SpaceSet) {
        if !spaces.is_empty() {
            self.spaces = spaces;
        }
    }

    /// Moves the window to exactly `spaces` (an alias that reads better at call
    /// sites).
    pub fn move_to_space(&mut self, spaces: SpaceSet) {
        self.set_spaces(spaces);
    }

    /// Adds `space` if absent, removes it if present — but never leaves the
    /// window untagged.
    pub fn toggle_space(&mut self, space: SpaceSet) {
        let next = self.spaces ^ space;
        if !next.is_empty() {
            self.spaces = next;
        }
    }

    /// The window's size clamped to its own min/max hints (logical pixels).
    #[must_use]
    pub fn clamp_size(&self, size: Size) -> Size {
        let mut w = size.w;
        let mut h = size.h;
        if let Some(min) = self.min_size {
            w = w.max(min.w);
            h = h.max(min.h);
        }
        if let Some(max) = self.max_size {
            if max.w > 0.0 {
                w = w.min(max.w);
            }
            if max.h > 0.0 {
                h = h.min(max.h);
            }
        }
        Size::new(w.max(1.0), h.max(1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> Window {
        Window::new(WindowId::new(1), SpaceSet::SPACE_1)
    }

    #[test]
    fn tiled_excludes_floating_and_fullscreen() {
        let mut w = window();
        assert!(w.is_tiled());
        w.floating = true;
        assert!(!w.is_tiled());
        w.floating = false;
        w.fullscreen = true;
        assert!(!w.is_tiled());
    }

    #[test]
    fn visibility_respects_spaces_and_pinning() {
        let mut w = window();
        assert!(w.is_on(SpaceSet::SPACE_1));
        assert!(!w.is_on(SpaceSet::SPACE_2));
        w.pinned = true;
        assert!(w.is_on(SpaceSet::SPACE_2));
    }

    #[test]
    fn toggle_space_never_empties() {
        let mut w = window();
        w.toggle_space(SpaceSet::SPACE_2);
        assert_eq!(w.spaces, SpaceSet::SPACE_1 | SpaceSet::SPACE_2);
        w.toggle_space(SpaceSet::SPACE_1);
        assert_eq!(w.spaces, SpaceSet::SPACE_2);
        w.toggle_space(SpaceSet::SPACE_2);
        assert_eq!(w.spaces, SpaceSet::SPACE_2, "the last space is kept");
    }

    #[test]
    fn float_geometry_is_remembered() {
        let mut w = window();
        let rect = Rect::new(10.0, 20.0, 100.0, 50.0);
        assert!(w.toggle_floating(rect));
        assert_eq!(w.float_geometry, Some(rect));
        assert!(!w.toggle_floating(Rect::new(0.0, 0.0, 1.0, 1.0)));
        assert_eq!(
            w.float_geometry,
            Some(rect),
            "geometry survives un-floating"
        );
    }

    #[test]
    fn clamp_size_respects_hints() {
        let mut w = window();
        w.min_size = Some(Size::new(100.0, 80.0));
        w.max_size = Some(Size::new(400.0, 0.0)); // height unconstrained
        assert_eq!(w.clamp_size(Size::new(50.0, 40.0)), Size::new(100.0, 80.0));
        assert_eq!(
            w.clamp_size(Size::new(900.0, 700.0)),
            Size::new(400.0, 700.0)
        );
    }
}

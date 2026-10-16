//! The `Layout` trait and the types it consumes and produces.

use crate::core::{Rect, WindowId};

/// The inputs a layout needs to arrange a tag's windows.
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutInput {
    /// Full area the layout may use, before gaps are applied.
    pub area: Rect,
    /// Inner/outer gap size in logical pixels.
    pub gaps: f64,
    /// Tiled windows on this tag, in their current order.
    pub windows: Vec<WindowId>,
    /// The focused window, if any.
    pub focused: Option<WindowId>,
}

impl LayoutInput {
    /// The area available to windows after the outer gaps are removed.
    #[must_use]
    pub fn usable_area(&self) -> Rect {
        self.area.deflated(self.gaps)
    }
}

/// Where a layout wants a window to be drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// The window this placement is for.
    pub window: WindowId,
    /// The target rectangle in logical coordinates.
    pub rect: Rect,
    /// Whether the window should be shown at all (hidden windows keep a rect for
    /// animation continuity).
    pub visible: bool,
}

/// A swappable arrangement strategy.
///
/// Implementations must be pure and deterministic: the same [`LayoutInput`] must
/// always yield the same [`Placement`] list.
pub trait Layout: core::fmt::Debug + Send + Sync {
    /// A stable, lowercase identifier (used by config and IPC).
    fn name(&self) -> &'static str;

    /// Computes placements for every window in `input`.
    fn arrange(&self, input: &LayoutInput) -> Vec<Placement>;
}

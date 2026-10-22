//! Focus targets and the per-output MRU stack (`COMPOSITOR.md` §6).

use crate::core::{LayerId, OutputId, WindowId};

/// What currently holds keyboard focus.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FocusTarget {
    /// A managed toplevel.
    Window(WindowId),
    /// A keyboard-interactive layer surface (a launcher, M6).
    LayerSurface(LayerId),
    /// A session-lock surface (M6).
    LockSurface(OutputId),
    /// Nothing is focused.
    #[default]
    None,
}

impl FocusTarget {
    /// The window id, if this targets a window.
    #[must_use]
    pub const fn window(self) -> Option<WindowId> {
        match self {
            Self::Window(id) => Some(id),
            _ => None,
        }
    }
}

/// A most-recently-used stack of focus targets, newest first.
///
/// The head is the current focus; walking the stack gives the "previous focus"
/// used by `focus-next`/`focus-previous` after a window closes.
#[derive(Clone, Debug, Default)]
pub struct FocusStack {
    order: Vec<FocusTarget>,
}

impl FocusStack {
    /// Creates an empty stack.
    #[must_use]
    pub const fn new() -> Self {
        Self { order: Vec::new() }
    }

    /// Moves `target` to the head (deduplicating).
    pub fn focus(&mut self, target: FocusTarget) {
        self.order.retain(|other| *other != target);
        self.order.insert(0, target);
    }

    /// Removes `target` from the stack (e.g. when its window closes).
    pub fn remove(&mut self, target: FocusTarget) {
        self.order.retain(|other| *other != target);
    }

    /// The current focus (the head), if any.
    #[must_use]
    pub fn current(&self) -> FocusTarget {
        self.order.first().copied().unwrap_or(FocusTarget::None)
    }

    /// Iterates the stack from most to least recent.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = FocusTarget> + '_ {
        self.order.iter().copied()
    }

    /// The most-recent target matching `predicate`, skipping the current head.
    ///
    /// Useful after a window closes: drop the dead one, then take the next
    /// target that still exists.
    #[must_use]
    pub fn previous_where(&self, predicate: impl Fn(FocusTarget) -> bool) -> Option<FocusTarget> {
        self.order
            .iter()
            .copied()
            .skip(1)
            .find(|target| predicate(*target))
    }

    /// Whether the stack is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// The number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.order.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_moves_to_head_and_dedupes() {
        let a = FocusTarget::Window(WindowId::new(1));
        let b = FocusTarget::Window(WindowId::new(2));
        let mut stack = FocusStack::new();
        stack.focus(a);
        stack.focus(b);
        assert_eq!(stack.current(), b);
        stack.focus(a);
        assert_eq!(stack.current(), a);
        assert_eq!(stack.len(), 2, "no duplicate entries");
    }

    #[test]
    fn removing_a_window_falls_back() {
        let a = FocusTarget::Window(WindowId::new(1));
        let b = FocusTarget::Window(WindowId::new(2));
        let mut stack = FocusStack::new();
        stack.focus(a);
        stack.focus(b);
        stack.remove(b);
        assert_eq!(stack.current(), a);
        stack.remove(a);
        assert_eq!(stack.current(), FocusTarget::None);
        assert!(stack.is_empty());
    }

    #[test]
    fn previous_where_skips_unrelated_targets() {
        let lock = FocusTarget::LockSurface(OutputId::new(0));
        let a = FocusTarget::Window(WindowId::new(1));
        let mut stack = FocusStack::new();
        stack.focus(a);
        stack.focus(lock);
        assert_eq!(stack.previous_where(|t| t.window().is_some()), Some(a));
    }
}

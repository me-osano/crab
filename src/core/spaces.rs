//! Space state: the dwm/mango model of numbered spaces (1..=32) per output.

use bitflags::bitflags;

bitflags! {
    /// A set of spaces attached to a window or currently shown on an output.
    ///
    /// crawm exposes 32 spaces per output, numbered `1..=32`. The first nine have
    /// named constants for convenience; all 32 are reachable through
    /// [`SpaceSet::from_index`].
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
    pub struct SpaceSet: u32 {
        /// Space 1.
        const SPACE_1 = 1 << 0;
        /// Space 2.
        const SPACE_2 = 1 << 1;
        /// Space 3.
        const SPACE_3 = 1 << 2;
        /// Space 4.
        const SPACE_4 = 1 << 3;
        /// Space 5.
        const SPACE_5 = 1 << 4;
        /// Space 6.
        const SPACE_6 = 1 << 5;
        /// Space 7.
        const SPACE_7 = 1 << 6;
        /// Space 8.
        const SPACE_8 = 1 << 7;
        /// Space 9.
        const SPACE_9 = 1 << 8;
    }
}

impl SpaceSet {
    /// Number of spaces per output.
    pub const COUNT: u8 = 32;

    /// Returns the mask for a 1-based space index, or `None` if out of range.
    #[must_use]
    pub const fn from_index(index: u8) -> Option<Self> {
        if index >= 1 && index <= Self::COUNT {
            // `from_bits_retain` keeps bits for spaces 10..=32, which have no named
            // constant; `from_bits_truncate` would strip them.
            Some(Self::from_bits_retain(1u32 << (index - 1)))
        } else {
            None
        }
    }

    /// Iterates the 1-based indices of every set space, ascending.
    pub fn iter_indices(self) -> impl Iterator<Item = u8> {
        (1..=Self::COUNT).filter(move |&i| Self::from_index(i).is_some_and(|m| self.contains(m)))
    }
}

/// The spaces visible on an output, plus the previous set for `toggle-back`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SpaceView {
    /// Spaces currently shown.
    pub visible: SpaceSet,
    /// Spaces shown before the most recent switch (for `toggle-back`).
    pub previous: SpaceSet,
}

impl SpaceView {
    /// Creates a view showing `visible` with no history.
    #[must_use]
    pub const fn new(visible: SpaceSet) -> Self {
        Self {
            visible,
            previous: SpaceSet::empty(),
        }
    }

    /// Switches the visible set, remembering the old one for `toggle-back`.
    pub fn view(&mut self, next: SpaceSet) {
        self.previous = self.visible;
        self.visible = next;
    }

    /// Swaps `visible` and `previous` (the `toggle-back` action).
    pub fn toggle_back(&mut self) {
        core::mem::swap(&mut self.visible, &mut self.previous);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_is_one_based() {
        assert_eq!(SpaceSet::from_index(1), Some(SpaceSet::SPACE_1));
        assert_eq!(SpaceSet::from_index(9), Some(SpaceSet::SPACE_9));
        assert_eq!(SpaceSet::from_index(10).unwrap().bits(), 1 << 9);
        assert_eq!(SpaceSet::from_index(0), None);
        assert_eq!(SpaceSet::from_index(33), None);
    }

    #[test]
    fn iterates_set_spaces() {
        let mask = SpaceSet::SPACE_1 | SpaceSet::SPACE_3 | SpaceSet::from_index(20).unwrap();
        assert_eq!(mask.iter_indices().collect::<Vec<_>>(), vec![1, 3, 20]);
    }

    #[test]
    fn toggle_back_round_trips() {
        let mut view = SpaceView::new(SpaceSet::SPACE_1);
        view.view(SpaceSet::SPACE_2);
        assert_eq!(view.visible, SpaceSet::SPACE_2);
        assert_eq!(view.previous, SpaceSet::SPACE_1);
        view.toggle_back();
        assert_eq!(view.visible, SpaceSet::SPACE_1);
        assert_eq!(view.previous, SpaceSet::SPACE_2);
    }
}

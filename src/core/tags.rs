//! Tag state: the dwm/mango model of numbered tags (1..=32) per output.

use bitflags::bitflags;

bitflags! {
    /// A set of tags attached to a window or currently shown on an output.
    ///
    /// crawm exposes 32 tags per output, numbered `1..=32`. The first nine have
    /// named constants for convenience; all 32 are reachable through
    /// [`TagMask::from_index`].
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
    pub struct TagMask: u32 {
        /// Tag 1.
        const TAG_1 = 1 << 0;
        /// Tag 2.
        const TAG_2 = 1 << 1;
        /// Tag 3.
        const TAG_3 = 1 << 2;
        /// Tag 4.
        const TAG_4 = 1 << 3;
        /// Tag 5.
        const TAG_5 = 1 << 4;
        /// Tag 6.
        const TAG_6 = 1 << 5;
        /// Tag 7.
        const TAG_7 = 1 << 6;
        /// Tag 8.
        const TAG_8 = 1 << 7;
        /// Tag 9.
        const TAG_9 = 1 << 8;
    }
}

impl TagMask {
    /// Number of tags per output.
    pub const COUNT: u8 = 32;

    /// Returns the mask for a 1-based tag index, or `None` if out of range.
    #[must_use]
    pub const fn from_index(index: u8) -> Option<Self> {
        if index >= 1 && index <= Self::COUNT {
            // `from_bits_retain` keeps bits for tags 10..=32, which have no named
            // constant; `from_bits_truncate` would strip them.
            Some(Self::from_bits_retain(1u32 << (index - 1)))
        } else {
            None
        }
    }

    /// Iterates the 1-based indices of every set tag, ascending.
    pub fn iter_indices(self) -> impl Iterator<Item = u8> {
        (1..=Self::COUNT).filter(move |&i| Self::from_index(i).is_some_and(|m| self.contains(m)))
    }
}

/// The tags visible on an output, plus the previous set for `toggle-back`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TagView {
    /// Tags currently shown.
    pub visible: TagMask,
    /// Tags shown before the most recent switch (for `toggle-back`).
    pub previous: TagMask,
}

impl TagView {
    /// Creates a view showing `visible` with no history.
    #[must_use]
    pub const fn new(visible: TagMask) -> Self {
        Self {
            visible,
            previous: TagMask::empty(),
        }
    }

    /// Switches the visible set, remembering the old one for `toggle-back`.
    pub fn view(&mut self, next: TagMask) {
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
        assert_eq!(TagMask::from_index(1), Some(TagMask::TAG_1));
        assert_eq!(TagMask::from_index(9), Some(TagMask::TAG_9));
        assert_eq!(TagMask::from_index(10).unwrap().bits(), 1 << 9);
        assert_eq!(TagMask::from_index(0), None);
        assert_eq!(TagMask::from_index(33), None);
    }

    #[test]
    fn iterates_set_tags() {
        let mask = TagMask::TAG_1 | TagMask::TAG_3 | TagMask::from_index(20).unwrap();
        assert_eq!(mask.iter_indices().collect::<Vec<_>>(), vec![1, 3, 20]);
    }

    #[test]
    fn toggle_back_round_trips() {
        let mut view = TagView::new(TagMask::TAG_1);
        view.view(TagMask::TAG_2);
        assert_eq!(view.visible, TagMask::TAG_2);
        assert_eq!(view.previous, TagMask::TAG_1);
        view.toggle_back();
        assert_eq!(view.visible, TagMask::TAG_1);
        assert_eq!(view.previous, TagMask::TAG_2);
    }
}

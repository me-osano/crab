//! Space state: the dwm/mango model of numbered spaces (1..=32) per output.

use bitflags::bitflags;

bitflags! {
    /// A set of spaces attached to a window or currently shown on an output.
    ///
    /// crab exposes 32 spaces per output, numbered `1..=32`. The first nine have
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

    /// Resolves a 1-based dynamic index to a space slot.
    ///
    /// `occupied` is the set of spaces that hold a window; after compaction its
    /// members are dense from 1, so index `n` names the `n`th occupied space, and
    /// any index past the last one names the empty space right after them
    /// (`n + 1`). Index `0` and a request for a 33rd space are out of range.
    #[must_use]
    pub fn from_dynamic_index(index: u8, occupied: Self) -> Option<Self> {
        let count = occupied.iter_indices().count() as u8;
        Self::from_index(index.min(count.saturating_add(1)))
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

/// The dense renumbering of spaces used for niri-style dynamic spaces.
///
/// A space exists while it holds a window or is the empty space past the last
/// occupied one. Renumbering keeps the relative order of the spaces that exist
/// and assigns them `1..=n`, so a space that loses its last window disappears and
/// every later space moves down. The view follows its content, so it never points
/// at a hole.
///
/// The fixed 32-slot [`SpaceSet`] is unchanged (ADR-0004); this is a policy on
/// top of it. An operation that can empty a space asks an
/// [`Output`](crate::core::Output) to
/// [`compact`](crate::core::Output::compact) itself and uses the returned remap
/// to renumber window membership.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpaceRemap {
    /// `slots[old]` is the new index, or `0` when the space disappeared.
    slots: [u8; 33],
}

impl SpaceRemap {
    /// Computes the remap for the current `occupied` spaces and `view`.
    ///
    /// Every occupied space is kept, plus any visible space past the last
    /// occupied one (where a view past the end lands on the empty space). A
    /// visible space that was emptied while later spaces still hold windows is
    /// removed; its view follows to the space that moves into its place, as in
    /// niri.
    ///
    /// `occupied` is the union of every window's space membership.
    #[must_use]
    pub fn new(occupied: SpaceSet, view: SpaceView) -> Self {
        let last_occupied = occupied.iter_indices().max().unwrap_or(0);
        let scratch = view
            .visible
            .iter_indices()
            .filter(|&index| index > last_occupied)
            .filter_map(SpaceSet::from_index)
            .fold(SpaceSet::empty(), |spaces, space| spaces | space);
        let kept = occupied | scratch;
        let mut slots = [0u8; 33];
        for (next, old) in (1u8..).zip(kept.iter_indices()) {
            slots[usize::from(old)] = next;
        }
        Self { slots }
    }

    /// The new 1-based index for `old`, or `0` if that space no longer exists.
    #[must_use]
    pub fn new_index(self, old: u8) -> u8 {
        self.slots.get(usize::from(old)).copied().unwrap_or(0)
    }

    /// The index a view should show in place of `old`.
    ///
    /// A surviving space keeps its new index. A removed one hands the view to
    /// the next surviving space, because that space moves into the empty slot;
    /// with nothing after it, the view falls back to the previous survivor.
    #[must_use]
    pub fn view_index(self, old: u8) -> u8 {
        let direct = self.new_index(old);
        if direct != 0 {
            return direct;
        }
        ((old.saturating_add(1))..=SpaceSet::COUNT)
            .find(|&next| self.new_index(next) != 0)
            .map_or_else(
                || {
                    (1..old)
                        .rev()
                        .map(|previous| self.new_index(previous))
                        .find(|&new| new != 0)
                        .unwrap_or(0)
                },
                |next| self.new_index(next),
            )
    }

    /// Renumbers a mask, dropping spaces that no longer exist.
    #[must_use]
    pub fn map_mask(self, mask: SpaceSet) -> SpaceSet {
        mask.iter_indices()
            .filter_map(|old| SpaceSet::from_index(self.new_index(old)))
            .fold(SpaceSet::empty(), |spaces, space| spaces | space)
    }

    /// Renumbers the visible and previous sets in place.
    ///
    /// A visible space that was removed hands the view to the next surviving
    /// space (see [`SpaceRemap::view_index`]), so closing the last window of the
    /// focused space shows the space that moved up. A previous set that named a
    /// removed space becomes empty, so `toggle-back` does nothing rather than
    /// jumping to a space that is gone.
    pub fn map_view(self, view: &mut SpaceView) {
        view.visible = view
            .visible
            .iter_indices()
            .filter_map(|old| SpaceSet::from_index(self.view_index(old)))
            .fold(SpaceSet::empty(), |spaces, space| spaces | space);
        view.previous = self.map_mask(view.previous);
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

    #[test]
    fn dynamic_indices_name_dense_spaces_then_the_next_empty_one() {
        let occupied = SpaceSet::SPACE_1 | SpaceSet::SPACE_2;
        assert_eq!(
            SpaceSet::from_dynamic_index(1, occupied),
            Some(SpaceSet::SPACE_1)
        );
        assert_eq!(
            SpaceSet::from_dynamic_index(2, occupied),
            Some(SpaceSet::SPACE_2)
        );
        assert_eq!(
            SpaceSet::from_dynamic_index(5, occupied),
            Some(SpaceSet::SPACE_3),
            "past the end means the empty space after it"
        );
        assert_eq!(
            SpaceSet::from_dynamic_index(9, SpaceSet::empty()),
            Some(SpaceSet::SPACE_1)
        );
        assert_eq!(SpaceSet::from_dynamic_index(0, occupied), None);
    }

    #[test]
    fn dynamic_indices_stop_at_the_last_slot() {
        let full = (1..=SpaceSet::COUNT).fold(SpaceSet::empty(), |spaces, index| {
            spaces | SpaceSet::from_index(index).unwrap()
        });
        assert_eq!(
            SpaceSet::from_dynamic_index(SpaceSet::COUNT, full),
            SpaceSet::from_index(SpaceSet::COUNT)
        );
        assert_eq!(
            SpaceSet::from_dynamic_index(SpaceSet::COUNT + 1, full),
            None
        );
    }

    #[test]
    fn remap_hands_an_emptied_focused_space_to_the_next_one() {
        let occupied = SpaceSet::SPACE_1 | SpaceSet::SPACE_3;
        let mut view = SpaceView::new(SpaceSet::SPACE_2);
        let remap = SpaceRemap::new(occupied, view);
        assert_eq!(remap.new_index(2), 0);
        assert_eq!(remap.new_index(3), 2);
        remap.map_view(&mut view);
        assert_eq!(
            view.visible,
            SpaceSet::SPACE_2,
            "the view shows the space that moved up"
        );
    }

    #[test]
    fn remap_keeps_visible_spaces_past_the_last_occupied_one() {
        let mut view = SpaceView::new(SpaceSet::SPACE_1 | SpaceSet::SPACE_4);
        let remap = SpaceRemap::new(SpaceSet::SPACE_1, view);
        assert_eq!(remap.new_index(4), 2);
        remap.map_view(&mut view);
        assert_eq!(view.visible, SpaceSet::SPACE_1 | SpaceSet::SPACE_2);
    }

    #[test]
    fn remap_closes_the_gap_left_by_an_empty_space() {
        let remap = SpaceRemap::new(
            SpaceSet::SPACE_1 | SpaceSet::SPACE_3 | SpaceSet::SPACE_4,
            SpaceView::new(SpaceSet::SPACE_4),
        );
        assert_eq!(remap.new_index(1), 1);
        assert_eq!(remap.new_index(2), 0, "space 2 holds nothing");
        assert_eq!(remap.new_index(3), 2);
        assert_eq!(remap.new_index(4), 3);
        assert_eq!(
            remap.map_mask(SpaceSet::SPACE_3 | SpaceSet::SPACE_4),
            SpaceSet::SPACE_2 | SpaceSet::SPACE_3
        );
    }

    #[test]
    fn remap_keeps_the_view_on_screen() {
        let mut view = SpaceView::new(SpaceSet::SPACE_4);
        view.previous = SpaceSet::SPACE_3;
        let remap = SpaceRemap::new(SpaceSet::SPACE_1, view);
        remap.map_view(&mut view);
        assert_eq!(view.visible, SpaceSet::SPACE_2);
        assert_eq!(
            view.previous,
            SpaceSet::empty(),
            "the previous space disappeared"
        );
    }

    #[test]
    fn remap_drops_a_mask_whose_space_is_gone() {
        let remap = SpaceRemap::new(SpaceSet::empty(), SpaceView::default());
        assert_eq!(remap.map_mask(SpaceSet::SPACE_9), SpaceSet::empty());
    }

    #[test]
    fn remap_of_dense_spaces_is_the_identity() {
        let occupied = SpaceSet::SPACE_1 | SpaceSet::SPACE_2;
        let remap = SpaceRemap::new(occupied, SpaceView::new(SpaceSet::SPACE_2));
        for index in 1..=2 {
            assert_eq!(remap.new_index(index), index);
        }
        assert_eq!(remap.new_index(3), 0);
    }

    #[test]
    fn remap_collapses_an_empty_desktop_to_space_one() {
        let mut view = SpaceView::new(SpaceSet::SPACE_5);
        SpaceRemap::new(SpaceSet::empty(), view).map_view(&mut view);
        assert_eq!(view.visible, SpaceSet::SPACE_1);
    }
}

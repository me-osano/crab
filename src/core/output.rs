//! The output and space model (`COMPOSITOR.md` §6).
//!
//! crawm uses **spaces per output** (ADR-0004): each monitor has its own space
//! view, and a space remembers its own layout and parameters (`Pertag`). When
//! several spaces are visible at once, the layout of the **lowest-numbered
//! visible space** arranges the combined window set — the dwm rule documented on
//! [`Output::current_space_index`].

use crate::core::OutputId;
use crate::core::{LayoutKind, LayoutParams, Rect, ScrollState, SpaceSet, SpaceView, WindowId};

/// Per-space layout state, remembered for the lifetime of the output.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpaceState {
    /// The arrangement strategy for this space.
    pub layout: LayoutKind,
    /// Its tunables (master ratio/count, gaps, …).
    pub params: LayoutParams,
    /// Persistent state for scrolling layouts (`COMPOSITOR.md` §7.7).
    pub scroll: ScrollState,
    /// Most-recently-focused windows on this space, newest first.
    pub focus_history: Vec<WindowId>,
}

impl SpaceState {
    /// Creates space state with the given layout and parameters.
    #[must_use]
    pub fn new(layout: LayoutKind, params: LayoutParams) -> Self {
        Self {
            layout,
            params,
            ..Self::default()
        }
    }

    /// Records `id` as newly focused on this space (most recent first).
    pub fn note_focus(&mut self, id: WindowId) {
        self.focus_history.retain(|&other| other != id);
        self.focus_history.insert(0, id);
    }

    /// Forgets a closed window.
    pub fn forget(&mut self, id: WindowId) {
        self.focus_history.retain(|&other| other != id);
    }
}

/// Number of spaces per output, as a `usize` for array indexing.
const SPACE_SLOTS: usize = SpaceSet::COUNT as usize;

/// The per-output redraw state machine (`COMPOSITOR.md` §8.2).
///
/// The compositor draws only when something asked it to, and never starts a
/// frame while the previous one is still being presented:
///
/// ```text
/// Idle ──queue_redraw──► Queued ──begin_frame──► WaitingForVBlank ──finish_frame──► Idle
///                            ▲                              │
///                            └──────── queue_redraw ────────┘
/// ```
///
/// Backends own the scheduling (a `calloop` timer, winit's `Redraw` event, or a
/// DRM vblank) and the pure model owns the transitions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RedrawState {
    /// Nothing to draw.
    #[default]
    Idle,
    /// A frame was requested and has not started yet.
    Queued,
    /// A frame is being presented; a request now schedules the next one.
    WaitingForVBlank,
}

/// One monitor: geometry plus the space state it owns.
#[derive(Clone, Debug, PartialEq)]
pub struct Output {
    /// Stable handle allocated by the compositor.
    pub id: OutputId,
    /// Connector name (`DP-1`, or `crawm-0` for the nested output).
    pub name: String,
    /// Full output rectangle in logical, post-scale coordinates.
    pub geometry: Rect,
    /// Output scale factor.
    pub scale: f64,
    /// Area left for windows after layer-shell exclusive zones (M6); equal to
    /// `geometry` until then.
    pub usable_area: Rect,
    /// Which spaces are visible and the previous set for `toggle-back`.
    pub view: SpaceView,
    /// Per-space layout state.
    pub space_states: [SpaceState; SPACE_SLOTS],
    /// Where this output is in the redraw cycle (`COMPOSITOR.md` §8.2).
    pub redraw: RedrawState,
}

impl Output {
    /// Creates an output whose first space is visible.
    ///
    /// `default` seeds every space's state, so `master_ratio`/`gaps` follow the
    /// config until a space remembers its own.
    #[must_use]
    pub fn new(
        id: OutputId,
        name: impl Into<String>,
        geometry: Rect,
        scale: f64,
        default: SpaceState,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            geometry,
            scale,
            usable_area: geometry,
            view: SpaceView::new(SpaceSet::SPACE_1),
            space_states: core::array::from_fn(|_| default.clone()),
            redraw: RedrawState::Idle,
        }
    }

    /// Updates the geometry (and, until layer-shell lands, the usable area).
    pub fn set_geometry(&mut self, geometry: Rect) {
        self.geometry = geometry;
        self.usable_area = geometry;
    }

    /// The 0-based slot for a mask with a single space, or `None` if it names no
    /// space or more than one.
    #[must_use]
    pub fn slot_of(mask: SpaceSet) -> Option<usize> {
        let bits = mask.bits();
        if bits == 0 || !bits.is_power_of_two() {
            return None;
        }
        let index = bits.trailing_zeros() as usize;
        (index < SPACE_SLOTS).then_some(index)
    }

    /// The slot whose layout arranges the current view: **the lowest-numbered
    /// visible space** (`COMPOSITOR.md` §6, the multi-space-view rule).
    #[must_use]
    pub fn current_space_index(&self) -> Option<usize> {
        let bits = self.view.visible.bits();
        (bits != 0).then(|| bits.trailing_zeros() as usize)
    }

    /// The space state that arranges the current view, if any space is visible.
    #[must_use]
    pub fn current_space_state(&self) -> Option<&SpaceState> {
        self.current_space_index()
            .map(|index| &self.space_states[index])
    }

    /// Mutable form of [`Output::current_space_state`].
    #[must_use]
    pub fn current_space_state_mut(&mut self) -> Option<&mut SpaceState> {
        self.current_space_index()
            .map(move |index| &mut self.space_states[index])
    }

    /// Views a single space.
    pub fn view_space(&mut self, space: SpaceSet) {
        if let Some(mask) = Self::single(space) {
            self.view.view(mask);
        }
    }

    /// Views several spaces at once (`view 1+3`), ignoring an empty set.
    pub fn view_spaces(&mut self, spaces: SpaceSet) {
        if !spaces.is_empty() {
            self.view.view(spaces);
        }
    }

    /// Toggles one space in the view, refusing to empty the visible set.
    pub fn toggle_view(&mut self, space: SpaceSet) {
        if let Some(mask) = Self::single(space) {
            let next = self.view.visible ^ mask;
            if !next.is_empty() {
                self.view.view(next);
            }
        }
    }

    /// Swaps to the previously-viewed space set.
    pub fn toggle_back(&mut self) {
        if !self.view.previous.is_empty() {
            self.view.toggle_back();
        }
    }

    /// Requests a frame.
    ///
    /// Returns `true` when the backend must schedule one right now, i.e. the
    /// output was idle. A request that arrives while a frame is queued or being
    /// presented is folded into it instead (`finish_frame` then reports the
    /// follow-up frame).
    pub fn queue_redraw(&mut self) -> bool {
        match self.redraw {
            RedrawState::Idle => {
                self.redraw = RedrawState::Queued;
                true
            }
            RedrawState::Queued => false,
            RedrawState::WaitingForVBlank => {
                self.redraw = RedrawState::Queued;
                false
            }
        }
    }

    /// Starts a frame if one is queued, marking the output as presenting.
    ///
    /// Returns `false` for a spurious frame event (or a second call): nothing
    /// was queued, so nothing has to be drawn.
    pub fn begin_frame(&mut self) -> bool {
        if self.redraw == RedrawState::Queued {
            self.redraw = RedrawState::WaitingForVBlank;
            true
        } else {
            false
        }
    }

    /// Marks the presented frame as done, at the vertical blank that ends it.
    ///
    /// Returns `true` if another frame is due because something asked for a
    /// redraw while this one was in flight.
    pub fn finish_frame(&mut self) -> bool {
        let due = self.redraw == RedrawState::Queued;
        self.redraw = RedrawState::Idle;
        due
    }

    /// Reduces `space` to a single-space mask, if it names exactly one space.
    fn single(space: SpaceSet) -> Option<SpaceSet> {
        Self::slot_of(space).map(|_| space)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output() -> Output {
        Output::new(
            OutputId::new(0),
            "crawm-0",
            Rect::new(0.0, 0.0, 1920.0, 1080.0),
            1.0,
            SpaceState::default(),
        )
    }

    #[test]
    fn first_space_is_visible_initially() {
        let out = output();
        assert_eq!(out.view.visible, SpaceSet::SPACE_1);
        assert_eq!(out.current_space_index(), Some(0));
    }

    #[test]
    fn view_space_remembers_previous() {
        let mut out = output();
        out.view_space(SpaceSet::SPACE_3);
        assert_eq!(out.view.visible, SpaceSet::SPACE_3);
        assert_eq!(out.view.previous, SpaceSet::SPACE_1);
        out.toggle_back();
        assert_eq!(out.view.visible, SpaceSet::SPACE_1);
    }

    #[test]
    fn multi_space_view_uses_lowest_numbered_layout() {
        let mut out = output();
        out.view_spaces(SpaceSet::SPACE_3 | SpaceSet::SPACE_5);
        assert_eq!(out.current_space_index(), Some(2), "SPACE_3 is slot 2");
    }

    #[test]
    fn toggle_never_empties_the_view() {
        let mut out = output();
        out.toggle_view(SpaceSet::SPACE_1);
        assert_eq!(out.view.visible, SpaceSet::SPACE_1);
        out.toggle_view(SpaceSet::SPACE_2);
        assert_eq!(out.view.visible, SpaceSet::SPACE_1 | SpaceSet::SPACE_2);
    }

    #[test]
    fn per_space_state_is_remembered() {
        let mut out = output();
        out.space_states[0].layout = LayoutKind::Monocle;
        out.space_states[0].params.master_ratio = 0.25;
        out.view_space(SpaceSet::SPACE_2);
        out.space_states[1].layout = LayoutKind::Tile;
        out.view_space(SpaceSet::SPACE_1);
        assert_eq!(
            out.current_space_state().unwrap().layout,
            LayoutKind::Monocle
        );
        assert!((out.current_space_state().unwrap().params.master_ratio - 0.25).abs() < 1e-9);
    }

    #[test]
    fn slot_of_rejects_multi_and_empty() {
        assert_eq!(Output::slot_of(SpaceSet::empty()), None);
        assert_eq!(Output::slot_of(SpaceSet::SPACE_1 | SpaceSet::SPACE_2), None);
        assert_eq!(Output::slot_of(SpaceSet::SPACE_2), Some(1));
    }

    #[test]
    fn redraws_coalesce_while_idle_or_queued() {
        let mut out = output();
        assert_eq!(out.redraw, RedrawState::Idle);
        assert!(
            out.queue_redraw(),
            "the first request must schedule a frame"
        );
        assert_eq!(out.redraw, RedrawState::Queued);
        assert!(!out.queue_redraw(), "a second request must not re-schedule");
        assert!(out.begin_frame());
        assert_eq!(out.redraw, RedrawState::WaitingForVBlank);
        assert!(!out.begin_frame(), "a frame is already in flight");
        assert!(!out.finish_frame(), "nothing was requested meanwhile");
        assert_eq!(out.redraw, RedrawState::Idle);
    }

    #[test]
    fn a_request_during_presentation_schedules_the_next_frame() {
        let mut out = output();
        out.queue_redraw();
        assert!(out.begin_frame());
        assert!(
            !out.queue_redraw(),
            "a presenting output must not be asked twice"
        );
        assert_eq!(out.redraw, RedrawState::Queued);
        assert!(out.finish_frame(), "the folded request needs a frame");
        assert_eq!(out.redraw, RedrawState::Idle);
    }

    #[test]
    fn a_spurious_frame_event_draws_nothing() {
        let mut out = output();
        assert!(!out.begin_frame(), "nothing was queued");
        assert_eq!(out.redraw, RedrawState::Idle);
        assert!(
            !out.finish_frame(),
            "a frame that never began finishes nothing"
        );
    }
}

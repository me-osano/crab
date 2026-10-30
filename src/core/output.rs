//! The output and space model (`COMPOSITOR.md` §6).
//!
//! crab uses **spaces per output** (ADR-0004): each monitor has its own space
//! view, and a space remembers its own layout and parameters (`Pertag`). When
//! several spaces are visible at once, the layout of the **lowest-numbered
//! visible space** arranges the combined window set — the dwm rule documented on
//! [`Output::current_space_index`].

use std::fmt;

use crate::core::OutputId;
use crate::core::{
    LayoutKind, LayoutParams, Rect, ScrollState, SpaceRemap, SpaceSet, SpaceView, WindowId,
};

/// Why an `[output.<name>]` mode or transform could not be parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputSpecError {
    spec: String,
    reason: &'static str,
}

impl OutputSpecError {
    fn new(spec: &str, reason: &'static str) -> Self {
        Self {
            spec: spec.to_owned(),
            reason,
        }
    }
}

impl fmt::Display for OutputSpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid output spec `{}`: {}", self.spec, self.reason)
    }
}

impl std::error::Error for OutputSpecError {}

/// A configured output mode: `WIDTHxHEIGHT` with an optional refresh rate.
///
/// The DRM backend selects a matching mode from the connector; the pure model
/// keeps the parsed form so validation can reject typos before the session
/// starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputMode {
    /// Horizontal pixels.
    pub width: i32,
    /// Vertical pixels.
    pub height: i32,
    /// Vertical refresh in millihertz, if the spec included `@RATE`.
    pub refresh_mhz: Option<u32>,
}

impl fmt::Display for OutputMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{}", self.width, self.height)?;
        if let Some(refresh_mhz) = self.refresh_mhz {
            write!(f, "@{:.3}", f64::from(refresh_mhz) / 1000.0)?;
        }
        Ok(())
    }
}

impl OutputMode {
    /// Parses `WIDTHxHEIGHT` or `WIDTHxHEIGHT@RATE`, e.g. `2560x1440@144`.
    ///
    /// # Errors
    /// Fails for missing dimensions, non-positive sizes, or a refresh rate that
    /// is not a positive finite number.
    pub fn parse(spec: &str) -> Result<Self, OutputSpecError> {
        let spec = spec.trim();
        let (size, refresh) = match spec.split_once('@') {
            Some((size, refresh)) => (size, Some(refresh)),
            None => (spec, None),
        };
        let (width, height) = size
            .split_once('x')
            .ok_or_else(|| OutputSpecError::new(spec, "expected `WIDTHxHEIGHT[@RATE]`"))?;
        let width = width
            .trim()
            .parse::<i32>()
            .map_err(|_| OutputSpecError::new(spec, "width must be a positive integer"))?;
        let height = height
            .trim()
            .parse::<i32>()
            .map_err(|_| OutputSpecError::new(spec, "height must be a positive integer"))?;
        if width <= 0 || height <= 0 {
            return Err(OutputSpecError::new(
                spec,
                "width and height must be positive",
            ));
        }
        let refresh_mhz = match refresh {
            Some(refresh) => {
                let hz = refresh
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| OutputSpecError::new(spec, "refresh rate must be a number"))?;
                if !(hz.is_finite() && hz > 0.0) {
                    return Err(OutputSpecError::new(spec, "refresh rate must be positive"));
                }
                let mhz = (hz * 1000.0).round();
                if mhz > f64::from(u32::MAX) {
                    return Err(OutputSpecError::new(spec, "refresh rate is too large"));
                }
                Some(mhz as u32)
            }
            None => None,
        };
        Ok(Self {
            width,
            height,
            refresh_mhz,
        })
    }

    /// The mode's pixel size.
    #[must_use]
    pub const fn size(self) -> (i32, i32) {
        (self.width, self.height)
    }
}

/// A configured output transform.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OutputTransform {
    /// No rotation or flip.
    #[default]
    Normal,
    /// Rotated 90° clockwise.
    Rotate90,
    /// Rotated 180°.
    Rotate180,
    /// Rotated 270° clockwise.
    Rotate270,
    /// Flipped horizontally.
    Flipped,
    /// Flipped and rotated 90°.
    Flipped90,
    /// Flipped and rotated 180°.
    Flipped180,
    /// Flipped and rotated 270°.
    Flipped270,
}

impl OutputTransform {
    /// Parses the transform names accepted in `[output.<name>] transform`.
    ///
    /// # Errors
    /// Fails for any name outside the documented set.
    pub fn parse(spec: &str) -> Result<Self, OutputSpecError> {
        match spec.trim().to_ascii_lowercase().as_str() {
            "normal" => Ok(Self::Normal),
            "90" | "rotate-90" => Ok(Self::Rotate90),
            "180" | "rotate-180" => Ok(Self::Rotate180),
            "270" | "rotate-270" => Ok(Self::Rotate270),
            "flipped" | "flip" => Ok(Self::Flipped),
            "flipped-90" | "flip-90" => Ok(Self::Flipped90),
            "flipped-180" | "flip-180" => Ok(Self::Flipped180),
            "flipped-270" | "flip-270" => Ok(Self::Flipped270),
            _ => Err(OutputSpecError::new(
                spec,
                "expected normal, 90, 180, 270, flipped, flipped-90, flipped-180, or flipped-270",
            )),
        }
    }

    /// The canonical name for this transform.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Rotate90 => "90",
            Self::Rotate180 => "180",
            Self::Rotate270 => "270",
            Self::Flipped => "flipped",
            Self::Flipped90 => "flipped-90",
            Self::Flipped180 => "flipped-180",
            Self::Flipped270 => "flipped-270",
        }
    }

    /// Whether this transform swaps the output's logical width and height.
    #[must_use]
    pub const fn swaps_axes(self) -> bool {
        matches!(
            self,
            Self::Rotate90 | Self::Rotate270 | Self::Flipped90 | Self::Flipped270
        )
    }
}

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
    /// Connector name (`DP-1`, or `crab-0` for the nested output).
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
    /// The state a newly created space starts with, and what a slot is reset to
    /// when its space is removed.
    pub default_space: SpaceState,
    /// Where this output is in the redraw cycle (`COMPOSITOR.md` §8.2).
    pub redraw: RedrawState,
}

impl Output {
    /// Creates an output whose first space is visible.
    ///
    /// `default` seeds every space's state, so `gaps` and the layout follow the
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
            default_space: default,
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

    /// Removes empty spaces and renumbers the rest (niri-style dynamic spaces).
    ///
    /// A space exists while it holds a window (`occupied` is the union of every
    /// window's membership) or is the empty space past the last occupied one,
    /// which is where a `view` past the end lands. Kept spaces become contiguous
    /// from 1 in their relative order, each space's [`SpaceState`] moves with it,
    /// and a freed slot is reset to [`Output::default_space`] so a space created
    /// later starts from the configured default.
    ///
    /// Callers must renumber window membership with the returned [`SpaceRemap`];
    /// the view is remapped in place (a removed focused space hands the view to
    /// the space that moves into its slot). This also drops a `previous` set that
    /// pointed at a removed space, making `toggle-back` a no-op instead of a jump
    /// to a space that is gone.
    #[must_use]
    pub fn compact(&mut self, occupied: SpaceSet) -> SpaceRemap {
        let remap = SpaceRemap::new(occupied, self.view);
        let old = self.space_states.clone();
        self.space_states = core::array::from_fn(|_| self.default_space.clone());
        for old_index in 1..=SpaceSet::COUNT {
            let new_index = remap.new_index(old_index);
            if new_index != 0 {
                self.space_states[usize::from(new_index - 1)] =
                    old[usize::from(old_index - 1)].clone();
            }
        }
        remap.map_view(&mut self.view);
        remap
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
            "crab-0",
            Rect::new(0.0, 0.0, 1920.0, 1080.0),
            1.0,
            SpaceState::default(),
        )
    }

    #[test]
    fn output_modes_parse_size_and_refresh() {
        let mode = OutputMode::parse("2560x1440@144").unwrap();
        assert_eq!(mode.size(), (2560, 1440));
        assert_eq!(mode.refresh_mhz, Some(144_000));

        let mode = OutputMode::parse("1920x1080@120.030").unwrap();
        assert_eq!(mode.refresh_mhz, Some(120_030));

        let mode = OutputMode::parse(" 800x600 ").unwrap();
        assert_eq!(mode.size(), (800, 600));
        assert_eq!(mode.refresh_mhz, None);
    }

    #[test]
    fn output_modes_reject_malformed_specs() {
        for spec in ["", "1920", "1920x", "x1080", "0x1080", "1920x0", "a x b"] {
            assert!(OutputMode::parse(spec).is_err(), "accepted {spec:?}");
        }
        for spec in [
            "1920x1080@",
            "1920x1080@0",
            "1920x1080@-60",
            "1920x1080@nope",
        ] {
            assert!(OutputMode::parse(spec).is_err(), "accepted {spec:?}");
        }
    }

    #[test]
    fn output_transforms_parse_and_round_trip() {
        for name in [
            "normal",
            "90",
            "180",
            "270",
            "flipped",
            "flipped-90",
            "flipped-180",
            "flipped-270",
        ] {
            let transform = OutputTransform::parse(name).unwrap();
            assert_eq!(transform.name(), name);
        }
        assert_eq!(
            OutputTransform::parse("ROTATE-90").unwrap(),
            OutputTransform::Rotate90
        );
        assert!(OutputTransform::parse("sideways").is_err());
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
        out.space_states[0].layout = LayoutKind::Floating;
        out.space_states[0].params.gaps = 25.0;
        out.view_space(SpaceSet::SPACE_2);
        out.space_states[1].layout = LayoutKind::Fair;
        out.view_space(SpaceSet::SPACE_1);
        assert_eq!(
            out.current_space_state().unwrap().layout,
            LayoutKind::Floating
        );
        assert!((out.current_space_state().unwrap().params.gaps - 25.0).abs() < 1e-9);
    }

    #[test]
    fn slot_of_rejects_multi_and_empty() {
        assert_eq!(Output::slot_of(SpaceSet::empty()), None);
        assert_eq!(Output::slot_of(SpaceSet::SPACE_1 | SpaceSet::SPACE_2), None);
        assert_eq!(Output::slot_of(SpaceSet::SPACE_2), Some(1));
    }

    #[test]
    fn compact_closes_a_gap_and_moves_the_view_with_it() {
        let mut out = output();
        out.view_space(SpaceSet::SPACE_3);
        let remap = out.compact(SpaceSet::SPACE_1 | SpaceSet::SPACE_3);
        assert_eq!(out.view.visible, SpaceSet::SPACE_2);
        assert_eq!(remap.map_mask(SpaceSet::SPACE_3), SpaceSet::SPACE_2);
        assert_eq!(remap.map_mask(SpaceSet::SPACE_1), SpaceSet::SPACE_1);
    }

    #[test]
    fn compact_keeps_an_empty_focused_space_after_the_occupied_ones() {
        let mut out = output();
        out.view_space(SpaceSet::SPACE_4);
        let _ = out.compact(SpaceSet::SPACE_1);
        assert_eq!(out.view.visible, SpaceSet::SPACE_2);
        assert_eq!(out.current_space_index(), Some(1));
    }

    #[test]
    fn compact_hands_an_emptied_focused_space_to_the_next_one() {
        let mut out = output();
        out.space_states[2].layout = LayoutKind::Fair;
        out.view_space(SpaceSet::SPACE_2);
        let _ = out.compact(SpaceSet::SPACE_1 | SpaceSet::SPACE_3);
        assert_eq!(out.view.visible, SpaceSet::SPACE_2);
        assert_eq!(out.current_space_state().unwrap().layout, LayoutKind::Fair);
    }

    #[test]
    fn compact_resets_freed_slots_and_moves_the_state_of_kept_spaces() {
        let mut out = output();
        out.space_states[2].layout = LayoutKind::Scroll;
        out.view_space(SpaceSet::SPACE_1);
        let _ = out.compact(SpaceSet::SPACE_1 | SpaceSet::SPACE_3);
        assert_eq!(out.space_states[1].layout, LayoutKind::Scroll);
        assert_eq!(out.space_states[2].layout, out.default_space.layout);
    }

    #[test]
    fn compact_drops_a_previous_view_of_a_removed_space() {
        let mut out = output();
        out.view_space(SpaceSet::SPACE_3);
        out.view_space(SpaceSet::SPACE_1);
        let _ = out.compact(SpaceSet::SPACE_1);
        assert_eq!(out.view.previous, SpaceSet::empty());
        out.toggle_back();
        assert_eq!(out.view.visible, SpaceSet::SPACE_1);
    }

    #[test]
    fn compact_leaves_no_empty_space_before_the_focused_one() {
        for visible_index in 1..=6u8 {
            for bits in 0u8..(1 << 6) {
                let occupied = (1..=6u8)
                    .filter(|index| bits & (1 << (index - 1)) != 0)
                    .fold(SpaceSet::empty(), |spaces, index| {
                        spaces | SpaceSet::from_index(index).unwrap()
                    });
                let mut out = output();
                out.view_space(SpaceSet::from_index(visible_index).unwrap());
                let remap = out.compact(occupied);
                let focused = out.view.visible.iter_indices().next().unwrap();
                let occupied = remap.map_mask(occupied);
                for index in 1..focused {
                    assert!(
                        occupied.contains(SpaceSet::from_index(index).unwrap()),
                        "space {index} is empty and before the focused space {focused}"
                    );
                }
            }
        }
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

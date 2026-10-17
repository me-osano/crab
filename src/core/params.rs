//! Layout parameters, kinds, and commands.
//!
//! These are the pure data types a space needs to describe *how* it arranges its
//! windows (`COMPOSITOR.md` §6, `SpaceState`). They live in `core` because the
//! window/output model references them; the layout *engine* that reads them
//! lives in [`crate::layout`], which depends on `core` (never the other way).

/// A layout **family**: the broad strategy for arranging a space (ADR-0004).
///
/// Families group the concrete [`LayoutKind`]s so the set can grow (M7 adds many
/// tiling modes) without becoming an undifferentiated list. M2 ships the `Stack`
/// and `Tile` families; `Scroll` (niri's scroller) joins in M3.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LayoutFamily {
    /// Windows keep their own geometry, stacked as the compositor chooses.
    Stack,
    /// Automatic, non-overlapping splits (`COMPOSITOR.md` §7.2).
    Tile,
}

impl LayoutFamily {
    /// Every family implemented today, in cycle order.
    pub const ALL: [Self; 2] = [Self::Stack, Self::Tile];

    /// A stable, lowercase identifier (used by config and IPC).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Stack => "stack",
            Self::Tile => "tile",
        }
    }

    /// Parses the identifier produced by [`LayoutFamily::name`].
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|family| family.name() == name)
    }
}

/// Which arrangement strategy a space uses: a family plus a mode.
///
/// M2 ships the `Stack` family's [`LayoutKind::Floating`] and
/// [`LayoutKind::Monocle`] modes and the `Tile` family's [`LayoutKind::Tile`]
/// mode; M3 adds the `Scroll` family and M7 adds more tiling modes
/// (`grid`, `deck`, …). Cycling has two actions, one per axis: see
/// [`LayoutKind::next_mode`] and [`LayoutKind::next_family`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LayoutKind {
    /// `Tile` family: master/stack tiling (`COMPOSITOR.md` §7.2).
    #[default]
    Tile,
    /// `Stack` family: one window fills the area; the rest are hidden.
    Monocle,
    /// `Stack` family: no automatic placement; windows keep their geometry.
    Floating,
}

impl LayoutKind {
    /// Every mode implemented today, in a stable order.
    pub const ALL: [Self; 3] = [Self::Tile, Self::Monocle, Self::Floating];

    /// The family this mode belongs to.
    #[must_use]
    pub const fn family(self) -> LayoutFamily {
        match self {
            Self::Tile => LayoutFamily::Tile,
            Self::Monocle | Self::Floating => LayoutFamily::Stack,
        }
    }

    /// The modes of `family`, in cycle order; the first is its default.
    #[must_use]
    pub const fn modes_in(family: LayoutFamily) -> &'static [Self] {
        match family {
            LayoutFamily::Stack => &[Self::Floating, Self::Monocle],
            LayoutFamily::Tile => &[Self::Tile],
        }
    }

    /// Whether this mode places windows itself.
    ///
    /// The stack family's `floating` mode is the only one that does not: its
    /// windows keep their own geometry, and the compositor cascades new ones.
    #[must_use]
    pub const fn is_free(self) -> bool {
        matches!(self, Self::Floating)
    }

    /// A stable, lowercase identifier (used by config and IPC).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Tile => "tile",
            Self::Monocle => "monocle",
            Self::Floating => "floating",
        }
    }

    /// Parses a mode name, accepting the family names as aliases for the
    /// family's default mode (`stack` → `floating`, `tile` → `tile`), and
    /// `free` as an alias for `floating`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        if let Some(kind) = Self::ALL.into_iter().find(|kind| kind.name() == name) {
            return Some(kind);
        }
        if name == "free" {
            return Some(Self::Floating);
        }
        LayoutFamily::from_name(name).and_then(|family| Self::modes_in(family).first().copied())
    }

    /// The next mode within the current family (the `cycle-layout-mode` action).
    ///
    /// A family with a single mode is a no-op, so pressing the bind on `tile`
    /// changes nothing until M7 adds `grid`/`deck`.
    #[must_use]
    pub fn next_mode(self) -> Self {
        let modes = Self::modes_in(self.family());
        let index = modes.iter().position(|kind| *kind == self).unwrap_or(0);
        modes[(index + 1) % modes.len()]
    }

    /// The default mode of the next family (the `cycle-layout-family` action).
    #[must_use]
    pub fn next_family(self) -> Self {
        let index = LayoutFamily::ALL
            .iter()
            .position(|family| *family == self.family())
            .unwrap_or(0);
        let next = LayoutFamily::ALL[(index + 1) % LayoutFamily::ALL.len()];
        Self::modes_in(next)[0]
    }
}

/// Which edge of the area the master column/row occupies in [`LayoutKind::Tile`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TilePosition {
    /// Master occupies the left side.
    #[default]
    Left,
    /// Master occupies the right side.
    Right,
    /// Master occupies the top.
    Top,
    /// Master occupies the bottom.
    Bottom,
}

impl TilePosition {
    /// Every variant, in a sensible order for cycling.
    pub const ALL: [Self; 4] = [Self::Left, Self::Right, Self::Top, Self::Bottom];

    /// A stable, lowercase identifier (used by config and IPC).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Top => "top",
            Self::Bottom => "bottom",
        }
    }

    /// Parses the identifier produced by [`TilePosition::name`].
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|pos| pos.name() == name)
    }

    /// Whether the master/stack split is horizontal (`Left`/`Right`).
    #[must_use]
    pub const fn is_vertical_split(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }
}

/// Tunables a layout reads. Some commands edit them in place.
///
/// Every space owns one of these, which is what makes per-space layout memory
/// (`Pertag`) work: switching layouts never resets `master_ratio`, `gaps`, etc.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutParams {
    /// Number of windows in the master area (`0` sends everything to the stack).
    pub master_count: u32,
    /// Fraction of the area the master column/row takes, in `MIN_RATIO..=MAX_RATIO`.
    pub master_ratio: f64,
    /// Outer/inner gap in logical pixels.
    pub gaps: f64,
    /// Window border width in logical pixels (`0` disables borders).
    pub border_width: f64,
    /// Which edge the master area occupies (tile layout only).
    pub position: TilePosition,
}

impl LayoutParams {
    /// Smallest allowed master ratio.
    pub const MIN_RATIO: f64 = 0.1;
    /// Largest allowed master ratio.
    pub const MAX_RATIO: f64 = 0.9;
    /// How much one master-ratio command changes the ratio.
    pub const RATIO_STEP: f64 = 0.05;

    /// Clamps `master_ratio` and non-negative sizes into range.
    pub fn clamp_ratio(&mut self) {
        self.master_ratio = self.master_ratio.clamp(Self::MIN_RATIO, Self::MAX_RATIO);
        self.gaps = self.gaps.max(0.0);
        self.border_width = self.border_width.max(0.0);
    }
}

impl Default for LayoutParams {
    fn default() -> Self {
        Self {
            master_count: 1,
            master_ratio: 0.6,
            gaps: 8.0,
            border_width: 2.0,
            position: TilePosition::Left,
        }
    }
}

/// Persistent scroll state for layouts that scroll (the M3 `Scroller`).
///
/// M2's layouts ignore it, but it is part of [`crate::core::SpaceState`] so the
/// type does not change when `Scroller` lands (ROADMAP §3B, COMPOSITOR.md §7.4).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScrollState {
    /// Index of the focused column.
    pub focused_column: usize,
    /// Horizontal view offset in logical pixels.
    pub offset: f64,
}

/// A direction, used for directional focus (`neighbor`) and commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Toward decreasing x.
    Left,
    /// Toward increasing x.
    Right,
    /// Toward decreasing y.
    Up,
    /// Toward increasing y.
    Down,
}

/// A layout-specific command issued by a bind or IPC (`COMPOSITOR.md` §7.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LayoutCommand {
    /// Add one window to the master area.
    IncMasterCount,
    /// Remove one window from the master area.
    DecMasterCount,
    /// Grow the master area.
    IncMasterRatio,
    /// Shrink the master area.
    DecMasterRatio,
}

impl LayoutCommand {
    /// Every command, for binds and IPC tables.
    pub const ALL: [Self; 4] = [
        Self::IncMasterCount,
        Self::DecMasterCount,
        Self::IncMasterRatio,
        Self::DecMasterRatio,
    ];

    /// A stable, kebab-case identifier (used by config and IPC).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::IncMasterCount => "inc-master-count",
            Self::DecMasterCount => "dec-master-count",
            Self::IncMasterRatio => "inc-master-ratio",
            Self::DecMasterRatio => "dec-master-ratio",
        }
    }

    /// Parses the identifier produced by [`LayoutCommand::name`].
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|cmd| cmd.name() == name)
    }

    /// Applies this command to `params` (clamping afterwards).
    pub fn apply(self, params: &mut LayoutParams) {
        match self {
            Self::IncMasterCount => params.master_count = params.master_count.saturating_add(1),
            Self::DecMasterCount => params.master_count = params.master_count.saturating_sub(1),
            Self::IncMasterRatio => params.master_ratio += LayoutParams::RATIO_STEP,
            Self::DecMasterRatio => params.master_ratio -= LayoutParams::RATIO_STEP,
        }
        params.clamp_ratio();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_names_round_trip_and_cycle() {
        for kind in LayoutKind::ALL {
            assert_eq!(LayoutKind::from_name(kind.name()), Some(kind));
        }
        // Family and `free` aliases resolve to the family's default mode.
        assert_eq!(LayoutKind::from_name("stack"), Some(LayoutKind::Floating));
        assert_eq!(LayoutKind::from_name("tile"), Some(LayoutKind::Tile));
        assert_eq!(LayoutKind::from_name("free"), Some(LayoutKind::Floating));
        assert_eq!(LayoutKind::from_name("nope"), None);

        // Cycling the *mode* stays inside a family; cycling the *family*
        // crosses families and lands on the next family's default mode.
        assert_eq!(LayoutKind::Floating.next_mode(), LayoutKind::Monocle);
        assert_eq!(LayoutKind::Monocle.next_mode(), LayoutKind::Floating);
        assert_eq!(
            LayoutKind::Tile.next_mode(),
            LayoutKind::Tile,
            "single mode"
        );
        assert_eq!(LayoutKind::Tile.next_family(), LayoutKind::Floating);
        assert_eq!(LayoutKind::Monocle.next_family(), LayoutKind::Tile);
    }

    #[test]
    fn families_group_their_modes() {
        assert_eq!(LayoutKind::Tile.family(), LayoutFamily::Tile);
        assert_eq!(LayoutKind::Monocle.family(), LayoutFamily::Stack);
        assert_eq!(LayoutKind::Floating.family(), LayoutFamily::Stack);
        assert!(LayoutKind::Floating.is_free());
        assert!(!LayoutKind::Tile.is_free());
        for family in LayoutFamily::ALL {
            assert_eq!(LayoutFamily::from_name(family.name()), Some(family));
            assert!(!LayoutKind::modes_in(family).is_empty());
        }
    }

    #[test]
    fn tile_positions_round_trip() {
        for pos in TilePosition::ALL {
            assert_eq!(TilePosition::from_name(pos.name()), Some(pos));
        }
        assert!(TilePosition::Left.is_vertical_split());
        assert!(!TilePosition::Top.is_vertical_split());
    }

    #[test]
    fn ratio_commands_clamp() {
        let mut params = LayoutParams::default();
        for _ in 0..100 {
            LayoutCommand::IncMasterRatio.apply(&mut params);
        }
        assert!((params.master_ratio - LayoutParams::MAX_RATIO).abs() < 1e-9);
        for _ in 0..100 {
            LayoutCommand::DecMasterRatio.apply(&mut params);
        }
        assert!((params.master_ratio - LayoutParams::MIN_RATIO).abs() < 1e-9);
    }

    #[test]
    fn master_count_is_saturating() {
        let mut params = LayoutParams::default();
        LayoutCommand::DecMasterCount.apply(&mut params);
        assert_eq!(params.master_count, 0);
        assert!(LayoutCommand::from_name("inc-master-count").is_some());
    }
}

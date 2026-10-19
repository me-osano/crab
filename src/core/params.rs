//! Layout parameters, kinds, and commands.
//!
//! These are the pure data types a space needs to describe *how* it arranges its
//! windows (`COMPOSITOR.md` §6, `SpaceState`). They live in `core` because the
//! window/output model references them; the layout *engine* that reads them
//! lives in [`crate::layout`], which depends on `core` (never the other way).

use crate::core::{Rect, Size, WindowId};

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
    /// An unbounded strip of columns that the view scrolls along (niri).
    Scroll,
}

impl LayoutFamily {
    /// Every family implemented today, in cycle order.
    pub const ALL: [Self; 3] = [Self::Stack, Self::Tile, Self::Scroll];

    /// A stable, lowercase identifier (used by config and IPC).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Stack => "stack",
            Self::Tile => "tile",
            Self::Scroll => "scroll",
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
/// mode; M3 adds the `Scroll` family. Cycling has two actions, one per axis:
/// see [`LayoutKind::next_mode`] and [`LayoutKind::next_family`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LayoutKind {
    /// `Tile` family: master/stack tiling (`COMPOSITOR.md` §7.2).
    #[default]
    Tile,
    /// `Stack` family: one window fills the area; the rest are hidden.
    Monocle,
    /// `Stack` family: no automatic placement; windows keep their geometry.
    Floating,
    /// `Scroll` family: an unbounded strip of columns, scrolled horizontally.
    Scroll,
    /// `Scroll` family: the same strip on the vertical axis.
    VerticalScroll,
}

impl LayoutKind {
    /// Every mode implemented today, in a stable order.
    pub const ALL: [Self; 5] = [
        Self::Tile,
        Self::Monocle,
        Self::Floating,
        Self::Scroll,
        Self::VerticalScroll,
    ];

    /// The family this mode belongs to.
    #[must_use]
    pub const fn family(self) -> LayoutFamily {
        match self {
            Self::Tile => LayoutFamily::Tile,
            Self::Monocle | Self::Floating => LayoutFamily::Stack,
            Self::Scroll | Self::VerticalScroll => LayoutFamily::Scroll,
        }
    }

    /// The modes of `family`, in cycle order; the first is its default.
    #[must_use]
    pub const fn modes_in(family: LayoutFamily) -> &'static [Self] {
        match family {
            LayoutFamily::Stack => &[Self::Floating, Self::Monocle],
            LayoutFamily::Tile => &[Self::Tile],
            LayoutFamily::Scroll => &[Self::Scroll, Self::VerticalScroll],
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
            Self::Scroll => "scroll",
            Self::VerticalScroll => "vertical-scroll",
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

/// The axis a scrolling layout runs along (`COMPOSITOR.md` §7.7).
///
/// `scroll` and `vertical-scroll` are one implementation seen from two sides:
/// everything below reads a rectangle or a direction in terms of the *main*
/// axis (the one the strip grows along) and the *cross* axis, so the vertical
/// variant is the same code with the two swapped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ScrollAxis {
    /// Columns run left to right and the view scrolls horizontally.
    #[default]
    Horizontal,
    /// Columns run top to bottom and the view scrolls vertically.
    Vertical,
}

impl ScrollAxis {
    /// Every variant, horizontal first.
    pub const ALL: [Self; 2] = [Self::Horizontal, Self::Vertical];

    /// A stable, lowercase identifier (used by config and IPC).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Horizontal => "horizontal",
            Self::Vertical => "vertical",
        }
    }

    /// Parses the identifier produced by [`ScrollAxis::name`].
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|axis| axis.name() == name)
    }

    /// The [`LayoutKind`] mode that scrolls along this axis.
    #[must_use]
    pub const fn kind(self) -> LayoutKind {
        match self {
            Self::Horizontal => LayoutKind::Scroll,
            Self::Vertical => LayoutKind::VerticalScroll,
        }
    }

    /// The main-axis extent of `rect` (its width when horizontal).
    #[must_use]
    pub const fn main_span(self, rect: Rect) -> f64 {
        match self {
            Self::Horizontal => rect.w,
            Self::Vertical => rect.h,
        }
    }

    /// The cross-axis extent of `rect`.
    #[must_use]
    pub const fn cross_span(self, rect: Rect) -> f64 {
        match self {
            Self::Horizontal => rect.h,
            Self::Vertical => rect.w,
        }
    }

    /// The cross-axis origin of `rect`.
    #[must_use]
    pub const fn cross_start(self, rect: Rect) -> f64 {
        match self {
            Self::Horizontal => rect.y,
            Self::Vertical => rect.x,
        }
    }

    /// The main-axis origin of `rect`.
    #[must_use]
    pub const fn main_start(self, rect: Rect) -> f64 {
        match self {
            Self::Horizontal => rect.x,
            Self::Vertical => rect.y,
        }
    }

    /// The main-axis extent of a [`Size`].
    #[must_use]
    pub const fn main_size(self, size: Size) -> f64 {
        match self {
            Self::Horizontal => size.w,
            Self::Vertical => size.h,
        }
    }

    /// A rectangle from main-axis and cross-axis coordinates and extents.
    #[must_use]
    pub const fn rect(self, main: f64, cross: f64, main_span: f64, cross_span: f64) -> Rect {
        match self {
            Self::Horizontal => Rect::new(main, cross, main_span, cross_span),
            Self::Vertical => Rect::new(cross, main, cross_span, main_span),
        }
    }

    /// How `direction` moves along the main axis: `Some(true)` forward (toward
    /// growing coordinates), `Some(false)` backward, `None` when it is not a
    /// main-axis direction at all.
    #[must_use]
    pub const fn main_direction(self, direction: Direction) -> Option<bool> {
        match (self, direction) {
            (Self::Horizontal, Direction::Right) | (Self::Vertical, Direction::Down) => Some(true),
            (Self::Horizontal, Direction::Left) | (Self::Vertical, Direction::Up) => Some(false),
            _ => None,
        }
    }

    /// How `direction` moves along the cross axis; see [`Self::main_direction`].
    #[must_use]
    pub const fn cross_direction(self, direction: Direction) -> Option<bool> {
        match (self, direction) {
            (Self::Horizontal, Direction::Down) | (Self::Vertical, Direction::Right) => Some(true),
            (Self::Horizontal, Direction::Up) | (Self::Vertical, Direction::Left) => Some(false),
            _ => None,
        }
    }
}

/// How wide a scroll column is (`COMPOSITOR.md` §7.7).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColumnWidth {
    /// A fraction of the view's main-axis span.
    Proportion(f64),
    /// A fixed span in logical pixels.
    Fixed(f64),
}

impl ColumnWidth {
    /// The width a new column gets: half the view, niri's default.
    pub const DEFAULT: Self = Self::Proportion(0.5);

    /// The span this asks for inside a view `span` logical pixels wide.
    ///
    /// Negative results are clamped away; a span larger than the view is
    /// allowed and is what `overspread` decides the fate of.
    #[must_use]
    pub fn resolve(self, span: f64) -> f64 {
        match self {
            Self::Proportion(fraction) => span * fraction.max(0.0),
            Self::Fixed(px) => px.max(0.0),
        }
    }

    /// Parses niri's forms: `0.5` (a bare number is a proportion), `proportion
    /// 0.5`, and `fixed 800`/`800px`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if let Some(rest) = text.strip_suffix("px") {
            return rest.trim().parse().ok().map(Self::Fixed);
        }
        if let Some(rest) = text.strip_prefix("proportion") {
            return rest.trim().parse().ok().map(Self::Proportion);
        }
        if let Some(rest) = text.strip_prefix("fixed") {
            return rest.trim().parse().ok().map(Self::Fixed);
        }
        text.parse().ok().map(Self::Proportion)
    }
}

impl core::fmt::Display for ColumnWidth {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Proportion(fraction) => write!(f, "proportion {fraction}"),
            Self::Fixed(px) => write!(f, "fixed {px}"),
        }
    }
}

/// How the windows inside one scroll column share it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ColumnDisplay {
    /// Windows split the column along the cross axis (niri's default).
    #[default]
    Normal,
    /// Every window shares the whole column; only the focused one is drawn.
    /// The tab bar itself arrives with M7 (`COMPOSITOR.md` §7.2).
    Tabbed,
}

impl ColumnDisplay {
    /// Every variant, in cycle order.
    pub const ALL: [Self; 2] = [Self::Normal, Self::Tabbed];

    /// A stable, lowercase identifier (used by config and IPC).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Tabbed => "tabbed",
        }
    }

    /// Parses the identifier produced by [`ColumnDisplay::name`].
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|display| display.name() == name)
    }
}

/// Where the view puts the focused column (niri's `center-focused-column`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ScrollCenter {
    /// Scroll only as far as it takes to keep the column visible.
    #[default]
    Never,
    /// Keep the focused column in the middle of the view.
    Always,
    /// Centre it only when it does not fit beside the previously focused one.
    OnOverflow,
}

impl ScrollCenter {
    /// Every variant, in cycle order.
    pub const ALL: [Self; 3] = [Self::Never, Self::Always, Self::OnOverflow];

    /// A stable, kebab-case identifier (used by config and IPC).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::Always => "always",
            Self::OnOverflow => "on-overflow",
        }
    }

    /// Parses the identifier produced by [`ScrollCenter::name`].
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|center| center.name() == name)
    }
}

/// The widths the `switch-preset-width` command cycles a column through.
///
/// Held as a fixed-size array so a space's parameters stay `Copy` and can be
/// copied in and out of the output model without allocating. M5's config grows
/// the cap if anyone needs more than [`WidthPresets::CAPACITY`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WidthPresets {
    entries: [ColumnWidth; WidthPresets::CAPACITY],
    len: usize,
}

impl WidthPresets {
    /// How many presets a space can hold.
    pub const CAPACITY: usize = 8;

    /// An empty list.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: [ColumnWidth::DEFAULT; Self::CAPACITY],
            len: 0,
        }
    }

    /// Appends `width`, ignoring anything past [`WidthPresets::CAPACITY`].
    pub fn push(&mut self, width: ColumnWidth) {
        if self.len < Self::CAPACITY {
            self.entries[self.len] = width;
            self.len += 1;
        }
    }

    /// The presets, in order.
    pub fn iter(&self) -> impl Iterator<Item = ColumnWidth> + '_ {
        self.entries[..self.len].iter().copied()
    }

    /// How many presets there are.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Whether the list is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The preset after `current`, wrapping round; `None` when there are none.
    ///
    /// A width that is not in the list starts the walk over at the first preset,
    /// so a column created before the presets were known still cycles sanely.
    #[must_use]
    pub fn next_after(&self, current: ColumnWidth) -> Option<ColumnWidth> {
        if self.is_empty() {
            return None;
        }
        let index = self
            .iter()
            .position(|width| width == current)
            .map_or(0, |index| (index + 1) % self.len);
        self.iter().nth(index)
    }
}

impl Default for WidthPresets {
    /// niri's thirds: one third, one half, two thirds of the view.
    fn default() -> Self {
        let mut presets = Self::new();
        presets.push(ColumnWidth::Proportion(1.0 / 3.0));
        presets.push(ColumnWidth::Proportion(0.5));
        presets.push(ColumnWidth::Proportion(2.0 / 3.0));
        presets
    }
}

/// One column of a scrolling layout: a width and the windows filling it.
#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    /// How wide the column asks to be, before size hints widen it.
    pub width: ColumnWidth,
    /// The windows it holds, first to last along the cross axis.
    pub windows: Vec<WindowId>,
    /// How those windows share the column.
    pub display: ColumnDisplay,
}

impl Column {
    /// A column holding one window.
    #[must_use]
    pub fn new(width: ColumnWidth, display: ColumnDisplay, window: WindowId) -> Self {
        Self {
            width,
            windows: vec![window],
            display,
        }
    }
}

/// Where the view sat relative to the focused column at the last arrange.
///
/// This is what keeps the focused window still when columns open or close
/// around it (niri's second design principle, `COMPOSITOR.md` §7.7): the offset
/// is re-derived each arrange from where the focused column sits in the strip
/// and where it was asked to sit on screen, so a column appearing or
/// disappearing before it moves the strip but not the window.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScrollAnchor {
    /// The window that was focused.
    pub window: Option<WindowId>,
    /// Main-axis strip coordinate of the focused column's leading edge.
    pub start: f64,
    /// The focused column's main-axis span.
    pub span: f64,
    /// Where that leading edge sat on screen (view coordinates).
    pub screen: f64,
}

/// Persistent state for scrolling layouts (`COMPOSITOR.md` §7.7).
///
/// A scroller is not a pure function of its window list the way the tiling
/// layouts are: the **columns** are the model. `arrange` reconciles them against
/// the windows it is handed — adopting unseen windows as new columns, dropping
/// columns whose windows have gone — so an untouched column keeps its width,
/// its neighbours, and its place in the strip no matter what happens elsewhere.
///
/// The rest is the view: [`ScrollState::offset`] is where the strip is scrolled
/// to, and [`ScrollAnchor`] remembers the pair of facts that let the next
/// arrange keep the focused window from drifting.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScrollState {
    /// The columns of the strip, first to last.
    pub columns: Vec<Column>,
    /// View offset along the main axis, in logical pixels.
    pub offset: f64,
    /// Where the focus was at the last arrange.
    pub anchor: ScrollAnchor,
    /// Set by [`LayoutCommand::CenterColumn`]; the next arrange centres the
    /// focused column and clears it.
    pub center_request: bool,
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
    /// The width a newly created scroll column gets.
    pub column_width: ColumnWidth,
    /// The widths `switch-preset-width` cycles through.
    pub preset_widths: WidthPresets,
    /// Where the view puts the focused column.
    pub center: ScrollCenter,
    /// Whether a column wider than the view keeps its width and spills past
    /// both edges (`true`) or is clamped to the view (`false`).
    pub overspread: bool,
    /// How the windows inside a column share it.
    pub column_display: ColumnDisplay,
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
        self.column_width = match self.column_width {
            ColumnWidth::Proportion(fraction) => ColumnWidth::Proportion(fraction.max(0.0)),
            ColumnWidth::Fixed(px) => ColumnWidth::Fixed(px.max(0.0)),
        };
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
            column_width: ColumnWidth::DEFAULT,
            preset_widths: WidthPresets::default(),
            center: ScrollCenter::Never,
            overspread: true,
            column_display: ColumnDisplay::Normal,
        }
    }
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
    /// Add one window to the master area (tiling layouts).
    IncMasterCount,
    /// Remove one window from the master area (tiling layouts).
    DecMasterCount,
    /// Grow the master area (tiling layouts).
    IncMasterRatio,
    /// Shrink the master area (tiling layouts).
    DecMasterRatio,
    /// Cycle the focused column's width through the presets (scrolling layouts).
    SwitchPresetWidth,
    /// Scroll the focused column to the middle of the view (scrolling layouts).
    CenterColumn,
}

impl LayoutCommand {
    /// Every command, for binds and IPC tables.
    pub const ALL: [Self; 6] = [
        Self::IncMasterCount,
        Self::DecMasterCount,
        Self::IncMasterRatio,
        Self::DecMasterRatio,
        Self::SwitchPresetWidth,
        Self::CenterColumn,
    ];

    /// A stable, kebab-case identifier (used by config and IPC).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::IncMasterCount => "inc-master-count",
            Self::DecMasterCount => "dec-master-count",
            Self::IncMasterRatio => "inc-master-ratio",
            Self::DecMasterRatio => "dec-master-ratio",
            Self::SwitchPresetWidth => "switch-preset-width",
            Self::CenterColumn => "center-column",
        }
    }

    /// Parses the identifier produced by [`LayoutCommand::name`].
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|cmd| cmd.name() == name)
    }

    /// Applies the **shared** commands to `params` (clamping afterwards).
    ///
    /// The commands a family owns itself — the scroller's width and centring —
    /// need that family's persistent state, so they are handled by the layout
    /// ([`crate::layout::apply_command`]) and do nothing here.
    pub fn apply(self, params: &mut LayoutParams) {
        match self {
            Self::IncMasterCount => params.master_count = params.master_count.saturating_add(1),
            Self::DecMasterCount => params.master_count = params.master_count.saturating_sub(1),
            Self::IncMasterRatio => params.master_ratio += LayoutParams::RATIO_STEP,
            Self::DecMasterRatio => params.master_ratio -= LayoutParams::RATIO_STEP,
            Self::SwitchPresetWidth | Self::CenterColumn => return,
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
        assert_eq!(LayoutKind::Scroll.next_mode(), LayoutKind::VerticalScroll);
        assert_eq!(LayoutKind::VerticalScroll.next_mode(), LayoutKind::Scroll);
        assert_eq!(LayoutKind::Tile.next_family(), LayoutKind::Scroll);
        assert_eq!(LayoutKind::Monocle.next_family(), LayoutKind::Tile);
        assert_eq!(LayoutKind::Scroll.next_family(), LayoutKind::Floating);
        assert_eq!(
            LayoutKind::VerticalScroll.next_family(),
            LayoutKind::Floating
        );
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

    #[test]
    fn scroll_kinds_belong_to_the_scroll_family() {
        assert_eq!(LayoutKind::Scroll.family(), LayoutFamily::Scroll);
        assert_eq!(LayoutKind::VerticalScroll.family(), LayoutFamily::Scroll);
        assert!(!LayoutKind::Scroll.is_free());
        assert_eq!(
            LayoutKind::modes_in(LayoutFamily::Scroll),
            &[LayoutKind::Scroll, LayoutKind::VerticalScroll]
        );
        for kind in [LayoutKind::Scroll, LayoutKind::VerticalScroll] {
            assert_eq!(LayoutKind::from_name(kind.name()), Some(kind));
        }
        // The family name is an alias for its default mode.
        assert_eq!(LayoutKind::from_name("scroll"), Some(LayoutKind::Scroll));
    }

    #[test]
    fn scroll_axes_round_trip_and_map_directions() {
        for axis in ScrollAxis::ALL {
            assert_eq!(ScrollAxis::from_name(axis.name()), Some(axis));
        }
        assert_eq!(ScrollAxis::Horizontal.kind(), LayoutKind::Scroll);
        assert_eq!(ScrollAxis::Vertical.kind(), LayoutKind::VerticalScroll);

        // The main axis of a horizontal scroller is x; `Right` walks it forward
        // and `Down` walks the cross axis.
        let axis = ScrollAxis::Horizontal;
        assert_eq!(axis.main_direction(Direction::Right), Some(true));
        assert_eq!(axis.main_direction(Direction::Left), Some(false));
        assert_eq!(axis.main_direction(Direction::Down), None);
        assert_eq!(axis.cross_direction(Direction::Down), Some(true));
        assert_eq!(axis.cross_direction(Direction::Up), Some(false));
        assert_eq!(axis.cross_direction(Direction::Right), None);

        // On the vertical axis the two swap round.
        let axis = ScrollAxis::Vertical;
        assert_eq!(axis.main_direction(Direction::Down), Some(true));
        assert_eq!(axis.cross_direction(Direction::Left), Some(false));
        assert_eq!(axis.main_direction(Direction::Right), None);

        // Every direction is exactly one of the two.
        for direction in [
            Direction::Left,
            Direction::Right,
            Direction::Up,
            Direction::Down,
        ] {
            for axis in ScrollAxis::ALL {
                assert_ne!(
                    axis.main_direction(direction).is_none(),
                    axis.cross_direction(direction).is_none(),
                    "{direction:?} on {axis:?}"
                );
            }
        }
    }

    #[test]
    fn scroll_axis_reads_and_writes_rects() {
        let rect = Rect::new(10.0, 20.0, 300.0, 400.0);
        let horizontal = ScrollAxis::Horizontal;
        assert_eq!(horizontal.main_start(rect), 10.0);
        assert_eq!(horizontal.cross_start(rect), 20.0);
        assert_eq!(horizontal.main_span(rect), 300.0);
        assert_eq!(horizontal.cross_span(rect), 400.0);
        assert_eq!(
            horizontal.rect(1.0, 2.0, 3.0, 4.0),
            Rect::new(1.0, 2.0, 3.0, 4.0)
        );

        let vertical = ScrollAxis::Vertical;
        assert_eq!(vertical.main_start(rect), 20.0);
        assert_eq!(vertical.cross_start(rect), 10.0);
        assert_eq!(vertical.main_span(rect), 400.0);
        assert_eq!(vertical.cross_span(rect), 300.0);
        assert_eq!(
            vertical.rect(1.0, 2.0, 3.0, 4.0),
            Rect::new(2.0, 1.0, 4.0, 3.0)
        );
        // Building a rect and reading it back is the identity.
        let built = vertical.rect(
            vertical.main_start(rect),
            vertical.cross_start(rect),
            vertical.main_span(rect),
            vertical.cross_span(rect),
        );
        assert_eq!(built, rect);
    }

    #[test]
    fn column_widths_parse_and_resolve() {
        assert_eq!(
            ColumnWidth::parse("0.5"),
            Some(ColumnWidth::Proportion(0.5))
        );
        assert_eq!(
            ColumnWidth::parse("proportion 0.25"),
            Some(ColumnWidth::Proportion(0.25))
        );
        assert_eq!(
            ColumnWidth::parse("fixed 800"),
            Some(ColumnWidth::Fixed(800.0))
        );
        assert_eq!(ColumnWidth::parse("800px"), Some(ColumnWidth::Fixed(800.0)));
        assert_eq!(ColumnWidth::parse("wide"), None);

        // Proportions are of the view; fixed sizes are absolute. Neither goes
        // negative, and a proportion may deliberately overspread.
        assert_eq!(ColumnWidth::Proportion(0.5).resolve(1000.0), 500.0);
        assert_eq!(ColumnWidth::Fixed(800.0).resolve(1000.0), 800.0);
        assert_eq!(ColumnWidth::Fixed(-5.0).resolve(1000.0), 0.0);
        assert_eq!(ColumnWidth::Proportion(2.0).resolve(1000.0), 2000.0);

        assert_eq!(ColumnWidth::Fixed(800.0).to_string(), "fixed 800");
        assert_eq!(
            ColumnWidth::DEFAULT.to_string(),
            format!("proportion {}", ColumnWidth::DEFAULT.resolve(1.0))
        );
    }

    #[test]
    fn width_presets_cycle_and_wrap() {
        let presets = WidthPresets::default();
        assert_eq!(presets.len(), 3);
        let thirds: Vec<_> = presets.iter().collect();
        assert_eq!(
            presets.next_after(thirds[0]),
            Some(thirds[1]),
            "in list order"
        );
        assert_eq!(presets.next_after(thirds[2]), Some(thirds[0]), "wraps");
        assert_eq!(
            presets.next_after(ColumnWidth::Fixed(123.0)),
            Some(thirds[0]),
            "an unknown width starts over"
        );

        assert_eq!(WidthPresets::new().next_after(ColumnWidth::DEFAULT), None);

        // The list is capped, so the model stays `Copy`.
        let mut full = WidthPresets::new();
        for index in 0..WidthPresets::CAPACITY + 4 {
            full.push(ColumnWidth::Fixed(index as f64));
        }
        assert_eq!(full.len(), WidthPresets::CAPACITY);
    }

    #[test]
    fn scroll_option_names_round_trip() {
        for display in ColumnDisplay::ALL {
            assert_eq!(ColumnDisplay::from_name(display.name()), Some(display));
        }
        for center in ScrollCenter::ALL {
            assert_eq!(ScrollCenter::from_name(center.name()), Some(center));
        }
        assert_eq!(
            ScrollCenter::from_name("on-overflow"),
            Some(ScrollCenter::OnOverflow)
        );
        assert_eq!(
            LayoutCommand::from_name("switch-preset-width"),
            Some(LayoutCommand::SwitchPresetWidth)
        );
        assert_eq!(
            LayoutCommand::from_name("center-column"),
            Some(LayoutCommand::CenterColumn)
        );
        // The shared commands still work, and the scroller's do not touch params.
        let mut params = LayoutParams::default();
        let before = params;
        LayoutCommand::SwitchPresetWidth.apply(&mut params);
        LayoutCommand::CenterColumn.apply(&mut params);
        assert_eq!(params, before);
    }
}

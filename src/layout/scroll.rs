//! The scrolling layout — niri's column strip (`COMPOSITOR.md` §7.2, §7.7).
//!
//! Windows are arranged into **columns** along one axis, and the view scrolls
//! along that axis to keep the focused column where the user asked for it.
//!
//! The columns are the layout's model, not a function of the window list: they
//! live in [`ScrollState`] and `arrange` only *reconciles* them against the
//! windows it is handed — adopting unseen windows as new columns, dropping
//! columns whose windows have gone, and touching nothing else. An existing
//! column therefore keeps its width, its neighbours and its place in the strip
//! however the world changes around it, which is what makes opening a window
//! leave the rest of the strip exactly where it was (niri's first design
//! principle) and closing a column to the left of the focused one not drag the
//! focused window sideways (its second).
//!
//! The exposed `scroll` mode runs horizontally, but the implementation is
//! axis-generic: everything below is expressed in terms of the **main** axis
//! (the one the strip grows along) and the **cross** axis (along a column), and
//! [`ScrollAxis`] turns those into ordinary coordinates.

use super::placement::{
    Layout, LayoutInput, LayoutWindow, Placement, apply_hints, directional_neighbor,
};
use crate::core::{
    Column, ColumnDisplay, ColumnWidth, Direction, LayoutCommand, LayoutKind, LayoutParams, Rect,
    ScrollAnchor, ScrollAxis, ScrollCenter, ScrollState, WidthPresets, WindowId,
};

/// The `scroll` layout: an unbounded strip of columns along one axis.
///
/// The columns themselves live in the space's [`ScrollState`], so this type is
/// just the axis it works on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scroll {
    axis: ScrollAxis,
}

impl Scroll {
    /// A scroller running along `axis`.
    #[must_use]
    pub const fn new(axis: ScrollAxis) -> Self {
        Self { axis }
    }
}

impl Layout for Scroll {
    fn kind(&self) -> LayoutKind {
        LayoutKind::Scroll
    }

    fn arrange(&self, mut input: LayoutInput<'_>) -> Vec<Placement> {
        arrange_scroll(self.axis, &mut input)
    }

    fn neighbor(&self, input: &LayoutInput<'_>, from: usize, dir: Direction) -> Option<usize> {
        neighbor_scroll(self.axis, input, from, dir)
    }

    fn command(
        &self,
        command: LayoutCommand,
        focused: Option<WindowId>,
        params: &mut LayoutParams,
        scroll: &mut ScrollState,
    ) {
        match command {
            // The column widths are the scroller's own state, so every command
            // that touches them is handled here rather than in `params`.
            LayoutCommand::SwitchPresetWidth => {
                if let Some(window) = focused {
                    switch_preset_width(scroll, window, &params.preset_widths);
                }
            }
            LayoutCommand::CenterColumn => scroll.center_request = true,
            LayoutCommand::MoveColumnLeft => {
                if let Some(window) = focused {
                    move_column(scroll, window, false);
                }
            }
            LayoutCommand::MoveColumnRight => {
                if let Some(window) = focused {
                    move_column(scroll, window, true);
                }
            }
            LayoutCommand::MoveWindowToColumnLeft => {
                if let Some(window) = focused {
                    move_window_to_column(scroll, window, false);
                }
            }
            LayoutCommand::MoveWindowToColumnRight => {
                if let Some(window) = focused {
                    move_window_to_column(scroll, window, true);
                }
            }
            LayoutCommand::ConsumeWindowIntoColumn => {
                if let Some(window) = focused {
                    // Consuming is a leftward window move; an emptied source
                    // column is dropped, so a single-window column merges whole.
                    move_window_to_column(scroll, window, false);
                }
            }
            LayoutCommand::ExpelWindowFromColumn => {
                if let Some(window) = focused {
                    expel_window_from_column(scroll, window);
                }
            }
            LayoutCommand::ExpandColumnToAvailableWidth => {
                if let Some(window) = focused {
                    set_column_width(scroll, window, ColumnWidth::Proportion(1.0));
                }
            }
            LayoutCommand::SetColumnWidth(width) => {
                if let Some(window) = focused {
                    set_column_width(scroll, window, width);
                }
            }
        }
    }
}

/// Computes the strip's placements, reconciling the column model first.
fn arrange_scroll(axis: ScrollAxis, input: &mut LayoutInput<'_>) -> Vec<Placement> {
    let area = input.usable_area();
    // Read the input out before taking the scroll state mutably; the window
    // list and parameters outlive the borrow of `input` either way.
    let windows = input.windows;
    let focused = input.focused;
    let params = input.params;
    let scroll = &mut *input.scroll;

    let gaps = params.gaps;
    let view_span = axis.main_span(area);
    let focused_window = focused
        .and_then(|index| windows.get(index))
        .map(|window| window.id);

    // 1. Bring the columns in line with the windows that exist.
    reconcile(scroll, windows, focused_window, params);

    // 2. Size them, lay them out along the strip, and decide where it is scrolled to.
    let spans: Vec<f64> = scroll
        .columns
        .iter()
        .map(|column| column_span(axis, column, windows, params, view_span))
        .collect();
    let strip = Strip::new(spans, gaps);
    let focused_column = focused_window.and_then(|id| column_of(scroll, id));
    update_view(
        scroll,
        focused_window,
        focused_column,
        &strip,
        view_span,
        params,
    );

    // 3. Place: columns along the main axis, their windows along the cross axis.
    // The result follows the input order, like every other layout, so callers
    // can index placements by the window indices they handed in.
    let mut placements: Vec<Option<Placement>> = vec![None; windows.len()];
    for (column_index, column) in scroll.columns.iter().enumerate() {
        let rect = axis.rect(
            axis.main_start(area) + strip.start(column_index) - scroll.offset,
            axis.cross_start(area),
            strip.span(column_index),
            axis.cross_span(area),
        );
        // In `tabbed` a column is a stack of one: all its windows share its
        // rect and only the focused one is drawn (the tab bar itself is M7).
        let shown = match column.display {
            ColumnDisplay::Normal => None,
            ColumnDisplay::Tabbed => Some(
                column
                    .windows
                    .iter()
                    .position(|id| Some(*id) == focused_window)
                    .unwrap_or(0),
            ),
        };
        for (stack, id) in column.windows.iter().enumerate() {
            let Some(index) = windows.iter().position(|window| window.id == *id) else {
                continue;
            };
            // In `tabbed` all of a column's windows share its rect, so a tab
            // switch is a visibility change and nothing moves.
            let slot = match column.display {
                ColumnDisplay::Normal => window_slot(axis, rect, stack, column.windows.len(), gaps),
                ColumnDisplay::Tabbed => rect,
            };
            placements[index] = Some(Placement {
                window: *id,
                rect: apply_hints(slot, &windows[index]),
                visible: shown.is_none_or(|shown| shown == stack),
                z_hint: i32::try_from(stack).unwrap_or(i32::MAX),
            });
        }
    }
    placements.into_iter().flatten().collect()
}

/// Where every column sits along the main axis.
#[derive(Debug)]
struct Strip {
    /// Each column's main-axis span, gaps excluded.
    spans: Vec<f64>,
    /// Each column's leading main-axis coordinate, gaps included.
    starts: Vec<f64>,
}

impl Strip {
    /// Lays `spans` out one after another with `gaps` between them.
    fn new(spans: Vec<f64>, gaps: f64) -> Self {
        let mut starts = Vec::with_capacity(spans.len());
        let mut cursor = 0.0;
        for span in &spans {
            starts.push(cursor);
            cursor += span + gaps;
        }
        Self { spans, starts }
    }

    /// The whole strip's main-axis extent.
    fn total(&self) -> f64 {
        match (self.starts.last(), self.spans.last()) {
            (Some(start), Some(span)) => start + span,
            _ => 0.0,
        }
    }

    /// The leading coordinate of column `index`.
    fn start(&self, index: usize) -> f64 {
        self.starts.get(index).copied().unwrap_or(0.0)
    }

    /// The span of column `index`.
    fn span(&self, index: usize) -> f64 {
        self.spans.get(index).copied().unwrap_or(0.0)
    }
}

/// Brings the column model in line with the windows that exist.
///
/// Columns whose windows have all gone are dropped; windows that are not in a
/// column yet become new columns *after the focused one*, which is what keeps a
/// window opening from reflowing the strip. When the focused window is itself
/// new, "the focused one" means the column that held the focus at the last
/// arrange, so the new window still lands beside the window the user was on.
fn reconcile(
    scroll: &mut ScrollState,
    windows: &[LayoutWindow],
    focused: Option<WindowId>,
    params: &LayoutParams,
) {
    for column in &mut scroll.columns {
        column
            .windows
            .retain(|id| windows.iter().any(|w| w.id == *id));
    }
    scroll.columns.retain(|column| !column.windows.is_empty());

    let insert_at = focused
        .and_then(|id| column_of(scroll, id))
        .or_else(|| scroll.anchor.window.and_then(|id| column_of(scroll, id)))
        .map_or(scroll.columns.len(), |index| index + 1)
        .min(scroll.columns.len());

    let new: Vec<Column> = windows
        .iter()
        .filter(|window| column_of(scroll, window.id).is_none())
        .map(|window| Column::new(params.column_width, params.column_display, window.id))
        .collect();
    for (offset, column) in new.into_iter().enumerate() {
        scroll.columns.insert(insert_at + offset, column);
    }
}

/// A column's main-axis span: what its width asks for, widened to fit the
/// minimum size of the widest window in it, and clamped to the view unless
/// columns are allowed to overspread.
fn column_span(
    axis: ScrollAxis,
    column: &Column,
    windows: &[LayoutWindow],
    params: &LayoutParams,
    view_span: f64,
) -> f64 {
    let mut span = column.width.resolve(view_span);
    for id in &column.windows {
        let Some(window) = windows.iter().find(|window| window.id == *id) else {
            continue;
        };
        if let Some(min) = window.min_size {
            span = span.max(axis.main_size(min).max(1.0));
        }
    }
    let span = span.max(1.0);
    if params.overspread {
        span
    } else {
        span.min(view_span)
    }
}

/// The rectangle window `stack` of `count` occupies inside a column.
fn window_slot(axis: ScrollAxis, column: Rect, stack: usize, count: usize, gaps: f64) -> Rect {
    if count <= 1 {
        return column;
    }
    let count = count as f64;
    let index = stack as f64;
    let span = (axis.cross_span(column) - gaps * (count - 1.0)) / count;
    axis.rect(
        axis.main_start(column),
        axis.cross_start(column) + index * (span.max(0.0) + gaps),
        axis.main_span(column),
        span.max(0.0),
    )
}

/// Decides where the view sits, and records where the focused column ended up.
///
/// The view follows the focused column. When the focus has not moved it keeps
/// the column where it already was on screen: the offset is re-derived from the
/// column's place in the strip, so columns opening or closing *before* the
/// focused one move the strip but not the window. A change of focus — or a
/// [`LayoutCommand::CenterColumn`] — applies the configured policy instead.
fn update_view(
    scroll: &mut ScrollState,
    focused_window: Option<WindowId>,
    focused_column: Option<usize>,
    strip: &Strip,
    view_span: f64,
    params: &LayoutParams,
) {
    let Some(index) = focused_column else {
        // Nothing in the strip has the focus: leave the view alone. The anchor
        // stays too — the compositor arranges once with a newly opened window
        // focused but not yet mapped, and the anchor is what tells the next
        // arrange which column that window should appear beside.
        if scroll.columns.is_empty() {
            scroll.offset = 0.0;
        }
        scroll.center_request = false;
        return;
    };

    let (start, span) = (strip.start(index), strip.span(index));
    let max_offset = (strip.total() - view_span).max(0.0);
    // `into_view` after the clamp is a no-op in practice, but it keeps the
    // focused column visible whatever the clamp decides.
    let settled = |offset: f64| into_view(offset, start, span, view_span).clamp(0.0, max_offset);

    let follow = scroll.center_request || scroll.anchor.window != focused_window;
    let offset = if follow {
        settled(match params.center {
            ScrollCenter::Always => centered(start, span, view_span),
            ScrollCenter::Never => into_view(scroll.offset, start, span, view_span),
            ScrollCenter::OnOverflow => {
                // Centre only what cannot sit beside the column that held the
                // focus before (niri's `on-overflow`).
                if span + params.gaps + scroll.anchor.span > view_span {
                    centered(start, span, view_span)
                } else {
                    into_view(scroll.offset, start, span, view_span)
                }
            }
        })
    } else {
        settled(start - scroll.anchor.screen)
    };

    scroll.offset = offset;
    scroll.anchor = ScrollAnchor {
        window: focused_window,
        start,
        span,
        screen: start - offset,
    };
    scroll.center_request = false;
}

/// The offset that scrolls `start..start + span` into view with the least
/// movement from `offset`.
fn into_view(offset: f64, start: f64, span: f64, view: f64) -> f64 {
    if span >= view {
        // Wider than the view: show its leading edge and let the rest spill.
        return start;
    }
    if start < offset {
        start
    } else if start + span > offset + view {
        start + span - view
    } else {
        offset
    }
}

/// The offset that puts `start..start + span` in the middle of the view.
fn centered(start: f64, span: f64, view: f64) -> f64 {
    start + (span - view) / 2.0
}

/// The index of the column holding `window`, if any.
fn column_of(scroll: &ScrollState, window: WindowId) -> Option<usize> {
    scroll
        .columns
        .iter()
        .position(|column| column.windows.contains(&window))
}

/// Moves the column holding `window` one place along the strip.
///
/// Returns whether the strip changed: the first and last columns cannot move
/// outward. The focused window keeps its place on screen because
/// [`update_view`] re-derives the offset from the new strip position.
fn move_column(scroll: &mut ScrollState, window: WindowId, forward: bool) -> bool {
    let Some(index) = column_of(scroll, window) else {
        return false;
    };
    let target = if forward {
        index + 1
    } else {
        let Some(target) = index.checked_sub(1) else {
            return false;
        };
        target
    };
    if target >= scroll.columns.len() {
        return false;
    }
    scroll.columns.swap(index, target);
    true
}

/// Moves the window into the neighbouring column.
///
/// The source column is dropped when the move empties it, which is what makes
/// [`LayoutCommand::ConsumeWindowIntoColumn`] merge a one-window column whole.
fn move_window_to_column(scroll: &mut ScrollState, window: WindowId, forward: bool) -> bool {
    let Some(source) = column_of(scroll, window) else {
        return false;
    };
    let Some(mut target) = (if forward {
        Some(source + 1)
    } else {
        source.checked_sub(1)
    }) else {
        return false;
    };
    if target >= scroll.columns.len() {
        return false;
    }

    scroll.columns[source].windows.retain(|id| *id != window);
    if scroll.columns[source].windows.is_empty() {
        scroll.columns.remove(source);
        if source < target {
            target -= 1;
        }
    }
    scroll.columns[target].windows.push(window);
    true
}

/// Splits the focused window out of its column into a new column on the right.
///
/// A column holding only that window has nothing to split, so the command is a
/// no-op there (niri's `expel-window-from-column`).
fn expel_window_from_column(scroll: &mut ScrollState, window: WindowId) -> bool {
    let Some(index) = column_of(scroll, window) else {
        return false;
    };
    if scroll.columns[index].windows.len() <= 1 {
        return false;
    }
    scroll.columns[index].windows.retain(|id| *id != window);
    let width = scroll.columns[index].width;
    let display = scroll.columns[index].display;
    scroll.columns.insert(
        index + 1,
        Column {
            width,
            windows: vec![window],
            display,
        },
    );
    true
}

/// Sets the width of the column holding `window`.
fn set_column_width(scroll: &mut ScrollState, window: WindowId, width: ColumnWidth) -> bool {
    let Some(index) = column_of(scroll, window) else {
        return false;
    };
    scroll.columns[index].width = width;
    true
}

/// Cycles the column holding `window` through the configured width presets.
fn switch_preset_width(scroll: &mut ScrollState, window: WindowId, presets: &WidthPresets) -> bool {
    let Some(index) = column_of(scroll, window) else {
        return false;
    };
    let current = scroll.columns[index].width;
    let Some(next) = presets.next_after(current) else {
        return false;
    };
    scroll.columns[index].width = next;
    true
}

/// Directional focus within the strip.
///
/// The main-axis directions walk to the neighbouring column and land on the
/// window in the same slot, clamped, so a shorter column still takes the focus;
/// the cross-axis directions walk the stack inside the current column. A window
/// the model does not know — it has not been arranged since it appeared — falls
/// back to the geometric search every other layout uses.
fn neighbor_scroll(
    axis: ScrollAxis,
    input: &LayoutInput<'_>,
    from: usize,
    dir: Direction,
) -> Option<usize> {
    let window = input.window_at(from)?.id;
    let scroll: &ScrollState = &*input.scroll;
    let Some(column) = column_of(scroll, window) else {
        return geometric_neighbor(axis, input, from, dir);
    };
    let stack = scroll.columns[column]
        .windows
        .iter()
        .position(|id| *id == window)?;

    if let Some(forward) = axis.cross_direction(dir) {
        let next = if forward {
            stack + 1
        } else {
            stack.checked_sub(1)?
        };
        let id = *scroll.columns[column].windows.get(next)?;
        return input.index_of(id);
    }

    let forward = axis.main_direction(dir)?;
    let next = if forward {
        column + 1
    } else {
        column.checked_sub(1)?
    };
    let neighbour = scroll.columns.get(next)?;
    let slot = stack.min(neighbour.windows.len().checked_sub(1)?);
    input.index_of(*neighbour.windows.get(slot)?)
}

/// The default geometric search, for a window the column model does not know.
fn geometric_neighbor(
    axis: ScrollAxis,
    input: &LayoutInput<'_>,
    from: usize,
    dir: Direction,
) -> Option<usize> {
    let mut scratch = ScrollState::default();
    let placements = Scroll::new(axis).arrange(LayoutInput {
        area: input.area,
        windows: input.windows,
        focused: input.focused,
        params: input.params,
        scroll: &mut scratch,
    });
    directional_neighbor(&placements, from, dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{ColumnWidth, Size};

    const AREA: Rect = Rect::new(0.0, 0.0, 1000.0, 800.0);
    const HORIZONTAL: ScrollAxis = ScrollAxis::Horizontal;

    fn list(count: usize) -> Vec<LayoutWindow> {
        (0..count)
            .map(|index| LayoutWindow::new(WindowId::new(index as u64 + 1)))
            .collect()
    }

    fn params() -> LayoutParams {
        LayoutParams {
            gaps: 0.0,
            ..LayoutParams::default()
        }
    }

    fn arrange(
        axis: ScrollAxis,
        windows: &[LayoutWindow],
        focused: Option<usize>,
        params: &LayoutParams,
        scroll: &mut ScrollState,
    ) -> Vec<Placement> {
        Scroll::new(axis).arrange(LayoutInput {
            area: AREA,
            windows,
            focused,
            params,
            scroll,
        })
    }

    fn rect(placements: &[Placement], index: usize) -> Rect {
        placements[index].rect
    }

    #[test]
    fn new_windows_become_columns_in_order() {
        let windows = list(3);
        let mut scroll = ScrollState::default();
        let placements = arrange(HORIZONTAL, &windows, Some(0), &params(), &mut scroll);

        assert_eq!(placements.len(), 3);
        assert_eq!(scroll.columns.len(), 3);
        // Half the view each, side by side, with the focused one at the edge.
        assert_eq!(rect(&placements, 0), Rect::new(0.0, 0.0, 500.0, 800.0));
        assert_eq!(rect(&placements, 1), Rect::new(500.0, 0.0, 500.0, 800.0));
        assert_eq!(rect(&placements, 2), Rect::new(1000.0, 0.0, 500.0, 800.0));
        assert_eq!(scroll.offset, 0.0, "the first column is already in view");
    }

    #[test]
    fn opening_a_window_leaves_the_other_columns_alone() {
        // The exit criterion of TODO §3.3: a new window must not reflow the
        // columns that were already there. It becomes a column of its own just
        // past the focused one; nothing changes size and the focus does not
        // move, so with the focus already on the last column the whole strip
        // stays put.
        let a = list(2);
        let b = list(3);
        let mut scroll = ScrollState::default();
        let before = arrange(HORIZONTAL, &a, Some(1), &params(), &mut scroll);
        assert_eq!(rect(&before, 0), Rect::new(0.0, 0.0, 500.0, 800.0));
        assert_eq!(rect(&before, 1), Rect::new(500.0, 0.0, 500.0, 800.0));

        let after = arrange(HORIZONTAL, &b, Some(1), &params(), &mut scroll);
        assert_eq!(scroll.columns.len(), 3, "the new window is a column");
        assert_eq!(rect(&after, 0), rect(&before, 0));
        assert_eq!(rect(&after, 1), rect(&before, 1), "the focused one too");
        assert_eq!(scroll.offset, 0.0, "and the view did not move");
        assert_eq!(rect(&after, 2), Rect::new(1000.0, 0.0, 500.0, 800.0));

        // Again once the strip is long enough for the focus to have scrolled
        // the view: a window arriving off to the right changes nothing at all.
        let focused = 2;
        let scrolled = arrange(HORIZONTAL, &b, Some(focused), &params(), &mut scroll);
        let grown = arrange(HORIZONTAL, &list(4), Some(focused), &params(), &mut scroll);
        assert_eq!(rect(&scrolled, 0), rect(&grown, 0));
        assert_eq!(rect(&scrolled, 1), rect(&grown, 1));
        assert_eq!(rect(&scrolled, 2), rect(&grown, 2));
        assert_eq!(scroll.offset, 500.0, "the view did not move either");
        assert_eq!(rect(&grown, 3), Rect::new(1000.0, 0.0, 500.0, 800.0));
    }

    #[test]
    fn a_new_window_opens_right_of_the_focused_column() {
        let windows = list(4);
        let mut scroll = ScrollState::default();
        // First three arrive together; the focus is on the middle one.
        arrange(HORIZONTAL, &windows[..3], Some(1), &params(), &mut scroll);
        assert_eq!(scroll.columns.len(), 3);
        // The fourth is new and focused, so it lands beside the middle column.
        arrange(HORIZONTAL, &windows, Some(3), &params(), &mut scroll);
        let order: Vec<WindowId> = scroll
            .columns
            .iter()
            .map(|column| column.windows[0])
            .collect();
        assert_eq!(
            order,
            vec![
                WindowId::new(1),
                WindowId::new(2),
                WindowId::new(4),
                WindowId::new(3)
            ]
        );
    }

    #[test]
    fn a_window_opened_between_arranges_still_lands_beside_the_focused_column() {
        // Opening a window takes two arranges in practice: the compositor
        // arranges once with the new window focused but not yet mapped (so the
        // layout sees nothing focused at all) and once with it in the list. The
        // "beside the focused column" memory has to survive that.
        let windows = list(3);
        let mut scroll = ScrollState::default();
        arrange(HORIZONTAL, &windows, Some(1), &params(), &mut scroll);

        let nothing_focused = arrange(HORIZONTAL, &windows, None, &params(), &mut scroll);
        assert_eq!(nothing_focused.len(), 3, "the strip is untouched");

        arrange(HORIZONTAL, &list(4), Some(3), &params(), &mut scroll);
        let order: Vec<WindowId> = scroll
            .columns
            .iter()
            .map(|column| column.windows[0])
            .collect();
        assert_eq!(
            order,
            vec![
                WindowId::new(1),
                WindowId::new(2),
                WindowId::new(4),
                WindowId::new(3)
            ],
            "the new window is a column just past the one that had the focus"
        );
    }

    #[test]
    fn closing_a_column_leaves_the_focused_window_where_it_was() {
        let windows = list(3);
        let mut scroll = ScrollState::default();
        let before = arrange(HORIZONTAL, &windows, Some(2), &params(), &mut scroll);
        assert_eq!(rect(&before, 2), Rect::new(500.0, 0.0, 500.0, 800.0));

        // The first column goes: the strip shifts left under the view, but the
        // focused window must stay exactly where it was.
        let full = list(3);
        let remaining = &full[1..];
        let after = arrange(HORIZONTAL, remaining, Some(1), &params(), &mut scroll);
        assert_eq!(
            rect(&after, 1),
            Rect::new(500.0, 0.0, 500.0, 800.0),
            "the focused window did not move"
        );
        assert_eq!(scroll.offset, 0.0);
    }

    #[test]
    fn centering_policy_moves_the_view() {
        let windows = list(3);
        let mut scroll = ScrollState::default();
        let centered = LayoutParams {
            gaps: 0.0,
            center: ScrollCenter::Always,
            ..LayoutParams::default()
        };
        let placements = arrange(HORIZONTAL, &windows, Some(1), &centered, &mut scroll);
        // The middle column's centre lands on the view's centre.
        let middle = rect(&placements, 1);
        assert_eq!(middle.center().x, AREA.center().x);
        assert_eq!(scroll.offset, 250.0);
    }

    #[test]
    fn on_overflow_centers_only_what_does_not_fit() {
        let windows = list(3);

        // Two half-view columns fit side by side, so nothing is centred.
        let half = LayoutParams {
            gaps: 0.0,
            center: ScrollCenter::OnOverflow,
            ..LayoutParams::default()
        };
        let mut scroll = ScrollState::default();
        arrange(HORIZONTAL, &windows, Some(0), &half, &mut scroll);
        let placements = arrange(HORIZONTAL, &windows, Some(1), &half, &mut scroll);
        let middle = rect(&placements, 1);
        assert_eq!(middle.x, 500.0, "the column simply slid into view");
        assert_eq!(scroll.offset, 0.0);

        // Three-quarter columns cannot, so the new one is centred.
        let wide = LayoutParams {
            gaps: 0.0,
            center: ScrollCenter::OnOverflow,
            column_width: ColumnWidth::Proportion(0.75),
            ..LayoutParams::default()
        };
        let mut scroll = ScrollState::default();
        arrange(HORIZONTAL, &windows, Some(0), &wide, &mut scroll);
        let placements = arrange(HORIZONTAL, &windows, Some(1), &wide, &mut scroll);
        let middle = rect(&placements, 1);
        assert_eq!(middle.center().x, AREA.center().x);
    }

    #[test]
    fn overspread_decides_whether_a_wide_column_spills() {
        let windows = list(1);
        let wide = LayoutParams {
            gaps: 0.0,
            column_width: ColumnWidth::Fixed(2000.0),
            ..LayoutParams::default()
        };
        let mut scroll = ScrollState::default();
        let spilled = arrange(HORIZONTAL, &windows, Some(0), &wide, &mut scroll);
        assert_eq!(rect(&spilled, 0).w, 2000.0);

        let clipped = LayoutParams {
            overspread: false,
            ..wide
        };
        let mut scroll = ScrollState::default();
        let fitted = arrange(HORIZONTAL, &windows, Some(0), &clipped, &mut scroll);
        assert_eq!(rect(&fitted, 0).w, AREA.w);
    }

    #[test]
    fn a_minimum_size_widens_its_column() {
        let mut windows = list(2);
        windows[0].min_size = Some(Size::new(400.0, 100.0));
        windows[1].min_size = Some(Size::new(700.0, 100.0));
        let mut scroll = ScrollState::default();
        let placements = arrange(HORIZONTAL, &windows, Some(0), &params(), &mut scroll);
        assert_eq!(rect(&placements, 0).w, 500.0, "the width is the wider ask");
        assert_eq!(rect(&placements, 1).w, 700.0, "the minimum wins");
        assert_eq!(rect(&placements, 1).x, 500.0, "and the next column moves");
    }

    #[test]
    fn windows_in_a_column_split_it_along_the_cross_axis() {
        // Two windows in one column: the model, not `arrange`, decides that.
        let windows = list(2);
        let mut scroll = ScrollState {
            columns: vec![Column {
                width: ColumnWidth::DEFAULT,
                windows: vec![WindowId::new(1), WindowId::new(2)],
                display: ColumnDisplay::Normal,
            }],
            ..ScrollState::default()
        };
        let placements = arrange(HORIZONTAL, &windows, Some(0), &params(), &mut scroll);
        assert_eq!(rect(&placements, 0), Rect::new(0.0, 0.0, 500.0, 400.0));
        assert_eq!(rect(&placements, 1), Rect::new(0.0, 400.0, 500.0, 400.0));
        assert!(placements.iter().all(|placement| placement.visible));
    }

    #[test]
    fn tabbed_shows_only_the_focused_window() {
        let windows = list(2);
        let mut scroll = ScrollState {
            columns: vec![Column {
                width: ColumnWidth::DEFAULT,
                windows: vec![WindowId::new(1), WindowId::new(2)],
                display: ColumnDisplay::Tabbed,
            }],
            ..ScrollState::default()
        };
        let placements = arrange(HORIZONTAL, &windows, Some(1), &params(), &mut scroll);
        assert!(!placements[0].visible);
        assert!(placements[1].visible);
        // Both keep the column's rectangle, so switching tabs is instant.
        assert_eq!(rect(&placements, 0), rect(&placements, 1));
        assert_eq!(rect(&placements, 1), Rect::new(0.0, 0.0, 500.0, 800.0));
    }

    #[test]
    fn the_vertical_variant_swaps_the_axes() {
        let windows = list(3);
        let mut scroll = ScrollState::default();
        let placements = arrange(
            ScrollAxis::Vertical,
            &windows,
            Some(0),
            &params(),
            &mut scroll,
        );
        assert_eq!(rect(&placements, 0), Rect::new(0.0, 0.0, 1000.0, 400.0));
        assert_eq!(rect(&placements, 1), Rect::new(0.0, 400.0, 1000.0, 400.0));
        assert_eq!(rect(&placements, 2), Rect::new(0.0, 800.0, 1000.0, 400.0));
    }

    #[test]
    fn neighbor_walks_columns_and_stacks() {
        let windows = list(4);
        let mut scroll = ScrollState::default();
        arrange(HORIZONTAL, &windows, Some(0), &params(), &mut scroll);
        let params = params();
        let input = LayoutInput {
            area: AREA,
            windows: &windows,
            focused: Some(0),
            params: &params,
            scroll: &mut scroll,
        };
        let scroll_layout = Scroll::new(HORIZONTAL);

        assert_eq!(scroll_layout.neighbor(&input, 0, Direction::Right), Some(1));
        assert_eq!(scroll_layout.neighbor(&input, 1, Direction::Left), Some(0));
        assert_eq!(scroll_layout.neighbor(&input, 3, Direction::Right), None);
        assert_eq!(scroll_layout.neighbor(&input, 0, Direction::Left), None);
        // One window per column: up and down have nowhere to go.
        assert_eq!(scroll_layout.neighbor(&input, 1, Direction::Down), None);

        // Give the second column a stack and the cross axis comes alive.
        scroll.columns[1].windows = vec![WindowId::new(2), WindowId::new(5)];
        let windows = list(5);
        let input = LayoutInput {
            area: AREA,
            windows: &windows,
            focused: Some(1),
            params: &params,
            scroll: &mut scroll,
        };
        assert_eq!(scroll_layout.neighbor(&input, 1, Direction::Down), Some(4));
        assert_eq!(scroll_layout.neighbor(&input, 4, Direction::Up), Some(1));
        assert_eq!(scroll_layout.neighbor(&input, 1, Direction::Right), Some(2));
    }

    #[test]
    fn arranging_twice_gives_the_same_answer() {
        let windows = list(5);
        let params = params();
        let mut scroll = ScrollState::default();
        let first = arrange(HORIZONTAL, &windows, Some(2), &params, &mut scroll);
        let second = arrange(HORIZONTAL, &windows, Some(2), &params, &mut scroll);
        assert_eq!(first, second);
    }

    #[test]
    fn an_empty_view_clears_the_strip() {
        let windows = list(3);
        let mut scroll = ScrollState::default();
        arrange(HORIZONTAL, &windows, Some(2), &params(), &mut scroll);
        assert!(!scroll.columns.is_empty());

        let placements = arrange(HORIZONTAL, &[], None, &params(), &mut scroll);
        assert!(placements.is_empty());
        assert!(scroll.columns.is_empty());
        assert_eq!(scroll.offset, 0.0);
    }

    #[test]
    fn commands_change_the_focused_column() {
        let windows = list(2);
        let mut params = params();
        let mut scroll = ScrollState::default();
        arrange(HORIZONTAL, &windows, Some(0), &params, &mut scroll);
        let scroll_layout = Scroll::new(HORIZONTAL);

        // Cycling walks the presets: 1/3, 1/2, 2/3, 1/3…
        scroll_layout.command(
            LayoutCommand::SwitchPresetWidth,
            Some(WindowId::new(1)),
            &mut params,
            &mut scroll,
        );
        assert_eq!(
            scroll.columns[0].width,
            ColumnWidth::Proportion(2.0 / 3.0),
            "half comes after a third"
        );
        scroll_layout.command(
            LayoutCommand::SwitchPresetWidth,
            Some(WindowId::new(1)),
            &mut params,
            &mut scroll,
        );
        assert_eq!(scroll.columns[0].width, ColumnWidth::Proportion(1.0 / 3.0));
        assert_eq!(
            scroll.columns[1].width,
            ColumnWidth::DEFAULT,
            "only the focused one"
        );

        // The width takes effect on the next arrange.
        let placements = arrange(HORIZONTAL, &windows, Some(0), &params, &mut scroll);
        assert!((rect(&placements, 0).w - 1000.0 / 3.0).abs() < 1e-9);

        // And centring is honoured once, by the next arrange.
        scroll_layout.command(
            LayoutCommand::CenterColumn,
            Some(WindowId::new(1)),
            &mut params,
            &mut scroll,
        );
        assert!(scroll.center_request);
        arrange(HORIZONTAL, &windows, Some(0), &params, &mut scroll);
        assert!(!scroll.center_request, "the request is consumed");
    }

    /// The window at the head of each column, i.e. the strip's order.
    fn order(scroll: &ScrollState) -> Vec<WindowId> {
        scroll
            .columns
            .iter()
            .map(|column| column.windows[0])
            .collect()
    }

    /// Arranges `count` windows with the focus on `focused` and returns the
    /// scroller state plus its parameters.
    fn arranged(count: usize, focused: usize) -> (ScrollState, LayoutParams) {
        let windows = list(count);
        let params = params();
        let mut scroll = ScrollState::default();
        arrange(HORIZONTAL, &windows, Some(focused), &params, &mut scroll);
        (scroll, params)
    }

    #[test]
    fn move_column_reorders_and_clamps_at_the_edges() {
        let (mut scroll, mut params) = arranged(3, 1);
        let layout = Scroll::new(HORIZONTAL);
        let window = WindowId::new(2);

        // The focused middle column moves right, then cannot move further.
        layout.command(
            LayoutCommand::MoveColumnRight,
            Some(window),
            &mut params,
            &mut scroll,
        );
        assert_eq!(
            order(&scroll),
            vec![WindowId::new(1), WindowId::new(3), WindowId::new(2)]
        );
        layout.command(
            LayoutCommand::MoveColumnRight,
            Some(window),
            &mut params,
            &mut scroll,
        );
        assert_eq!(
            order(&scroll),
            vec![WindowId::new(1), WindowId::new(3), WindowId::new(2)],
            "the last column cannot move outward"
        );

        // And back to the start.
        for _ in 0..2 {
            layout.command(
                LayoutCommand::MoveColumnLeft,
                Some(window),
                &mut params,
                &mut scroll,
            );
        }
        assert_eq!(
            order(&scroll),
            vec![WindowId::new(2), WindowId::new(1), WindowId::new(3)]
        );
        layout.command(
            LayoutCommand::MoveColumnLeft,
            Some(window),
            &mut params,
            &mut scroll,
        );
        assert_eq!(order(&scroll)[0], WindowId::new(2), "clamped at the start");
    }

    #[test]
    fn move_window_to_column_merges_and_drops_the_empty_source() {
        let (mut scroll, mut params) = arranged(3, 1);
        let layout = Scroll::new(HORIZONTAL);
        let window = WindowId::new(2);

        layout.command(
            LayoutCommand::MoveWindowToColumnRight,
            Some(window),
            &mut params,
            &mut scroll,
        );
        assert_eq!(
            order(&scroll),
            vec![WindowId::new(1), WindowId::new(3)],
            "the emptied source column went away"
        );
        assert_eq!(
            scroll.columns[1].windows,
            vec![WindowId::new(3), WindowId::new(2)],
            "the window joined the right column"
        );
    }

    #[test]
    fn consume_merges_the_focused_column_into_the_left() {
        let (mut scroll, mut params) = arranged(3, 1);
        let layout = Scroll::new(HORIZONTAL);
        let window = WindowId::new(2);

        layout.command(
            LayoutCommand::ConsumeWindowIntoColumn,
            Some(window),
            &mut params,
            &mut scroll,
        );
        assert_eq!(order(&scroll), vec![WindowId::new(1), WindowId::new(3)]);
        assert_eq!(
            scroll.columns[0].windows,
            vec![WindowId::new(1), WindowId::new(2)]
        );

        // A column with several windows gives up only the focused one.
        scroll.columns[0].windows = vec![WindowId::new(1), WindowId::new(2)];
        scroll.columns[1].windows = vec![WindowId::new(3)];
        layout.command(
            LayoutCommand::ConsumeWindowIntoColumn,
            Some(WindowId::new(3)),
            &mut params,
            &mut scroll,
        );
        assert_eq!(
            scroll.columns[0].windows,
            vec![WindowId::new(1), WindowId::new(2), WindowId::new(3)]
        );
        assert_eq!(scroll.columns.len(), 1);
    }

    #[test]
    fn expel_splits_a_window_into_a_new_column() {
        let windows = list(2);
        let (mut scroll, mut params) = arranged(2, 0);
        let layout = Scroll::new(HORIZONTAL);
        scroll.columns = vec![Column {
            width: ColumnWidth::Fixed(700.0),
            windows: vec![WindowId::new(1), WindowId::new(2)],
            display: ColumnDisplay::Normal,
        }];

        layout.command(
            LayoutCommand::ExpelWindowFromColumn,
            Some(WindowId::new(2)),
            &mut params,
            &mut scroll,
        );
        assert_eq!(scroll.columns.len(), 2);
        assert_eq!(scroll.columns[0].windows, vec![WindowId::new(1)]);
        assert_eq!(scroll.columns[1].windows, vec![WindowId::new(2)]);
        assert_eq!(
            scroll.columns[1].width,
            ColumnWidth::Fixed(700.0),
            "the new column keeps the source width"
        );
        let placements = arrange(HORIZONTAL, &windows, Some(1), &params, &mut scroll);
        assert_eq!(placements.len(), 2, "both windows are still placed");

        // A one-window column has nothing to split.
        layout.command(
            LayoutCommand::ExpelWindowFromColumn,
            Some(WindowId::new(2)),
            &mut params,
            &mut scroll,
        );
        assert_eq!(scroll.columns.len(), 2);
    }

    #[test]
    fn expand_and_set_width_resize_the_focused_column() {
        let windows = list(3);
        let (mut scroll, mut params) = arranged(3, 0);
        let layout = Scroll::new(HORIZONTAL);
        let window = WindowId::new(1);

        layout.command(
            LayoutCommand::ExpandColumnToAvailableWidth,
            Some(window),
            &mut params,
            &mut scroll,
        );
        assert_eq!(scroll.columns[0].width, ColumnWidth::Proportion(1.0));
        let placements = arrange(HORIZONTAL, &windows, Some(0), &params, &mut scroll);
        assert_eq!(rect(&placements, 0).w, AREA.w, "the column fills the view");

        layout.command(
            LayoutCommand::SetColumnWidth(ColumnWidth::Fixed(320.0)),
            Some(window),
            &mut params,
            &mut scroll,
        );
        assert_eq!(scroll.columns[0].width, ColumnWidth::Fixed(320.0));
        let placements = arrange(HORIZONTAL, &windows, Some(0), &params, &mut scroll);
        assert_eq!(rect(&placements, 0).w, 320.0);
    }

    #[test]
    fn commands_without_a_focused_window_do_nothing() {
        let (mut scroll, mut params) = arranged(3, 0);
        let before = scroll.clone();
        let layout = Scroll::new(HORIZONTAL);
        for command in [
            LayoutCommand::SwitchPresetWidth,
            LayoutCommand::MoveColumnLeft,
            LayoutCommand::MoveColumnRight,
            LayoutCommand::MoveWindowToColumnLeft,
            LayoutCommand::MoveWindowToColumnRight,
            LayoutCommand::ConsumeWindowIntoColumn,
            LayoutCommand::ExpelWindowFromColumn,
            LayoutCommand::ExpandColumnToAvailableWidth,
            LayoutCommand::SetColumnWidth(ColumnWidth::Fixed(100.0)),
        ] {
            layout.command(command, None, &mut params, &mut scroll);
        }
        assert_eq!(scroll.columns, before.columns);
        assert_eq!(scroll.offset, before.offset);
    }
}

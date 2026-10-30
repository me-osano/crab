//! Balanced-grid tiling — the `Fair` layout (`COMPOSITOR.md` §7.2).
//!
//! Windows are laid out in `ceil(sqrt(n))` equal-width columns, filled top to
//! bottom and left to right. When `n` does not divide evenly the first columns
//! hold one fewer window than the last ones, so the grid stays balanced instead
//! of leaving a ragged final column. This is mango's `fair`.

use super::placement::{Layout, LayoutInput, Placement, apply_hints};
use crate::core::{LayoutKind, Rect};

/// Balanced-grid tiling.
#[derive(Clone, Copy, Debug, Default)]
pub struct Fair;

impl Layout for Fair {
    fn kind(&self) -> LayoutKind {
        LayoutKind::Fair
    }

    fn arrange(&self, input: LayoutInput<'_>) -> Vec<Placement> {
        arrange_fair(&input)
    }
}

/// Computes balanced-grid placements for `input`.
pub(super) fn arrange_fair(input: &LayoutInput<'_>) -> Vec<Placement> {
    let count = input.windows.len();
    if count == 0 {
        return Vec::new();
    }

    let area = input.usable_area();
    let gaps = input.params.gaps;

    // The smallest column count whose square holds every window.
    let cols = (1..=count)
        .find(|cols| cols * cols >= count)
        .expect("count is at least one");
    let base_rows = count / cols;
    let remainder = count % cols;
    let first_group_cols = cols - remainder;
    let first_group_count = first_group_cols * base_rows;
    let max_rows = base_rows + usize::from(remainder > 0);

    // Every column is equally wide. The shorter columns use the whole height
    // for `base_rows` rows; the taller ones split it across `max_rows`.
    let column_span = span(area.w, cols, gaps);
    let base_row_span = span(area.h, base_rows, gaps);
    let max_row_span = span(area.h, max_rows, gaps);

    input
        .windows
        .iter()
        .enumerate()
        .map(|(index, window)| {
            let (column, row, row_span) = if index < first_group_count {
                (index / base_rows, index % base_rows, base_row_span)
            } else {
                let offset = index - first_group_count;
                (
                    first_group_cols + offset / max_rows,
                    offset % max_rows,
                    max_row_span,
                )
            };
            let rect = Rect::new(
                area.x + column as f64 * (column_span + gaps),
                area.y + row as f64 * (row_span + gaps),
                column_span,
                row_span,
            );
            Placement {
                window: window.id,
                rect: apply_hints(rect, window),
                visible: true,
                z_hint: 0,
            }
        })
        .collect()
}

/// One slot's extent: the usable span minus the gaps between slots.
fn span(total: f64, slots: usize, gaps: f64) -> f64 {
    let gaps = gaps * slots.saturating_sub(1) as f64;
    ((total - gaps) / slots as f64).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::super::placement::{Layout as _, LayoutInput, LayoutWindow};
    use super::Fair;
    use crate::core::{LayoutParams, Rect, ScrollState, Size, WindowId};

    fn windows(count: usize) -> Vec<LayoutWindow> {
        (0..count)
            .map(|i| LayoutWindow::new(WindowId::new(i as u64 + 1)))
            .collect()
    }

    fn arrange_with(count: usize, params: LayoutParams) -> Vec<Rect> {
        let windows = windows(count);
        let mut scroll = ScrollState::default();
        let placements = Fair.arrange(LayoutInput {
            area: Rect::new(0.0, 0.0, 1000.0, 1000.0),
            windows: &windows,
            focused: Some(0),
            params: &params,
            scroll: &mut scroll,
        });
        placements.into_iter().map(|p| p.rect).collect()
    }

    #[test]
    fn single_window_fills_the_area() {
        let params = LayoutParams {
            gaps: 10.0,
            ..Default::default()
        };
        assert_eq!(
            arrange_with(1, params),
            vec![Rect::new(10.0, 10.0, 980.0, 980.0)]
        );
    }

    #[test]
    fn two_windows_share_one_row() {
        let params = LayoutParams {
            gaps: 0.0,
            ..Default::default()
        };
        assert_eq!(
            arrange_with(2, params),
            vec![
                Rect::new(0.0, 0.0, 500.0, 1000.0),
                Rect::new(500.0, 0.0, 500.0, 1000.0),
            ]
        );
    }

    #[test]
    fn three_windows_balance_a_short_and_a_tall_column() {
        let params = LayoutParams {
            gaps: 0.0,
            ..Default::default()
        };
        assert_eq!(
            arrange_with(3, params),
            vec![
                Rect::new(0.0, 0.0, 500.0, 1000.0),
                Rect::new(500.0, 0.0, 500.0, 500.0),
                Rect::new(500.0, 500.0, 500.0, 500.0),
            ]
        );
    }

    #[test]
    fn four_windows_form_a_square_grid() {
        let params = LayoutParams {
            gaps: 0.0,
            ..Default::default()
        };
        assert_eq!(
            arrange_with(4, params),
            vec![
                Rect::new(0.0, 0.0, 500.0, 500.0),
                Rect::new(0.0, 500.0, 500.0, 500.0),
                Rect::new(500.0, 0.0, 500.0, 500.0),
                Rect::new(500.0, 500.0, 500.0, 500.0),
            ]
        );
    }

    #[test]
    fn twelve_windows_fill_four_by_three() {
        let params = LayoutParams {
            gaps: 0.0,
            ..Default::default()
        };
        let rects = arrange_with(12, params);
        assert_eq!(rects.len(), 12);
        assert_eq!(rects[0], Rect::new(0.0, 0.0, 250.0, 1000.0 / 3.0));
        assert_eq!(rects[3], Rect::new(250.0, 0.0, 250.0, 1000.0 / 3.0));
        assert_eq!(
            rects[11],
            Rect::new(750.0, 2000.0 / 3.0, 250.0, 1000.0 / 3.0)
        );
    }

    #[test]
    fn gaps_separate_columns_and_rows() {
        let params = LayoutParams {
            gaps: 10.0,
            ..Default::default()
        };
        let rects = arrange_with(4, params);
        assert_eq!(rects[0], Rect::new(10.0, 10.0, 485.0, 485.0));
        assert_eq!(rects[1], Rect::new(10.0, 505.0, 485.0, 485.0));
        assert_eq!(rects[2], Rect::new(505.0, 10.0, 485.0, 485.0));
        assert_eq!(rects[3], Rect::new(505.0, 505.0, 485.0, 485.0));
    }

    #[test]
    fn min_size_is_a_soft_constraint() {
        let params = LayoutParams {
            gaps: 0.0,
            ..Default::default()
        };
        let mut windows = windows(2);
        windows[0].min_size = Some(Size::new(700.0, 0.0));
        let mut scroll = ScrollState::default();
        let placements = Fair.arrange(LayoutInput {
            area: Rect::new(0.0, 0.0, 1000.0, 1000.0),
            windows: &windows,
            focused: None,
            params: &params,
            scroll: &mut scroll,
        });
        assert_eq!(placements[0].rect.w, 700.0, "a wider minimum wins");
    }
}

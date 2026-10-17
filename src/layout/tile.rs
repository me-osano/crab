//! Master/stack tiling — the `Tile` layout (`COMPOSITOR.md` §7.2).
//!
//! The usable area is split into a **master** column/row and a **stack**. The
//! master holds `master_count` windows stacked along the perpendicular axis;
//! the stack lays its windows out in a grid. Which edge the master occupies is
//! the [`TilePosition`] parameter, so `Left`/`Right`/`Top`/`Bottom` all come
//! from one implementation (dwm/mango style).

use super::placement::{Layout, LayoutInput, Placement, apply_hints};
use crate::core::{LayoutKind, Rect, TilePosition};

/// Master/stack tiling.
#[derive(Clone, Copy, Debug, Default)]
pub struct Tile;

impl Layout for Tile {
    fn kind(&self) -> LayoutKind {
        LayoutKind::Tile
    }

    fn arrange(&self, input: LayoutInput<'_>) -> Vec<Placement> {
        arrange_tile(&input)
    }
}

/// Computes master/stack placements for `input`.
pub(super) fn arrange_tile(input: &LayoutInput<'_>) -> Vec<Placement> {
    let count = input.windows.len();
    if count == 0 {
        return Vec::new();
    }

    let area = input.usable_area();
    let gaps = input.params.gaps;
    let masters = (input.params.master_count as usize).min(count);
    let stack = count - masters;

    let (master_area, stack_area) = split(
        area,
        masters,
        stack,
        gaps,
        input.params.master_ratio,
        input.params.position,
    );

    let mut rects = Vec::with_capacity(count);
    if let Some(master_area) = master_area {
        // Windows in a master column stack vertically; a master row spreads sideways.
        rects.extend(strip(
            master_area,
            masters,
            gaps,
            input.params.position.is_vertical_split(),
        ));
    }
    if let Some(stack_area) = stack_area {
        // The stack runs along the same axis as the master (dwm's `tile`), which
        // is why `Tile` is a single-column stack and `Grid` is a separate layout.
        rects.extend(strip(
            stack_area,
            stack,
            gaps,
            input.params.position.is_vertical_split(),
        ));
    }

    input
        .windows
        .iter()
        .zip(rects)
        .map(|(window, rect)| Placement {
            window: window.id,
            rect: apply_hints(rect, window),
            visible: true,
            z_hint: 0,
        })
        .collect()
}

/// Splits `area` into the master and stack halves.
///
/// A half that has no windows comes back as `None`, and the surviving half
/// takes the whole area — so a single window fills it and `master_count = 0`
/// gives a pure stack.
fn split(
    area: Rect,
    masters: usize,
    stack: usize,
    gaps: f64,
    ratio: f64,
    position: TilePosition,
) -> (Option<Rect>, Option<Rect>) {
    if masters == 0 {
        return (None, Some(area));
    }
    if stack == 0 {
        return (Some(area), None);
    }

    let vertical = position.is_vertical_split();
    let span = if vertical { area.w } else { area.h };
    let primary = (span - gaps) * ratio.clamp(0.0, 1.0);
    let secondary = (span - gaps) - primary;

    let half = |x: f64, y: f64, w: f64, h: f64| {
        if vertical {
            Rect::new(x, y, w.max(0.0), h)
        } else {
            Rect::new(x, y, w, h.max(0.0))
        }
    };

    match position {
        TilePosition::Left => (
            Some(half(area.x, area.y, primary, area.h)),
            Some(half(area.x + primary + gaps, area.y, secondary, area.h)),
        ),
        TilePosition::Right => (
            Some(half(area.x + secondary + gaps, area.y, primary, area.h)),
            Some(half(area.x, area.y, secondary, area.h)),
        ),
        TilePosition::Top => (
            Some(half(area.x, area.y, area.w, primary)),
            Some(half(area.x, area.y + primary + gaps, area.w, secondary)),
        ),
        TilePosition::Bottom => (
            Some(half(area.x, area.y + secondary + gaps, area.w, primary)),
            Some(half(area.x, area.y, area.w, secondary)),
        ),
    }
}

/// Divides `area` into `count` equal slots stacked along one axis.
///
/// `vertical` means the slots are rows (a master *column* stacks its windows
/// top to bottom); otherwise they are columns.
fn strip(area: Rect, count: usize, gaps: f64, vertical: bool) -> Vec<Rect> {
    if count == 0 {
        return Vec::new();
    }
    let count = count as f64;
    (0..count as usize)
        .map(|index| {
            let index = index as f64;
            if vertical {
                let height = (area.h - gaps * (count - 1.0)) / count;
                Rect::new(
                    area.x,
                    area.y + index * (height + gaps),
                    area.w,
                    height.max(0.0),
                )
            } else {
                let width = (area.w - gaps * (count - 1.0)) / count;
                Rect::new(
                    area.x + index * (width + gaps),
                    area.y,
                    width.max(0.0),
                    area.h,
                )
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::placement::{Layout as _, LayoutInput, LayoutWindow};
    use super::Tile;
    use crate::core::{LayoutParams, Rect, ScrollState, Size, WindowId};

    fn windows(count: usize) -> Vec<LayoutWindow> {
        (0..count)
            .map(|i| LayoutWindow::new(WindowId::new(i as u64 + 1)))
            .collect()
    }

    fn arrange_with(count: usize, params: LayoutParams) -> Vec<Rect> {
        let windows = windows(count);
        let mut scroll = ScrollState::default();
        let placements = Tile.arrange(LayoutInput {
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
        let rects = arrange_with(1, params);
        assert_eq!(rects, vec![Rect::new(10.0, 10.0, 980.0, 980.0)]);
    }

    #[test]
    fn master_ratio_splits_the_column() {
        let params = LayoutParams {
            gaps: 0.0,
            master_ratio: 0.5,
            ..Default::default()
        };
        let rects = arrange_with(2, params);
        assert_eq!(rects[0], Rect::new(0.0, 0.0, 500.0, 1000.0));
        assert_eq!(rects[1], Rect::new(500.0, 0.0, 500.0, 1000.0));
    }

    #[test]
    fn stack_windows_tile_below_each_other() {
        let params = LayoutParams {
            gaps: 0.0,
            master_count: 1,
            master_ratio: 0.5,
            ..Default::default()
        };
        let rects = arrange_with(3, params);
        // Master fills the left column; the two stack windows split the right.
        assert_eq!(rects[0], Rect::new(0.0, 0.0, 500.0, 1000.0));
        assert_eq!(rects[1], Rect::new(500.0, 0.0, 500.0, 500.0));
        assert_eq!(rects[2], Rect::new(500.0, 500.0, 500.0, 500.0));
    }

    #[test]
    fn zero_master_is_a_pure_stack() {
        let params = LayoutParams {
            gaps: 0.0,
            master_count: 0,
            ..Default::default()
        };
        let rects = arrange_with(2, params);
        // With no master, the stack fills the whole area as rows.
        assert_eq!(rects[0], Rect::new(0.0, 0.0, 1000.0, 500.0));
        assert_eq!(rects[1], Rect::new(0.0, 500.0, 1000.0, 500.0));
    }

    #[test]
    fn top_position_splits_horizontally() {
        let params = LayoutParams {
            gaps: 0.0,
            master_ratio: 0.5,
            position: crate::core::TilePosition::Top,
            ..Default::default()
        };
        let rects = arrange_with(2, params);
        assert_eq!(rects[0], Rect::new(0.0, 0.0, 1000.0, 500.0));
        assert_eq!(rects[1], Rect::new(0.0, 500.0, 1000.0, 500.0));
    }

    #[test]
    fn min_size_is_a_soft_constraint() {
        let params = LayoutParams {
            gaps: 0.0,
            master_ratio: 0.5,
            ..Default::default()
        };
        let mut windows = windows(2);
        windows[0].min_size = Some(Size::new(700.0, 0.0));
        let mut scroll = ScrollState::default();
        let placements = Tile.arrange(LayoutInput {
            area: Rect::new(0.0, 0.0, 1000.0, 1000.0),
            windows: &windows,
            focused: None,
            params: &params,
            scroll: &mut scroll,
        });
        assert_eq!(placements[0].rect.w, 700.0, "a wider minimum wins");
    }
}

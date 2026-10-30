//! Integration test: the pure layout engine is reachable from `tests/` now that
//! the package exposes a library target (`src/lib.rs`).
//!
//! These are the *golden* layout tests (`COMPOSITOR.md` §7, TODO §2.8): exact
//! rectangles for small window counts. Randomized invariants live in
//! `tests/layout_props.rs`.

use crab::core::{Direction, LayoutKind, LayoutParams, Rect, ScrollState, WindowId};
use crab::layout::{Fair, Layout as _, LayoutInput, LayoutWindow, by_name};

fn windows(count: usize) -> Vec<LayoutWindow> {
    (0..count)
        .map(|i| LayoutWindow::new(WindowId::new(i as u64 + 1)))
        .collect()
}

fn params(gaps: f64) -> LayoutParams {
    LayoutParams {
        gaps,
        ..LayoutParams::default()
    }
}

fn arrange(
    layout: &dyn crab::layout::Layout,
    windows: &[LayoutWindow],
    params: &LayoutParams,
) -> Vec<Rect> {
    let mut scroll = ScrollState::default();
    layout
        .arrange(LayoutInput {
            area: Rect::new(0.0, 0.0, 1000.0, 1000.0),
            windows,
            focused: Some(0),
            params,
            scroll: &mut scroll,
        })
        .into_iter()
        .map(|placement| placement.rect)
        .collect()
}

#[test]
fn fair_arranges_from_outside_the_crate() {
    let windows = windows(2);
    let mut scroll = ScrollState::default();
    let placements = Fair.arrange(LayoutInput {
        area: Rect::new(0.0, 0.0, 1000.0, 1000.0),
        windows: &windows,
        focused: Some(1),
        params: &params(10.0),
        scroll: &mut scroll,
    });
    let visible: Vec<_> = placements.iter().filter(|p| p.visible).collect();
    assert_eq!(visible.len(), 2, "every tiled window is shown");
    assert_eq!(visible[0].window, WindowId::new(1));
    assert_eq!(visible[0].rect, Rect::new(10.0, 10.0, 485.0, 980.0));
    assert_eq!(visible[1].rect, Rect::new(505.0, 10.0, 485.0, 980.0));
}

#[test]
fn fair_golden_rectangles() {
    // One window fills the usable area.
    assert_eq!(
        arrange(&Fair, &windows(1), &params(0.0)),
        vec![Rect::new(0.0, 0.0, 1000.0, 1000.0)]
    );

    // Two windows: two equal columns.
    assert_eq!(
        arrange(&Fair, &windows(2), &params(0.0)),
        vec![
            Rect::new(0.0, 0.0, 500.0, 1000.0),
            Rect::new(500.0, 0.0, 500.0, 1000.0),
        ]
    );

    // Three windows: one short column and one tall column.
    assert_eq!(
        arrange(&Fair, &windows(3), &params(0.0)),
        vec![
            Rect::new(0.0, 0.0, 500.0, 1000.0),
            Rect::new(500.0, 0.0, 500.0, 500.0),
            Rect::new(500.0, 500.0, 500.0, 500.0),
        ]
    );
}

#[test]
fn fair_golden_rectangles_for_twelve_windows() {
    // A snapshot of the whole arrangement: four columns of three rows.
    let rects = arrange(&Fair, &windows(12), &params(0.0));
    assert_eq!(rects.len(), 12);
    assert_eq!(rects[0], Rect::new(0.0, 0.0, 250.0, 1000.0 / 3.0));
    assert_eq!(rects[3], Rect::new(250.0, 0.0, 250.0, 1000.0 / 3.0));
    assert_eq!(
        rects[11],
        Rect::new(750.0, 2000.0 / 3.0, 250.0, 1000.0 / 3.0)
    );
}

#[test]
fn fair_splits_never_overlap_and_stay_inside() {
    let area = Rect::new(0.0, 0.0, 1000.0, 1000.0);
    for count in 1..=12usize {
        let placements = arrange(&Fair, &windows(count), &params(8.0));
        assert_eq!(placements.len(), count);
        for rect in &placements {
            assert!(area.contains_rect(rect), "count {count}: {rect:?} escaped");
        }
        for (i, a) in placements.iter().enumerate() {
            for b in &placements[i + 1..] {
                assert!(
                    a.intersection(b).is_none(),
                    "count {count}: {a:?} overlaps {b:?}"
                );
            }
        }
    }
}

#[test]
fn directional_focus_moves_right_from_the_first_column() {
    let windows = windows(3);
    let p = params(0.0);
    let mut scroll = ScrollState::default();
    let input = LayoutInput {
        area: Rect::new(0.0, 0.0, 1000.0, 1000.0),
        windows: &windows,
        focused: Some(0),
        params: &p,
        scroll: &mut scroll,
    };
    // Window 0 is the lone window in the first column; moving right lands in
    // the taller second column.
    let right = Fair.neighbor(&input, 0, Direction::Right);
    assert!(matches!(right, Some(1 | 2)));
    // Window 0 — nothing is to its left.
    assert_eq!(Fair.neighbor(&input, 0, Direction::Left), None);
}

#[test]
fn layout_names_resolve() {
    assert_eq!(by_name("fair").unwrap().kind(), LayoutKind::Fair);
    assert_eq!(by_name("tile").unwrap().kind(), LayoutKind::Fair);
    assert_eq!(by_name("floating").unwrap().kind(), LayoutKind::Floating);
    assert_eq!(by_name("stack").unwrap().kind(), LayoutKind::Floating);
    assert!(by_name("monocle").is_none());
    assert!(by_name("vertical-scroll").is_none());
    assert!(by_name("nope").is_none());
}

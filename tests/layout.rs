//! Integration test: the pure layout engine is reachable from `tests/` now that
//! the package exposes a library target (`src/lib.rs`).
//!
//! These are the *golden* layout tests (`COMPOSITOR.md` §7, TODO §2.8): exact
//! rectangles for small window counts. Randomized invariants live in
//! `tests/layout_props.rs`.

use crawm::core::{Direction, LayoutKind, LayoutParams, Rect, ScrollState, WindowId};
use crawm::layout::{Layout as _, LayoutInput, LayoutWindow, Monocle, Tile, by_name};

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
    layout: &dyn crawm::layout::Layout,
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
fn monocle_arranges_from_outside_the_crate() {
    let windows = windows(2);
    let mut scroll = ScrollState::default();
    let placements = Monocle.arrange(LayoutInput {
        area: Rect::new(0.0, 0.0, 1000.0, 1000.0),
        windows: &windows,
        focused: Some(1),
        params: &params(10.0),
        scroll: &mut scroll,
    });
    let visible: Vec<_> = placements.iter().filter(|p| p.visible).collect();
    assert_eq!(visible.len(), 1, "only the focused window is shown");
    assert_eq!(visible[0].window, WindowId::new(2));
    assert_eq!(visible[0].rect, Rect::new(10.0, 10.0, 980.0, 980.0));
}

#[test]
fn tile_golden_rectangles() {
    // One window fills the usable area.
    assert_eq!(
        arrange(&Tile, &windows(1), &params(0.0)),
        vec![Rect::new(0.0, 0.0, 1000.0, 1000.0)]
    );

    // Two windows: a 50/50 split.
    let mut half = params(0.0);
    half.master_ratio = 0.5;
    assert_eq!(
        arrange(&Tile, &windows(2), &half),
        vec![
            Rect::new(0.0, 0.0, 500.0, 1000.0),
            Rect::new(500.0, 0.0, 500.0, 1000.0),
        ]
    );

    // Three windows, master_count 1: one master + a two-cell stack.
    let mut three = params(0.0);
    three.master_ratio = 0.5;
    three.master_count = 1;
    assert_eq!(
        arrange(&Tile, &windows(3), &three),
        vec![
            Rect::new(0.0, 0.0, 500.0, 1000.0),
            Rect::new(500.0, 0.0, 500.0, 500.0),
            Rect::new(500.0, 500.0, 500.0, 500.0),
        ]
    );
}

#[test]
fn tile_golden_rectangles_for_twelve_windows() {
    // A snapshot of the whole arrangement: 2 masters, 10 stack windows.
    let mut p = params(0.0);
    p.master_ratio = 0.5;
    p.master_count = 2;
    let rects = arrange(&Tile, &windows(12), &p);
    assert_eq!(rects.len(), 12);
    // Two masters stack down the left half.
    assert_eq!(rects[0], Rect::new(0.0, 0.0, 500.0, 500.0));
    assert_eq!(rects[1], Rect::new(0.0, 500.0, 500.0, 500.0));
    // Ten stack windows fill the right half as a single column of equal rows.
    assert_eq!(rects[2], Rect::new(500.0, 0.0, 500.0, 100.0));
    assert_eq!(rects[11], Rect::new(500.0, 900.0, 500.0, 100.0));
}

#[test]
fn tile_splits_never_overlap_and_stay_inside() {
    let area = Rect::new(0.0, 0.0, 1000.0, 1000.0);
    for count in 1..=12usize {
        let mut p = params(8.0);
        p.master_count = 1;
        let placements = arrange(&Tile, &windows(count), &p);
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
fn directional_focus_moves_right_from_master() {
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
    // Window 0 (master, left) — moving right lands on the stack.
    let right = Tile.neighbor(&input, 0, Direction::Right);
    assert!(matches!(right, Some(1 | 2)));
    // Window 0 — nothing is to its left.
    assert_eq!(Tile.neighbor(&input, 0, Direction::Left), None);
}

#[test]
fn layout_names_resolve() {
    assert_eq!(by_name("tile").unwrap().kind(), LayoutKind::Tile);
    assert_eq!(by_name("monocle").unwrap().kind(), LayoutKind::Monocle);
    assert_eq!(by_name("floating").unwrap().kind(), LayoutKind::Floating);
    assert!(by_name("nope").is_none());
}

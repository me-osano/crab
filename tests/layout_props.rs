//! Randomized invariants for the pure layout engine (`COMPOSITOR.md` §7.1,
//! TODO §2.8).
//!
//! These are the properties every layout must hold no matter how many windows
//! it is handed or how the parameters are set. They are expressed with
//! `proptest` so the search finds the awkward counts and ratios, not just the
//! ones a human thought of.

use crab::core::{
    Column, ColumnDisplay, ColumnWidth, Direction, LayoutKind, LayoutParams, Rect, ScrollAxis,
    ScrollCenter, ScrollState, WindowId,
};
use crab::layout::{Layout as _, LayoutInput, LayoutWindow, Placement, Scroll, arrange};
use proptest::prelude::*;

const AREA: Rect = Rect::new(0.0, 0.0, 1280.0, 720.0);

fn sample_windows(count: usize) -> Vec<LayoutWindow> {
    (0..count)
        .map(|i| LayoutWindow::new(WindowId::new(i as u64 + 1)))
        .collect()
}

fn scroll_params(
    gaps: f64,
    column_width: ColumnWidth,
    overspread: bool,
    center: ScrollCenter,
) -> LayoutParams {
    LayoutParams {
        gaps,
        column_width,
        overspread,
        center,
        ..LayoutParams::default()
    }
}

fn scroll_arrange(
    axis: ScrollAxis,
    windows: &[LayoutWindow],
    focused: Option<usize>,
    params: &LayoutParams,
    scroll: &mut ScrollState,
) -> Vec<Placement> {
    // Exercise the axis-generic engine directly; the exposed `scroll` mode is
    // always horizontal.
    Scroll::new(axis).arrange(LayoutInput {
        area: AREA,
        windows,
        focused,
        params,
        scroll,
    })
}

fn params(gaps: f64) -> LayoutParams {
    LayoutParams {
        gaps,
        ..LayoutParams::default()
    }
}

fn arrange_kind(
    kind: LayoutKind,
    count: usize,
    params: &LayoutParams,
    focused: Option<usize>,
) -> Vec<Placement> {
    let windows = sample_windows(count);
    let mut scroll = ScrollState::default();
    arrange(
        kind,
        LayoutInput {
            area: AREA,
            windows: &windows,
            focused,
            params,
            scroll: &mut scroll,
        },
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Every window gets exactly one placement, and placements stay inside the
    /// usable area.
    ///
    /// This is a property of the *tiling* family; a scroller deliberately puts
    /// columns off-screen, so only their focused one is bound to the view.
    #[test]
    fn placements_are_complete_and_contained(
        count in 0usize..24,
        gaps in 0.0f64..24.0,
    ) {
        let params = params(gaps);
        let placements = arrange_kind(LayoutKind::Fair, count, &params, Some(0));
        prop_assert_eq!(placements.len(), count);
        for placement in &placements {
            prop_assert!(placement.visible);
            prop_assert!(
                AREA.contains_rect(&placement.rect),
                "placement {:?} escaped {:?}",
                placement.rect,
                AREA
            );
        }
    }

    /// Tiled windows never overlap each other.
    #[test]
    fn fair_placements_are_disjoint(
        count in 1usize..24,
        gaps in 0.0f64..24.0,
    ) {
        let params = params(gaps);
        let placements = arrange_kind(LayoutKind::Fair, count, &params, Some(0));
        for (i, a) in placements.iter().enumerate() {
            for b in &placements[i + 1..] {
                prop_assert!(
                    a.rect.intersection(&b.rect).is_none(),
                    "{:?} overlaps {:?}",
                    a.rect,
                    b.rect
                );
            }
        }
    }

    /// Arranging is a pure function: identical input yields identical output.
    #[test]
    fn arrange_is_deterministic(
        kind in prop::sample::select(LayoutKind::ALL.to_vec()),
        count in 0usize..12,
        gaps in 0.0f64..16.0,
        focused in prop::option::of(0usize..12),
    ) {
        let params = params(gaps);
        let focused = focused.filter(|index| *index < count);
        let a = arrange_kind(kind, count, &params, focused);
        let b = arrange_kind(kind, count, &params, focused);
        prop_assert_eq!(a, b);
    }

    /// Floating arranges nothing; the compositor owns those rectangles.
    #[test]
    fn floating_places_nothing(count in 0usize..16) {
        let params = params(0.0);
        let placements = arrange_kind(LayoutKind::Floating, count, &params, Some(0));
        prop_assert!(placements.is_empty());
    }

    /// Directional focus never leaves the layout or returns the source.
    #[test]
    fn neighbor_stays_in_range(
        count in 1usize..12,
        from in 0usize..12,
        direction in prop::sample::select(vec![
            Direction::Left,
            Direction::Right,
            Direction::Up,
            Direction::Down,
        ]),
    ) {
        let from = from % count;
        let params = params(8.0);
        let windows: Vec<LayoutWindow> = (0..count)
            .map(|i| LayoutWindow::new(WindowId::new(i as u64 + 1)))
            .collect();
        let mut scroll = ScrollState::default();
        let input = LayoutInput {
            area: AREA,
            windows: &windows,
            focused: Some(from),
            params: &params,
            scroll: &mut scroll,
        };
        // The default trait method arranges internally; use the dispatch.
        if let Some(next) = crab::layout::neighbor(LayoutKind::Fair, &input, from, direction) {
            prop_assert!(next < count);
            prop_assert_ne!(next, from);
        }
    }

    /// Every window a scroller is handed gets placed, whether or not its
    /// column is on screen.
    #[test]
    fn scroll_places_every_window(
        count in 0usize..24,
        gaps in 0.0f64..24.0,
        proportion in 0.1f64..2.0,
        overspread in any::<bool>(),
        axis in prop::sample::select(ScrollAxis::ALL.to_vec()),
        focused in prop::option::of(0usize..24),
    ) {
        let windows = sample_windows(count);
        let focused = focused.filter(|index| *index < count);
        let params = scroll_params(
            gaps,
            ColumnWidth::Proportion(proportion),
            overspread,
            ScrollCenter::Never,
        );
        let mut scroll = ScrollState::default();
        let placements = scroll_arrange(axis, &windows, focused, &params, &mut scroll);
        prop_assert_eq!(placements.len(), count);
        for (index, placement) in placements.iter().enumerate() {
            prop_assert_eq!(placement.window, WindowId::new(index as u64 + 1));
        }
    }

    /// The focused column is always at least partly in view, whatever the width
    /// policy, the centring policy, or how long the strip has grown.
    #[test]
    fn scroll_keeps_the_focused_column_in_view(
        count in 1usize..24,
        gaps in 0.0f64..24.0,
        proportion in 0.1f64..2.0,
        overspread in any::<bool>(),
        center in prop::sample::select(ScrollCenter::ALL.to_vec()),
        axis in prop::sample::select(ScrollAxis::ALL.to_vec()),
        focused_seed in 0usize..24,
    ) {
        let windows = sample_windows(count);
        let focused = focused_seed % count;
        let params = scroll_params(gaps, ColumnWidth::Proportion(proportion), overspread, center);
        let mut scroll = ScrollState::default();
        let placements = scroll_arrange(axis, &windows, Some(focused), &params, &mut scroll);
        let usable = AREA.deflated(gaps);
        prop_assert!(
            placements[focused].rect.intersection(&usable).is_some(),
            "the focused column {:?} is off screen in {:?}",
            placements[focused].rect,
            usable
        );
        prop_assert!(placements[focused].visible);
    }

    /// Columns are laid end to end, so with one window each they never overlap
    /// — including the ones scrolled off the far edge.
    #[test]
    fn scroll_columns_do_not_overlap(
        count in 0usize..24,
        gaps in 0.0f64..24.0,
        proportion in 0.1f64..2.0,
        overspread in any::<bool>(),
        axis in prop::sample::select(ScrollAxis::ALL.to_vec()),
    ) {
        let windows = sample_windows(count);
        let params = scroll_params(
            gaps,
            ColumnWidth::Proportion(proportion),
            overspread,
            ScrollCenter::Never,
        );
        let mut scroll = ScrollState::default();
        let placements = scroll_arrange(axis, &windows, None, &params, &mut scroll);
        for (index, a) in placements.iter().enumerate() {
            for b in &placements[index + 1..] {
                prop_assert!(
                    a.rect.intersection(&b.rect).is_none(),
                    "{:?} overlaps {:?}",
                    a.rect,
                    b.rect
                );
            }
        }
    }

    /// However the model was built, windows only ever overlap by *sharing* a
    /// tabbed column: two rectangles are either disjoint or identical.
    #[test]
    fn scroll_windows_share_or_clear_each_other(
        groups in prop::collection::vec(1usize..4, 1..6),
        gaps in 0.0f64..16.0,
        proportion in 0.2f64..1.0,
        display in prop::sample::select(ColumnDisplay::ALL.to_vec()),
        axis in prop::sample::select(ScrollAxis::ALL.to_vec()),
    ) {
        // Adoption gives every window a column of its own, so a strip with
        // several windows per column has to be seeded by hand.
        let mut scroll = ScrollState::default();
        let mut windows = Vec::new();
        let mut next = 1u64;
        for group in &groups {
            let ids: Vec<WindowId> = (0..*group)
                .map(|_| {
                    let id = WindowId::new(next);
                    next += 1;
                    id
                })
                .collect();
            windows.extend(ids.iter().map(|id| LayoutWindow::new(*id)));
            scroll.columns.push(Column {
                width: ColumnWidth::Proportion(proportion),
                windows: ids,
                display,
            });
        }

        let params = scroll_params(
            gaps,
            ColumnWidth::Proportion(proportion),
            true,
            ScrollCenter::Never,
        );
        let placements = scroll_arrange(axis, &windows, Some(0), &params, &mut scroll);
        prop_assert_eq!(placements.len(), windows.len());
        for (index, a) in placements.iter().enumerate() {
            for b in &placements[index + 1..] {
                let overlap = a.rect.intersection(&b.rect);
                prop_assert!(
                    overlap.is_none() || a.rect == b.rect,
                    "{:?} and {:?} overlap without sharing a column",
                    a.rect,
                    b.rect
                );
            }
        }
    }

    /// Arranging twice over the same model and the same world agrees with
    /// itself: the scroller's memory does not drift.
    #[test]
    fn scroll_arranging_twice_is_stable(
        count in 0usize..24,
        gaps in 0.0f64..24.0,
        proportion in 0.1f64..2.0,
        overspread in any::<bool>(),
        center in prop::sample::select(ScrollCenter::ALL.to_vec()),
        axis in prop::sample::select(ScrollAxis::ALL.to_vec()),
        focused in prop::option::of(0usize..24),
    ) {
        let windows = sample_windows(count);
        let focused = focused.filter(|index| *index < count);
        let params = scroll_params(gaps, ColumnWidth::Proportion(proportion), overspread, center);
        let mut scroll = ScrollState::default();
        let first = scroll_arrange(axis, &windows, focused, &params, &mut scroll);
        let second = scroll_arrange(axis, &windows, focused, &params, &mut scroll);
        prop_assert_eq!(first, second);
    }

    /// A window opening at the end of the strip reflows nothing: every column
    /// up to and including the focused one keeps its rectangle, and so does the
    /// view (`COMPOSITOR.md` §7.7, TODO §3.3's exit criterion).
    #[test]
    fn scroll_opening_leaves_the_strip_alone(
        count in 1usize..16,
        gaps in 0.0f64..24.0,
        proportion in 0.1f64..1.5,
        center in prop::sample::select(ScrollCenter::ALL.to_vec()),
        axis in prop::sample::select(ScrollAxis::ALL.to_vec()),
        focused_seed in 0usize..16,
    ) {
        let windows = sample_windows(count);
        let focused = focused_seed % count;
        let params = scroll_params(gaps, ColumnWidth::Proportion(proportion), true, center);
        let mut scroll = ScrollState::default();
        let before = scroll_arrange(axis, &windows, Some(focused), &params, &mut scroll);

        let mut grown = windows.clone();
        grown.push(LayoutWindow::new(WindowId::new(count as u64 + 1)));
        let after = scroll_arrange(axis, &grown, Some(focused), &params, &mut scroll);

        prop_assert_eq!(after.len(), count + 1);
        for index in 0..=focused {
            prop_assert_eq!(
                before[index].rect,
                after[index].rect,
                "window {} moved when a new one arrived",
                index
            );
        }
        // The new one is a column of its own, so it clears every other window.
        for index in 0..count {
            prop_assert!(
                after[index].rect.intersection(&after[count].rect).is_none(),
                "the new column overlaps window {index}"
            );
        }
    }
}

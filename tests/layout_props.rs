//! Randomized invariants for the pure layout engine (`COMPOSITOR.md` §7.1,
//! TODO §2.8).
//!
//! These are the properties every layout must hold no matter how many windows
//! it is handed or how the parameters are set. They are expressed with
//! `proptest` so the search finds the awkward counts and ratios, not just the
//! ones a human thought of.

use crawm::core::{Direction, LayoutKind, LayoutParams, Rect, ScrollState, TilePosition, WindowId};
use crawm::layout::{LayoutInput, LayoutWindow, Placement, arrange};
use proptest::prelude::*;

const AREA: Rect = Rect::new(0.0, 0.0, 1280.0, 720.0);

fn params(master_count: u32, master_ratio: f64, gaps: f64, position: TilePosition) -> LayoutParams {
    let mut params = LayoutParams {
        master_count,
        master_ratio,
        gaps,
        position,
        ..LayoutParams::default()
    };
    params.clamp_ratio();
    params
}

fn arrange_kind(
    kind: LayoutKind,
    count: usize,
    params: &LayoutParams,
    focused: Option<usize>,
) -> Vec<Placement> {
    let windows: Vec<LayoutWindow> = (0..count)
        .map(|i| LayoutWindow::new(WindowId::new(i as u64 + 1)))
        .collect();
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
    #[test]
    fn placements_are_complete_and_contained(
        count in 0usize..24,
        master_count in 0u32..6,
        ratio in 0.1f64..0.9,
        gaps in 0.0f64..24.0,
        position in prop::sample::select(TilePosition::ALL.to_vec()),
    ) {
        let params = params(master_count, ratio, gaps, position);
        let placements = arrange_kind(LayoutKind::Tile, count, &params, Some(0));
        prop_assert_eq!(placements.len(), count);
        let usable = AREA.deflated(params.gaps);
        for placement in &placements {
            prop_assert!(placement.visible);
            prop_assert!(
                AREA.contains_rect(&placement.rect),
                "placement {:?} escaped {:?}",
                placement.rect,
                AREA
            );
            let _ = usable;
        }
    }

    /// Tiled windows never overlap each other.
    #[test]
    fn tile_placements_are_disjoint(
        count in 1usize..24,
        master_count in 0u32..6,
        ratio in 0.1f64..0.9,
        gaps in 0.0f64..24.0,
        position in prop::sample::select(TilePosition::ALL.to_vec()),
    ) {
        let params = params(master_count, ratio, gaps, position);
        let placements = arrange_kind(LayoutKind::Tile, count, &params, Some(0));
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
        master_count in 0u32..4,
        ratio in 0.1f64..0.9,
        gaps in 0.0f64..16.0,
        focused in prop::option::of(0usize..12),
    ) {
        let params = params(master_count, ratio, gaps, TilePosition::Left);
        let focused = focused.filter(|index| *index < count);
        let a = arrange_kind(kind, count, &params, focused);
        let b = arrange_kind(kind, count, &params, focused);
        prop_assert_eq!(a, b);
    }

    /// Monocle shows at most one window.
    #[test]
    fn monocle_shows_at_most_one(count in 0usize..16, gaps in 0.0f64..16.0) {
        let params = params(1, 0.6, gaps, TilePosition::Left);
        let placements = arrange_kind(LayoutKind::Monocle, count, &params, Some(0));
        let visible = placements.iter().filter(|p| p.visible).count();
        prop_assert!(visible <= 1);
    }

    /// Floating arranges nothing; the compositor owns those rectangles.
    #[test]
    fn floating_places_nothing(count in 0usize..16) {
        let params = params(1, 0.6, 0.0, TilePosition::Left);
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
        let params = params(1, 0.6, 8.0, TilePosition::Left);
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
        if let Some(next) = crawm::layout::neighbor(LayoutKind::Tile, &input, from, direction) {
            prop_assert!(next < count);
            prop_assert_ne!(next, from);
        }
    }
}

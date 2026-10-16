//! Integration test: the pure modules are reachable from `tests/` now that the
//! package exposes a library target (`src/lib.rs`). Before that, this suite could
//! only shell out to the binary (see `tests/cli.rs`) and could not call layout
//! code directly.

use crawm::core::{Rect, WindowId};
use crawm::layout::{Layout as _, LayoutInput, Monocle, by_name};

fn input() -> LayoutInput {
    LayoutInput {
        area: Rect::new(0.0, 0.0, 1000.0, 1000.0),
        gaps: 10.0,
        windows: vec![WindowId::new(1), WindowId::new(2)],
        focused: Some(WindowId::new(2)),
    }
}

#[test]
fn monocle_arranges_from_outside_the_crate() {
    let placements = Monocle.arrange(&input());
    let visible: Vec<_> = placements.iter().filter(|p| p.visible).collect();
    assert_eq!(visible.len(), 1, "only the focused window is shown");
    assert_eq!(visible[0].window, WindowId::new(2));
    // The focused rect is inset by the gaps.
    assert_eq!(visible[0].rect, Rect::new(10.0, 10.0, 980.0, 980.0));
}

#[test]
fn layout_names_resolve() {
    assert_eq!(by_name("monocle").unwrap().name(), "monocle");
    assert_eq!(by_name("floating").unwrap().name(), "floating");
    assert!(by_name("nope").is_none());
}

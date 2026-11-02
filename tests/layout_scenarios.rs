//! B41: multi-window, multi-space, multi-output layout scenarios.
//!
//! niri's `src/layout/tests.rs` drives a fake layout through add, move, resize,
//! and close, then asserts on the resulting rectangles without a compositor or
//! a GPU. This is crab's equivalent: a small [`Scene`] builds pure
//! [`Output`]/[`SpaceState`]/[`LayoutWindow`] values and calls the same
//! `crab::layout::arrange` the compositor uses.
//!
//! The compositor-level protocol harness lives in
//! `src/compositor/state/multi_output.rs`; these scenarios are the layout half
//! and stay entirely inside the Smithay-free library.

use crab::core::{
    LayoutCommand, LayoutKind, LayoutParams, Output, OutputId, Rect, SpaceSet, SpaceState, WindowId,
};
use crab::layout::{self, LayoutInput, LayoutWindow, Placement};

/// A left 1920×1080 monitor and a right 2560×1440 one, matching the protocol
/// harness's `DP-1`/`DP-2`.
const LEFT: Rect = Rect::new(0.0, 0.0, 1920.0, 1080.0);
const RIGHT: Rect = Rect::new(1920.0, 0.0, 2560.0, 1440.0);

/// One window in the scenario. Only the layout-relevant fields are modelled.
#[derive(Clone, Debug)]
struct SceneWindow {
    id: WindowId,
    output: OutputId,
    spaces: SpaceSet,
    min_size: Option<crab::core::Size>,
}

/// Two outputs, their windows, and the layout state that survives a re-arrange.
struct Scene {
    outputs: Vec<Output>,
    windows: Vec<SceneWindow>,
    focus: Option<WindowId>,
    next_id: u64,
}

impl Scene {
    /// A two-output scene whose every space starts in `kind` with no gaps, so
    /// the expected rectangles stay exact. Gap handling is covered by the
    /// golden layout tests.
    fn new(kind: LayoutKind) -> Self {
        let params = LayoutParams {
            gaps: 0.0,
            ..LayoutParams::default()
        };
        let default = SpaceState::new(kind, params);
        Self {
            outputs: vec![
                Output::new(OutputId::new(0), "DP-1", LEFT, 1.0, default.clone()),
                Output::new(OutputId::new(1), "DP-2", RIGHT, 1.0, default),
            ],
            windows: Vec::new(),
            focus: None,
            next_id: 0,
        }
    }

    /// Adds a window on output `output` in `spaces` and focuses it, like a
    /// client mapping a new toplevel on the focused output.
    fn open(&mut self, output: usize, spaces: SpaceSet) -> WindowId {
        self.next_id += 1;
        let id = WindowId::new(self.next_id);
        self.windows.push(SceneWindow {
            id,
            output: self.outputs[output].id,
            spaces,
            min_size: None,
        });
        self.focus = Some(id);
        id
    }

    fn window(&self, id: WindowId) -> &SceneWindow {
        self.windows
            .iter()
            .find(|window| window.id == id)
            .expect("the scenario window exists")
    }

    /// Removes a window and compacts every output, exactly like the
    /// compositor's `compact_spaces`: empty spaces disappear and later spaces
    /// move down with their layout state.
    fn close(&mut self, id: WindowId) {
        let Some(index) = self.windows.iter().position(|window| window.id == id) else {
            return;
        };
        self.windows.remove(index);
        self.compact();
        if self.focus == Some(id) {
            self.focus = self.windows.last().map(|window| window.id);
        }
    }

    /// Moves a window to exactly `spaces` and shows that space, the way
    /// `move_to_space` does.
    fn move_to_space(&mut self, id: WindowId, spaces: SpaceSet) {
        if let Some(window) = self.windows.iter_mut().find(|window| window.id == id) {
            window.spaces = spaces;
        }
        let output = self.window(id).output;
        let index = self.output_index(output);
        self.outputs[index].view_space(spaces);
        self.compact();
        self.focus = Some(id);
    }

    /// Reassigns a window to another output, as output migration does when a
    /// monitor is removed or a window is sent to a peer monitor.
    fn move_to_output(&mut self, id: WindowId, output: usize) {
        let target = self.outputs[output].id;
        if let Some(window) = self.windows.iter_mut().find(|window| window.id == id) {
            window.output = target;
            window.spaces = SpaceSet::SPACE_1;
        }
        self.compact();
        self.focus = Some(id);
    }

    /// Swaps two windows in layout order, the pure half of `move_window`.
    fn swap(&mut self, a: WindowId, b: WindowId) {
        let (Some(a), Some(b)) = (
            self.windows.iter().position(|window| window.id == a),
            self.windows.iter().position(|window| window.id == b),
        ) else {
            return;
        };
        self.windows.swap(a, b);
    }

    /// Shows exactly `spaces` on an output.
    fn view(&mut self, output: usize, spaces: SpaceSet) {
        self.outputs[output].view_space(spaces);
    }

    /// Runs a layout command against an output's current space, targeting the
    /// scene's focused window the way the compositor does.
    fn command(&mut self, output: usize, command: LayoutCommand) {
        let focused = self.focus;
        if let Some(state) = self.outputs[output].current_space_state_mut() {
            layout::apply_command(
                state.layout,
                command,
                focused,
                &mut state.params,
                &mut state.scroll,
            );
        }
    }

    /// The placements for an output's current view, writing the scroller's
    /// updated state back so the next arrange sees the same columns.
    fn arrange(&mut self, output: usize) -> Vec<Placement> {
        let runtime = &self.outputs[output];
        let Some(state) = runtime.current_space_state() else {
            return Vec::new();
        };
        let kind = state.layout;
        let params = state.params;
        let mut scroll = state.scroll.clone();
        let view = runtime.view.visible;
        let area = runtime.usable_area;
        let output_id = runtime.id;
        let windows: Vec<LayoutWindow> = self
            .windows
            .iter()
            .filter(|window| window.output == output_id && !(window.spaces & view).is_empty())
            .map(|window| LayoutWindow {
                id: window.id,
                min_size: window.min_size,
                max_size: None,
            })
            .collect();
        let focused = self
            .focus
            .and_then(|id| windows.iter().position(|window| window.id == id));
        let placements = layout::arrange(
            kind,
            LayoutInput {
                area,
                windows: &windows,
                focused,
                params: &params,
                scroll: &mut scroll,
            },
        );
        if let Some(state) = self.outputs[output].current_space_state_mut() {
            state.scroll = scroll;
        }
        placements
    }

    fn compact(&mut self) {
        for output in &mut self.outputs {
            let occupied = self
                .windows
                .iter()
                .filter(|window| window.output == output.id)
                .fold(SpaceSet::empty(), |spaces, window| spaces | window.spaces);
            let remap = output.compact(occupied);
            for window in &mut self.windows {
                if window.output == output.id {
                    window.spaces = remap.map_mask(window.spaces);
                }
            }
        }
    }

    fn output_index(&self, output: OutputId) -> usize {
        self.outputs
            .iter()
            .position(|candidate| candidate.id == output)
            .expect("the scenario output exists")
    }
}

/// The rectangle `placements` gave `id`.
fn rect(placements: &[Placement], id: WindowId) -> Rect {
    placements
        .iter()
        .find(|placement| placement.window == id)
        .unwrap_or_else(|| panic!("window {id:?} was not placed"))
        .rect
}

/// The placed window ids, in placement order.
fn ids(placements: &[Placement]) -> Vec<WindowId> {
    placements
        .iter()
        .map(|placement| placement.window)
        .collect()
}

#[test]
fn windows_on_two_outputs_arrange_independently() {
    let mut scene = Scene::new(LayoutKind::Fair);
    let first = scene.open(0, SpaceSet::SPACE_1);
    let second = scene.open(0, SpaceSet::SPACE_1);
    let right_window = scene.open(1, SpaceSet::SPACE_1);

    let left = scene.arrange(0);
    assert_eq!(ids(&left), vec![first, second]);
    assert_eq!(rect(&left, first), Rect::new(0.0, 0.0, 960.0, 1080.0));
    assert_eq!(rect(&left, second), Rect::new(960.0, 0.0, 960.0, 1080.0));

    let right = scene.arrange(1);
    assert_eq!(ids(&right), vec![right_window]);
    assert_eq!(
        rect(&right, right_window),
        RIGHT,
        "one tile fills the right output"
    );
}

#[test]
fn moving_a_window_to_another_space_reflows_the_source() {
    let mut scene = Scene::new(LayoutKind::Fair);
    let first = scene.open(0, SpaceSet::SPACE_1);
    let second = scene.open(0, SpaceSet::SPACE_1);
    let moved = scene.open(0, SpaceSet::SPACE_1);
    assert_eq!(scene.arrange(0).len(), 3);

    scene.move_to_space(moved, SpaceSet::SPACE_2);
    assert_eq!(scene.window(moved).spaces, SpaceSet::SPACE_2);
    assert_eq!(
        scene.outputs[0].view.visible,
        SpaceSet::SPACE_2,
        "moving a window shows its destination"
    );

    let destination = scene.arrange(0);
    assert_eq!(ids(&destination), vec![moved]);
    assert_eq!(rect(&destination, moved), LEFT);

    scene.view(0, SpaceSet::SPACE_1);
    let source = scene.arrange(0);
    assert_eq!(ids(&source), vec![first, second]);
    assert_eq!(rect(&source, first), Rect::new(0.0, 0.0, 960.0, 1080.0));
    assert_eq!(rect(&source, second), Rect::new(960.0, 0.0, 960.0, 1080.0));
}

#[test]
fn closing_a_window_reflows_the_survivors() {
    let mut scene = Scene::new(LayoutKind::Fair);
    let first = scene.open(0, SpaceSet::SPACE_1);
    let middle = scene.open(0, SpaceSet::SPACE_1);
    let last = scene.open(0, SpaceSet::SPACE_1);
    // Three windows: one full-height column and a two-row column.
    assert_eq!(
        rect(&scene.arrange(0), middle),
        Rect::new(960.0, 0.0, 960.0, 540.0)
    );

    scene.close(middle);
    let after = scene.arrange(0);
    assert_eq!(ids(&after), vec![first, last]);
    assert_eq!(
        rect(&after, first),
        Rect::new(0.0, 0.0, 960.0, 1080.0),
        "the first column keeps its width"
    );
    assert_eq!(
        rect(&after, last),
        Rect::new(960.0, 0.0, 960.0, 1080.0),
        "the survivor expands into the freed column"
    );
    assert_eq!(scene.window(first).spaces, SpaceSet::SPACE_1);
}

#[test]
fn moving_a_window_between_outputs_reflows_both() {
    let mut scene = Scene::new(LayoutKind::Fair);
    let moved = scene.open(0, SpaceSet::SPACE_1);
    let left = scene.open(0, SpaceSet::SPACE_1);
    let right = scene.open(1, SpaceSet::SPACE_1);

    scene.move_to_output(moved, 1);
    assert_eq!(scene.window(moved).output, RIGHT_ID);

    let source = scene.arrange(0);
    assert_eq!(ids(&source), vec![left]);
    assert_eq!(rect(&source, left), LEFT, "the survivor fills the source");

    let target = scene.arrange(1);
    assert_eq!(ids(&target), vec![moved, right]);
    assert_eq!(
        rect(&target, moved),
        Rect::new(1920.0, 0.0, 1280.0, 1440.0),
        "the moved window takes the first half of the target"
    );
    assert_eq!(rect(&target, right), Rect::new(3200.0, 0.0, 1280.0, 1440.0));
}

/// The right output's id, used by the cross-output scenario.
const RIGHT_ID: OutputId = OutputId::new(1);

#[test]
fn swapping_windows_swaps_their_tiles() {
    let mut scene = Scene::new(LayoutKind::Fair);
    let first = scene.open(0, SpaceSet::SPACE_1);
    let second = scene.open(0, SpaceSet::SPACE_1);
    let before = scene.arrange(0);
    assert_eq!(rect(&before, first), Rect::new(0.0, 0.0, 960.0, 1080.0));

    scene.swap(first, second);
    let after = scene.arrange(0);
    assert_eq!(ids(&after), vec![second, first]);
    assert_eq!(
        rect(&after, second),
        Rect::new(0.0, 0.0, 960.0, 1080.0),
        "the swapped window takes the first tile"
    );
    assert_eq!(rect(&after, first), Rect::new(960.0, 0.0, 960.0, 1080.0));
}

#[test]
fn closing_a_scroll_column_before_the_focus_keeps_it_still() {
    let mut scene = Scene::new(LayoutKind::Scroll);
    let first = scene.open(0, SpaceSet::SPACE_1);
    let middle = scene.open(0, SpaceSet::SPACE_1);
    let focused = scene.open(0, SpaceSet::SPACE_1);
    scene.focus = Some(focused);

    // Three half-view columns; the focused one is scrolled to the right edge.
    let before = scene.arrange(0);
    let still = rect(&before, focused);
    assert_eq!(still, Rect::new(960.0, 0.0, 960.0, 1080.0));
    assert_eq!(rect(&before, middle), Rect::new(0.0, 0.0, 960.0, 1080.0));

    // Closing a column to its left shifts the strip but not the focused window.
    scene.close(first);
    let after = scene.arrange(0);
    assert_eq!(ids(&after), vec![middle, focused]);
    assert_eq!(
        rect(&after, focused),
        still,
        "the focused column stayed on screen"
    );
    assert_eq!(
        rect(&after, middle),
        Rect::new(0.0, 0.0, 960.0, 1080.0),
        "the surviving column before it moved into the gap"
    );
}

#[test]
fn moving_a_scroll_column_reorders_the_strip_without_moving_the_focus() {
    let mut scene = Scene::new(LayoutKind::Scroll);
    let first = scene.open(0, SpaceSet::SPACE_1);
    let middle = scene.open(0, SpaceSet::SPACE_1);
    let focused = scene.open(0, SpaceSet::SPACE_1);
    scene.focus = Some(focused);

    // The focused third column sits at the right edge of the view.
    let before = scene.arrange(0);
    assert_eq!(rect(&before, first), Rect::new(-960.0, 0.0, 960.0, 1080.0));
    assert_eq!(rect(&before, focused), Rect::new(960.0, 0.0, 960.0, 1080.0));

    // Moving it one place left swaps it with the middle column. The focused
    // window keeps its place on screen; the middle column moves to the end.
    scene.command(0, LayoutCommand::MoveColumnLeft);
    let after = scene.arrange(0);
    assert_eq!(rect(&after, first), Rect::new(0.0, 0.0, 960.0, 1080.0));
    assert_eq!(rect(&after, focused), Rect::new(960.0, 0.0, 960.0, 1080.0));
    assert_eq!(rect(&after, middle), Rect::new(1920.0, 0.0, 960.0, 1080.0));
}

#[test]
fn consuming_and_expelling_windows_keeps_every_window_placed() {
    let mut scene = Scene::new(LayoutKind::Scroll);
    let first = scene.open(0, SpaceSet::SPACE_1);
    let focused = scene.open(0, SpaceSet::SPACE_1);
    let third = scene.open(0, SpaceSet::SPACE_1);
    scene.focus = Some(focused);
    scene.arrange(0);

    // Consuming the focused window merges it into the column on its left,
    // where the two windows share the column's height.
    scene.command(0, LayoutCommand::ConsumeWindowIntoColumn);
    let merged = scene.arrange(0);
    assert_eq!(rect(&merged, first), Rect::new(0.0, 0.0, 960.0, 540.0));
    assert_eq!(rect(&merged, focused), Rect::new(0.0, 540.0, 960.0, 540.0));
    assert_eq!(rect(&merged, third), Rect::new(960.0, 0.0, 960.0, 1080.0));

    // Expelling it splits it back out into its own column on the right, and
    // the source column returns to one window. The focused window keeps its
    // place on screen, so the strip scrolls rather than dragging it sideways.
    scene.command(0, LayoutCommand::ExpelWindowFromColumn);
    let split = scene.arrange(0);
    assert_eq!(rect(&split, first), Rect::new(-960.0, 0.0, 960.0, 1080.0));
    assert_eq!(rect(&split, focused), Rect::new(0.0, 0.0, 960.0, 1080.0));
    assert_eq!(rect(&split, third), Rect::new(960.0, 0.0, 960.0, 1080.0));
}

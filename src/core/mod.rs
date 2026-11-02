//! Pure data model for crab: identifiers, geometry, colors, spaces, windows,
//! outputs, focus, rules, and connector-to-output planning.
//!
//! This module has **no** Smithay or Wayland dependencies (see `ROADMAP.md` §5),
//! so it is fast to test and can be re-promoted into its own crate if a future
//! out-of-process consumer needs it (`crawl-desktop`'s ADR-0005 split rule,
//! read in reverse).
//!
//! [`Window`] and [`Output`] implement the model from `COMPOSITOR.md` §6; the
//! layout engine that arranges them lives in [`crate::layout`].

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod color;
pub mod focus;
pub mod geometry;
pub mod ids;
pub mod output;
pub mod output_plan;
pub mod params;
pub mod rules;
pub mod spaces;
pub mod window;

pub use color::Color;
pub use focus::{FocusStack, FocusTarget};
pub use geometry::{Point, Rect, Size};
pub use ids::{LayerId, OutputId, WindowId};
pub use output::{Output, OutputMode, OutputSpecError, OutputTransform, RedrawState, SpaceState};
pub use output_plan::{
    ConnectorInfo, ConnectorMode, OutputPlan, OutputPlanError, OutputPlanner, OutputPreference,
};
pub use params::{
    Column, ColumnDisplay, ColumnWidth, Direction, HeightPresets, LayoutCommand, LayoutFamily,
    LayoutKind, LayoutParams, ScrollAnchor, ScrollAxis, ScrollCenter, ScrollState, WidthPresets,
    WindowHeight,
};
pub use rules::WindowRule;
pub use spaces::{SpaceRemap, SpaceSet, SpaceView};
pub use window::{AppliedRules, Window};

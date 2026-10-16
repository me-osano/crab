//! Pure data model for crawm: identifiers, geometry, colors, and tag state.
//!
//! This crate has **no** Smithay or Wayland dependencies (see `ROADMAP.md` §5), so
//! it is fast to test and safe to reuse from any future out-of-process consumer.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod color;
pub mod geometry;
pub mod ids;
pub mod tags;

pub use color::Color;
pub use geometry::{Point, Rect, Size};
pub use ids::{LayerId, OutputId, WindowId};
pub use tags::{TagMask, TagView};

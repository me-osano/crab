//! Pure animation primitives: springs, easings, and animated scalars.
//!
//! Nothing here reads a clock: the caller advances animations by calling
//! [`Animated::tick`] with a `dt` derived from the **frame clock** (vblank or
//! `wp_presentation`), never from `Instant::now()` inside a redraw (ROADMAP §9,
//! COMPOSITOR.md §9).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod animated;
mod curve;

pub use animated::Animated;
pub use curve::{Easing, SpringParams, lerp_point};

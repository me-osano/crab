//! Pure animation primitives: curves, animated values, and the frame clock.
//!
//! Nothing here reads a clock of its own. The compositor re-times [`Clocks`]
//! once per frame from the backend's presentation time — the loop clock under
//! the nested backend today, the vertical blank or `wp_presentation` once the
//! bare-metal backend lands — and every animated value is sampled against what
//! it reports (TODO.md §3.2). Sampling rather than stepping is what makes one
//! clock enough: a value is a pure function of frame time, so nothing can drift
//! and reading a value never changes it (COMPOSITOR.md §9, ROADMAP.md §9).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod animatable;
mod animated;
mod clock;
mod curve;
mod spring;

pub use animatable::Animatable;
pub use animated::Animated;
pub use clock::{Clock, Clocks, Kind};
pub use curve::{CubicBezier, Curve, Easing};
pub use spring::{SpringParams, SpringWeights};

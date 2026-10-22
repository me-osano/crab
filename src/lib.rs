//! crab's pure, Smithay-free library surface.
//!
//! The compositor binary (`src/main.rs`) is a thin entry point that owns
//! everything touching Smithay — backends, handlers, render, input. The logic
//! that can be pure lives here so it is **testable without graphics** and
//! reusable (the "modules first" split rule from `crawl-desktop`'s ADR-0005;
//! ROADMAP.md §5):
//!
//! * [`core`] — identifiers, geometry, colors, spaces, window model;
//! * [`config`] — TOML parsing, validation, and defaults;
//! * [`layout`] — the [`layout::Layout`] trait and its implementations;
//! * [`animation`] — curves, animated values, and the frame clock.
//!
//! Nothing here may depend on Smithay or Wayland; that rule is enforced by
//! `scripts/check-boundaries.sh`, which also rejects any non-pure `mod` added to
//! this file. Anything touching Smithay belongs in the binary, not the library.

#![deny(unsafe_op_in_unsafe_fn)]

pub mod animation;
pub mod config;
pub mod core;
pub mod layout;

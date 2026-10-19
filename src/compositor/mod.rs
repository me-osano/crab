//! The compositor: Smithay state, protocol handlers, input, and drawing.
//!
//! Everything here is **binary-side**. These modules exist to hold crawm's two
//! halves together — Smithay's protocol and rendering machinery, and the pure
//! `crawm::{core, layout, animation, config}` library — which is exactly why
//! none of them may live under `src/{core,config,layout,animation}` and why
//! `scripts/check-boundaries.sh` keeps them out.
//!
//! [`state`] owns [`Crawm`](state::Crawm) and the top-level event dispatch; the
//! rest hangs off it. [`handlers`] is one module per protocol, [`input`] turns
//! backend events into seat events and holds the binds, [`layout`] applies the
//! pure layout engine to real windows, and [`render`] draws a frame.
//!
//! Backends (`src/backend/`) sit outside this module: they are the platform
//! layer that drives it, not part of it.

pub(crate) mod handlers;
pub(crate) mod input;
pub(crate) mod layout;
pub(crate) mod render;
pub(crate) mod state;

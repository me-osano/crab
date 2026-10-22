//! The compositor: Smithay state, protocol handlers, input, and drawing.
//!
//! Everything here is **binary-side**. These modules exist to hold crab's two
//! halves together — Smithay's protocol and rendering machinery, and the pure
//! `crab::{core, layout, animation, config}` library — which is exactly why
//! none of them may live under `src/{core,config,layout,animation}` and why
//! `scripts/check-boundaries.sh` keeps them out.
//!
//! [`state`] owns [`Crab`](state::Crab) and the top-level event dispatch; the
//! rest hangs off it. [`handlers`] is one module per protocol, [`input`] turns
//! backend events into seat events and runs the compiled binds (see [`binds`]),
//! [`spawn`] launches `spawn:`/`[autostart]` children, [`layout`] applies the
//! pure layout engine to real windows, [`fit`] describes how a window's
//! committed surface maps onto where it is drawn — the renderer and pointer
//! input both read it — and [`render`] draws a frame.
//!
//! Backends (`src/backend/`) sit outside this module: they are the platform
//! layer that drives it, not part of it.

pub(crate) mod binds;
pub(crate) mod fit;
pub(crate) mod handlers;
pub(crate) mod input;
pub(crate) mod layout;
pub(crate) mod render;
pub(crate) mod spawn;
pub(crate) mod state;

//! Backends.
//!
//! `winit` (nested) is available now; `tty` (bare-metal DRM/KMS) arrives in M4 and
//! `headless` exists for tests.

pub mod headless;
pub mod tty;
pub mod winit;

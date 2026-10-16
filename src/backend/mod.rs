//! Backends.
//!
//! Which backends are compiled in is controlled by cargo features on the `crawm`
//! package: `backend-winit` (nested window, the default dev path) and
//! `backend-tty` (bare-metal DRM/KMS, M4). `headless` is always available so the
//! test suite can exercise the compositor without graphics.

pub mod headless;

#[cfg(feature = "backend-tty")]
pub mod tty;

#[cfg(feature = "backend-winit")]
pub mod winit;

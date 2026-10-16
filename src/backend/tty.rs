//! Bare-metal DRM/KMS backend — **not implemented yet**.
//!
//! Phase 4 (M4) adds `libseat`/`udev` session handling, DRM/KMS + GBM + EGL, and
//! the `libinput` seat. Until then this is an explicit, honest failure rather than
//! a silent no-op.

use anyhow::bail;
use crawm::config::Config;

/// Attempts to start the TTY backend.
///
/// # Errors
/// Always fails for now; the backend lands in M4.
pub fn run(_config: &Config) -> anyhow::Result<()> {
    bail!("the tty (DRM/KMS) backend is not implemented yet — see ROADMAP milestone M4")
}

//! Headless backend — no rendering, used by tests and CI.
//!
//! A full headless backend (with real Wayland globals and an in-memory renderer)
//! arrives alongside the test suite in later phases; for now it validates the
//! config path and returns immediately.

use anyhow::Context as _;
use crab::config::Config;

/// Runs the headless backend.
///
/// # Errors
/// Currently only fails if the configuration is invalid.
pub fn run(config: &Config) -> anyhow::Result<()> {
    crab::config::validate(config).context("validating config")?;
    tracing::info!(
        gaps = config.gaps,
        layout = %config.layout,
        "crab (headless) validated config; nothing to render"
    );
    Ok(())
}

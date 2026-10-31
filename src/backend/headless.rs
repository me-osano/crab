//! Headless backend — no rendering, used by tests and CI.
//!
//! With the `backend-winit` feature it builds the real compositor state on an
//! in-process Wayland `Display` and drops it, so CI exercises global
//! registration, seat/keyboard setup, and bind compilation without a GPU or a
//! window. Without that feature it only validates the config.

use anyhow::Context as _;
use crab::config::Config;

/// Runs the headless backend.
///
/// # Errors
/// Fails if the configuration is invalid or, with `backend-winit`, if the
/// compositor state cannot be constructed.
pub fn run(config: &Config) -> anyhow::Result<()> {
    crab::config::validate(config).context("validating config")?;
    #[cfg(feature = "backend-winit")]
    smoke_state(config)?;
    tracing::info!(
        gaps = config.gaps,
        layout = %config.layout,
        "crab (headless) validated config; nothing to render"
    );
    Ok(())
}

/// Builds and immediately drops the compositor state on a fresh display.
///
/// This is the GPU-free smoke path: it registers every global, creates the
/// seat/keyboard, and compiles binds exactly as the winit backend does before
/// it opens a window.
#[cfg(feature = "backend-winit")]
fn smoke_state(config: &Config) -> anyhow::Result<()> {
    use smithay::reexports::wayland_server::Display;

    use crate::compositor::state::Crab;

    let display = Display::<Crab>::new()
        .map_err(|err| anyhow::anyhow!("create the headless Wayland display: {err:?}"))?;
    let handle = display.handle();
    let state = Crab::new_without_outputs(&handle, config);
    tracing::info!(
        binds = state.binds.len(),
        outputs = state.outputs.len(),
        "crab (headless) built compositor state"
    );
    drop(state);
    Ok(())
}

#[cfg(all(test, feature = "backend-winit"))]
mod tests {
    use super::*;

    #[test]
    fn builds_compositor_state_without_a_gpu() {
        smoke_state(&Config::default()).expect("headless compositor state");
    }
}

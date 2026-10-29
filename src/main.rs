//! crab — a Wayland compositor merging mango-style spaces, niri scrolling, and cosmic
//! protocol breadth.
//!
//! This binary is the thin Smithay entry point: backend selection, logging, and
//! the top-level event loop. Everything that touches Smithay lives in
//! `src/compositor/` (state, handlers, input, render) and `src/backend/`, while
//! the Smithay-free logic lives in the `crab` library (see `src/lib.rs` and
//! `ROADMAP.md` §5), which is also what `tests/` drives.
//!
//! Which backends are compiled in is a cargo feature: `backend-winit` (the
//! default, nested development path) and `backend-tty` (bare-metal DRM/KMS, M4).
//! `--backend headless` is always available so tests can start the compositor
//! without graphics.

#![deny(unsafe_op_in_unsafe_fn)]

mod backend;
mod cli;
// The compositor is built by whichever backend actually uses it: M1's winit
// backend builds the state, runs the handlers, and draws. The headless path
// stays state-free, and the tty backend (M4) bails before it needs state, so
// gating on `backend-winit` alone keeps every feature combination warning-free;
// M4 widens this to `any(backend-winit, backend-tty)`.
#[cfg(feature = "backend-winit")]
mod compositor;
#[cfg(feature = "backend-winit")]
mod ipc;
mod logging;

use std::path::PathBuf;

use anyhow::Context as _;
use clap::Parser as _;
use crab::config::Config;

use crate::cli::{Args, Backend};

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    logging::init(args.log_filter.as_deref());
    logging::install_panic_hook();

    let (config, config_path) = resolve_config(&args)?;
    let pending = config.pending_sections();
    if !pending.is_empty() {
        tracing::info!(
            sections = ?pending,
            "config sections parsed but not applied yet (their milestones are M4-M6)"
        );
    }

    if args.validate {
        // Validate even a default config, then report and exit.
        crab::config::validate(&config).map_err(|err| anyhow::anyhow!(err))?;
        println!(
            "ok: config valid (gaps={}, border-width={}, layout={}, animations={})",
            config.gaps, config.border_width, config.layout, config.animations.enabled
        );
        return Ok(());
    }

    tracing::info!(backend = ?args.backend, "starting crab");
    match args.backend {
        Backend::Winit => run_winit(&config, config_path.as_deref()),
        Backend::Tty => run_tty(&config),
        Backend::Headless => backend::headless::run(&config),
    }
}

#[cfg(feature = "backend-winit")]
fn run_winit(config: &Config, config_path: Option<&std::path::Path>) -> anyhow::Result<()> {
    backend::winit::run(config, config_path)
}

#[cfg(not(feature = "backend-winit"))]
fn run_winit(_config: &Config, _config_path: Option<&std::path::Path>) -> anyhow::Result<()> {
    anyhow::bail!("this crab build has no `winit` backend; enable the `backend-winit` feature")
}

#[cfg(feature = "backend-tty")]
fn run_tty(config: &Config) -> anyhow::Result<()> {
    backend::tty::run(config)
}

#[cfg(not(feature = "backend-tty"))]
fn run_tty(_config: &Config) -> anyhow::Result<()> {
    anyhow::bail!("this crab build has no `tty` backend; enable the `backend-tty` feature")
}

/// Loads the config from `--config`, else the default path if it exists, else
/// built-in defaults.
fn resolve_config(args: &Args) -> anyhow::Result<(Config, Option<PathBuf>)> {
    if let Some(path) = &args.config {
        let config = crab::config::load(path)
            .with_context(|| format!("loading config from {}", path.display()))?;
        return Ok((config, Some(path.clone())));
    }
    if let Some(path) = crab::config::default_path().filter(|path| path.exists()) {
        tracing::debug!(path = %path.display(), "loading default config");
        let config = crab::config::load(&path)
            .with_context(|| format!("loading config from {}", path.display()))?;
        return Ok((config, Some(path)));
    }
    Ok((Config::default(), None))
}

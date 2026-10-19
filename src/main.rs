//! crawm — a Wayland compositor merging mango-style spaces, niri scrolling, and cosmic
//! protocol breadth.
//!
//! This binary is the thin Smithay entry point: backend selection, logging, and
//! the top-level event loop. Everything that touches Smithay lives in
//! `src/compositor/` (state, handlers, input, render) and `src/backend/`, while
//! the Smithay-free logic lives in the `crawm` library (see `src/lib.rs` and
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
mod logging;

use anyhow::Context as _;
use clap::Parser as _;
use crawm::config::Config;

use crate::cli::{Args, Backend};

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    logging::init(args.log_filter.as_deref());
    logging::install_panic_hook();

    let config = resolve_config(&args)?;

    if args.validate {
        // Validate even a default config, then report and exit.
        crawm::config::validate(&config).map_err(|err| anyhow::anyhow!(err))?;
        println!(
            "ok: config valid (gaps={}, border-width={}, layout={}, animations={})",
            config.gaps, config.border_width, config.layout, config.animations
        );
        return Ok(());
    }

    tracing::info!(backend = ?args.backend, "starting crawm");
    match args.backend {
        Backend::Winit => run_winit(&config),
        Backend::Tty => run_tty(&config),
        Backend::Headless => backend::headless::run(&config),
    }
}

#[cfg(feature = "backend-winit")]
fn run_winit(config: &Config) -> anyhow::Result<()> {
    backend::winit::run(config)
}

#[cfg(not(feature = "backend-winit"))]
fn run_winit(_config: &Config) -> anyhow::Result<()> {
    anyhow::bail!("this crawm build has no `winit` backend; enable the `backend-winit` feature")
}

#[cfg(feature = "backend-tty")]
fn run_tty(config: &Config) -> anyhow::Result<()> {
    backend::tty::run(config)
}

#[cfg(not(feature = "backend-tty"))]
fn run_tty(_config: &Config) -> anyhow::Result<()> {
    anyhow::bail!("this crawm build has no `tty` backend; enable the `backend-tty` feature")
}

/// Loads the config from `--config`, or falls back to built-in defaults.
fn resolve_config(args: &Args) -> anyhow::Result<Config> {
    match &args.config {
        Some(path) => crawm::config::load(path)
            .with_context(|| format!("loading config from {}", path.display())),
        None => Ok(Config::default()),
    }
}

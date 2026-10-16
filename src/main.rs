//! crawm — a Wayland compositor merging mango tags, niri scrolling, and cosmic
//! protocol breadth.
//!
//! This binary owns everything that touches Smithay; the pure logic lives in the
//! `crawm-*` libraries (see `ROADMAP.md` §5).

#![deny(unsafe_op_in_unsafe_fn)]

mod backend;
mod cli;
mod logging;

use anyhow::Context as _;
use clap::Parser as _;
use crawm_config::Config;

use crate::cli::{Args, Backend};

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    logging::init(args.log_filter.as_deref());
    logging::install_panic_hook();

    let config = resolve_config(&args)?;

    if args.validate {
        // Validate even a default config, then report and exit.
        crawm_config::validate(&config).map_err(|err| anyhow::anyhow!(err))?;
        println!(
            "ok: config valid (gaps={}, border-width={}, layout={}, animations={})",
            config.gaps, config.border_width, config.layout, config.animations
        );
        return Ok(());
    }

    tracing::info!(backend = ?args.backend, "starting crawm");
    match args.backend {
        Backend::Winit => backend::winit::run(&config),
        Backend::Tty => backend::tty::run(&config),
        Backend::Headless => backend::headless::run(&config),
    }
}

/// Loads the config from `--config`, or falls back to built-in defaults.
fn resolve_config(args: &Args) -> anyhow::Result<Config> {
    match &args.config {
        Some(path) => crawm_config::load(path)
            .with_context(|| format!("loading config from {}", path.display())),
        None => Ok(Config::default()),
    }
}

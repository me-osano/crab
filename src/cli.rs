//! Command-line surface for the compositor.

use std::path::PathBuf;

use clap::{Parser, ValueEnum};

/// A Wayland compositor merging mango-style spaces, niri scrolling, and cosmic breadth.
#[derive(Debug, Parser)]
#[command(name = "crawm", version, about, long_about = None)]
pub struct Args {
    /// Which backend to run.
    #[arg(long, value_enum, default_value_t = Backend::Winit)]
    pub backend: Backend,

    /// Path to a TOML config file. Without this, `$XDG_CONFIG_HOME/crawm/config.toml`
    /// is used if it exists, else built-in defaults.
    #[arg(long, short)]
    pub config: Option<PathBuf>,

    /// Parse and validate the configuration, print a summary, and exit.
    #[arg(long)]
    pub validate: bool,

    /// `tracing` filter directive (overrides `RUST_LOG`).
    #[arg(long, env = "CRAWM_LOG")]
    pub log_filter: Option<String>,
}

/// The compositor backend to start.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Backend {
    /// A nested window on an existing Wayland/X11 session (development).
    Winit,
    /// A bare-metal DRM/KMS session (arrives in M4).
    Tty,
    /// No rendering; for tests and CI (partial).
    Headless,
}

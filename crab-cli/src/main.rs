//! `crab-cli` — a thin CLI over the crab IPC socket.
//!
//! ```text
//! crab-cli version
//! crab-cli outputs | spaces | windows | focused-window | layouts
//! crab-cli dispatch '{"view-space":{"index":2}}'
//! crab-cli watch
//! ```

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use crab_ipc::{Action, Client, Request, Response};

/// Query and control a running crab instance.
#[derive(Debug, Parser)]
#[command(name = "crab-cli", version, about)]
struct Args {
    /// Emit compact JSON instead of pretty-printed JSON.
    #[arg(long, global = true)]
    raw: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print the compositor name and protocol version.
    Version,
    /// List outputs.
    Outputs,
    /// List the spaces that currently exist on each output.
    Spaces,
    /// List managed windows.
    Windows,
    /// Describe the focused window.
    FocusedWindow,
    /// List available layout families and modes.
    Layouts,
    /// Run an action (JSON, e.g. `{"view-space":{"index":2}}`).
    Dispatch {
        /// The action as a JSON object.
        action: String,
    },
    /// Stream events as JSON lines until interrupted.
    Watch,
    /// Re-read and apply the active config file.
    ConfigReload,
    /// Parse and validate the active config file without applying.
    ConfigValidate,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let mut client = Client::connect_default().context("connecting to crab")?;

    match args.command {
        Command::Version => print(&client.call(&Request::Version)?, args.raw),
        Command::Outputs => print(&client.call(&Request::Outputs)?, args.raw),
        Command::Spaces => print(&client.call(&Request::Spaces)?, args.raw),
        Command::Windows => print(&client.call(&Request::Windows)?, args.raw),
        Command::FocusedWindow => print(&client.call(&Request::FocusedWindow)?, args.raw),
        Command::Layouts => print(&client.call(&Request::Layouts)?, args.raw),
        Command::Dispatch { action: json } => {
            let action: Action = serde_json::from_str(&json)
                .with_context(|| format!("parsing action JSON: {json}"))?;
            print(&client.call(&Request::Dispatch { action })?, args.raw);
        }
        Command::ConfigReload => print(&client.call(&Request::ConfigReload)?, args.raw),
        Command::ConfigValidate => print(&client.call(&Request::ConfigValidate)?, args.raw),
        Command::Watch => {
            client.send(&Request::Subscribe)?;
            loop {
                match client.read_event() {
                    // Forward compatibility: an old CLI ignores event kinds it
                    // does not know instead of failing the stream.
                    Ok(crab_ipc::Event::Unknown) => {}
                    Ok(event) => println!("{}", serde_json::to_string(&event)?),
                    Err(crab_ipc::IpcError::Closed) => break,
                    Err(err) => return Err(err).context("reading event stream"),
                }
            }
        }
    }

    Ok(())
}

fn print(response: &Response, raw: bool) {
    let text = if raw {
        serde_json::to_string(response)
    } else {
        serde_json::to_string_pretty(response)
    };
    match text {
        Ok(text) => println!("{text}"),
        Err(err) => eprintln!("crab-cli: failed to encode response: {err}"),
    }
}

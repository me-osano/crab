//! `crawm-cli` — a thin CLI over the crawm IPC socket.
//!
//! ```text
//! crawm-cli version
//! crawm-cli outputs | tags | windows | focused-window | layouts
//! crawm-cli dispatch '{"view-tag":{"index":2}}'
//! crawm-cli watch
//! ```

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use crawm_ipc::{Action, Client, Request, Response};

/// Query and control a running crawm instance.
#[derive(Debug, Parser)]
#[command(name = "crawm-cli", version, about)]
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
    /// List the tags visible on each output.
    Tags,
    /// List managed windows.
    Windows,
    /// Describe the focused window.
    FocusedWindow,
    /// List available layout names.
    Layouts,
    /// Run an action (JSON, e.g. `{"view-tag":{"index":2}}`).
    Dispatch {
        /// The action as a JSON object.
        action: String,
    },
    /// Stream events as JSON lines until interrupted.
    Watch,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let mut client = Client::connect_default().context("connecting to crawm")?;

    match args.command {
        Command::Version => print(&client.call(&Request::Version)?, args.raw),
        Command::Outputs => print(&client.call(&Request::Outputs)?, args.raw),
        Command::Tags => print(&client.call(&Request::Tags)?, args.raw),
        Command::Windows => print(&client.call(&Request::Windows)?, args.raw),
        Command::FocusedWindow => print(&client.call(&Request::FocusedWindow)?, args.raw),
        Command::Layouts => print(&client.call(&Request::Layouts)?, args.raw),
        Command::Dispatch { action: json } => {
            let action: Action = serde_json::from_str(&json)
                .with_context(|| format!("parsing action JSON: {json}"))?;
            print(&client.call(&Request::Dispatch { action })?, args.raw);
        }
        Command::Watch => {
            client.send(&Request::Subscribe)?;
            loop {
                match client.read_event() {
                    Ok(event) => println!("{}", serde_json::to_string(&event)?),
                    Err(crawm_ipc::IpcError::Closed) => break,
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
        Err(err) => eprintln!("crawm-cli: failed to encode response: {err}"),
    }
}

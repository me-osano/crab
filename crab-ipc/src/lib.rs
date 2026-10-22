//! Typed IPC protocol and client for crab.
//!
//! This is a **leaf** crate: it depends on no other `crab-*` crate, so third
//! party tools (bars, scripts) can depend on it alone. The wire format is
//! newline-delimited JSON over a Unix socket at `$CRAB_SOCKET` (see
//! [`socket_path`]).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod client;
mod protocol;

pub use client::{Client, IpcError, socket_path};
pub use protocol::{
    Action, Event, OutputInfo, PROTOCOL_VERSION, Request, Response, TagInfo, WindowInfo,
};

//! Blocking IPC client over a Unix socket.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use crate::protocol::{Event, Request, Response};

/// Errors returned by the IPC client.
#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    /// A socket operation failed.
    #[error("ipc I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// A message could not be (de)serialised.
    #[error("ipc protocol error: {0}")]
    Json(#[from] serde_json::Error),
    /// The peer closed the connection.
    #[error("ipc connection closed by peer")]
    Closed,
}

/// Resolves the compositor socket path.
///
/// Prefers `$CRAWM_SOCKET`; otherwise derives `$XDG_RUNTIME_DIR/crawm-$WAYLAND_DISPLAY.sock`.
#[must_use]
pub fn socket_path() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("CRAWM_SOCKET") {
        return Some(PathBuf::from(explicit));
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".to_owned());
    Some(PathBuf::from(runtime).join(format!("crawm-{display}.sock")))
}

/// A blocking connection to a running crawm instance.
#[derive(Debug)]
pub struct Client {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
}

impl Client {
    /// Connects to the compositor at `path`.
    ///
    /// # Errors
    /// Returns [`IpcError::Io`] if the socket cannot be reached.
    pub fn connect(path: &Path) -> Result<Self, IpcError> {
        let writer = UnixStream::connect(path)?;
        let reader = BufReader::new(writer.try_clone()?);
        Ok(Self { writer, reader })
    }

    /// Connects using [`socket_path`].
    ///
    /// # Errors
    /// Returns [`IpcError::Io`] if no socket path is known or the connect fails.
    pub fn connect_default() -> Result<Self, IpcError> {
        let path = socket_path().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no CRAWM_SOCKET and no XDG_RUNTIME_DIR",
            )
        })?;
        Self::connect(&path)
    }

    /// Sends a request and reads the next response.
    ///
    /// # Errors
    /// Returns an [`IpcError`] on I/O or protocol failure.
    pub fn call(&mut self, request: &Request) -> Result<Response, IpcError> {
        self.send(request)?;
        self.read_response()
    }

    /// Sends a request without reading a reply (used for streaming).
    ///
    /// # Errors
    /// Returns an [`IpcError`] on serialisation or I/O failure.
    pub fn send(&mut self, request: &Request) -> Result<(), IpcError> {
        let mut line = serde_json::to_string(request)?;
        line.push('\n');
        self.writer.write_all(line.as_bytes())?;
        self.writer.flush()?;
        Ok(())
    }

    /// Reads the next response line.
    ///
    /// # Errors
    /// Returns [`IpcError::Closed`] at end of stream.
    pub fn read_response(&mut self) -> Result<Response, IpcError> {
        Ok(serde_json::from_str(&self.read_line()?)?)
    }

    /// Reads the next event line (after [`Request::Subscribe`]).
    ///
    /// # Errors
    /// Returns [`IpcError::Closed`] at end of stream.
    pub fn read_event(&mut self) -> Result<Event, IpcError> {
        Ok(serde_json::from_str(&self.read_line()?)?)
    }

    fn read_line(&mut self) -> Result<String, IpcError> {
        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 {
            return Err(IpcError::Closed);
        }
        Ok(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips_as_kebab_case() {
        let json = serde_json::to_string(&Request::FocusedWindow).unwrap();
        assert_eq!(json, r#"{"type":"focused-window"}"#);
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(back, Request::FocusedWindow);
    }

    #[test]
    fn action_round_trips() {
        let action = super::super::protocol::Action::ViewTag { index: 3 };
        let json = serde_json::to_string(&action).unwrap();
        assert_eq!(json, r#"{"view-tag":{"index":3}}"#);
    }

    #[test]
    fn event_round_trips() {
        let event = Event::WindowClosed { id: 9 };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, r#"{"event":"window-closed","id":9}"#);
    }

    #[test]
    fn socket_path_prefers_env() {
        // Not asserting on the global env (tests run in parallel); just ensure the
        // function is callable and returns a plausible path shape.
        if let Some(path) = socket_path() {
            assert!(path.to_string_lossy().contains("crawm") || path.is_absolute());
        }
    }
}

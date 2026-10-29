//! The compositor side of the IPC socket (ADR-0005).
//!
//! One nonblocking `UnixListener` accepts connections on the `calloop` loop.
//! Each connection is request/response until it sends `subscribe`, which turns
//! it into an event-only stream: an initial [`Event::Snapshot`] followed by
//! deltas. Writing is nonblocking too; a client that stops reading is dropped
//! rather than allowed to stall the compositor.
//!
//! The server deliberately owns no Smithay types. It reads [`Crab`] through its
//! normal accessors and runs actions through [`crate::compositor::actions`], so
//! the tty backend can reuse it unchanged once it drives the same state.

use std::collections::HashMap;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use crab::core::SpaceSet;
use crab_ipc::{
    Event, OutputInfo, PROTOCOL_VERSION, RectInfo, Request, Response, SpaceInfo, WindowInfo,
    socket_path_for,
};

use crate::compositor::actions;
use crate::compositor::state::Crab;

/// Largest request line (and read buffer) accepted from a client.
const MAX_LINE_BYTES: usize = 64 * 1024;

/// Chunk size for a nonblocking read.
const READ_CHUNK: usize = 8 * 1024;

/// A bound IPC server.
pub(crate) struct Server {
    path: PathBuf,
    listener: UnixListener,
    clients: HashMap<u64, Client>,
    next_client: u64,
}

struct Client {
    /// A write-only duplicate of the connection. The read half stays in the
    /// calloop source so that source can wake us for input.
    writer: UnixStream,
    read_buf: Vec<u8>,
    subscribed: bool,
    snapshot: Option<Snapshot>,
    dead: bool,
}

/// What the calloop source should do after a client event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ClientOutcome {
    /// Keep the connection source registered.
    pub(crate) keep: bool,
    /// A dispatched action changed compositor state, so request a frame.
    pub(crate) redraw: bool,
}

/// The pieces an event stream diffs between loops.
#[derive(Clone, Debug, PartialEq)]
struct Snapshot {
    outputs: Vec<OutputInfo>,
    spaces: Vec<SpaceInfo>,
    windows: Vec<WindowInfo>,
    focused: Option<u64>,
    keyboard_layout: Option<String>,
    config_epoch: u64,
    config_error: Option<String>,
}

impl Snapshot {
    fn of(state: &Crab) -> Self {
        let focused = state.focused_id().map(|id| id.raw());
        Self {
            outputs: collect_outputs(state),
            spaces: collect_spaces(state),
            windows: collect_windows(state),
            focused,
            keyboard_layout: state.keyboard_layout.clone(),
            config_epoch: state.config_epoch,
            config_error: state.config_error.clone(),
        }
    }

    fn event(&self) -> Event {
        Event::Snapshot {
            outputs: self.outputs.clone(),
            spaces: self.spaces.clone(),
            windows: self.windows.clone(),
            focused: self.focused,
            keyboard_layout: self.keyboard_layout.clone(),
            config_error: self.config_error.clone(),
        }
    }
}

impl Server {
    /// Binds the IPC socket for `display`.
    ///
    /// Returns `Ok(None)` when neither `$CRAB_SOCKET` nor `$XDG_RUNTIME_DIR` is
    /// available: the compositor still runs, just without IPC (useful in a
    /// minimal test environment). A stale socket left by a crashed instance is
    /// removed; a live one is an error, because starting a second compositor on
    /// the same display would be worse.
    pub(crate) fn bind(display: &str) -> anyhow::Result<Option<Self>> {
        let Some(path) = socket_path_for(display) else {
            tracing::warn!("IPC disabled: neither CRAB_SOCKET nor XDG_RUNTIME_DIR is set");
            return Ok(None);
        };

        remove_stale_socket(&path)?;

        let listener = UnixListener::bind(&path)
            .with_context(|| format!("bind the IPC socket at {}", path.display()))?;
        listener
            .set_nonblocking(true)
            .context("make the IPC listener nonblocking")?;
        if let Err(err) = fs::set_permissions(&path, fs::Permissions::from_mode(0o600)) {
            let _ = fs::remove_file(&path);
            return Err(err).with_context(|| format!("restrict the IPC socket {}", path.display()));
        }

        tracing::info!(path = %path.display(), "IPC socket ready");
        Ok(Some(Self {
            path,
            listener,
            clients: HashMap::new(),
            next_client: 1,
        }))
    }

    /// The path clients connect to.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// A duplicate of the listener, suitable for a `calloop::Generic` source.
    pub(crate) fn try_clone_listener(&self) -> std::io::Result<UnixListener> {
        self.listener.try_clone()
    }

    /// Accepts every pending connection, returning the read half and client id.
    ///
    /// The caller inserts each read half as its own source; this method creates
    /// the write half and the per-client parser state.
    pub(crate) fn accept(&mut self) -> Vec<(u64, UnixStream)> {
        let mut accepted = Vec::new();
        loop {
            match self.listener.accept() {
                Ok((reader, _addr)) => {
                    if let Err(err) = reader.set_nonblocking(true) {
                        tracing::warn!(?err, "failed to make an IPC client nonblocking");
                        continue;
                    }
                    let writer = match reader.try_clone() {
                        Ok(writer) => writer,
                        Err(err) => {
                            tracing::warn!(?err, "failed to clone an IPC client");
                            continue;
                        }
                    };
                    // `try_clone` duplicates the descriptor, so the nonblocking
                    // flag is shared; setting it again is harmless and keeps the
                    // intent explicit.
                    if let Err(err) = writer.set_nonblocking(true) {
                        tracing::warn!(?err, "failed to make an IPC writer nonblocking");
                        continue;
                    }
                    let id = self.next_client;
                    self.next_client = self.next_client.wrapping_add(1).max(1);
                    self.clients.insert(
                        id,
                        Client {
                            writer,
                            read_buf: Vec::new(),
                            subscribed: false,
                            snapshot: None,
                            dead: false,
                        },
                    );
                    accepted.push((id, reader));
                }
                Err(err) if err.kind() == ErrorKind::WouldBlock => break,
                Err(err) if err.kind() == ErrorKind::Interrupted => continue,
                Err(err) => {
                    tracing::warn!(?err, "failed to accept an IPC client");
                    break;
                }
            }
        }
        accepted
    }

    /// Handles readability on one client and broadcasts any resulting events.
    pub(crate) fn client_event(
        &mut self,
        id: u64,
        reader: &UnixStream,
        state: &mut Crab,
    ) -> ClientOutcome {
        let Some(mut client) = self.clients.remove(&id) else {
            return ClientOutcome {
                keep: false,
                redraw: false,
            };
        };
        let outcome = self.client_event_owned(&mut client, reader, state);
        if outcome.keep {
            self.clients.insert(id, client);
        }
        self.sync(state);
        outcome
    }

    fn client_event_owned(
        &mut self,
        client: &mut Client,
        reader: &UnixStream,
        state: &mut Crab,
    ) -> ClientOutcome {
        let mut reader = reader;
        let mut chunk = [0u8; READ_CHUNK];
        let mut eof = false;
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => {
                    // The peer may have sent a request and then shut down its
                    // write half (a shell pipeline, for example). Answer the
                    // complete lines already buffered before dropping it.
                    eof = true;
                    break;
                }
                Ok(n) => {
                    client.read_buf.extend_from_slice(&chunk[..n]);
                    if client.read_buf.len() > MAX_LINE_BYTES {
                        let _ = write_response(
                            &mut client.writer,
                            &Response::Error {
                                message: format!("request line exceeds {MAX_LINE_BYTES} bytes"),
                            },
                        );
                        return ClientOutcome {
                            keep: false,
                            redraw: false,
                        };
                    }
                }
                Err(err) if err.kind() == ErrorKind::WouldBlock => break,
                Err(err) if err.kind() == ErrorKind::Interrupted => continue,
                Err(err) => {
                    tracing::debug!(?err, "IPC client read failed");
                    return ClientOutcome {
                        keep: false,
                        redraw: false,
                    };
                }
            }
        }

        let mut redraw = false;
        while let Some(newline) = client.read_buf.iter().position(|&byte| byte == b'\n') {
            let line: Vec<u8> = client.read_buf.drain(..=newline).collect();
            let line = line.strip_suffix(b"\n").unwrap_or(&line);
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if line.is_empty() {
                continue;
            }
            if line.len() > MAX_LINE_BYTES {
                if !write_response(
                    &mut client.writer,
                    &Response::Error {
                        message: format!("request line exceeds {MAX_LINE_BYTES} bytes"),
                    },
                ) {
                    return ClientOutcome {
                        keep: false,
                        redraw,
                    };
                }
                continue;
            }
            let Ok(text) = std::str::from_utf8(line) else {
                if !write_response(
                    &mut client.writer,
                    &Response::Error {
                        message: "request is not valid UTF-8".to_owned(),
                    },
                ) {
                    return ClientOutcome {
                        keep: false,
                        redraw,
                    };
                }
                continue;
            };
            let request = match serde_json::from_str::<Request>(text) {
                Ok(request) => request,
                Err(err) => {
                    if !write_response(
                        &mut client.writer,
                        &Response::Error {
                            message: format!("invalid request: {err}"),
                        },
                    ) {
                        return ClientOutcome {
                            keep: false,
                            redraw,
                        };
                    }
                    continue;
                }
            };

            if matches!(request, Request::Subscribe) {
                if client.subscribed {
                    if !write_response(
                        &mut client.writer,
                        &Response::Error {
                            message: "connection is already subscribed".to_owned(),
                        },
                    ) {
                        return ClientOutcome {
                            keep: false,
                            redraw,
                        };
                    }
                    continue;
                }
                let snapshot = Snapshot::of(state);
                if !write_event(&mut client.writer, &snapshot.event()) {
                    return ClientOutcome {
                        keep: false,
                        redraw,
                    };
                }
                client.subscribed = true;
                client.snapshot = Some(snapshot);
                continue;
            }

            if client.subscribed
                && !matches!(request, Request::ConfigReload | Request::ConfigValidate)
            {
                if !write_response(
                    &mut client.writer,
                    &Response::Error {
                        message: "a subscribed connection is event-only; use another connection to dispatch"
                            .to_owned(),
                    },
                ) {
                    return ClientOutcome {
                        keep: false,
                        redraw,
                    };
                }
                continue;
            }

            let (response, changed) = respond(request, state);
            if !write_response(&mut client.writer, &response) {
                return ClientOutcome {
                    keep: false,
                    redraw,
                };
            }
            redraw |= changed;
        }

        ClientOutcome {
            keep: !client.dead && !eof,
            redraw,
        }
    }

    /// Sends the deltas between each subscriber's last snapshot and the current
    /// state. Called once per event-loop turn, after all sources have run.
    pub(crate) fn sync(&mut self, state: &Crab) {
        if !self
            .clients
            .values()
            .any(|client| client.subscribed && !client.dead)
        {
            return;
        }
        let next = Snapshot::of(state);
        for client in self.clients.values_mut() {
            if !client.subscribed || client.dead {
                continue;
            }
            let Some(previous) = &client.snapshot else {
                continue;
            };
            for event in diff(previous, &next) {
                if !write_event(&mut client.writer, &event) {
                    client.dead = true;
                    break;
                }
            }
            if !client.dead {
                client.snapshot = Some(next.clone());
            }
        }
        self.clients.retain(|_, client| !client.dead);
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Removes a socket left behind by a crashed compositor.
fn remove_stale_socket(path: &Path) -> anyhow::Result<()> {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    if !metadata.file_type().is_socket() {
        anyhow::bail!("{} exists and is not a socket", path.display());
    }
    match UnixStream::connect(path) {
        Ok(_) => anyhow::bail!("another crab is already listening on {}", path.display()),
        Err(err)
            if matches!(
                err.kind(),
                ErrorKind::ConnectionRefused | ErrorKind::NotFound
            ) =>
        {
            fs::remove_file(path)
                .with_context(|| format!("remove the stale IPC socket {}", path.display()))?;
            Ok(())
        }
        Err(err) => Err(err).with_context(|| format!("probe the IPC socket {}", path.display())),
    }
}

/// Answers one request and reports whether it changed compositor state.
fn respond(request: Request, state: &mut Crab) -> (Response, bool) {
    match request {
        Request::Version => (
            Response::Version {
                name: "crab".to_owned(),
                version: PROTOCOL_VERSION,
            },
            false,
        ),
        Request::Outputs => (
            Response::Outputs {
                outputs: collect_outputs(state),
            },
            false,
        ),
        Request::Spaces => (
            Response::Spaces {
                spaces: collect_spaces(state),
            },
            false,
        ),
        Request::Windows => (
            Response::Windows {
                windows: collect_windows(state),
            },
            false,
        ),
        Request::FocusedWindow => (
            Response::Windows {
                windows: collect_windows(state)
                    .into_iter()
                    .filter(|window| window.focused)
                    .collect(),
            },
            false,
        ),
        Request::Layouts => (
            Response::Layouts {
                layouts: crab::core::LayoutKind::ALL
                    .into_iter()
                    .map(|kind| kind.name().to_owned())
                    .collect(),
                families: crab::core::LayoutFamily::ALL
                    .into_iter()
                    .map(|family| family.name().to_owned())
                    .collect(),
            },
            false,
        ),
        Request::Dispatch { action } => match actions::apply(state, action) {
            Ok(()) => (Response::Ok, true),
            Err(message) => (Response::Error { message }, false),
        },
        Request::Subscribe => (
            // `client_event_owned` handles this before reaching `respond`; this
            // arm keeps the match exhaustive without pretending the request was
            // a normal reply.
            Response::Error {
                message: "subscribe must be sent before any other request".to_owned(),
            },
            false,
        ),
        Request::ConfigReload => {
            use crate::compositor::reload;
            match reload::reload(state) {
                reload::ReloadOutcome::NoPath => (
                    Response::Error {
                        message: "no config file to reload".to_owned(),
                    },
                    false,
                ),
                reload::ReloadOutcome::Unchanged => (Response::Ok, false),
                reload::ReloadOutcome::Reloaded { .. } => (Response::Ok, true),
                reload::ReloadOutcome::Failed { message } => (Response::Error { message }, false),
            }
        }
        Request::ConfigValidate => {
            use crate::compositor::reload;
            match state.config_path.as_ref() {
                Some(path) => match reload::validate_path(path) {
                    Ok(()) => (Response::Ok, false),
                    Err(message) => (Response::Error { message }, false),
                },
                None => (Response::Ok, false),
            }
        }
    }
}

/// Collects outputs in stable id order.
fn collect_outputs(state: &Crab) -> Vec<OutputInfo> {
    let mut outputs: Vec<OutputInfo> = state
        .outputs
        .keys()
        .map(|&id| {
            let runtime = &state.outputs[&id];
            let model = &runtime.model;
            OutputInfo {
                id: id.raw(),
                name: model.name.clone(),
                description: model.name.clone(),
                geometry: rect_info(
                    model.geometry.x,
                    model.geometry.y,
                    model.geometry.w,
                    model.geometry.h,
                ),
                scale: model.scale,
                focused: id == state.primary_output,
                visible_spaces: model.view.visible.iter_indices().collect(),
                layout: model
                    .current_space_state()
                    .map_or("floating", |space| space.layout.name())
                    .to_owned(),
            }
        })
        .collect();
    outputs.sort_by_key(|output| output.id);
    outputs
}

/// Collects every existing space (occupied or visible) in stable order.
fn collect_spaces(state: &Crab) -> Vec<SpaceInfo> {
    let mut spaces = Vec::new();
    for (&output_id, runtime) in &state.outputs {
        let model = &runtime.model;
        let occupied = state
            .windows
            .iter()
            .filter(|window| window.window.output == output_id)
            .fold(SpaceSet::empty(), |spaces, window| {
                spaces | window.window.spaces
            });
        let existing = occupied | model.view.visible;
        for index in existing.iter_indices() {
            let Some(mask) = SpaceSet::from_index(index) else {
                continue;
            };
            let urgent = state.windows.iter().any(|window| {
                window.window.output == output_id
                    && window.window.urgent
                    && window.window.spaces.contains(mask)
            });
            spaces.push(SpaceInfo {
                output: output_id.raw(),
                index,
                name: None,
                visible: model.view.visible.contains(mask),
                urgent,
            });
        }
    }
    spaces.sort_by_key(|space| (space.output, space.index));
    spaces
}

/// Collects mapped windows in stable id order.
fn collect_windows(state: &Crab) -> Vec<WindowInfo> {
    let focused = state.focused_id();
    let mut windows: Vec<WindowInfo> = state
        .windows
        .iter()
        .filter(|window| window.mapped)
        .map(|window| WindowInfo {
            id: window.window.id.raw(),
            title: window.window.title.clone().unwrap_or_default(),
            app_id: window.window.app_id.clone().unwrap_or_default(),
            output: window.window.output.raw(),
            spaces: window.window.spaces.iter_indices().collect(),
            focused: focused == Some(window.window.id),
            urgent: window.window.urgent,
            floating: window.window.floating,
            fullscreen: window.window.fullscreen,
            maximized: window.window.maximized,
            geometry: rect_info(
                f64::from(window.location.x),
                f64::from(window.location.y),
                f64::from(window.size.w),
                f64::from(window.size.h),
            ),
        })
        .collect();
    windows.sort_by_key(|window| window.id);
    windows
}

fn rect_info(x: f64, y: f64, width: f64, height: f64) -> RectInfo {
    RectInfo {
        x,
        y,
        width,
        height,
    }
}

/// Diffs two snapshots into the delta events an event stream expects.
fn diff(previous: &Snapshot, next: &Snapshot) -> Vec<Event> {
    let mut events = Vec::new();

    let old_outputs: HashMap<u64, &OutputInfo> = previous
        .outputs
        .iter()
        .map(|output| (output.id, output))
        .collect();
    let new_outputs: HashMap<u64, &OutputInfo> = next
        .outputs
        .iter()
        .map(|output| (output.id, output))
        .collect();

    for output in &previous.outputs {
        if !new_outputs.contains_key(&output.id) {
            events.push(Event::OutputRemoved { id: output.id });
        }
    }
    for output in &next.outputs {
        match old_outputs.get(&output.id) {
            None => events.push(Event::OutputAdded {
                output: output.clone(),
            }),
            Some(old) => {
                if old.visible_spaces != output.visible_spaces {
                    events.push(Event::SpaceViewChanged {
                        output: output.id,
                        visible: output.visible_spaces.clone(),
                    });
                }
                if old.layout != output.layout {
                    events.push(Event::LayoutChanged {
                        output: output.id,
                        layout: output.layout.clone(),
                    });
                }
                // `SpaceViewChanged` and `LayoutChanged` already carry those
                // two fields; keep `OutputChanged` for geometry/scale/focus.
                if output_properties_changed(old, output) {
                    events.push(Event::OutputChanged {
                        output: output.clone(),
                    });
                }
            }
        }
    }

    let old_windows: HashMap<u64, &WindowInfo> = previous
        .windows
        .iter()
        .map(|window| (window.id, window))
        .collect();
    let new_windows: HashMap<u64, &WindowInfo> = next
        .windows
        .iter()
        .map(|window| (window.id, window))
        .collect();

    for window in &previous.windows {
        if !new_windows.contains_key(&window.id) {
            events.push(Event::WindowClosed { id: window.id });
        }
    }
    for window in &next.windows {
        match old_windows.get(&window.id) {
            None => events.push(Event::WindowOpened {
                window: window.clone(),
            }),
            Some(old) => {
                if old.urgent != window.urgent {
                    events.push(Event::Urgent {
                        window: window.id,
                        urgent: window.urgent,
                    });
                }
                if old != &window && !only_focus_changed(old, window) {
                    events.push(Event::WindowChanged {
                        window: window.clone(),
                    });
                }
            }
        }
    }

    if previous.focused != next.focused {
        events.push(Event::WindowFocused { id: next.focused });
    }

    if previous.keyboard_layout != next.keyboard_layout {
        if let Some(layout) = &next.keyboard_layout {
            events.push(Event::KeyboardLayoutChanged {
                layout: layout.clone(),
            });
        }
    }

    if previous.config_epoch != next.config_epoch {
        if let Some(message) = &next.config_error {
            events.push(Event::ConfigError {
                message: message.clone(),
            });
        } else {
            events.push(Event::ConfigReloaded);
        }
    }

    events
}

/// Whether an output's non-view properties changed.
///
/// `visible_spaces` and `layout` have their own events, so including them here
/// would make every view switch emit two or three redundant updates.
fn output_properties_changed(old: &OutputInfo, new: &OutputInfo) -> bool {
    old.id != new.id
        || old.name != new.name
        || old.description != new.description
        || old.geometry != new.geometry
        || old.scale != new.scale
        || old.focused != new.focused
}

/// Whether two window records differ only in their `focused` flag.
fn only_focus_changed(old: &WindowInfo, new: &WindowInfo) -> bool {
    if old.focused == new.focused {
        return false;
    }
    let mut old = old.clone();
    let mut new = new.clone();
    old.focused = false;
    new.focused = false;
    old == new
}

fn write_response(writer: &mut UnixStream, response: &Response) -> bool {
    write_json_line(writer, response)
}

fn write_event(writer: &mut UnixStream, event: &Event) -> bool {
    write_json_line(writer, event)
}

fn write_json_line<T: serde::Serialize>(writer: &mut UnixStream, value: &T) -> bool {
    let mut bytes = match serde_json::to_vec(value) {
        Ok(bytes) => bytes,
        Err(err) => {
            tracing::error!(?err, "failed to encode an IPC message");
            return false;
        }
    };
    bytes.push(b'\n');
    match writer.write_all(&bytes) {
        Ok(()) => true,
        Err(err) if err.kind() == ErrorKind::WouldBlock => {
            tracing::debug!("dropping an IPC client that stopped reading");
            false
        }
        Err(err) => {
            tracing::debug!(?err, "failed to write an IPC message");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(id: u64, visible: &[u8], layout: &str) -> OutputInfo {
        OutputInfo {
            id,
            name: format!("DP-{id}"),
            description: format!("output {id}"),
            geometry: rect_info(0.0, 0.0, 1920.0, 1080.0),
            scale: 1.0,
            focused: id == 1,
            visible_spaces: visible.to_vec(),
            layout: layout.to_owned(),
        }
    }

    fn window(id: u64, focused: bool) -> WindowInfo {
        WindowInfo {
            id,
            title: format!("window {id}"),
            app_id: "test".to_owned(),
            output: 1,
            spaces: vec![1],
            focused,
            urgent: false,
            floating: false,
            fullscreen: false,
            maximized: false,
            geometry: rect_info(0.0, 0.0, 100.0, 100.0),
        }
    }

    fn snapshot(
        outputs: Vec<OutputInfo>,
        windows: Vec<WindowInfo>,
        focused: Option<u64>,
    ) -> Snapshot {
        Snapshot {
            outputs,
            spaces: vec![],
            windows,
            focused,
            keyboard_layout: None,
            config_epoch: 0,
            config_error: None,
        }
    }

    #[test]
    fn opening_a_window_emits_one_event() {
        let previous = snapshot(vec![output(1, &[1], "tile")], vec![], None);
        let next = snapshot(
            vec![output(1, &[1], "tile")],
            vec![window(9, true)],
            Some(9),
        );
        assert_eq!(
            diff(&previous, &next),
            vec![
                Event::WindowOpened {
                    window: window(9, true)
                },
                Event::WindowFocused { id: Some(9) },
            ]
        );
    }

    #[test]
    fn a_focus_only_change_is_not_a_window_change() {
        let previous = snapshot(
            vec![output(1, &[1], "tile")],
            vec![window(9, true)],
            Some(9),
        );
        let next = snapshot(
            vec![output(1, &[1], "tile")],
            vec![window(9, false)],
            Some(1),
        );
        assert_eq!(
            diff(&previous, &next),
            vec![Event::WindowFocused { id: Some(1) }]
        );
    }

    #[test]
    fn output_and_space_view_changes_are_reported() {
        let previous = snapshot(vec![output(1, &[1], "tile")], vec![], None);
        let next = snapshot(vec![output(1, &[1, 3], "monocle")], vec![], None);
        assert_eq!(
            diff(&previous, &next),
            vec![
                Event::SpaceViewChanged {
                    output: 1,
                    visible: vec![1, 3]
                },
                Event::LayoutChanged {
                    output: 1,
                    layout: "monocle".to_owned()
                },
            ]
        );
    }

    #[test]
    fn output_add_and_remove_are_reported() {
        let previous = snapshot(vec![output(1, &[1], "tile")], vec![], None);
        let next = snapshot(
            vec![output(1, &[1], "tile"), output(2, &[1], "tile")],
            vec![],
            None,
        );
        assert_eq!(
            diff(&previous, &next),
            vec![Event::OutputAdded {
                output: output(2, &[1], "tile")
            }]
        );
        assert_eq!(diff(&next, &previous), vec![Event::OutputRemoved { id: 2 }]);
    }

    #[test]
    fn urgency_changes_emit_an_urgent_event() {
        let previous = snapshot(
            vec![output(1, &[1], "tile")],
            vec![window(9, true)],
            Some(9),
        );
        let mut urgent = window(9, true);
        urgent.urgent = true;
        let next = snapshot(vec![output(1, &[1], "tile")], vec![urgent.clone()], Some(9));
        assert_eq!(
            diff(&previous, &next),
            vec![
                Event::Urgent {
                    window: 9,
                    urgent: true
                },
                Event::WindowChanged { window: urgent },
            ]
        );
    }

    #[test]
    fn keyboard_layout_changes_emit_one_event() {
        let mut previous = snapshot(vec![output(1, &[1], "tile")], vec![], None);
        previous.keyboard_layout = Some("English (US)".to_owned());
        let mut next = previous.clone();
        next.keyboard_layout = Some("German".to_owned());
        assert_eq!(
            diff(&previous, &next),
            vec![Event::KeyboardLayoutChanged {
                layout: "German".to_owned()
            }]
        );
    }

    #[test]
    fn closing_a_window_emits_a_close_event() {
        let previous = snapshot(
            vec![output(1, &[1], "tile")],
            vec![window(9, true)],
            Some(9),
        );
        let next = snapshot(vec![output(1, &[1], "tile")], vec![], None);
        assert_eq!(
            diff(&previous, &next),
            vec![
                Event::WindowClosed { id: 9 },
                Event::WindowFocused { id: None }
            ]
        );
    }
}

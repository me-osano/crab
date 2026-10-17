//! Nested `winit` backend: a real Wayland compositor inside a window.
//!
//! This owns the `calloop` event loop, the Wayland `Display`, and every global
//! built by [`Crawm::new`], then wires three sources into the loop
//! (ROADMAP.md §6 M1):
//!
//! 1. **winit** — window events (resize, redraw, input, close);
//! 2. **listening socket** — new client connections;
//! 3. **display fd** — client requests, which must be dispatched even when no
//!    window event arrives (a client waiting on a `configure` never sends one).
//!
//! The tty backend (M4) reuses [`Crawm`] and the same three sources; only the
//! renderer and the input backend differ.

use std::os::unix::io::AsFd as _;
use std::sync::Arc;

use anyhow::Context as _;
use crawm::config::Config;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::winit::{self, WinitEvent, WinitGraphicsBackend};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{EventLoop, Interest, Mode, PostAction};
use smithay::reexports::wayland_server::{Display, ListeningSocket};

use crate::state::{ClientState, Crawm};
use crate::{input, render};

/// Base name for crawm's Wayland socket; `bind_auto` appends `-N`.
const SOCKET_BASENAME: &str = "crawm";

/// Everything the calloop sources read and write.
struct LoopData {
    display: Display<Crawm>,
    state: Crawm,
    backend: WinitGraphicsBackend<GlesRenderer>,
    socket: ListeningSocket,
}

/// Runs the nested compositor until the window is closed.
///
/// # Errors
/// Fails if the Wayland display, the socket, the winit backend, or the GLES
/// renderer cannot be created.
pub fn run(config: &Config) -> anyhow::Result<()> {
    let display: Display<Crawm> =
        Display::new().map_err(|err| anyhow::anyhow!("create the Wayland display: {err:?}"))?;
    let dh = display.handle();
    let mut state = Crawm::new(&dh);

    let socket = ListeningSocket::bind_auto(SOCKET_BASENAME, 0..32)
        .map_err(|err| anyhow::anyhow!("bind a Wayland socket: {err:?}"))?;
    let socket_name = socket
        .socket_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| SOCKET_BASENAME.to_owned());

    let (backend, winit_source) = winit::init::<GlesRenderer>()
        .map_err(|err| anyhow::anyhow!("initialise the winit backend: {err:?}"))?;
    state.set_size(backend.window_size());
    backend.window().request_redraw();

    let mut event_loop: EventLoop<'static, LoopData> =
        EventLoop::try_new().context("create the calloop event loop")?;
    let signal = event_loop.get_signal();

    // 1. Window events. winit drives redraws, so rendering happens here.
    event_loop
        .handle()
        .insert_source(
            winit_source,
            move |event, _, data: &mut LoopData| match event {
                WinitEvent::Resized { .. } => {
                    data.state.set_size(data.backend.window_size());
                    data.backend.window().request_redraw();
                }
                WinitEvent::Redraw => render::render(&data.state, &mut data.backend),
                WinitEvent::Input(event) => {
                    input::handle(&mut data.state, event);
                    data.backend.window().request_redraw();
                }
                WinitEvent::CloseRequested => signal.stop(),
                _ => {}
            },
        )
        .map_err(|err| anyhow::anyhow!("insert the winit event source: {err:?}"))?;

    // 2. New client connections on the listening socket.
    let socket_fd = socket
        .as_fd()
        .try_clone_to_owned()
        .context("clone the listening socket fd")?;
    event_loop
        .handle()
        .insert_source(
            Generic::new(socket_fd, Interest::READ, Mode::Level),
            |_, _, data: &mut LoopData| {
                loop {
                    match data.socket.accept() {
                        Ok(Some(stream)) => {
                            match data
                                .display
                                .handle()
                                .insert_client(stream, Arc::new(ClientState::default()))
                            {
                                Ok(_client) => tracing::debug!("client connected"),
                                Err(err) => tracing::warn!(?err, "failed to insert a client"),
                            }
                        }
                        Ok(None) => break,
                        Err(err) => {
                            tracing::warn!(?err, "failed to accept a client");
                            break;
                        }
                    }
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|err| anyhow::anyhow!("insert the socket source: {err:?}"))?;

    // 3. Client requests. Without this a client that is waiting for a
    //    `configure` (and therefore not sending window events) would stall.
    let display_fd = display
        .as_fd()
        .try_clone_to_owned()
        .context("clone the Wayland display fd")?;
    event_loop
        .handle()
        .insert_source(
            Generic::new(display_fd, Interest::READ, Mode::Level),
            |_, _, data: &mut LoopData| {
                let LoopData {
                    display,
                    state,
                    backend,
                    ..
                } = data;
                if let Err(err) = display.dispatch_clients(state) {
                    tracing::error!(?err, "failed to dispatch Wayland clients");
                }
                if let Err(err) = display.flush_clients() {
                    tracing::error!(?err, "failed to flush Wayland clients");
                }
                state.prune_dead();
                backend.window().request_redraw();
                Ok(PostAction::Continue)
            },
        )
        .map_err(|err| anyhow::anyhow!("insert the display source: {err:?}"))?;

    let mut data = LoopData {
        display,
        state,
        backend,
        socket,
    };
    tracing::info!(
        socket = %socket_name,
        gaps = config.gaps,
        "crawm (winit) ready — connect clients with WAYLAND_DISPLAY={socket_name}"
    );

    event_loop
        .run(None, &mut data, |data| {
            if let Err(err) = data.display.flush_clients() {
                tracing::error!(?err, "failed to flush Wayland clients");
            }
        })
        .map_err(|err| anyhow::anyhow!("run the calloop event loop: {err:?}"))?;
    Ok(())
}

//! Nested `winit` backend: a real Wayland compositor inside a window.
//!
//! This owns the `calloop` event loop, the Wayland `Display`, and every global
//! built by [`Crab::new`], then wires three sources into the loop
//! (ROADMAP.md §6 M1):
//!
//! 1. **winit** — window events (resize, redraw, input, close);
//! 2. **listening socket** — new client connections;
//! 3. **display fd** — client requests, which must be dispatched even when no
//!    window event arrives (a client waiting on a `configure` never sends one).
//!
//! The tty backend (M4) reuses [`Crab`] and the same three sources; only the
//! renderer and the input backend differ.

use std::os::unix::io::AsFd as _;
use std::sync::Arc;

use anyhow::Context as _;
use crab::config::Config;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::winit::{self, WinitEvent, WinitGraphicsBackend};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{EventLoop, Interest, Mode, PostAction};
use smithay::reexports::wayland_server::{Display, ListeningSocket};

use crate::compositor::state::{ClientState, Crab};
use crate::compositor::{input, render};

/// Base name for crab's Wayland socket; `bind_auto` appends `-N`.
const SOCKET_BASENAME: &str = "crab";

/// Everything the calloop sources read and write.
struct LoopData {
    display: Display<Crab>,
    state: Crab,
    backend: WinitGraphicsBackend<GlesRenderer>,
    socket: ListeningSocket,
}

/// Requests a frame, asking winit to deliver a `Redraw` only when the output
/// was idle.
///
/// Requests that arrive while a frame is queued or being presented fold into
/// it; `Output::finish_frame` then reports the follow-up frame, so no request
/// is lost (`COMPOSITOR.md` §8.2).
fn request_redraw(data: &mut LoopData) {
    if data.state.output_state.queue_redraw() {
        data.backend.window().request_redraw();
    }
}

/// Runs the nested compositor until the window is closed.
///
/// # Errors
/// Fails if the Wayland display, the socket, the winit backend, or the GLES
/// renderer cannot be created.
pub fn run(config: &Config) -> anyhow::Result<()> {
    let display: Display<Crab> =
        Display::new().map_err(|err| anyhow::anyhow!("create the Wayland display: {err:?}"))?;
    let dh = display.handle();
    let mut state = Crab::new(&dh, config);

    let socket = ListeningSocket::bind_auto(SOCKET_BASENAME, 0..32)
        .map_err(|err| anyhow::anyhow!("bind a Wayland socket: {err:?}"))?;
    let socket_name = socket
        .socket_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| SOCKET_BASENAME.to_owned());
    state.set_wayland_display(socket_name.clone());

    let (backend, winit_source) = winit::init::<GlesRenderer>()
        .map_err(|err| anyhow::anyhow!("initialise the winit backend: {err:?}"))?;
    // The host draws its own pointer over our window; hide it so the software
    // cursor in `render` is the only one visible (TODO.md §1.3).
    backend.window().set_cursor_visible(false);
    state.set_size(backend.window_size());
    // Seed the redraw state machine so every later frame takes the same path.
    state.output_state.queue_redraw();
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
                    request_redraw(data);
                }
                WinitEvent::Redraw => {
                    // winit's frame events are requests themselves (an expose or
                    // a resize), so fold them in before starting the frame.
                    data.state.output_state.queue_redraw();
                    if data.state.output_state.begin_frame() {
                        // Re-time the animation clock to the frame being drawn.
                        // winit reports no presentation timestamp, so the loop
                        // clock read here stands in for the vertical blank; the
                        // tty backend will pass the real one (TODO.md §3.2).
                        let frame_time = data.state.start_time.elapsed().as_secs_f64();
                        data.state.advance_frame(frame_time);
                        render::render(&mut data.state, &mut data.backend);
                        // Keep frames coming while anything is still moving.
                        // This folds into the frame just presented, so
                        // `finish_frame` reports the follow-up and winit
                        // schedules it; once everything settles the loop stops
                        // requesting frames and the compositor goes idle.
                        if data.state.is_animating() {
                            data.state.output_state.queue_redraw();
                        }
                        // The swap in `render` waits for the vertical blank; a
                        // request that arrived during it needs another frame.
                        if data.state.output_state.finish_frame() {
                            data.backend.window().request_redraw();
                        }
                    }
                }
                WinitEvent::Input(event) => {
                    input::handle(&mut data.state, event);
                    request_redraw(data);
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
                if state.output_state.queue_redraw() {
                    backend.window().request_redraw();
                }
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
        "crab (winit) ready — connect clients with WAYLAND_DISPLAY={socket_name}"
    );
    data.state.run_autostart();

    event_loop
        .run(None, &mut data, |data| {
            if let Err(err) = data.display.flush_clients() {
                tracing::error!(?err, "failed to flush Wayland clients");
            }
        })
        .map_err(|err| anyhow::anyhow!("run the calloop event loop: {err:?}"))?;
    Ok(())
}

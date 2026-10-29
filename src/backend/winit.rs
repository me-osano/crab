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
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use crab::config::Config;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::winit::{self, WinitEvent, WinitGraphicsBackend};
use smithay::reexports::calloop::channel::{self as calloop_channel};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{EventLoop, Interest, Mode, PostAction};
use smithay::reexports::wayland_server::{Display, ListeningSocket};
use smithay::utils::Transform;

use crate::compositor::reload;
use crate::compositor::state::{ClientState, Crab};
use crate::compositor::{input, render};
use crate::ipc;

/// Base name for crab's Wayland socket; `bind_auto` appends `-N`.
const SOCKET_BASENAME: &str = "crab";

/// Everything the calloop sources read and write.
struct LoopData {
    display: Display<Crab>,
    state: Crab,
    backend: WinitGraphicsBackend<GlesRenderer>,
    socket: ListeningSocket,
    /// `None` when no runtime directory is available; the compositor still runs.
    ipc: Option<ipc::Server>,
    #[allow(dead_code)]
    config_watcher: Option<RecommendedWatcher>,
    #[allow(dead_code)]
    reload_generation: u64,
    #[allow(dead_code)]
    pending_reload_gen: u64,
}

/// Requests a frame, asking winit to deliver a `Redraw` only when the output
/// was idle.
///
/// Requests that arrive while a frame is queued or being presented fold into
/// it; `Output::finish_frame` then reports the follow-up frame, so no request
/// is lost (`COMPOSITOR.md` §8.2).
fn request_redraw(data: &mut LoopData) {
    if data.state.output_mut().model.queue_redraw() {
        data.backend.window().request_redraw();
    }
}

fn reload_config_now(data: &mut LoopData) {
    match reload::reload(&mut data.state) {
        reload::ReloadOutcome::Reloaded { needs_redraw } => {
            tracing::info!("config reloaded");
            if needs_redraw {
                request_redraw(data);
            }
        }
        reload::ReloadOutcome::Failed { message } => {
            tracing::error!(%message, "config reload failed, keeping previous config");
            request_redraw(data);
        }
        reload::ReloadOutcome::NoPath => {
            tracing::debug!("config reload requested but no config file is set");
        }
        reload::ReloadOutcome::Unchanged => {}
    }
}

/// Runs the nested compositor until the window is closed.
///
/// # Errors
/// Fails if the Wayland display, the socket, the winit backend, or the GLES
/// renderer cannot be created.
pub fn run(config: &Config, config_path: Option<&std::path::Path>) -> anyhow::Result<()> {
    let display: Display<Crab> =
        Display::new().map_err(|err| anyhow::anyhow!("create the Wayland display: {err:?}"))?;
    let dh = display.handle();
    // Winit's default GL framebuffer is bottom-up, so the renderer must flip
    // it; this is not the output's Wayland transform, which stays Normal.
    let mut state = Crab::new(&dh, config, Transform::Flipped180);
    state.set_config_path(config_path.map(|p| p.to_path_buf()));

    let socket = ListeningSocket::bind_auto(SOCKET_BASENAME, 0..32)
        .map_err(|err| anyhow::anyhow!("bind a Wayland socket: {err:?}"))?;
    let socket_name = socket
        .socket_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| SOCKET_BASENAME.to_owned());
    state.set_wayland_display(socket_name.clone());

    let ipc = match ipc::Server::bind(&socket_name) {
        Ok(server) => server,
        Err(err) => {
            tracing::warn!(%err, "IPC server disabled");
            None
        }
    };
    if let Some(server) = &ipc {
        state.set_ipc_socket(server.path().to_path_buf());
    }

    let (backend, winit_source) = winit::init::<GlesRenderer>()
        .map_err(|err| anyhow::anyhow!("initialise the winit backend: {err:?}"))?;
    // The host draws its own pointer over our window; hide it so the software
    // cursor in `render` is the only one visible (TODO.md §1.3).
    backend.window().set_cursor_visible(false);
    state.set_size(backend.window_size());
    // Seed the redraw state machine so every later frame takes the same path.
    state.output_mut().model.queue_redraw();
    backend.window().request_redraw();

    let mut event_loop: EventLoop<'static, LoopData> =
        EventLoop::try_new().context("create the calloop event loop")?;
    let signal = Rc::new(event_loop.get_signal());
    let winit_signal = Rc::clone(&signal);

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
                    data.state.output_mut().model.queue_redraw();
                    if data.state.output_mut().model.begin_frame() {
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
                            data.state.output_mut().model.queue_redraw();
                        }
                        // The swap in `render` waits for the vertical blank; a
                        // request that arrived during it needs another frame.
                        if data.state.output_mut().model.finish_frame() {
                            data.backend.window().request_redraw();
                        }
                    }
                }
                WinitEvent::Input(event) => {
                    input::handle(&mut data.state, event);
                    request_redraw(data);
                }
                WinitEvent::CloseRequested => winit_signal.stop(),
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
                if state.output_mut().model.queue_redraw() {
                    backend.window().request_redraw();
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|err| anyhow::anyhow!("insert the display source: {err:?}"))?;

    // 4. IPC clients. The listener source accepts connections and inserts a
    //    per-client read source; writes go through the server's cloned writer.
    if let Some(server) = &ipc {
        let listener = server
            .try_clone_listener()
            .context("clone the IPC listener")?;
        let accept_handle = event_loop.handle();
        let ipc_signal = Rc::clone(&signal);
        event_loop
            .handle()
            .insert_source(
                Generic::new(listener, Interest::READ, Mode::Level),
                move |_, _, data: &mut LoopData| {
                    let Some(ipc) = &mut data.ipc else {
                        return Ok(PostAction::Continue);
                    };
                    for (id, stream) in ipc.accept() {
                        let handle = accept_handle.clone();
                        let client_signal = Rc::clone(&ipc_signal);
                        let inserted = handle.insert_source(
                            Generic::new(stream, Interest::READ, Mode::Level),
                            move |_, reader, data: &mut LoopData| {
                                let LoopData {
                                    ipc: Some(ipc),
                                    state,
                                    backend,
                                    ..
                                } = data
                                else {
                                    return Ok(PostAction::Remove);
                                };
                                let outcome = ipc.client_event(id, reader, state);
                                if outcome.redraw && state.output_mut().model.queue_redraw() {
                                    backend.window().request_redraw();
                                }
                                if state.quit_requested {
                                    client_signal.stop();
                                }
                                Ok(if outcome.keep {
                                    PostAction::Continue
                                } else {
                                    PostAction::Remove
                                })
                            },
                        );
                        if let Err(err) = inserted {
                            tracing::warn!(?err, "failed to insert an IPC client source");
                        }
                    }
                    Ok(PostAction::Continue)
                },
            )
            .map_err(|err| anyhow::anyhow!("insert the IPC listener source: {err:?}"))?;
    }

    // File watcher for hot reload (best effort).
    let (config_tx, config_rx) = calloop_channel::channel::<()>();
    let mut config_watcher: Option<RecommendedWatcher> = None;
    if let Some(path) = config_path {
        let watch_path = path.to_path_buf();
        if let Some(parent) = watch_path.parent().map(|p| p.to_path_buf()) {
            let tx_clone = config_tx.clone();
            match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if let Ok(ev) = res {
                    match ev.kind {
                        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
                            // Only trigger for the target config or its parent writes.
                            let relevant = ev.paths.is_empty()
                                || ev.paths.iter().any(|p| p == &watch_path)
                                || ev
                                    .paths
                                    .iter()
                                    .any(|p| p.file_name() == watch_path.file_name());
                            if relevant {
                                let _ = tx_clone.send(());
                            }
                        }
                        _ => {}
                    }
                }
            }) {
                Ok(mut watcher) => {
                    if let Err(err) = watcher.watch(&parent, RecursiveMode::NonRecursive) {
                        tracing::warn!(%err, path = %parent.display(), "failed to watch config directory");
                    } else {
                        config_watcher = Some(watcher);
                        tracing::info!(path = %parent.display(), "watching config directory for changes");
                    }
                }
                Err(err) => {
                    tracing::warn!(%err, "failed to create config watcher");
                }
            }
        } else {
            tracing::warn!("config path has no parent directory; hot reload disabled");
        }
    }

    let mut data = LoopData {
        display,
        state,
        backend,
        socket,
        ipc,
        config_watcher,
        reload_generation: 0,
        pending_reload_gen: 0,
    };
    tracing::info!(
        socket = %socket_name,
        gaps = config.gaps,
        "crab (winit) ready — connect clients with WAYLAND_DISPLAY={socket_name}"
    );
    data.state.run_autostart();

    // Config file change channel (debounced with timer).
    let debounce_handle = event_loop.handle();
    event_loop
        .handle()
        .insert_source(config_rx, move |event, _, data: &mut LoopData| {
            let calloop_channel::Event::Msg(()) = event else {
                return;
            };
            data.reload_generation = data.reload_generation.wrapping_add(1);
            let generation = data.reload_generation;
            let handle = debounce_handle.clone();
            let _ = handle.insert_source(
                Timer::from_duration(Duration::from_millis(250)),
                move |_, _, data: &mut LoopData| {
                    if generation == data.reload_generation {
                        reload_config_now(data);
                    }
                    TimeoutAction::Drop
                },
            );
        })
        .map_err(|err| anyhow::anyhow!("insert config reload channel: {err:?}"))?;

    let run_signal = Rc::clone(&signal);
    event_loop
        .run(None, &mut data, |data| {
            if let Err(err) = data.display.flush_clients() {
                tracing::error!(?err, "failed to flush Wayland clients");
            }
            if let Some(ipc) = &mut data.ipc {
                ipc.sync(&data.state);
            }
            if data.state.quit_requested {
                run_signal.stop();
            }
        })
        .map_err(|err| anyhow::anyhow!("run the calloop event loop: {err:?}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {}

//! Nested `winit` backend: opens a window on an existing session and clears it to
//! a solid color.
//!
//! This is the M0 smoke test (TODO 0.5). It intentionally stops short of creating
//! Wayland globals — those arrive with the real state in M1.

use anyhow::Context as _;
use crawm::config::Config;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::{Frame as _, Renderer as _};
use smithay::backend::winit::{self, WinitEvent};
use smithay::reexports::calloop::EventLoop;
use smithay::utils::{Rectangle, Transform};

/// Background color (linear-ish sRGB values) of the placeholder surface.
const BACKGROUND: [f32; 4] = [0.05, 0.06, 0.09, 1.0];

/// Runs the nested compositor until the window is closed.
///
/// # Errors
/// Fails if the event loop, the winit backend, or the GLES renderer cannot be
/// created.
pub fn run(config: &Config) -> anyhow::Result<()> {
    let mut event_loop: EventLoop<'static, ()> =
        EventLoop::try_new().context("create calloop event loop")?;
    let (mut backend, winit_source) = winit::init::<GlesRenderer>()
        .map_err(|err| anyhow::anyhow!("initialise winit backend: {err:?}"))?;
    let signal = event_loop.get_signal();

    // Ask for the first frame before the window is shown.
    backend.window().request_redraw();

    event_loop
        .handle()
        .insert_source(winit_source, move |event, _, _data| match event {
            WinitEvent::Resized { .. } => backend.window().request_redraw(),
            WinitEvent::Redraw => {
                let size = backend.window_size();
                let damage = Rectangle::from_size(size);
                match backend.bind() {
                    Ok((renderer, mut framebuffer)) => {
                        match renderer.render(&mut framebuffer, size, Transform::Flipped180) {
                            Ok(mut frame) => {
                                if let Err(err) = frame.clear(BACKGROUND.into(), &[damage]) {
                                    tracing::error!(%err, "failed to clear frame");
                                }
                            }
                            Err(err) => tracing::error!(%err, "failed to begin frame"),
                        }
                    }
                    Err(err) => {
                        tracing::error!(%err, "failed to bind framebuffer");
                        return;
                    }
                }
                if let Err(err) = backend.submit(Some(&[damage])) {
                    tracing::error!(%err, "failed to submit frame");
                }
            }
            WinitEvent::CloseRequested => signal.stop(),
            _ => {}
        })
        .map_err(|err| anyhow::anyhow!("insert winit event source: {err:?}"))?;

    tracing::info!(
        gaps = config.gaps,
        layout = %config.layout,
        "crawm (winit) running — close the window to exit"
    );

    event_loop
        .run(None, &mut (), |_| {})
        .map_err(|err| anyhow::anyhow!("run calloop event loop: {err:?}"))?;
    Ok(())
}

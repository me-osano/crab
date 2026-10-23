//! Bare-metal DRM/KMS backend.
//!
//! M4 is being built from the bottom up. This first stage opens a `libseat`
//! session, enumerates the seat's DRM devices through udev, and probes each
//! device's connectors and modes. It deliberately stops before allocating
//! buffers or modesetting: the KMS/GBM/EGL renderer is the next stage, and a
//! half-initialised output would be worse than an honest failure.
//!
//! The probed connector information is what the next stage will turn into
//! `Output` models and `OutputRuntime`s.

use std::path::Path;

use anyhow::{Context as _, bail};
use crab::config::Config;
use smithay::backend::drm::{DrmDevice, DrmDeviceFd};
use smithay::backend::session::Session as _;
use smithay::backend::session::libseat::LibSeatSession;
use smithay::backend::udev::UdevBackend;
use smithay::reexports::drm::control::Device as _;
use smithay::reexports::drm::control::connector;
use smithay::reexports::rustix::fs::OFlags;
use smithay::utils::DeviceFd;

/// Attempts to start the TTY backend.
///
/// # Errors
/// Fails when there is no seat, no DRM device, or no connected connector. It
/// also fails after successful probing until the KMS renderer lands; see
/// `TODO.md` milestone M4.
pub fn run(_config: &Config) -> anyhow::Result<()> {
    let (mut session, _notifier) =
        LibSeatSession::new().map_err(|err| anyhow::anyhow!("open a libseat session: {err:?}"))?;
    let seat = session.seat();
    tracing::info!(seat = %seat, "opened the libseat session");

    let udev = UdevBackend::new(&seat).with_context(|| format!("monitor udev on seat {seat}"))?;
    let devices: Vec<_> = udev
        .device_list()
        .map(|(id, path)| (id, path.to_path_buf()))
        .collect();
    if devices.is_empty() {
        bail!("no DRM devices found on seat {seat}");
    }

    let mut probed = 0usize;
    let mut connected = 0usize;
    for (id, path) in devices {
        tracing::info!(device = %id, path = %path.display(), "probing a DRM device");
        match probe_device(&mut session, &path) {
            Ok(count) => {
                probed += 1;
                connected += count;
            }
            Err(err) => {
                tracing::warn!(
                    device = %id,
                    path = %path.display(),
                    %err,
                    "could not probe the DRM device"
                );
            }
        }
    }

    if probed == 0 {
        bail!("could not initialise any DRM device on seat {seat}");
    }
    bail!(
        "the tty backend probed {probed} DRM device(s) with {connected} connected connector(s) \
         on seat {seat}, but KMS rendering is not wired yet — see TODO.md M4"
    )
}

/// Opens one DRM node through the session and logs its connectors.
///
/// Returns the number of connected connectors. Keeping the device and its
/// notifier alive for the whole probe is required: the notifier owns the shared
/// device state and handles session pause/resume once the event loop runs.
fn probe_device(session: &mut LibSeatSession, path: &Path) -> anyhow::Result<usize> {
    let fd = session
        .open(path, OFlags::RDWR | OFlags::CLOEXEC)
        .with_context(|| format!("open {} through libseat", path.display()))?;
    let fd = DrmDeviceFd::new(DeviceFd::from(fd));
    let (device, _notifier) = DrmDevice::new(fd, true)
        .with_context(|| format!("initialise the DRM device {}", path.display()))?;
    let resources = device
        .resource_handles()
        .with_context(|| format!("read resource handles from {}", path.display()))?;

    let mut connected = 0usize;
    for &handle in resources.connectors() {
        let info = device
            .get_connector(handle, false)
            .with_context(|| format!("read connector {handle:?} from {}", path.display()))?;
        if info.state() == connector::State::Connected {
            connected += 1;
        }
        tracing::info!(
            connector = %info,
            state = ?info.state(),
            modes = info.modes().len(),
            "DRM connector"
        );
        for mode in info.modes() {
            let (width, height) = mode.size();
            tracing::debug!(
                connector = %info,
                name = %mode.name().to_string_lossy(),
                width,
                height,
                refresh = mode.vrefresh(),
                "DRM mode"
            );
        }
    }
    Ok(connected)
}

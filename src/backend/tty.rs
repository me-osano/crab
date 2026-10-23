//! Bare-metal DRM/KMS backend.
//!
//! M4 is being built from the bottom up. This stage opens a `libseat` session,
//! enumerates the seat's DRM devices through udev, probes each device's
//! connectors and modes, and asks the pure [`OutputPlanner`] to choose a mode
//! and place every connected output. It deliberately stops before allocating
//! buffers or modesetting: the KMS/GBM/EGL renderer is the next stage, and a
//! half-initialised output would be worse than an honest failure.
//!
//! The resulting [`OutputPlan`](crab::core::OutputPlan)s are the contract the
//! renderer will consume: stable output IDs, selected modes, positions, scale,
//! transform, and physical size.

use std::path::Path;

use anyhow::{Context as _, bail};
use crab::config::Config;
use crab::core::{ConnectorInfo, ConnectorMode, OutputPlanner};
use smithay::backend::drm::{DrmDevice, DrmDeviceFd};
use smithay::backend::session::Session as _;
use smithay::backend::session::libseat::LibSeatSession;
use smithay::backend::udev::UdevBackend;
use smithay::reexports::drm::control::connector;
use smithay::reexports::drm::control::{Device as _, Mode, ModeTypeFlags};
use smithay::reexports::rustix::fs::OFlags;
use smithay::utils::DeviceFd;

/// Attempts to start the TTY backend.
///
/// # Errors
/// Fails when there is no seat, no DRM device, no connected connector, or a
/// connector cannot be planned. It also fails after successful planning until
/// the KMS renderer lands; see `TODO.md` milestone M4.
pub fn run(config: &Config) -> anyhow::Result<()> {
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
    let mut connectors = Vec::new();
    for (dev_id, path) in devices {
        tracing::info!(device = %dev_id, path = %path.display(), "probing a DRM device");
        // The device node path is more stable than udev's `dev_t` when a
        // connector is unplugged and replugged during the session.
        let device_key = path.display().to_string();
        match probe_device(&mut session, &path, &device_key) {
            Ok(mut found) => {
                probed += 1;
                connectors.append(&mut found);
            }
            Err(err) => {
                tracing::warn!(
                    device = %dev_id,
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
    let connected = connectors
        .iter()
        .filter(|connector| connector.connected)
        .count();
    if connected == 0 {
        bail!("the tty backend found no connected connectors on seat {seat}");
    }

    let preferences = config
        .output_preferences()
        .context("resolve [output.<name>] preferences")?;
    let mut planner = OutputPlanner::new();
    let plans = planner
        .plan(&connectors, &preferences)
        .context("plan DRM outputs")?;

    for plan in &plans {
        tracing::info!(
            id = %plan.id,
            key = %plan.key,
            connector = %plan.connector,
            device = %plan.device,
            mode = %plan.mode,
            position = ?plan.position,
            scale = plan.scale,
            transform = plan.transform.name(),
            logical_size = ?plan.logical_size(),
            "planned output"
        );
    }

    bail!(
        "the tty backend planned {} output(s) from {probed} DRM device(s) on seat {seat}, \
         but KMS rendering is not wired yet — see TODO.md M4",
        plans.len()
    )
}

/// Opens one DRM node through the session and probes its connectors.
///
/// Keeping the device and its notifier alive for the whole probe is required:
/// the notifier owns the shared device state and handles session pause/resume
/// once the event loop runs.
fn probe_device(
    session: &mut LibSeatSession,
    path: &Path,
    device_key: &str,
) -> anyhow::Result<Vec<ConnectorInfo>> {
    let fd = session
        .open(path, OFlags::RDWR | OFlags::CLOEXEC)
        .with_context(|| format!("open {} through libseat", path.display()))?;
    let fd = DrmDeviceFd::new(DeviceFd::from(fd));
    let (device, _notifier) = DrmDevice::new(fd, true)
        .with_context(|| format!("initialise the DRM device {}", path.display()))?;
    let resources = device
        .resource_handles()
        .with_context(|| format!("read resource handles from {}", path.display()))?;

    let mut connectors = Vec::new();
    for &handle in resources.connectors() {
        let info = device
            .get_connector(handle, false)
            .with_context(|| format!("read connector {handle:?} from {}", path.display()))?;
        let name = info.to_string();
        let connected = info.state() == connector::State::Connected;
        let physical_size_mm = info.size().and_then(|(width, height)| {
            Some((i32::try_from(width).ok()?, i32::try_from(height).ok()?))
        });
        let mut modes = Vec::with_capacity(info.modes().len());
        for mode in info.modes() {
            let (width, height) = mode.size();
            let refresh_mhz = mode_refresh_mhz(mode);
            let preferred = mode.mode_type().contains(ModeTypeFlags::PREFERRED);
            modes.push(ConnectorMode {
                width: i32::from(width),
                height: i32::from(height),
                refresh_mhz,
                preferred,
            });
            tracing::debug!(
                connector = %name,
                name = %mode.name().to_string_lossy(),
                width,
                height,
                refresh_mhz = ?refresh_mhz,
                preferred,
                "DRM mode"
            );
        }
        tracing::info!(
            connector = %name,
            connected,
            modes = modes.len(),
            "DRM connector"
        );
        connectors.push(ConnectorInfo {
            name,
            device: device_key.to_owned(),
            connected,
            modes,
            physical_size_mm,
        });
    }
    Ok(connectors)
}

/// Computes a mode's vertical refresh in millihertz from its pixel clock and
/// totals.
///
/// DRM's `vrefresh()` is an integer and cannot distinguish `59.94` from `60` or
/// `120.030` from `120`. The exact value is `clock / (htotal * vtotal)`, where
/// the clock is in kHz; multiplying by `1_000_000` gives millihertz directly.
fn mode_refresh_mhz(mode: &Mode) -> Option<u32> {
    let (_, _, htotal) = mode.hsync();
    let (_, _, vtotal) = mode.vsync();
    refresh_mhz(mode.clock(), htotal, vtotal)
}

/// The pure refresh-rate calculation behind [`mode_refresh_mhz`].
fn refresh_mhz(clock_khz: u32, htotal: u16, vtotal: u16) -> Option<u32> {
    let total = u64::from(htotal) * u64::from(vtotal);
    if clock_khz == 0 || total == 0 {
        return None;
    }
    let mhz = u64::from(clock_khz).saturating_mul(1_000_000) / total;
    u32::try_from(mhz).ok().filter(|mhz| *mhz > 0)
}

#[cfg(test)]
mod tests {
    use super::refresh_mhz;

    #[test]
    fn refresh_is_computed_from_clock_and_totals() {
        // 1920x1080 at 60 Hz: 148.5 MHz / (2200 * 1125).
        assert_eq!(refresh_mhz(148_500, 2200, 1125), Some(60_000));
        // The same calculation keeps millihertz precision.
        assert_eq!(refresh_mhz(148_500, 2200, 1124), Some(60_053));
    }

    #[test]
    fn zero_clock_or_totals_do_not_divide_by_zero() {
        assert_eq!(refresh_mhz(0, 2200, 1125), None);
        assert_eq!(refresh_mhz(148_500, 0, 1125), None);
        assert_eq!(refresh_mhz(148_500, 2200, 0), None);
    }
}

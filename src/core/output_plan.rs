//! Pure connector-to-output planning.
//!
//! A backend discovers connectors and the modes each one reports, then asks an
//! [`OutputPlanner`] to choose a mode, place the output in global logical space,
//! and allocate a stable [`OutputId`]. Nothing here touches a graphics API, so
//! the decisions can be tested without hardware.
//!
//! The planner deliberately keeps identity separate from configuration:
//! `[output.<name>]` is keyed by the connector name (`DP-1`), while the stable
//! identity is device-qualified (`/dev/dri/card0:DP-1`) so two GPUs with the
//! same connector names cannot collide.

use std::collections::BTreeMap;
use std::fmt;

use super::geometry::{Rect, Size};
use super::ids::OutputId;
use super::output::{OutputMode, OutputTransform};

/// How far a requested refresh rate may differ from a reported mode's rate.
///
/// Display modes are often advertised as `59.94 Hz` when a user writes `60`, and
/// a computed mode refresh can round a millihertz differently from the config.
/// One hertz is wide enough for those cases without making `60` match `75`.
const REFRESH_TOLERANCE_MHZ: u32 = 1000;

/// One mode reported by a connector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConnectorMode {
    /// Horizontal pixels.
    pub width: i32,
    /// Vertical pixels.
    pub height: i32,
    /// Vertical refresh in millihertz, if the backend could compute it.
    pub refresh_mhz: Option<u32>,
    /// Whether the connector marks this as its preferred mode.
    pub preferred: bool,
}

impl ConnectorMode {
    /// The mode's pixel size.
    #[must_use]
    pub const fn size(self) -> (i32, i32) {
        (self.width, self.height)
    }

    /// The mode's area in pixels, used to choose the largest fallback mode.
    fn area(self) -> i64 {
        i64::from(self.width) * i64::from(self.height)
    }

    /// How far this mode's refresh is from `requested`, for mode matching.
    fn refresh_distance(self, requested: u32) -> Option<u32> {
        self.refresh_mhz.map(|refresh| refresh.abs_diff(requested))
    }
}

impl fmt::Display for ConnectorMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{}", self.width, self.height)?;
        if let Some(refresh_mhz) = self.refresh_mhz {
            write!(f, "@{:.3}", f64::from(refresh_mhz) / 1000.0)?;
        }
        Ok(())
    }
}

/// What a backend learned about one connector.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectorInfo {
    /// Connector name as reported by the backend (`DP-1`, `eDP-1`, …).
    pub name: String,
    /// Stable device key, used to disambiguate identical connector names.
    ///
    /// An empty string means "single device"; [`ConnectorInfo::key`] then uses
    /// just the connector name.
    pub device: String,
    /// Whether a display is currently attached.
    pub connected: bool,
    /// Modes the connector reports, in backend order.
    pub modes: Vec<ConnectorMode>,
    /// Physical size in millimetres, if the connector reports one.
    pub physical_size_mm: Option<(i32, i32)>,
}

impl ConnectorInfo {
    /// Creates a connector on the default (unnamed) device.
    #[must_use]
    pub fn new(name: impl Into<String>, connected: bool, modes: Vec<ConnectorMode>) -> Self {
        Self {
            name: name.into(),
            device: String::new(),
            connected,
            modes,
            physical_size_mm: None,
        }
    }

    /// Attaches a device key to this connector.
    #[must_use]
    pub fn with_device(mut self, device: impl Into<String>) -> Self {
        self.device = device.into();
        self
    }

    /// Attaches a physical size in millimetres.
    #[must_use]
    pub fn with_physical_size(mut self, size_mm: (i32, i32)) -> Self {
        self.physical_size_mm = Some(size_mm);
        self
    }

    /// The stable, device-qualified identity used for output IDs.
    #[must_use]
    pub fn key(&self) -> String {
        if self.device.is_empty() {
            self.name.clone()
        } else {
            format!("{}:{}", self.device, self.name)
        }
    }
}

/// A resolved `[output.<name>]` table, in the form the planner consumes.
///
/// `config` resolves its string values into this type; the planner never parses
/// TOML or configuration syntax itself.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OutputPreference {
    /// Requested mode, if the user named one.
    pub mode: Option<OutputMode>,
    /// Requested logical scale.
    pub scale: Option<f64>,
    /// Requested transform.
    pub transform: Option<OutputTransform>,
    /// Requested position in global logical space.
    pub position: Option<(i32, i32)>,
}

/// A connector resolved into everything needed to register an output.
#[derive(Clone, Debug, PartialEq)]
pub struct OutputPlan {
    /// Stable handle allocated by the planner.
    pub id: OutputId,
    /// Device-qualified identity used for [`OutputPlan::id`].
    pub key: String,
    /// Connector name, also the key used for `[output.<name>]`.
    pub connector: String,
    /// Device key that owns the connector.
    pub device: String,
    /// Selected mode.
    pub mode: ConnectorMode,
    /// Top-left corner in global logical coordinates.
    pub position: (i32, i32),
    /// Logical scale factor.
    pub scale: f64,
    /// Requested output transform.
    pub transform: OutputTransform,
    /// Physical size in millimetres, if the connector reported one.
    pub physical_size_mm: Option<(i32, i32)>,
}

impl OutputPlan {
    /// The output's size in logical coordinates after scale and rotation.
    #[must_use]
    pub fn logical_size(&self) -> Size {
        let (width, height) = (f64::from(self.mode.width), f64::from(self.mode.height));
        let (width, height) = if self.transform.swaps_axes() {
            (height, width)
        } else {
            (width, height)
        };
        Size::new(width / self.scale, height / self.scale)
    }

    /// The output rectangle in global logical coordinates.
    #[must_use]
    pub fn geometry(&self) -> Rect {
        let size = self.logical_size();
        Rect::new(
            f64::from(self.position.0),
            f64::from(self.position.1),
            size.w,
            size.h,
        )
    }
}

/// Why a connector could not be planned.
#[derive(Clone, Debug, PartialEq)]
pub enum OutputPlanError {
    /// A connected connector reported no modes.
    NoModes {
        /// Connector name.
        connector: String,
    },
    /// The configured mode is not available on the connector.
    ModeUnavailable {
        /// Connector name.
        connector: String,
        /// Mode the user asked for.
        requested: OutputMode,
        /// Modes the connector actually reported.
        available: Vec<ConnectorMode>,
    },
    /// The configured scale is not a positive, finite number.
    InvalidScale {
        /// Connector name.
        connector: String,
        /// The invalid value.
        scale: f64,
    },
}

impl fmt::Display for OutputPlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoModes { connector } => {
                write!(f, "connected connector `{connector}` reported no modes")
            }
            Self::ModeUnavailable {
                connector,
                requested,
                ..
            } => write!(
                f,
                "connector `{connector}` has no mode matching `{requested}`"
            ),
            Self::InvalidScale { connector, scale } => {
                write!(f, "connector `{connector}` has invalid scale {scale}")
            }
        }
    }
}

impl std::error::Error for OutputPlanError {}

/// Allocates stable output IDs and turns connector reports into plans.
///
/// Call [`OutputPlanner::plan`] again after every hotplug. IDs are remembered
/// by device-qualified connector key, so unplugging and replugging the same
/// connector reuses its old ID and its space state.
#[derive(Debug, Default)]
pub struct OutputPlanner {
    /// IDs already handed out, keyed by [`ConnectorInfo::key`].
    ids: BTreeMap<String, OutputId>,
    /// The next ID to allocate.
    next_id: u64,
}

impl OutputPlanner {
    /// Creates an empty planner.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Plans every connected connector, in the order the backend reported them.
    ///
    /// Configured modes, positions, scales, and transforms are honoured.
    /// Unconfigured outputs use their preferred mode (or the largest available
    /// one) and are placed to the right of every configured output.
    ///
    /// # Errors
    /// Fails for a connected connector with no modes, a configured mode that the
    /// connector does not report, or an invalid scale. Disconnected connectors
    /// are ignored.
    pub fn plan(
        &mut self,
        connectors: &[ConnectorInfo],
        preferences: &BTreeMap<String, OutputPreference>,
    ) -> Result<Vec<OutputPlan>, OutputPlanError> {
        let connected: Vec<(
            &ConnectorInfo,
            OutputPreference,
            ConnectorMode,
            f64,
            OutputTransform,
        )> = connectors
            .iter()
            .filter(|connector| connector.connected)
            .map(|connector| {
                let preference = preferences
                    .get(&connector.name)
                    .copied()
                    .unwrap_or_default();
                let scale = preference.scale.unwrap_or(1.0);
                if !(scale.is_finite() && scale > 0.0) {
                    return Err(OutputPlanError::InvalidScale {
                        connector: connector.name.clone(),
                        scale,
                    });
                }
                let mode = select_mode(connector, preference.mode)?;
                let transform = preference.transform.unwrap_or_default();
                Ok((connector, preference, mode, scale, transform))
            })
            .collect::<Result<_, _>>()?;

        // Auto-placement starts after every explicitly positioned output, so an
        // auto output can never overlap one the user placed.
        let mut next_x = 0.0_f64;
        for (_, preference, mode, scale, transform) in &connected {
            if let Some((x, _)) = preference.position {
                let size = logical_size(*mode, *scale, *transform);
                next_x = next_x.max(f64::from(x) + size.w);
            }
        }

        let mut plans = Vec::with_capacity(connected.len());
        for (connector, preference, mode, scale, transform) in connected {
            let size = logical_size(mode, scale, transform);
            let position = preference.position.unwrap_or_else(|| {
                let x = next_x.round() as i32;
                next_x = next_x.max(f64::from(x) + size.w);
                (x, 0)
            });
            next_x = next_x.max(f64::from(position.0) + size.w);
            let key = connector.key();
            let id = self.id_for(&key);
            plans.push(OutputPlan {
                id,
                key,
                connector: connector.name.clone(),
                device: connector.device.clone(),
                mode,
                position,
                scale,
                transform,
                physical_size_mm: connector.physical_size_mm,
            });
        }
        Ok(plans)
    }

    /// Returns the stable ID for `key`, allocating one on first use.
    fn id_for(&mut self, key: &str) -> OutputId {
        if let Some(id) = self.ids.get(key) {
            return *id;
        }
        let id = OutputId::new(self.next_id);
        self.next_id += 1;
        self.ids.insert(key.to_owned(), id);
        id
    }
}

/// Selects the mode for a connector.
fn select_mode(
    connector: &ConnectorInfo,
    requested: Option<OutputMode>,
) -> Result<ConnectorMode, OutputPlanError> {
    if connector.modes.is_empty() {
        return Err(OutputPlanError::NoModes {
            connector: connector.name.clone(),
        });
    }

    let Some(requested) = requested else {
        return Ok(default_mode(&connector.modes));
    };

    let mut candidates: Vec<ConnectorMode> = connector
        .modes
        .iter()
        .copied()
        .filter(|mode| mode.width == requested.width && mode.height == requested.height)
        .collect();
    if candidates.is_empty() {
        return Err(mode_unavailable(connector, requested));
    }

    if let Some(refresh) = requested.refresh_mhz {
        candidates.retain(|mode| {
            mode.refresh_distance(refresh)
                .is_some_and(|distance| distance <= REFRESH_TOLERANCE_MHZ)
        });
        if candidates.is_empty() {
            return Err(mode_unavailable(connector, requested));
        }
        candidates.sort_by(|a, b| {
            a.refresh_distance(refresh)
                .unwrap_or(u32::MAX)
                .cmp(&b.refresh_distance(refresh).unwrap_or(u32::MAX))
                .then_with(|| b.preferred.cmp(&a.preferred))
                .then_with(|| b.refresh_mhz.unwrap_or(0).cmp(&a.refresh_mhz.unwrap_or(0)))
        });
        return Ok(candidates[0]);
    }

    // A size-only request takes the fastest mode of that size. A user who
    // writes `1920x1080` on a 144 Hz panel means the useful mode, not the
    // panel's conservative preferred fallback.
    candidates.sort_by(|a, b| {
        b.refresh_mhz
            .unwrap_or(0)
            .cmp(&a.refresh_mhz.unwrap_or(0))
            .then_with(|| b.preferred.cmp(&a.preferred))
    });
    Ok(candidates[0])
}

/// Chooses a mode when the user did not ask for one.
fn default_mode(modes: &[ConnectorMode]) -> ConnectorMode {
    modes
        .iter()
        .copied()
        .filter(|mode| mode.preferred)
        .max_by_key(|mode| (mode.refresh_mhz.unwrap_or(0), mode.area()))
        .or_else(|| {
            modes
                .iter()
                .copied()
                .max_by_key(|mode| (mode.area(), mode.refresh_mhz.unwrap_or(0)))
        })
        .expect("select_mode checked that modes is not empty")
}

/// Builds the "configured mode is unavailable" error.
fn mode_unavailable(connector: &ConnectorInfo, requested: OutputMode) -> OutputPlanError {
    OutputPlanError::ModeUnavailable {
        connector: connector.name.clone(),
        requested,
        available: connector.modes.clone(),
    }
}

/// The logical size of a mode after scale and rotation.
fn logical_size(mode: ConnectorMode, scale: f64, transform: OutputTransform) -> Size {
    let (width, height) = (f64::from(mode.width), f64::from(mode.height));
    let (width, height) = if transform.swaps_axes() {
        (height, width)
    } else {
        (width, height)
    };
    Size::new(width / scale, height / scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(width: i32, height: i32, refresh_mhz: u32, preferred: bool) -> ConnectorMode {
        ConnectorMode {
            width,
            height,
            refresh_mhz: Some(refresh_mhz),
            preferred,
        }
    }

    fn connector(name: &str, modes: Vec<ConnectorMode>) -> ConnectorInfo {
        ConnectorInfo::new(name, true, modes)
    }

    fn preference(mode: Option<OutputMode>, scale: Option<f64>) -> OutputPreference {
        OutputPreference {
            mode,
            scale,
            ..OutputPreference::default()
        }
    }

    #[test]
    fn unconfigured_outputs_take_the_preferred_mode_and_auto_position() {
        let connectors = vec![
            connector(
                "DP-1",
                vec![
                    mode(1920, 1080, 60_000, false),
                    mode(2560, 1440, 144_000, true),
                ],
            ),
            connector("DP-2", vec![mode(1920, 1080, 60_000, true)]),
        ];
        let mut planner = OutputPlanner::new();
        let plans = planner.plan(&connectors, &BTreeMap::new()).unwrap();

        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].mode.size(), (2560, 1440));
        assert_eq!(plans[0].position, (0, 0));
        assert_eq!(plans[1].mode.size(), (1920, 1080));
        assert_eq!(plans[1].position, (2560, 0));
        assert_ne!(plans[0].id, plans[1].id);
    }

    #[test]
    fn configured_refresh_matches_a_nearby_hardware_rate() {
        let connectors = vec![connector("DP-1", vec![mode(1920, 1080, 59_940, false)])];
        let mut preferences = BTreeMap::new();
        preferences.insert(
            "DP-1".to_owned(),
            preference(Some(OutputMode::parse("1920x1080@60").unwrap()), None),
        );

        let mut planner = OutputPlanner::new();
        let plans = planner.plan(&connectors, &preferences).unwrap();
        assert_eq!(plans[0].mode.refresh_mhz, Some(59_940));
    }

    #[test]
    fn configured_decimal_refresh_matches_its_millihertz_value() {
        let connectors = vec![connector("eDP-1", vec![mode(1920, 1080, 120_030, false)])];
        let mut preferences = BTreeMap::new();
        preferences.insert(
            "eDP-1".to_owned(),
            preference(Some(OutputMode::parse("1920x1080@120.030").unwrap()), None),
        );

        let mut planner = OutputPlanner::new();
        let plans = planner.plan(&connectors, &preferences).unwrap();
        assert_eq!(plans[0].mode.refresh_mhz, Some(120_030));
    }

    #[test]
    fn size_only_request_takes_the_fastest_mode() {
        let connectors = vec![connector(
            "DP-1",
            vec![
                mode(1920, 1080, 60_000, true),
                mode(1920, 1080, 144_000, false),
            ],
        )];
        let mut preferences = BTreeMap::new();
        preferences.insert(
            "DP-1".to_owned(),
            preference(Some(OutputMode::parse("1920x1080").unwrap()), None),
        );

        let mut planner = OutputPlanner::new();
        let plans = planner.plan(&connectors, &preferences).unwrap();
        assert_eq!(plans[0].mode.refresh_mhz, Some(144_000));
    }

    #[test]
    fn an_unavailable_configured_mode_is_an_error() {
        let connectors = vec![connector("DP-1", vec![mode(1920, 1080, 60_000, true)])];
        let mut preferences = BTreeMap::new();
        preferences.insert(
            "DP-1".to_owned(),
            preference(Some(OutputMode::parse("3840x2160@60").unwrap()), None),
        );

        let mut planner = OutputPlanner::new();
        let err = planner.plan(&connectors, &preferences).unwrap_err();
        assert!(matches!(err, OutputPlanError::ModeUnavailable { .. }));
    }

    #[test]
    fn auto_position_starts_after_configured_outputs() {
        let connectors = vec![
            connector("DP-1", vec![mode(1920, 1080, 60_000, true)]),
            connector("DP-2", vec![mode(1920, 1080, 60_000, true)]),
        ];
        let mut preferences = BTreeMap::new();
        preferences.insert(
            "DP-1".to_owned(),
            OutputPreference {
                position: Some((1920, 0)),
                ..OutputPreference::default()
            },
        );

        let mut planner = OutputPlanner::new();
        let plans = planner.plan(&connectors, &preferences).unwrap();
        assert_eq!(plans[0].position, (1920, 0));
        assert_eq!(plans[1].position, (3840, 0));
    }

    #[test]
    fn logical_size_accounts_for_scale_and_rotation() {
        let connectors = vec![connector("DP-1", vec![mode(1920, 1080, 60_000, true)])];
        let mut preferences = BTreeMap::new();
        preferences.insert(
            "DP-1".to_owned(),
            OutputPreference {
                scale: Some(1.25),
                transform: Some(OutputTransform::Rotate90),
                ..OutputPreference::default()
            },
        );

        let mut planner = OutputPlanner::new();
        let plans = planner.plan(&connectors, &preferences).unwrap();
        assert_eq!(plans[0].logical_size(), Size::new(864.0, 1536.0));
        assert_eq!(plans[0].geometry(), Rect::new(0.0, 0.0, 864.0, 1536.0));
    }

    #[test]
    fn disconnected_connectors_are_ignored() {
        let connectors = vec![
            ConnectorInfo::new("DP-1", false, vec![mode(1920, 1080, 60_000, true)]),
            connector("DP-2", vec![mode(1920, 1080, 60_000, true)]),
        ];
        let mut planner = OutputPlanner::new();
        let plans = planner.plan(&connectors, &BTreeMap::new()).unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].connector, "DP-2");
    }

    #[test]
    fn ids_are_stable_across_replugs() {
        let dp1 = connector("DP-1", vec![mode(1920, 1080, 60_000, true)]);
        let dp2 = connector("DP-2", vec![mode(1920, 1080, 60_000, true)]);
        let mut planner = OutputPlanner::new();

        let first = planner
            .plan(&[dp1.clone(), dp2.clone()], &BTreeMap::new())
            .unwrap();
        let (dp1_id, dp2_id) = (first[0].id, first[1].id);

        let only_dp2 = planner
            .plan(std::slice::from_ref(&dp2), &BTreeMap::new())
            .unwrap();
        assert_eq!(only_dp2[0].id, dp2_id);

        let both_again = planner.plan(&[dp1, dp2], &BTreeMap::new()).unwrap();
        assert_eq!(both_again[0].id, dp1_id);
        assert_eq!(both_again[1].id, dp2_id);
    }

    #[test]
    fn identical_names_on_different_devices_get_distinct_ids() {
        let gpu0 = connector("DP-1", vec![mode(1920, 1080, 60_000, true)]).with_device("gpu0");
        let gpu1 = connector("DP-1", vec![mode(1920, 1080, 60_000, true)]).with_device("gpu1");

        let mut planner = OutputPlanner::new();
        let plans = planner.plan(&[gpu0, gpu1], &BTreeMap::new()).unwrap();
        assert_eq!(plans.len(), 2);
        assert_ne!(plans[0].id, plans[1].id);
        assert_eq!(plans[0].position, (0, 0));
        assert_eq!(plans[1].position, (1920, 0));
    }

    #[test]
    fn a_connected_connector_without_modes_is_an_error() {
        let connectors = vec![connector("DP-1", Vec::new())];
        let mut planner = OutputPlanner::new();
        let err = planner.plan(&connectors, &BTreeMap::new()).unwrap_err();
        assert!(matches!(err, OutputPlanError::NoModes { .. }));
    }

    #[test]
    fn an_invalid_scale_is_an_error() {
        let connectors = vec![connector("DP-1", vec![mode(1920, 1080, 60_000, true)])];
        let mut preferences = BTreeMap::new();
        preferences.insert("DP-1".to_owned(), preference(None, Some(0.0)));

        let mut planner = OutputPlanner::new();
        let err = planner.plan(&connectors, &preferences).unwrap_err();
        assert!(matches!(err, OutputPlanError::InvalidScale { .. }));
    }
}

//! TOML configuration: parsing, validation, and defaults.
//!
//! The config format is **TOML** (ADR-0003), parsed with `serde` and the `toml`
//! crate. Unknown keys are ignored, so a config keeps working across versions;
//! a key that is *present* but has the wrong type is an error, so a typo is
//! reported rather than silently defaulted.
//!
//! `src/config` has no Smithay/Wayland dependency (ROADMAP §5); it depends only
//! on [`crate::core`] for shared primitives like [`crate::core::Color`], and
//! only *downward* (`config → core`).
//!
//! ```
//! let config = crawm::config::parse("gaps = 4\n").unwrap();
//! assert_eq!(config.gaps, 4);
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::path::{Path, PathBuf};

use crate::core::{Color, LayoutKind, LayoutParams};
use serde::Deserialize;

/// The compositor configuration.
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// Outer/inner gap in logical pixels.
    pub gaps: i32,
    /// Window border width in logical pixels (`0` disables borders).
    pub border_width: i32,
    /// Border color of the focused window.
    pub border_color: Color,
    /// Border color of unfocused windows.
    pub unfocused_border_color: Color,
    /// Default number of windows in the master area.
    pub master_count: i32,
    /// Default master-area fraction, within `LayoutParams::MIN_RATIO..=MAX_RATIO`.
    pub master_ratio: f64,
    /// Default layout name for new spaces.
    pub layout: String,
    /// Whether animations are enabled (also toggled at runtime by IPC).
    pub animations: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            gaps: 8,
            border_width: 2,
            border_color: Color::from_hex("#7aa2f7").expect("static color is valid"),
            unfocused_border_color: Color::from_hex("#3b4261").expect("static color is valid"),
            master_count: 1,
            master_ratio: 0.6,
            layout: "tile".to_owned(),
            animations: true,
        }
    }
}

/// Errors produced while loading a configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file could not be read.
    #[error("failed to read config {path}: {source}")]
    Io {
        /// Path that could not be read.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The TOML could not be parsed, or a value had the wrong type.
    #[error("failed to parse config: {0}")]
    Parse(String),
    /// The config parsed but is semantically invalid.
    #[error("invalid config: {0}")]
    Invalid(String),
}

/// The on-disk shape, before validation.
///
/// Every field is optional so a partial file inherits the defaults. Unknown keys
/// are ignored (`serde`'s default), which keeps older configs working as crawm
/// grows; present-but-wrongly-typed keys still fail.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawConfig {
    gaps: Option<i64>,
    border_width: Option<i64>,
    border_color: Option<String>,
    unfocused_border_color: Option<String>,
    master_count: Option<i64>,
    master_ratio: Option<f64>,
    layout: Option<String>,
    animations: Option<RawAnimations>,
}

/// The `[animations]` table.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawAnimations {
    enabled: Option<bool>,
}

impl RawConfig {
    fn into_config(self) -> Result<Config, ConfigError> {
        let mut config = Config::default();

        if let Some(gaps) = self.gaps {
            config.gaps = checked_i32(gaps, "gaps")?;
        }
        if let Some(width) = self.border_width {
            config.border_width = checked_i32(width, "border_width")?;
        }
        if let Some(hex) = self.border_color {
            config.border_color = Color::from_hex(&hex)
                .map_err(|err| ConfigError::Parse(format!("`border_color`: {err}")))?;
        }
        if let Some(hex) = self.unfocused_border_color {
            config.unfocused_border_color = Color::from_hex(&hex)
                .map_err(|err| ConfigError::Parse(format!("`unfocused_border_color`: {err}")))?;
        }
        if let Some(count) = self.master_count {
            config.master_count = checked_i32(count, "master_count")?;
        }
        if let Some(ratio) = self.master_ratio {
            config.master_ratio = ratio;
        }
        if let Some(layout) = self.layout {
            config.layout = layout;
        }
        if let Some(enabled) = self.animations.and_then(|animations| animations.enabled) {
            config.animations = enabled;
        }

        Ok(config)
    }
}

/// Parses a configuration from TOML text (does not validate).
///
/// # Errors
/// Returns [`ConfigError::Parse`] on malformed TOML or wrongly-typed values.
pub fn parse(text: &str) -> Result<Config, ConfigError> {
    let raw: RawConfig = toml::from_str(text).map_err(|err| ConfigError::Parse(err.to_string()))?;
    raw.into_config()
}

/// Loads and validates a configuration from `path`.
///
/// # Errors
/// Returns a [`ConfigError`] if the file cannot be read, parsed, or validated.
pub fn load(path: &Path) -> Result<Config, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let config = parse(&text)?;
    validate(&config)?;
    Ok(config)
}

/// Checks semantic constraints that parsing cannot (e.g. non-negative sizes).
///
/// # Errors
/// Returns [`ConfigError::Invalid`] describing the first violated constraint.
pub fn validate(config: &Config) -> Result<(), ConfigError> {
    if config.gaps < 0 {
        return Err(ConfigError::Invalid("`gaps` must be >= 0".to_owned()));
    }
    if config.border_width < 0 {
        return Err(ConfigError::Invalid(
            "`border_width` must be >= 0".to_owned(),
        ));
    }
    if config.master_count < 0 {
        return Err(ConfigError::Invalid(
            "`master_count` must be >= 0".to_owned(),
        ));
    }
    if !(LayoutParams::MIN_RATIO..=LayoutParams::MAX_RATIO).contains(&config.master_ratio) {
        return Err(ConfigError::Invalid(format!(
            "`master_ratio` must be within {}..={}",
            LayoutParams::MIN_RATIO,
            LayoutParams::MAX_RATIO
        )));
    }
    if LayoutKind::from_name(&config.layout).is_none() {
        return Err(ConfigError::Invalid(format!(
            "`layout` must be one of {:?}",
            LayoutKind::ALL.map(LayoutKind::name)
        )));
    }
    Ok(())
}

fn checked_i32(raw: i64, name: &str) -> Result<i32, ConfigError> {
    i32::try_from(raw).map_err(|_| ConfigError::Invalid(format!("`{name}` is out of range")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        validate(&Config::default()).unwrap();
    }

    #[test]
    fn parses_overrides() {
        let cfg = parse(
            r##"
            gaps = 4
            border_width = 3
            border_color = "#ff0000"
            unfocused_border_color = "#222222"
            master_count = 2
            master_ratio = 0.4
            layout = "floating"

            [animations]
            enabled = false
            "##,
        )
        .unwrap();
        assert_eq!(cfg.gaps, 4);
        assert_eq!(cfg.border_width, 3);
        assert_eq!(cfg.border_color, Color::rgb(1.0, 0.0, 0.0));
        let grey = 0x22 as f32 / 255.0;
        assert_eq!(cfg.unfocused_border_color, Color::rgb(grey, grey, grey));
        assert_eq!(cfg.master_count, 2);
        assert!((cfg.master_ratio - 0.4).abs() < 1e-9);
        assert_eq!(cfg.layout, "floating");
        assert!(!cfg.animations);
    }

    #[test]
    fn partial_config_inherits_defaults() {
        let cfg = parse("gaps = 2\n").unwrap();
        assert_eq!(cfg.gaps, 2);
        assert_eq!(cfg.border_width, Config::default().border_width);
        assert_eq!(cfg.layout, Config::default().layout);
    }

    #[test]
    fn ignores_unknown_keys() {
        let cfg = parse("future_thing = 42\n").unwrap();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn rejects_wrong_type() {
        assert!(matches!(
            parse("gaps = \"lots\""),
            Err(ConfigError::Parse(_))
        ));
    }

    #[test]
    fn rejects_negative_gaps() {
        let cfg = parse("gaps = -1").unwrap();
        assert!(matches!(validate(&cfg), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn rejects_unknown_layout() {
        let cfg = parse("layout = \"spiral\"").unwrap();
        let err = validate(&cfg).unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
        assert!(err.to_string().contains("layout"));
    }

    #[test]
    fn rejects_out_of_range_ratio() {
        let cfg = parse("master_ratio = 1.5").unwrap();
        assert!(matches!(validate(&cfg), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn rejects_negative_master_count() {
        let cfg = parse("master_count = -1").unwrap();
        assert!(matches!(validate(&cfg), Err(ConfigError::Invalid(_))));
    }
}

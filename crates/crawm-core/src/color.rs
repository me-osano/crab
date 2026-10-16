//! RGBA colors used by config, borders, and effects.

use core::fmt;

/// An RGBA color with components in `0.0..=1.0` (straight, non-premultiplied).
///
/// Values are stored as `f32` to match what the GLES renderer consumes. Color
/// management (linear working space, HDR) arrives post-0.1 (ADR-0002); for now
/// these are treated as sRGB-encoded.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    /// Red.
    pub r: f32,
    /// Green.
    pub g: f32,
    /// Blue.
    pub b: f32,
    /// Alpha.
    pub a: f32,
}

impl Color {
    /// Creates a color from explicit components.
    #[must_use]
    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// Creates an opaque color.
    #[must_use]
    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self::rgba(r, g, b, 1.0)
    }

    /// Parses `#rrggbb` or `#rrggbbaa` (the leading `#` is optional).
    ///
    /// # Errors
    /// Returns [`ColorParseError`] if the string is not a valid hex color.
    pub fn from_hex(input: &str) -> Result<Self, ColorParseError> {
        let hex = input.strip_prefix('#').unwrap_or(input);
        if hex.len() != 6 && hex.len() != 8 {
            return Err(ColorParseError::Length);
        }
        let byte = |range: core::ops::Range<usize>| -> Result<u8, ColorParseError> {
            u8::from_str_radix(&hex[range], 16).map_err(|_| ColorParseError::Digit)
        };
        let r = byte(0..2)?;
        let g = byte(2..4)?;
        let b = byte(4..6)?;
        let a = if hex.len() == 8 { byte(6..8)? } else { 0xff };
        Ok(Self::rgba(
            f32::from(r) / 255.0,
            f32::from(g) / 255.0,
            f32::from(b) / 255.0,
            f32::from(a) / 255.0,
        ))
    }
}

impl Default for Color {
    fn default() -> Self {
        Self::rgb(0.0, 0.0, 0.0)
    }
}

/// Error returned when a hex color cannot be parsed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorParseError {
    /// The string (minus an optional `#`) was not 6 or 8 hex digits.
    Length,
    /// A component contained a non-hexadecimal character.
    Digit,
}

impl fmt::Display for ColorParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Length => f.write_str("expected #rrggbb or #rrggbbaa"),
            Self::Digit => f.write_str("contains a non-hexadecimal digit"),
        }
    }
}

impl std::error::Error for ColorParseError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rgb_and_rgba() {
        assert_eq!(Color::from_hex("#ff0000"), Ok(Color::rgb(1.0, 0.0, 0.0)));
        assert_eq!(Color::from_hex("00ff00"), Ok(Color::rgb(0.0, 1.0, 0.0)));
        assert_eq!(
            Color::from_hex("#00000080"),
            Ok(Color::rgba(0.0, 0.0, 0.0, 128.0 / 255.0))
        );
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(Color::from_hex("#12345"), Err(ColorParseError::Length));
        assert_eq!(Color::from_hex("#gggggg"), Err(ColorParseError::Digit));
    }
}

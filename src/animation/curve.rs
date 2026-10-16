//! Easing curves and spring parameters.

use crate::core::Point;

/// Parameters of a damped spring, in SI-ish units (stiffness `N/m`, damping `N·s/m`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringParams {
    /// Spring stiffness (`k`).
    pub stiffness: f64,
    /// Damping coefficient (`c`).
    pub damping: f64,
}

impl SpringParams {
    /// A spring that reaches its target quickly with minimal overshoot.
    ///
    /// `damping = 2·√200` (critically damped); written literally because
    /// `f64::sqrt` is not `const`.
    pub const DEFAULT: Self = Self {
        stiffness: 200.0,
        damping: 28.284_271_247_461_9,
    };

    /// A critically damped spring for the given stiffness (`c = 2·√k`).
    #[must_use]
    pub fn critically_damped(stiffness: f64) -> Self {
        Self {
            stiffness,
            damping: 2.0 * stiffness.max(0.0).sqrt(),
        }
    }
}

impl Default for SpringParams {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A time-parameterised easing function over the unit interval `0.0..=1.0`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Easing {
    /// No easing; constant velocity.
    #[default]
    Linear,
    /// Fast start, gentle finish.
    EaseOutCubic,
    /// Gentle start and finish.
    EaseInOutCubic,
}

impl Easing {
    /// Evaluates the curve at `t` (clamped to `0.0..=1.0`).
    #[must_use]
    pub fn eval(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => t,
            Self::EaseOutCubic => 1.0 - (1.0 - t).powi(3),
            Self::EaseInOutCubic => {
                if t < 0.5 {
                    4.0 * t.powi(3)
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                }
            }
        }
    }
}

/// Linearly interpolates between two points (`t` clamped to `0.0..=1.0`).
///
/// Used for gesture-driven drag previews, where the pointer drives the value
/// directly rather than through a spring.
#[must_use]
pub fn lerp_point(a: Point, b: Point, t: f64) -> Point {
    let t = t.clamp(0.0, 1.0);
    Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easings_span_the_unit_interval() {
        for easing in [Easing::Linear, Easing::EaseOutCubic, Easing::EaseInOutCubic] {
            assert!((easing.eval(0.0) - 0.0).abs() < 1e-9);
            assert!((easing.eval(1.0) - 1.0).abs() < 1e-9);
            assert!((easing.eval(-5.0) - 0.0).abs() < 1e-9, "clamps low");
            assert!((easing.eval(5.0) - 1.0).abs() < 1e-9, "clamps high");
        }
    }

    #[test]
    fn lerp_point_midpoint() {
        let p = lerp_point(Point::new(0.0, 0.0), Point::new(10.0, 20.0), 0.5);
        assert_eq!(p, Point::new(5.0, 10.0));
    }
}

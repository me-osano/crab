//! The curves an animated value can follow to its target.

use super::SpringParams;

/// A time-parameterised easing function over the unit interval.
///
/// These are the curves mango-style configs name directly; the cubic Bézier
/// below covers everything else.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Easing {
    /// No easing; constant velocity.
    #[default]
    Linear,
    /// Fast start, gentle finish.
    EaseOutQuad,
    /// Faster start, gentler finish.
    EaseOutCubic,
    /// Fastest start, gentlest finish.
    EaseOutExpo,
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
            Self::EaseOutQuad => 1.0 - (1.0 - t).powi(2),
            Self::EaseOutCubic => 1.0 - (1.0 - t).powi(3),
            Self::EaseOutExpo => {
                if t >= 1.0 {
                    1.0
                } else {
                    1.0 - (-10.0 * t).exp2()
                }
            }
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

/// A CSS-style cubic Bézier timing function.
///
/// The curve runs from `(0, 0)` to `(1, 1)` through two control points. The
/// solver bisects for the curve parameter whose x is the requested progress and
/// then reads its y, the same shape CSS `cubic-bezier()` and niri's curves have
/// (niri follows libadwaita's solver; see `NOTICE`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CubicBezier {
    /// The first control point.
    pub p1: (f64, f64),
    /// The second control point.
    pub p2: (f64, f64),
}

impl CubicBezier {
    /// Creates a curve from its two control points.
    #[must_use]
    pub const fn new(p1: (f64, f64), p2: (f64, f64)) -> Self {
        Self { p1, p2 }
    }

    /// Evaluates the curve at `x` (clamped to `0.0..=1.0`).
    #[must_use]
    pub fn eval(self, x: f64) -> f64 {
        let x = x.clamp(0.0, 1.0);
        if x <= 0.0 {
            return 0.0;
        }
        if x >= 1.0 {
            return 1.0;
        }
        self.y_for_t(self.t_for_x(x))
    }

    /// One coordinate of the curve at parameter `t`, with endpoints `0` and `1`.
    fn coordinate(self, t: f64, c1: f64, c2: f64) -> f64 {
        let omt = 1.0 - t;
        3.0 * omt * omt * t * c1 + 3.0 * omt * t * t * c2 + t * t * t
    }

    /// The curve's x coordinate at parameter `t`.
    fn x_for_t(self, t: f64) -> f64 {
        self.coordinate(t, self.p1.0, self.p2.0)
    }

    /// The curve's y coordinate at parameter `t`.
    fn y_for_t(self, t: f64) -> f64 {
        self.coordinate(t, self.p1.1, self.p2.1)
    }

    /// The parameter whose x coordinate is `x`, found by bisection.
    ///
    /// Newton's method would converge faster but can wander off on a steep
    /// curve; 30 halvings of the unit interval resolve far past f64 precision.
    fn t_for_x(self, x: f64) -> f64 {
        let (mut low, mut high) = (0.0, 1.0);
        for _ in 0..30 {
            let t = (low + high) / 2.0;
            if x < self.x_for_t(t) {
                high = t;
            } else {
                low = t;
            }
        }
        (low + high) / 2.0
    }
}

/// How an animated value travels to its target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Curve {
    /// Physics: the value is pulled toward its target, carrying its velocity
    /// across a retarget.
    Spring(SpringParams),
    /// A fixed-duration easing.
    Easing {
        /// How long the value takes, in milliseconds.
        duration_ms: u32,
        /// The easing applied over that time.
        kind: Easing,
    },
    /// A fixed-duration cubic Bézier, for hand-tuned motion.
    Bezier {
        /// How long the value takes, in milliseconds.
        duration_ms: u32,
        /// The two control points.
        control: CubicBezier,
    },
}

impl Curve {
    /// The curve that jumps straight to the target (`reduce-motion`, M5).
    pub const INSTANT: Self = Self::Easing {
        duration_ms: 0,
        kind: Easing::Linear,
    };

    /// How long a segment following this curve lasts, in seconds.
    ///
    /// For a spring that is its settle time ([`SpringParams::settle_secs`]),
    /// which is a property of its parameters rather than a duration anyone
    /// asked for; for the rest it is the configured duration.
    #[must_use]
    pub fn settle_secs(self) -> f64 {
        match self {
            Self::Spring(params) => params.settle_secs(),
            Self::Easing { duration_ms, .. } | Self::Bezier { duration_ms, .. } => {
                f64::from(duration_ms) / 1000.0
            }
        }
    }

    /// Whether the value jumps straight to its target.
    #[must_use]
    pub fn is_instant(self) -> bool {
        match self {
            Self::Spring(_) => false,
            Self::Easing { duration_ms, .. } | Self::Bezier { duration_ms, .. } => duration_ms == 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_easing_spans_the_unit_interval() {
        for easing in [
            Easing::Linear,
            Easing::EaseOutQuad,
            Easing::EaseOutCubic,
            Easing::EaseOutExpo,
            Easing::EaseInOutCubic,
        ] {
            assert!((easing.eval(0.0) - 0.0).abs() < 1e-9, "{easing:?} at 0");
            assert!((easing.eval(1.0) - 1.0).abs() < 1e-9, "{easing:?} at 1");
            assert!(
                (easing.eval(-5.0) - 0.0).abs() < 1e-9,
                "{easing:?} clamps low"
            );
            assert!(
                (easing.eval(5.0) - 1.0).abs() < 1e-9,
                "{easing:?} clamps high"
            );
        }
    }

    #[test]
    fn easing_curves_are_monotone() {
        for easing in [
            Easing::EaseOutQuad,
            Easing::EaseOutCubic,
            Easing::EaseOutExpo,
            Easing::EaseInOutCubic,
        ] {
            let mut previous = 0.0;
            for step in 0..=100 {
                let value = easing.eval(f64::from(step) / 100.0);
                assert!(value >= previous, "{easing:?} went backwards at {step}");
                previous = value;
            }
        }
    }

    #[test]
    fn a_symmetric_bezier_passes_through_the_middle() {
        // The CSS `ease-in-out` shape is symmetric about (0.5, 0.5).
        let curve = CubicBezier::new((0.42, 0.0), (0.58, 1.0));
        assert!(
            (curve.eval(0.5) - 0.5).abs() < 1e-6,
            "got {}",
            curve.eval(0.5)
        );
        assert_eq!(curve.eval(0.0), 0.0);
        assert_eq!(curve.eval(1.0), 1.0);
    }

    #[test]
    fn a_bezier_can_start_fast() {
        // `ease-out` reaches most of the way early, unlike linear.
        let curve = CubicBezier::new((0.0, 0.0), (0.58, 1.0));
        assert!(curve.eval(0.25) > 0.25, "got {}", curve.eval(0.25));
        assert!(curve.eval(0.5) > 0.5);
    }

    #[test]
    fn duration_and_instancy_follow_the_curve() {
        let spring = Curve::Spring(SpringParams::DEFAULT);
        assert!(
            spring.settle_secs() > 0.0,
            "a spring settles on its own clock"
        );
        assert!(!spring.is_instant());

        let easing = Curve::Easing {
            duration_ms: 250,
            kind: Easing::EaseOutCubic,
        };
        assert_eq!(easing.settle_secs(), 0.25);
        assert!(!easing.is_instant());

        assert!(Curve::INSTANT.is_instant());
        assert_eq!(Curve::INSTANT.settle_secs(), 0.0);
    }
}

//! Damped-spring parameters and the closed-form solution they describe.
//!
//! Adapted from niri's `src/animation/spring.rs` (GPL-3.0-or-later), whose
//! solution follows libadwaita's spring animation; see `NOTICE`. Only the
//! solution is shared: here it is sampled through [`SpringWeights`] rather than
//! fixed to `f64`, so a rectangle springs as happily as a scalar.
//!
//! A spring is evaluated analytically instead of integrated per frame, so an
//! animation is a pure function of time: a stall, a repeated frame, or a late
//! start cannot make two animations of one event drift apart
//! (`COMPOSITOR.md` §9).

/// How long a spring may take to settle before it is cut off (seconds).
///
/// A spring needs damping to settle at all; this keeps a pathological config
/// (no damping at all) from pinning the redraw machine to a frame per vblank
/// forever. M5 validates the config ranges, so it should never be reached.
const MAX_SETTLE_SECS: f64 = 5.0;

/// Parameters of a damped spring with unit mass, in SI-ish units.
///
/// The config surface (M5) mirrors niri's: a damping ratio, a stiffness, and an
/// epsilon, validated to `0.1..=10`, `>= 1`, and `0.00001..=0.1`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringParams {
    /// Spring stiffness `k`, in `N/m`.
    pub stiffness: f64,
    /// Damping coefficient `c`, in `N·s/m`; `c = 2·ζ·√k` for damping ratio `ζ`.
    pub damping: f64,
    /// How small the remaining displacement may get, as a fraction of the
    /// distance the spring was asked to cover, before it counts as settled.
    pub epsilon: f64,
}

impl SpringParams {
    /// A spring that reaches its target quickly without overshooting.
    ///
    /// The numbers niri uses for window movement: critically damped, stiffness
    /// 800, epsilon `0.0001` — about a third of a second.
    pub const DEFAULT: Self = Self {
        stiffness: 800.0,
        // 2·√800 (critically damped); written out because `f64::sqrt` is not `const`.
        damping: 56.568_542_494_923_8,
        epsilon: 0.000_1,
    };

    /// Builds parameters from a damping ratio (`ζ = 1` is critical damping).
    ///
    /// The epsilon is [`SpringParams::DEFAULT`]'s; set it afterwards to taste.
    #[must_use]
    pub fn from_ratio(damping_ratio: f64, stiffness: f64) -> Self {
        let stiffness = stiffness.max(0.0);
        Self {
            stiffness,
            damping: 2.0 * damping_ratio.max(0.0) * stiffness.sqrt(),
            epsilon: Self::DEFAULT.epsilon,
        }
    }

    /// A critically damped spring of the given stiffness (`c = 2·√k`).
    #[must_use]
    pub fn critically_damped(stiffness: f64) -> Self {
        Self::from_ratio(1.0, stiffness)
    }

    /// The spring's damping ratio (`ζ = c / (2·√k)`), or `1.0` when `k = 0`.
    #[must_use]
    pub fn damping_ratio(&self) -> f64 {
        let critical = 2.0 * self.stiffness.max(0.0).sqrt();
        if critical <= f64::EPSILON {
            1.0
        } else {
            self.damping.max(0.0) / critical
        }
    }

    /// How long the spring takes to settle, in seconds.
    ///
    /// That is the time at which the displacement has shrunk to `epsilon` times
    /// the distance covered: `-ln(ε)` divided by the rate the spring decays at.
    /// A critically damped spring carries a `1 + βt` factor, which the fixed
    /// point below folds in, so the animation ends on a sub-pixel remainder
    /// instead of snapping the last pixel. Capped at [`MAX_SETTLE_SECS`].
    #[must_use]
    pub fn settle_secs(&self) -> f64 {
        let beta = (self.damping / 2.0).max(0.0);
        let omega0 = self.stiffness.max(0.0).sqrt();
        let target = -self.epsilon.clamp(f64::MIN_POSITIVE, 1.0).ln();

        if beta < omega0 {
            // Underdamped: the envelope `e^(-βt)` bounds the ring, and the
            // amplitude factor it hides is at most `ω₀/ω₁` — a few percent for
            // any sane damping ratio.
            if beta <= f64::EPSILON {
                return MAX_SETTLE_SECS;
            }
            (target / beta).min(MAX_SETTLE_SECS)
        } else if (beta - omega0).abs() <= f64::from(f32::EPSILON) {
            // Critically damped: `(1 + βt)·e^(-βt)`, refined by
            // `t ← (ln(1 + βt) + target)/β`, which converges in a few steps.
            if beta <= f64::EPSILON {
                return MAX_SETTLE_SECS;
            }
            let mut t = target / beta;
            for _ in 0..8 {
                t = ((1.0 + beta * t).ln() + target) / beta;
            }
            t.min(MAX_SETTLE_SECS)
        } else {
            // Overdamped: the slow real root `λ = β - ω₂` dominates, with a
            // constant `(1 + β/ω₂)/2` in front of it.
            let omega2 = (beta * beta - omega0 * omega0).sqrt();
            let lambda = beta - omega2;
            if lambda <= f64::EPSILON {
                return MAX_SETTLE_SECS;
            }
            let constant = 0.5 * (1.0 + beta / omega2);
            ((constant.ln() + target) / lambda).min(MAX_SETTLE_SECS)
        }
    }

    /// The scalar weights of the solution `t` seconds after it started.
    #[must_use]
    pub fn weights(&self, t: f64) -> SpringWeights {
        let beta = (self.damping / 2.0).max(0.0);
        let omega0 = self.stiffness.max(0.0).sqrt();

        // f64::EPSILON is too coarse a tolerance for this comparison (niri uses
        // f32's); at the boundary the critically damped branch is the limit of
        // both others, so picking it is always right.
        if (beta - omega0).abs() <= f64::from(f32::EPSILON) {
            let envelope = (-beta * t).exp();
            SpringWeights {
                value_from: envelope * (1.0 + beta * t),
                value_velocity: envelope * t,
                speed_from: -beta * beta * t * envelope,
                speed_velocity: envelope * (1.0 - beta * t),
            }
        } else if beta < omega0 {
            let omega1 = (omega0 * omega0 - beta * beta).sqrt();
            let envelope = (-beta * t).exp();
            let (sin, cos) = (omega1 * t).sin_cos();
            SpringWeights {
                value_from: envelope * (cos + beta / omega1 * sin),
                value_velocity: envelope * sin / omega1,
                speed_from: -envelope * omega0 * omega0 / omega1 * sin,
                speed_velocity: envelope * (cos - beta / omega1 * sin),
            }
        } else {
            let omega2 = (beta * beta - omega0 * omega0).sqrt();
            // Written as exponentials of the two real roots rather than with
            // `sinh`/`cosh`, so that a stiff, strongly damped spring cannot
            // overflow the hyperbolic functions before the envelope cancels
            // them: both exponents here are negative.
            let slow = ((omega2 - beta) * t).exp();
            let fast = (-(omega2 + beta) * t).exp();
            let hyperbolic_cos = 0.5 * (slow + fast);
            let hyperbolic_sin = 0.5 * (slow - fast);
            SpringWeights {
                value_from: hyperbolic_cos + beta / omega2 * hyperbolic_sin,
                value_velocity: hyperbolic_sin / omega2,
                speed_from: -omega0 * omega0 / omega2 * hyperbolic_sin,
                speed_velocity: hyperbolic_cos - beta / omega2 * hyperbolic_sin,
            }
        }
    }
}

impl Default for SpringParams {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// The scalar coefficients a spring's value and speed are built from.
///
/// For a spring moving from `from` to `to` with initial velocity `v₀`:
///
/// ```text
/// value(t) = to + (from - to)·value_from + v₀·value_velocity
/// speed(t) =      (from - to)·speed_from + v₀·speed_velocity
/// ```
///
/// Because the solution is linear in those two vectors, the weights are all the
/// engine needs to animate any [`Animatable`](crate::animation::Animatable) value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringWeights {
    /// Coefficient of the initial displacement in the value.
    pub value_from: f64,
    /// Coefficient of the initial velocity in the value.
    pub value_velocity: f64,
    /// Coefficient of the initial displacement in the speed.
    pub speed_from: f64,
    /// Coefficient of the initial velocity in the speed.
    pub speed_velocity: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spring_starts_where_it_was_asked_to() {
        let params = SpringParams::DEFAULT;
        let w = params.weights(0.0);
        assert!((w.value_from - 1.0).abs() < 1e-12, "value starts at `from`");
        assert!(w.value_velocity.abs() < 1e-12, "and moves by the velocity");
        assert!(
            w.speed_from.abs() < 1e-12,
            "no speed without a displacement"
        );
        assert!((w.speed_velocity - 1.0).abs() < 1e-12);
    }

    #[test]
    fn the_envelope_decays_to_epsilon_by_the_settle_time() {
        // value = from·w.value_from for a spring released from rest, so the
        // settle time is where the remaining distance is epsilon of the trip.
        for ratio in [0.5, 0.9, 1.0, 1.1, 2.0] {
            let params = SpringParams::from_ratio(ratio, 800.0);
            let remaining = params.weights(params.settle_secs()).value_from.abs();
            let tolerance = params.epsilon * 4.0; // the overdamped tail overshoots `epsilon` a little
            assert!(
                remaining <= tolerance,
                "ratio {ratio}: {remaining} left after {:?}s",
                params.settle_secs()
            );
        }
    }

    #[test]
    fn critical_damping_settles_faster_than_the_ratios_around_it() {
        let settle = |ratio| SpringParams::from_ratio(ratio, 800.0).settle_secs();
        assert!(settle(1.0) < settle(0.5), "underdamped rings on");
        assert!(settle(1.0) < settle(2.0), "overdamped crawls back");
        assert!(settle(1.0) > 0.0);
    }

    #[test]
    fn a_spring_never_moves_faster_than_it_starts() {
        // A velocity handed over by a gesture decays like everything else.
        let params = SpringParams::from_ratio(1.0, 800.0);
        let start = params.weights(0.0).speed_velocity;
        for step in 1..40 {
            let t = f64::from(step) / 60.0;
            assert!(
                params.weights(t).speed_velocity.abs() <= start,
                "speed grew at t={t}"
            );
        }
    }

    #[test]
    fn a_spring_without_damping_is_cut_off() {
        let params = SpringParams {
            stiffness: 800.0,
            damping: 0.0,
            epsilon: 0.0001,
        };
        assert_eq!(params.settle_secs(), MAX_SETTLE_SECS);
    }

    #[test]
    fn a_ratio_round_trips() {
        let params = SpringParams::from_ratio(0.7, 500.0);
        assert!((params.damping_ratio() - 0.7).abs() < 1e-12);
        assert_eq!(
            SpringParams::critically_damped(800.0),
            SpringParams::from_ratio(1.0, 800.0)
        );
    }
}

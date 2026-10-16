//! An interruptible animated scalar driven by a damped spring.

use crate::curve::SpringParams;

/// A scalar that moves toward a target under spring physics.
///
/// Retargeting is interruptible: [`Animated::set_target`] keeps the current value
/// and velocity, so a new animation blends smoothly out of the old one.
#[derive(Clone, Copy, Debug)]
pub struct Animated {
    value: f64,
    target: f64,
    velocity: f64,
    spring: SpringParams,
}

impl Animated {
    /// Creates an animation settled on `value`.
    #[must_use]
    pub fn new(value: f64) -> Self {
        Self {
            value,
            target: value,
            velocity: 0.0,
            spring: SpringParams::DEFAULT,
        }
    }

    /// Creates an animation settled on `value` with custom spring parameters.
    #[must_use]
    pub fn with_spring(value: f64, spring: SpringParams) -> Self {
        Self {
            value,
            target: value,
            velocity: 0.0,
            spring,
        }
    }

    /// The current value.
    #[must_use]
    pub fn value(&self) -> f64 {
        self.value
    }

    /// The current target.
    #[must_use]
    pub fn target(&self) -> f64 {
        self.target
    }

    /// Retargets without resetting velocity (interruptible).
    pub fn set_target(&mut self, target: f64) {
        self.target = target;
    }

    /// Immediately jumps to `value`, dropping any velocity.
    pub fn snap_to(&mut self, value: f64) {
        self.value = value;
        self.target = value;
        self.velocity = 0.0;
    }

    /// Advances the spring by `dt` seconds and returns the new value.
    ///
    /// `dt` is clamped to 100 ms so a stalled frame cannot blow up the
    /// integrator (semi-implicit Euler).
    pub fn tick(&mut self, dt: f64) -> f64 {
        let dt = dt.clamp(0.0, 0.1);
        let force = -self.spring.stiffness * (self.value - self.target)
            - self.spring.damping * self.velocity;
        self.velocity += force * dt;
        self.value += self.velocity * dt;
        self.value
    }

    /// Whether the animation is still visibly moving.
    #[must_use]
    pub fn is_animating(&self, epsilon: f64) -> bool {
        (self.value - self.target).abs() > epsilon || self.velocity.abs() > epsilon
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settles_on_target() {
        let mut a = Animated::new(0.0);
        a.set_target(100.0);
        for _ in 0..600 {
            a.tick(1.0 / 60.0);
        }
        assert!((a.value() - 100.0).abs() < 0.5, "got {}", a.value());
        assert!(!a.is_animating(0.5));
    }

    #[test]
    fn retarget_keeps_velocity() {
        let mut a = Animated::new(0.0);
        a.set_target(100.0);
        for _ in 0..5 {
            a.tick(1.0 / 60.0);
        }
        let moving = a.value();
        assert!(moving > 0.0, "should have started moving");
        a.set_target(-100.0); // interrupt
        assert_eq!(a.value(), moving, "value is preserved across a retarget");
    }
}

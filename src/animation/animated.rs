//! A value that moves toward a target, sampled from the frame clock.

use super::{Animatable, Clocks, Curve, Kind};

/// A value animated toward a target along a [`Curve`].
///
/// The value is a pure function of the time the [`Clocks`] report, so an
/// animation can be sampled as often as anyone likes — twice in a frame, or not
/// at all for ten frames — without the value, its phase, or its ending time
/// changing. That is what lets one frame clock keep a whole layout in step
/// (`COMPOSITOR.md` §9).
///
/// Retargeting is interruptible: the value and, for a spring, the velocity
/// carry over into the new segment, so a change of mind blends out of the old
/// motion rather than restarting from a standstill.
#[derive(Clone, Copy, Debug)]
pub struct Animated<T: Animatable> {
    /// Where the current segment started.
    from: T,
    /// Where the current segment ends: the resting value.
    to: T,
    /// Velocity carried into the segment, in value units per second.
    velocity: T,
    /// Frame time the segment started at, in seconds since the compositor did.
    start: f64,
    /// The curve the segment follows.
    curve: Curve,
    /// The event that last moved the value.
    kind: Kind,
}

impl<T: Animatable> Animated<T> {
    /// A value at rest on `value`, which future moves will follow `curve` for.
    #[must_use]
    pub fn new(value: T, curve: Curve, kind: Kind) -> Self {
        Self {
            from: value,
            to: value,
            velocity: T::zero(),
            start: f64::NEG_INFINITY,
            curve,
            kind,
        }
    }

    /// The value this animation is heading for.
    #[must_use]
    pub fn target(&self) -> T {
        self.to
    }

    /// The curve future segments follow.
    #[must_use]
    pub fn curve(&self) -> Curve {
        self.curve
    }

    /// Replaces the curve future segments follow (a config reload, M5).
    pub fn set_curve(&mut self, curve: Curve) {
        self.curve = curve;
    }

    /// The kind of event that last moved this value.
    #[must_use]
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// The value now, per the frame clock.
    #[must_use]
    pub fn value(&self, clocks: &Clocks) -> T {
        let elapsed = self.elapsed(clocks);
        if self.finished(elapsed) {
            return self.to;
        }
        if elapsed <= 0.0 {
            // The segment has only just started: its value is its start,
            // exactly, rather than `from + (to - from)·1` rounded.
            return self.from;
        }
        match self.curve {
            Curve::Spring(params) => {
                let weights = params.weights(elapsed);
                self.to
                    .add(self.from.sub(self.to).scale(weights.value_from))
                    .add(self.velocity.scale(weights.value_velocity))
            }
            Curve::Easing { kind, .. } => {
                self.interpolate(kind.eval(elapsed / self.curve.settle_secs()))
            }
            Curve::Bezier { control, .. } => {
                self.interpolate(control.eval(elapsed / self.curve.settle_secs()))
            }
        }
    }

    /// Whether the value is still moving, and so whether another frame is worth
    /// drawing.
    ///
    /// The redraw machine asks this to decide between stopping — idle CPU must
    /// stay near zero (ROADMAP §9) — and requesting one more frame.
    #[must_use]
    pub fn is_animating(&self, clocks: &Clocks) -> bool {
        !self.finished(self.elapsed(clocks))
    }

    /// How fast the value is changing now, in value units per second.
    ///
    /// Only a running spring has velocity to hand on; the time-parameterised
    /// curves restart from the current value and ignore it. A settled value is
    /// at rest, whatever the closed form says.
    #[must_use]
    pub fn velocity(&self, clocks: &Clocks) -> T {
        let elapsed = self.elapsed(clocks);
        match self.curve {
            Curve::Spring(params) if !self.finished(elapsed) => {
                let weights = params.weights(elapsed);
                self.from
                    .sub(self.to)
                    .scale(weights.speed_from)
                    .add(self.velocity.scale(weights.speed_velocity))
            }
            _ => T::zero(),
        }
    }

    /// Retargets the value, interruptibly.
    ///
    /// The current value becomes the new segment's start, and a running spring
    /// hands over its velocity, so an interruption blends out of the old motion
    /// instead of restarting. Asking for the target it is already heading for
    /// changes nothing but the recorded [`Kind`], which is what makes it safe to
    /// call this from every re-layout.
    pub fn animate(&mut self, target: T, kind: Kind, clocks: &Clocks) {
        let from = self.value(clocks);
        let velocity = self.velocity(clocks);
        self.kind = kind;
        if self.to == target {
            return;
        }
        if from == target && velocity == T::zero() {
            self.snap_to(target);
            return;
        }
        self.from = from;
        self.to = target;
        self.velocity = velocity;
        self.start = clocks.now();
    }

    /// Jumps to `value`, dropping any motion.
    ///
    /// This is for movement that must not animate: a window under the pointer
    /// mid-drag has to track it 1:1, and a re-layout that only repeats the
    /// current placement should not restart anything.
    pub fn snap_to(&mut self, value: T) {
        self.from = value;
        self.to = value;
        self.velocity = T::zero();
        self.start = f64::NEG_INFINITY;
    }

    /// `from + (to - from)·progress`.
    fn interpolate(&self, progress: f64) -> T {
        self.from.add(self.to.sub(self.from).scale(progress))
    }

    /// Seconds since the current segment started.
    ///
    /// An animation that completes instantly reports an infinite age instead,
    /// which every `finished` check below reads as "done" — the zero-duration
    /// and reduce-motion fast path.
    fn elapsed(&self, clocks: &Clocks) -> f64 {
        let clock = clocks.clock(self.kind);
        if clock.complete_instantly() {
            return f64::INFINITY;
        }
        (clock.now() - self.start).max(0.0)
    }

    /// Whether the segment has run its course at `elapsed` seconds in.
    fn finished(&self, elapsed: f64) -> bool {
        elapsed >= self.curve.settle_secs()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::{CubicBezier, Easing, SpringParams};
    use crate::core::{Point, Rect};

    /// A clock stopped at `seconds`.
    fn at(seconds: f64) -> Clocks {
        let mut clocks = Clocks::new();
        clocks.advance(seconds);
        clocks
    }

    fn spring() -> Curve {
        Curve::Spring(SpringParams::DEFAULT)
    }

    #[test]
    fn a_new_value_rests_where_it_was_put() {
        let clocks = at(3.0);
        let animated = Animated::new(5.0, spring(), Kind::WindowMove);
        assert_eq!(animated.value(&clocks), 5.0);
        assert!(!animated.is_animating(&clocks), "nothing asked it to move");
        assert_eq!(animated.target(), 5.0);
    }

    #[test]
    fn a_spring_arrives_and_then_stops_asking_for_frames() {
        let animated = {
            let mut a = Animated::new(0.0, spring(), Kind::WindowMove);
            a.animate(100.0, Kind::WindowMove, &at(0.0));
            a
        };
        let settle = SpringParams::DEFAULT.settle_secs();

        let mid = animated.value(&at(settle / 2.0));
        assert!(mid > 0.0 && mid < 100.0, "halfway there, got {mid}");
        assert!(animated.is_animating(&at(settle / 2.0)));

        assert_eq!(
            animated.value(&at(settle)),
            100.0,
            "lands exactly on target"
        );
        assert!(!animated.is_animating(&at(settle)));
        assert!(!animated.is_animating(&at(60.0)));
    }

    #[test]
    fn the_value_only_ever_moves_forward_while_settling() {
        let mut animated = Animated::new(0.0, spring(), Kind::WindowMove);
        animated.animate(100.0, Kind::WindowMove, &at(0.0));
        let mut previous = 0.0;
        for step in 0..=60 {
            let value = animated.value(&at(f64::from(step) / 60.0));
            assert!(value >= previous, "went backwards at step {step}");
            previous = value;
            assert!(value <= 100.0, "overshot at step {step}");
        }
    }

    #[test]
    fn an_underdamped_spring_overshoots_before_resting() {
        let curve = Curve::Spring(SpringParams::from_ratio(0.4, 800.0));
        let mut animated = Animated::new(0.0, curve, Kind::WindowMove);
        animated.animate(100.0, Kind::WindowMove, &at(0.0));
        let peak = (1..=120)
            .map(|step| animated.value(&at(f64::from(step) / 120.0)))
            .fold(f64::NAN, f64::max);
        assert!(
            peak > 100.0,
            "a bouncy spring should overshoot, peaked at {peak}"
        );
        assert_eq!(animated.value(&at(curve.settle_secs())), 100.0);
    }

    #[test]
    fn a_retarget_keeps_the_value_and_the_velocity() {
        let mut animated = Animated::new(0.0, spring(), Kind::WindowMove);
        animated.animate(100.0, Kind::WindowMove, &at(0.0));

        let turnover = at(0.1);
        let value = animated.value(&turnover);
        let velocity = animated.velocity(&turnover);
        assert!(velocity > 0.0, "it should be moving by now");

        animated.animate(-100.0, Kind::WindowMove, &turnover);
        assert_eq!(animated.value(&turnover), value, "no jump on the interrupt");
        assert!(
            (animated.velocity(&turnover) - velocity).abs() < 1e-9,
            "a spring hands its velocity over"
        );
        assert_eq!(animated.target(), -100.0);
    }

    #[test]
    fn asking_for_the_target_again_does_not_restart_it() {
        let mut animated = Animated::new(0.0, spring(), Kind::WindowMove);
        animated.animate(100.0, Kind::WindowMove, &at(0.0));
        let before = animated.value(&at(0.2));
        animated.animate(100.0, Kind::WindowMove, &at(0.2));
        assert_eq!(
            animated.value(&at(0.2)),
            before,
            "re-layout restarted nothing"
        );
        assert_eq!(animated.kind(), Kind::WindowMove);
    }

    #[test]
    fn a_retarget_records_the_new_kind() {
        let mut animated = Animated::new(0.0, spring(), Kind::WindowMove);
        animated.animate(100.0, Kind::WindowResize, &at(0.0));
        assert_eq!(animated.kind(), Kind::WindowResize);
    }

    #[test]
    fn a_retarget_to_where_it_already_is_does_not_animate() {
        let mut animated = Animated::new(10.0, spring(), Kind::WindowMove);
        animated.animate(10.0, Kind::WindowMove, &at(0.0));
        assert!(!animated.is_animating(&at(0.0)));
        assert_eq!(animated.value(&at(0.0)), 10.0);
    }

    #[test]
    fn snap_to_drops_the_motion() {
        let mut animated = Animated::new(0.0, spring(), Kind::WindowMove);
        animated.animate(100.0, Kind::WindowMove, &at(0.0));
        animated.snap_to(42.0);
        assert_eq!(animated.value(&at(0.2)), 42.0);
        assert_eq!(animated.velocity(&at(0.2)), 0.0);
        assert!(!animated.is_animating(&at(0.2)));
        assert_eq!(animated.target(), 42.0);
    }

    #[test]
    fn a_timed_curve_runs_for_its_duration_and_no_longer() {
        let curve = Curve::Easing {
            duration_ms: 250,
            kind: Easing::EaseOutCubic,
        };
        let mut animated = Animated::new(0.0, curve, Kind::LayoutSwitch);
        animated.animate(10.0, Kind::LayoutSwitch, &at(1.0));

        assert_eq!(animated.value(&at(1.0)), 0.0, "it starts where it was");
        let quarter = animated.value(&at(1.0625));
        assert!(quarter > 0.0 && quarter < 10.0, "moving, got {quarter}");
        assert!(animated.is_animating(&at(1.249)));
        assert_eq!(animated.value(&at(1.25)), 10.0, "lands exactly on target");
        assert!(!animated.is_animating(&at(1.25)));
    }

    #[test]
    fn a_bezier_curve_moves_the_same_way() {
        let curve = Curve::Bezier {
            duration_ms: 200,
            control: CubicBezier::new((0.0, 0.0), (0.58, 1.0)),
        };
        let mut animated = Animated::new(0.0, curve, Kind::Scroll);
        animated.animate(100.0, Kind::Scroll, &at(0.0));
        assert!(animated.value(&at(0.1)) > 50.0, "an ease-out starts fast");
        assert_eq!(animated.value(&at(0.2)), 100.0);
    }

    #[test]
    fn an_instant_curve_jumps() {
        let mut animated = Animated::new(0.0, Curve::INSTANT, Kind::WindowMove);
        animated.animate(100.0, Kind::WindowMove, &at(0.0));
        assert_eq!(animated.value(&at(0.0)), 100.0);
        assert!(!animated.is_animating(&at(0.0)));
    }

    #[test]
    fn reduce_motion_jumps_one_kind_at_a_time() {
        let mut clocks = Clocks::new();
        clocks.set_complete_instantly(Kind::WindowMove, true);

        let mut animated = Animated::new(0.0, spring(), Kind::WindowMove);
        animated.animate(100.0, Kind::WindowMove, &clocks);
        assert_eq!(animated.value(&clocks), 100.0);
        assert!(!animated.is_animating(&clocks));

        let mut resizing = Animated::new(0.0, spring(), Kind::WindowResize);
        resizing.animate(100.0, Kind::WindowResize, &clocks);
        assert_eq!(resizing.value(&clocks), 0.0, "another kind still animates");
        assert!(resizing.is_animating(&clocks));
    }

    #[test]
    fn sampling_never_changes_the_phase() {
        // Two animations started in the same frame agree in every frame, no
        // matter how often either is read: one clock, no per-value drift.
        let mut watched = Animated::new(0.0, spring(), Kind::WindowMove);
        let mut ignored = watched;
        let start = at(0.0);
        watched.animate(100.0, Kind::WindowMove, &start);
        ignored.animate(100.0, Kind::WindowMove, &start);

        for step in 0..=30 {
            let clocks = at(f64::from(step) / 60.0);
            let heard = watched.value(&clocks);
            let _ = ignored.value(&clocks);
            assert_eq!(watched.value(&clocks), heard, "reading a value moved it");
            assert_eq!(ignored.value(&clocks), heard, "they drifted apart");
        }
    }

    #[test]
    fn compound_values_move_component_by_component() {
        let from = Rect::new(0.0, 0.0, 100.0, 200.0);
        let to = Rect::new(50.0, -30.0, 400.0, 500.0);
        let mut animated = Animated::new(from, spring(), Kind::WindowMove);
        animated.animate(to, Kind::WindowMove, &at(0.0));

        let mid = animated.value(&at(SpringParams::DEFAULT.settle_secs() / 2.0));
        assert!(mid.x > from.x && mid.x < to.x, "x moved, got {}", mid.x);
        assert!(
            mid.y < from.y && mid.y > to.y,
            "y moved the other way, got {}",
            mid.y
        );
        assert!(mid.w > from.w && mid.w < to.w, "w grew, got {}", mid.w);
        assert!(mid.h > from.h && mid.h < to.h, "h grew, got {}", mid.h);
        assert_eq!(
            animated.value(&at(1.0)),
            to,
            "settles on the whole rectangle"
        );

        let mut point = Animated::new(Point::new(0.0, 0.0), spring(), Kind::Scroll);
        point.animate(Point::new(-80.0, 0.0), Kind::Scroll, &at(0.0));
        let mid = point.value(&at(0.05));
        assert!(mid.x < 0.0 && mid.x > -80.0, "got {mid:?}");
        assert_eq!(mid.y, 0.0);
    }
}

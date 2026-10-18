//! The frame clock every animation is sampled against (`COMPOSITOR.md` §9).
//!
//! Nothing here reads a clock of its own. The compositor re-times [`Clocks`]
//! once per frame from the backend's presentation time — the loop's own clock
//! under the nested backend today, the vertical blank (or `wp_presentation`)
//! once the bare-metal backend lands — and every animated value is sampled
//! against what it reports. Two values moved by one event therefore see exactly
//! the same time, and no amount of frames missed or repeated can drift them
//! apart.

/// The kinds of animation, each with its own synced clock.
///
/// The kind is the *event* that last moved a value, which is the granularity
/// per-event config (M5) and the reduce-motion switch work at. All kinds share
/// one time; only their reduce-motion flags differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A window appearing.
    WindowOpen,
    /// A window going away.
    WindowClose,
    /// A window moving.
    WindowMove,
    /// A window changing size.
    WindowResize,
    /// The visible space set changing.
    SpaceSwitch,
    /// The layout of the current space changing.
    LayoutSwitch,
    /// A scrolling layout moving its strip.
    Scroll,
    /// The focus ring blending between focused and unfocused.
    Border,
}

impl Kind {
    /// How many kinds there are.
    pub const COUNT: usize = 8;

    /// This kind's slot in [`Clocks`].
    fn index(self) -> usize {
        self as usize
    }
}

/// One frame of time, as a [`Kind`]'s animations see it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clock {
    /// Seconds since the compositor started.
    now: f64,
    /// Whether this kind jumps straight to its targets.
    complete_instantly: bool,
}

impl Clock {
    /// The frame time, in seconds since the compositor started.
    #[must_use]
    pub fn now(self) -> f64 {
        self.now
    }

    /// Whether this kind's values jump straight to their targets.
    #[must_use]
    pub fn complete_instantly(self) -> bool {
        self.complete_instantly
    }
}

/// The compositor's frame clock: one clock per [`Kind`], all re-timed from the
/// same frame.
///
/// Re-timing happens once per frame, in [`Clocks::advance`], so every value read
/// during that frame — a window's rectangle, the scroll offset, the border
/// colour — is sampled at one instant.
#[derive(Clone, Copy, Debug)]
pub struct Clocks {
    now: f64,
    complete_instantly: [bool; Kind::COUNT],
}

impl Clocks {
    /// A clock stopped at zero.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            now: 0.0,
            complete_instantly: [false; Kind::COUNT],
        }
    }

    /// Re-times the clock to a frame, in seconds since the compositor started.
    ///
    /// A time that does not move forward is ignored: a backend that repeats a
    /// timestamp (a double frame, or a clock that restarted) must not rewind
    /// animations into their past.
    pub fn advance(&mut self, now: f64) {
        if now > self.now {
            self.now = now;
        }
    }

    /// The frame time, in seconds since the compositor started.
    #[must_use]
    pub fn now(self) -> f64 {
        self.now
    }

    /// The frame timestamp clients get, in milliseconds.
    #[must_use]
    pub fn now_ms(self) -> u32 {
        (self.now * 1000.0) as u32
    }

    /// The clock for one kind of animation.
    #[must_use]
    pub fn clock(self, kind: Kind) -> Clock {
        Clock {
            now: self.now,
            complete_instantly: self.complete_instantly[kind.index()],
        }
    }

    /// Makes one kind jump straight to its targets (`reduce-motion` per event).
    pub fn set_complete_instantly(&mut self, kind: Kind, instant: bool) {
        self.complete_instantly[kind.index()] = instant;
    }

    /// Whether one kind jumps straight to its targets.
    #[must_use]
    pub fn complete_instantly(&self, kind: Kind) -> bool {
        self.complete_instantly[kind.index()]
    }

    /// Turns reduce-motion on or off for every kind at once.
    ///
    /// This is the single switch of `COMPOSITOR.md` §9; M5 wires it to config
    /// and to an IPC toggle.
    pub fn set_reduce_motion(&mut self, reduce_motion: bool) {
        self.complete_instantly = [reduce_motion; Kind::COUNT];
    }
}

impl Default for Clocks {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_reads_the_same_time() {
        let mut clocks = Clocks::new();
        clocks.advance(1.5);
        for kind in [
            Kind::WindowOpen,
            Kind::WindowClose,
            Kind::WindowMove,
            Kind::WindowResize,
            Kind::SpaceSwitch,
            Kind::LayoutSwitch,
            Kind::Scroll,
            Kind::Border,
        ] {
            assert_eq!(clocks.clock(kind).now(), 1.5, "{kind:?} drifted");
        }
        assert_eq!(clocks.now_ms(), 1500);
    }

    #[test]
    fn time_only_moves_forward() {
        let mut clocks = Clocks::new();
        clocks.advance(1.0);
        clocks.advance(0.5);
        assert_eq!(clocks.now(), 1.0, "a rewind must be ignored");
        clocks.advance(1.0);
        assert_eq!(clocks.now(), 1.0, "a repeated frame is not a rewind");
        clocks.advance(2.0);
        assert_eq!(clocks.now(), 2.0);
    }

    #[test]
    fn reduce_motion_is_per_kind_and_can_be_global() {
        let mut clocks = Clocks::new();
        assert!(!clocks.clock(Kind::WindowMove).complete_instantly());

        clocks.set_complete_instantly(Kind::WindowMove, true);
        assert!(clocks.complete_instantly(Kind::WindowMove));
        assert!(
            !clocks.complete_instantly(Kind::WindowResize),
            "one kind does not affect another"
        );

        clocks.set_reduce_motion(true);
        for kind in [Kind::WindowMove, Kind::Scroll, Kind::Border] {
            assert!(clocks.complete_instantly(kind), "{kind:?} left behind");
        }
        clocks.set_reduce_motion(false);
        assert!(!clocks.complete_instantly(Kind::WindowMove));
    }
}

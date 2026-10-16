//! Logical-coordinate geometry.
//!
//! crawm lays out in **logical** coordinates (`f64`) and converts to physical
//! pixels at a single rounding site in the renderer (ROADMAP §3E). Keeping this
//! crate free of any integer/scale types makes that rule hard to violate.

/// A point in logical coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    /// Horizontal offset.
    pub x: f64,
    /// Vertical offset.
    pub y: f64,
}

impl Point {
    /// Creates a point.
    #[must_use]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// A size in logical coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
}

impl Size {
    /// Creates a size.
    #[must_use]
    pub const fn new(w: f64, h: f64) -> Self {
        Self { w, h }
    }
}

/// An axis-aligned rectangle in logical coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
}

impl Rect {
    /// Creates a rectangle from its origin and size.
    #[must_use]
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    /// Creates a rectangle at the origin with the given size.
    #[must_use]
    pub const fn from_size(size: Size) -> Self {
        Self::new(0.0, 0.0, size.w, size.h)
    }

    /// The right edge (`x + w`).
    #[must_use]
    pub fn right(&self) -> f64 {
        self.x + self.w
    }

    /// The bottom edge (`y + h`).
    #[must_use]
    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }

    /// Returns a copy shrunk by `gap` on every side, clamped to zero size.
    #[must_use]
    pub fn deflated(&self, gap: f64) -> Self {
        let w = (self.w - 2.0 * gap).max(0.0);
        let h = (self.h - 2.0 * gap).max(0.0);
        Self::new(self.x + gap, self.y + gap, w, h)
    }

    /// Whether `point` lies inside the rectangle (inclusive of the top/left edge).
    #[must_use]
    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.x && point.x < self.right() && point.y >= self.y && point.y < self.bottom()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deflate_clamps_to_zero() {
        let r = Rect::new(0.0, 0.0, 10.0, 4.0).deflated(8.0);
        assert_eq!(r, Rect::new(8.0, 8.0, 0.0, 0.0));
    }

    #[test]
    fn contains_is_half_open() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(r.contains(Point::new(0.0, 0.0)));
        assert!(!r.contains(Point::new(10.0, 5.0)));
    }
}

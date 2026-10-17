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

    /// Creates a rectangle from a top-left corner and a size.
    #[must_use]
    pub const fn from_loc_and_size(location: Point, size: Size) -> Self {
        Self {
            x: location.x,
            y: location.y,
            w: size.w,
            h: size.h,
        }
    }

    /// Returns a copy moved by (`dx`, `dy`).
    #[must_use]
    pub const fn translated(&self, dx: f64, dy: f64) -> Self {
        Self::new(self.x + dx, self.y + dy, self.w, self.h)
    }

    /// The center point.
    #[must_use]
    pub fn center(&self) -> Point {
        Point::new(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    /// Whether `other` lies entirely inside this rectangle.
    #[must_use]
    pub fn contains_rect(&self, other: &Rect) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && other.right() <= self.right()
            && other.bottom() <= self.bottom()
    }

    /// The overlapping rectangle, or `None` when the two do not overlap.
    #[must_use]
    pub fn intersection(&self, other: &Rect) -> Option<Rect> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        (right > x && bottom > y).then(|| Self::new(x, y, right - x, bottom - y))
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

    #[test]
    fn center_and_translation() {
        let r = Rect::new(0.0, 0.0, 10.0, 20.0);
        assert_eq!(r.center(), Point::new(5.0, 10.0));
        assert_eq!(r.translated(2.0, 3.0), Rect::new(2.0, 3.0, 10.0, 20.0));
    }

    #[test]
    fn intersection_and_containment() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(5.0, 5.0, 10.0, 10.0);
        assert_eq!(a.intersection(&b), Some(Rect::new(5.0, 5.0, 5.0, 5.0)));
        assert_eq!(a.intersection(&Rect::new(20.0, 20.0, 1.0, 1.0)), None);
        assert!(a.contains_rect(&Rect::new(1.0, 1.0, 9.0, 9.0)));
        assert!(!a.contains_rect(&Rect::new(1.0, 1.0, 10.0, 10.0)));
    }
}

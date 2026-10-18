//! The arithmetic the engine needs from the values it animates.

use crate::core::{Point, Rect, Size};

/// A value [`Animated`](crate::animation::Animated) can move.
///
/// The engine treats a value as a vector space: a spring's closed-form solution
/// is linear in its initial displacement and velocity, so addition, subtraction,
/// and scaling by a scalar are all that is needed — the same maths moves a
/// scalar, a point, a size, or a whole rectangle.
pub trait Animatable: Copy + PartialEq {
    /// The additive identity: a value that does not move.
    #[must_use]
    fn zero() -> Self;

    /// The component-wise sum.
    #[must_use]
    fn add(self, other: Self) -> Self;

    /// The component-wise difference.
    #[must_use]
    fn sub(self, other: Self) -> Self;

    /// Every component scaled by `factor`.
    #[must_use]
    fn scale(self, factor: f64) -> Self;
}

impl Animatable for f64 {
    fn zero() -> Self {
        0.0
    }

    fn add(self, other: Self) -> Self {
        self + other
    }

    fn sub(self, other: Self) -> Self {
        self - other
    }

    fn scale(self, factor: f64) -> Self {
        self * factor
    }
}

impl Animatable for Point {
    fn zero() -> Self {
        Self::new(0.0, 0.0)
    }

    fn add(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y)
    }

    fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y)
    }

    fn scale(self, factor: f64) -> Self {
        Self::new(self.x * factor, self.y * factor)
    }
}

impl Animatable for Size {
    fn zero() -> Self {
        Self::new(0.0, 0.0)
    }

    fn add(self, other: Self) -> Self {
        Self::new(self.w + other.w, self.h + other.h)
    }

    fn sub(self, other: Self) -> Self {
        Self::new(self.w - other.w, self.h - other.h)
    }

    fn scale(self, factor: f64) -> Self {
        Self::new(self.w * factor, self.h * factor)
    }
}

impl Animatable for Rect {
    fn zero() -> Self {
        Self::new(0.0, 0.0, 0.0, 0.0)
    }

    fn add(self, other: Self) -> Self {
        Self::new(
            self.x + other.x,
            self.y + other.y,
            self.w + other.w,
            self.h + other.h,
        )
    }

    fn sub(self, other: Self) -> Self {
        Self::new(
            self.x - other.x,
            self.y - other.y,
            self.w - other.w,
            self.h - other.h,
        )
    }

    fn scale(self, factor: f64) -> Self {
        Self::new(
            self.x * factor,
            self.y * factor,
            self.w * factor,
            self.h * factor,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_arithmetic_is_component_wise() {
        let a = Rect::new(1.0, 2.0, 3.0, 4.0);
        let b = Rect::new(10.0, 20.0, 30.0, 40.0);
        assert_eq!(a.add(b), Rect::new(11.0, 22.0, 33.0, 44.0));
        assert_eq!(b.sub(a), Rect::new(9.0, 18.0, 27.0, 36.0));
        assert_eq!(a.scale(2.0), Rect::new(2.0, 4.0, 6.0, 8.0));
        assert_eq!(a.sub(a), Rect::zero());
    }

    #[test]
    fn points_and_sizes_add_up() {
        assert_eq!(
            Point::new(1.0, 2.0).add(Point::zero()),
            Point::new(1.0, 2.0)
        );
        assert_eq!(Size::new(3.0, 4.0).scale(0.5), Size::new(1.5, 2.0));
        assert_eq!(f64::zero().add(2.5).sub(0.5), 2.0);
    }
}

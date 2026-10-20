//! How a window's committed surface maps onto the rectangle it is drawn at.
//!
//! The two are the same size only while the client is the size the layout asked
//! it to be: during a resize the rectangle belongs to the animation, a client
//! takes a few frames to answer a configure, and some clients never do.
//! [`WindowFit`] is the single description of what happens then — the pixels
//! stretched over the rectangle, cut back to it, or left exactly as they are —
//! and it has two readers. The renderer maps every surface element *forward*
//! through it ([`super::render`]), and pointer input maps positions *backward*
//! through it ([`super::state`]), so a click lands on the pixels the user saw.
//! Stating both here is what keeps them from drifting apart (TODO.md §3.5).

use smithay::utils::{Buffer as BufferCoords, Logical, Physical, Point, Rectangle, Size};

/// Where one window is being drawn this frame, and how big its client has
/// committed the window geometry to be.
#[derive(Clone, Copy, Debug)]
pub(crate) struct WindowFit {
    /// The rectangle the window is drawn at: its animated placement.
    drawn: Rectangle<i32, Logical>,
    /// Where the toplevel's `wl_surface` origin sits in that rectangle's
    /// space: the rectangle's origin pulled back by the client's own
    /// decoration inset, which is where the surface tree is placed before the
    /// fit moves it.
    surface_origin: Point<i32, Logical>,
    /// The size the client last committed for its window geometry, which is
    /// what its buffer has to say for itself.
    committed: Size<i32, Logical>,
    /// Whether a rectangle bigger than [`Self::committed`] may be filled by
    /// growing the pixels the client drew.
    ///
    /// It may only while the client is being asked for exactly this rectangle:
    /// then the growth is the animation catching up and it is gone within the
    /// resize. A window that refused the size is cropped instead, because
    /// stretched pixels are pixels the client is not drawing — it would be
    /// answering clicks in places it never painted.
    may_stretch: bool,
}

impl WindowFit {
    /// The fit for a window drawn at `drawn`, with its surface placed at
    /// `surface_origin`, its client having committed `committed`.
    pub(crate) fn new(
        drawn: Rectangle<i32, Logical>,
        surface_origin: Point<i32, Logical>,
        committed: Size<i32, Logical>,
        may_stretch: bool,
    ) -> Self {
        Self {
            drawn,
            surface_origin,
            committed,
            may_stretch,
        }
    }

    /// Where the window's `wl_surface` origin is placed when nothing is
    /// scaled, which is where the renderer puts the surface tree.
    pub(crate) fn surface_origin(&self) -> Point<i32, Logical> {
        self.surface_origin
    }

    /// The origin to report to the seat for this window, so that
    /// `position - origin` gives the surface-local point *drawn* at `position`.
    ///
    /// With nothing stretched, that is simply where the surface is placed. It
    /// is not while the rectangle is bigger than what the client committed:
    /// then a drawn position's distance from the rectangle's origin is `scale`
    /// times the surface's, and the origin has to sit further along by the
    /// difference for the subtraction to land on the surface point behind it.
    /// Without this, a click halfway down a window being stretched to twice
    /// its height would be handed to the client a whole window too far down
    /// (TODO.md §3.5).
    pub(crate) fn input_origin(&self, position: Point<f64, Logical>) -> Point<f64, Logical> {
        let delta = position - self.drawn.loc.to_f64();
        let (x, y) = (self.x(), self.y());
        self.surface_origin.to_f64()
            + Point::from((x.origin_shift(delta.x), y.origin_shift(delta.y)))
    }

    /// Maps one element of the window's surface tree onto the rectangle.
    ///
    /// `natural` is where smithay would have drawn the element — its position
    /// in the tree, with the window geometry landing on the rectangle — and
    /// `src` the part of the buffer it samples. Nothing of the element inside
    /// the rectangle yields nothing: an element wholly outside it is not
    /// drawn.
    pub(crate) fn map_element(
        &self,
        natural: Rectangle<i32, Physical>,
        src: Rectangle<f64, BufferCoords>,
    ) -> Option<(Rectangle<f64, BufferCoords>, Rectangle<i32, Physical>)> {
        // The fit is stated in logical coordinates, and the renderer draws at
        // scale 1 — M4 owns the conversion site — so those are also the pixels
        // the elements live in.
        let (x, y) = (self.x(), self.y());

        let ((dx, dw), (cut_x_lo, cut_x_hi)) = x.cut(
            x.map(f64::from(natural.loc.x)),
            f64::from(natural.size.w) * x.scale,
        )?;
        let ((dy, dh), (cut_y_lo, cut_y_hi)) = y.cut(
            y.map(f64::from(natural.loc.y)),
            f64::from(natural.size.h) * y.scale,
        )?;

        // How much of the buffer one drawn pixel of this element covers, which
        // is its buffer scale for an ordinary surface. Pixels cut from the
        // drawing take their share of the buffer with them; a stretched axis
        // cuts none, so the whole of it is still sampled.
        let (sw, sh) = (
            src.size.w / f64::from(natural.size.w).max(1.0),
            src.size.h / f64::from(natural.size.h).max(1.0),
        );
        let src = Rectangle::new(
            Point::from((src.loc.x + cut_x_lo * sw, src.loc.y + cut_y_lo * sh)),
            Size::from((
                src.size.w - (cut_x_lo + cut_x_hi) * sw,
                src.size.h - (cut_y_lo + cut_y_hi) * sh,
            )),
        );
        let dst = Rectangle::new(
            Point::from((dx, dy)).to_i32_round(),
            Size::from((dw, dh)).to_i32_round(),
        );
        Some((src, dst))
    }

    /// The horizontal axis of this fit.
    fn x(&self) -> Axis {
        Axis::fit(
            (f64::from(self.drawn.loc.x), f64::from(self.drawn.size.w)),
            f64::from(self.committed.w),
            self.may_stretch,
        )
    }

    /// The vertical axis of this fit.
    fn y(&self) -> Axis {
        Axis::fit(
            (f64::from(self.drawn.loc.y), f64::from(self.drawn.size.h)),
            f64::from(self.committed.h),
            self.may_stretch,
        )
    }
}

/// One axis of a window's fit: how a coordinate on the client's surface moves
/// when it is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Axis {
    /// What is added to a surface coordinate times [`Self::scale`].
    offset: f64,
    /// Multiplier from surface pixels to drawn pixels.
    scale: f64,
    /// The extent the surface is cut to on this axis, present only when the
    /// window is cropped rather than stretched.
    cut: Option<(f64, f64)>,
}

impl Axis {
    /// Fits one axis of a window whose rectangle spans `origin` for `size`
    /// pixels and whose client committed `committed` of them.
    ///
    /// A rectangle the client has not filled yet is filled by growing what it
    /// drew, anchored on the rectangle's origin so the window's top-left
    /// corner stays put: the pixels that spill past the far edge are the
    /// client's own decoration shadows, which are meant to spill. A rectangle
    /// the client has overfilled is cut back, pixel for pixel, so nothing
    /// spills into whatever sits beside it.
    ///
    /// A window at rest is neither: it is drawn exactly as the client drew it,
    /// which is what keeps a client's decoration shadows around its edges.
    fn fit((origin, size): (f64, f64), committed: f64, may_stretch: bool) -> Self {
        if committed > 0.0 && size > committed && may_stretch {
            let scale = size / committed;
            // The map is `origin + (p - origin) * scale`, which over surface
            // coordinates is the affine `offset + p * scale`.
            return Axis {
                offset: origin - origin * scale,
                scale,
                cut: None,
            };
        }
        let cut = (committed > size).then_some((origin, origin + size));
        Axis {
            offset: 0.0,
            scale: 1.0,
            cut,
        }
    }

    /// Where a coordinate on the client's surface lands on this axis.
    fn map(&self, value: f64) -> f64 {
        self.offset + value * self.scale
    }

    /// How much further out a drawn position sits from the rectangle's origin
    /// than the surface position behind it does — the length the stretch added
    /// — which is what input has to add to the surface's placed origin.
    ///
    /// An axis that is not stretched scaled nothing, so this is zero there and
    /// a window at rest keeps the coordinates it is drawn with.
    fn origin_shift(&self, distance: f64) -> f64 {
        distance - distance / self.scale
    }

    /// Cuts the drawn extent `[at, at + size)` to the part the window's
    /// rectangle covers: what is left, and how much was taken from each end.
    ///
    /// An axis that is not cut keeps the whole extent, decorations and all; an
    /// element lying wholly past the rectangle's edges is cut to nothing.
    fn cut(&self, at: f64, size: f64) -> Option<((f64, f64), (f64, f64))> {
        let Some((floor, ceiling)) = self.cut else {
            return Some(((at, size), (0.0, 0.0)));
        };
        let (lo, hi) = (at, at + size);
        let kept_lo = lo.max(floor);
        let kept_hi = hi.min(ceiling);
        (kept_hi > kept_lo).then_some(((kept_lo, kept_hi - kept_lo), (kept_lo - lo, hi - kept_hi)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A window drawn at `drawn`, its client having committed `committed`,
    /// with no decoration inset: the surface origin sits on the rectangle's.
    fn fit(drawn: (i32, i32, i32, i32), committed: (i32, i32), may_stretch: bool) -> WindowFit {
        WindowFit::new(
            Rectangle::new((drawn.0, drawn.1).into(), (drawn.2, drawn.3).into()),
            (drawn.0, drawn.1).into(),
            (committed.0, committed.1).into(),
            may_stretch,
        )
    }

    /// The same window as a client-side-decorated one: its surface origin is
    /// `inset` pixels before the rectangle's origin, the margin the client
    /// drew around its geometry.
    fn inset_fit(
        drawn: (i32, i32, i32, i32),
        committed: (i32, i32),
        may_stretch: bool,
        inset: (i32, i32),
    ) -> WindowFit {
        WindowFit::new(
            Rectangle::new((drawn.0, drawn.1).into(), (drawn.2, drawn.3).into()),
            (drawn.0 - inset.0, drawn.1 - inset.1).into(),
            (committed.0, committed.1).into(),
            may_stretch,
        )
    }

    /// One element of a surface tree at `loc`, `size` big, sampling all of a
    /// `buffer`-sized buffer.
    fn element(
        loc: (i32, i32),
        size: (i32, i32),
        buffer: (f64, f64),
    ) -> (Rectangle<i32, Physical>, Rectangle<f64, BufferCoords>) {
        (
            Rectangle::new((loc.0, loc.1).into(), (size.0, size.1).into()),
            Rectangle::new((0.0, 0.0).into(), buffer.into()),
        )
    }

    #[test]
    fn a_window_at_rest_is_drawn_exactly_as_the_client_drew_it() {
        // A client-side-decorated window: its buffer is the geometry with a
        // 25 pixel shadow margin on every side, and at rest none of that may
        // move or shrink, shadows included.
        let (natural, src) = element((75, 175), (519, 1080), (519.0, 1080.0));
        let (src, dst) = inset_fit((100, 200, 469, 1030), (469, 1030), true, (25, 25))
            .map_element(natural, src)
            .expect("the whole window is beside its rectangle");
        assert_eq!(dst, natural);
        assert_eq!(src, element((0, 0), (0, 0), (519.0, 1080.0)).1);
    }

    #[test]
    fn a_rectangle_the_client_has_not_filled_is_stretched_over() {
        // The rectangle has grown to twice what the client committed: the
        // buffer covers all of it, sampled from all of itself.
        let (natural, src) = element((100, 200), (469, 1030), (469.0, 1030.0));
        let (src, dst) = fit((100, 200, 940, 1030), (469, 1030), true)
            .map_element(natural, src)
            .expect("the window is inside its rectangle");
        assert_eq!(dst, Rectangle::new((100, 200).into(), (940, 1030).into()));
        assert_eq!(src, element((0, 0), (0, 0), (469.0, 1030.0)).1);
    }

    #[test]
    fn a_growing_rectangle_keeps_the_windows_top_left_corner() {
        // Stretching is anchored on the origin, so the geometry's top-left
        // corner stays exactly where it was. Everything else scales with it,
        // decoration shadows included: the margin above and left is still
        // above and left, just twice as wide.
        let (natural, src) = element((75, 175), (519, 1080), (519.0, 1080.0));
        let (_, dst) = inset_fit((100, 200, 940, 1030), (469, 1030), true, (25, 25))
            .map_element(natural, src)
            .expect("the window is beside its rectangle");
        // The geometry's origin lands on the rectangle's origin, and the whole
        // element scales by 940/469 horizontally and 1:1 vertically.
        assert_eq!(dst.loc.x, 50);
        assert_eq!(dst.loc.y, 175);
        assert_eq!(dst.size.w, 1040);
        assert_eq!(dst.size.h, 1080);
    }

    #[test]
    fn a_rectangle_the_client_overfills_is_cut_back_to_it() {
        // The client committed 940 and the rectangle is 469 wide: nothing may
        // be drawn past the rectangle's right edge, and the buffer is sampled
        // from the part of itself that would have been there.
        let (natural, src) = element((100, 200), (940, 1030), (940.0, 1030.0));
        let (src, dst) = fit((100, 200, 469, 1030), (940, 1030), true)
            .map_element(natural, src)
            .expect("the window's start is inside its rectangle");
        assert_eq!(dst, Rectangle::new((100, 200).into(), (469, 1030).into()));
        assert_eq!(src, element((0, 0), (0, 0), (469.0, 1030.0)).1);
    }

    #[test]
    fn cutting_takes_the_pixels_behind_it_with_it() {
        // The same window with its shadow margins: the cut lands on the
        // rectangle's edges, so what survives horizontally is the geometry and
        // the margin before it — and the buffer is sampled from that, not from
        // its start. The other axis is left alone, since there the client
        // filled exactly what was asked, shadows included.
        let (natural, src) = element((75, 175), (990, 1080), (990.0, 1080.0));
        let (src, dst) = inset_fit((100, 200, 469, 1030), (940, 1030), true, (25, 25))
            .map_element(natural, src)
            .expect("the window's start is inside its rectangle");
        assert_eq!(dst, Rectangle::new((100, 175).into(), (469, 1080).into()));
        assert_eq!(src.loc.x, 25.0);
        assert_eq!(src.size.w, 469.0);
        assert_eq!(src.loc.y, 0.0);
        assert_eq!(src.size.h, 1080.0);
    }

    #[test]
    fn a_client_that_refused_the_size_is_not_stretched() {
        // It committed 469 while its rectangle is 940 wide and is not going to
        // grow: its pixels stay its own size, and the empty part of the
        // rectangle is the honest picture of a client that said no. Stretched
        // pixels would put the pointer where nothing was drawn.
        let (natural, src) = element((100, 200), (469, 1030), (469.0, 1030.0));
        let (src, dst) = fit((100, 200, 940, 1030), (469, 1030), false)
            .map_element(natural, src)
            .expect("the window is inside its rectangle");
        assert_eq!(dst, natural);
        assert_eq!(src, element((0, 0), (0, 0), (469.0, 1030.0)).1);
    }

    #[test]
    fn an_element_beyond_the_rectangle_is_not_drawn() {
        // A subsurface sitting past the right edge of a window that overfills
        // its rectangle has nothing left of it inside.
        let (natural, src) = element((700, 300), (100, 100), (100.0, 100.0));
        assert!(
            fit((100, 200, 469, 1030), (940, 1030), true)
                .map_element(natural, src)
                .is_none()
        );
    }

    #[test]
    fn input_at_rest_takes_the_surface_where_it_is() {
        // Nothing is scaled, so every position maps straight through: the
        // origin the seat is given is the one the surface is drawn at.
        let fit = inset_fit((100, 200, 469, 1030), (469, 1030), true, (25, 25));
        let origin = fit.input_origin((300.0, 500.0).into());
        assert_eq!(origin, (75.0, 175.0).into());
    }

    #[test]
    fn a_click_lands_on_the_pixels_the_user_saw() {
        // The rectangle is exactly twice what the client committed, so a
        // position 470 pixels in from the rectangle's origin — the client's
        // whole committed width — draws surface pixels 235 in, plus the 25
        // pixel decoration inset: 260, and that is what the client has to be
        // told. The unscaled subtraction would have said 494, a window's width
        // further down than anything was drawn.
        let fit = inset_fit((100, 200, 940, 1030), (470, 1030), true, (25, 25));
        let position = Point::from((570.0, 200.0));
        let origin = fit.input_origin(position);
        assert_eq!(position - origin, (260.0, 25.0).into());
        // Vertically nothing was scaled, so that axis is untouched.
        assert_eq!(origin.y, 175.0);
    }

    #[test]
    fn input_through_a_refused_window_is_untouched() {
        // The window is cropped rather than stretched, so its pixels are where
        // the client drew them and so are the coordinates handed to it.
        let fit = inset_fit((100, 200, 940, 1030), (469, 1030), false, (25, 25));
        let origin = fit.input_origin((569.0, 200.0).into());
        assert_eq!(origin, (75.0, 175.0).into());
    }
}

//! The paint side of ADR 0031 §1 step 4: where on the 150 dpi bitmap the region is filled black.
//!
//! # What this decides, and what it leaves to the engine
//!
//! The region arrives in **display points from the top-left** (ADR 0029's frame amendment). The
//! paint converts it to user space with burrow's one region conversion
//! ([`Region::to_content_space`]), and **PDFium's own page-to-device transform** maps the four
//! corners to pixels ([`PageToDevice`], `FPDF_PageToDevice`, with the render's start, size and
//! `rotate = 0`). That transform returns rounded integers, so the rectangle is widened by
//! [`PAINT_MARGIN`]: **one pixel on each side for that rounding, and one more for §2 rule 8**.
//! Every pixel in the result is then filled opaque black, in PDFium's heap, by the caller.
//!
//! This is one of **two sources** at run time (§1 step 6). The other is `cross_check`, which
//! maps the same region by burrow's own display-space arithmetic, through neither the
//! conversion nor the transform, and holds every pixel of *its* rectangle to be the paint.
//! Nothing here is trusted by that check; it is written so that it can be wrong and be caught.
//!
//! # Rule 8: the edge is a pixel edge
//!
//! Every pixel the region touches is painted, and one more on each side, because hinting and
//! hairlines can put ink up to about a pixel beyond where the arithmetic places it. What lies
//! outside the painted rectangle is **not detectable at run time**: a glyph, a line, an image or
//! a signature crossing the edge stays visible outside it.
//!
//! # Not reachable yet
//!
//! Nothing outside this crate's tests calls this module until ADR 0031's integration slice wires
//! it to the render, the cross-check and the write (owner decision 90, item 3).

#![allow(
    dead_code,
    reason = "ADR 0031 S9a: no caller outside tests until the integration slice wires the render"
)]

use burrow_types::{Error, Limits, Result, Stage};

use super::MAX_FLATTEN_PIXELS;
use crate::pdfsyntax::geometry::Rect;
use crate::pdfsyntax::region::{PageFrame, Region};

/// How far the paint is widened past the corners the transform returns, in pixels, on every side:
/// one for `FPDF_PageToDevice`'s rounding to whole pixels, one for §2 rule 8.
///
/// **Why the rounding pixel is needed.** A corner the transform places at *x* is returned as the
/// nearest whole pixel, so the region's own edge pixel can lie up to half a pixel beyond it -- and
/// burrow's check (which floors and ceils the exact value outward, then adds rule 8's pixel) holds
/// that pixel to be paint. With one pixel for rounding the paint's edge is never inside the check's.
pub const PAINT_MARGIN: u32 = 2;

/// The black the region is filled with, as `FPDFBitmap_FillRect` takes it: `0xAARRGGBB`.
pub const OPAQUE_BLACK: u32 = 0xFF00_0000;

/// A rectangle of whole pixels, **half-open**: columns `left..right`, rows `top..bottom`, from the
/// bitmap's top-left. Never empty, and never outside the bitmap it was made for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelRect {
    /// First column painted.
    pub left: u32,
    /// First row painted.
    pub top: u32,
    /// One past the last column painted.
    pub right: u32,
    /// One past the last row painted.
    pub bottom: u32,
}

impl PixelRect {
    /// Width in pixels; positive.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.right - self.left
    }

    /// Height in pixels; positive.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.bottom - self.top
    }

    /// Whether pixel (`x`, `y`) is inside.
    #[must_use]
    pub const fn contains(&self, x: u32, y: u32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }

    /// The whole-pixel rectangle from `left..right` by `top..bottom`, given as real numbers and
    /// widened by `margin` on every side, clamped into a `width` x `height` bitmap.
    ///
    /// `left` and `top` are floored and `right` and `bottom` ceiled **before** the margin, so the
    /// rectangle covers every pixel the real one touches. Shared by both sides because "outward to
    /// whole pixels, then clamp" is arithmetic with no transform in it; what each side feeds it is
    /// what differs.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] for a value that is not finite (each side refuses those first), and
    /// [`Error::InvalidArgument`] when nothing of the rectangle meets the bitmap: a region drawn
    /// wholly off the page paints nothing, and a redaction that painted nothing must not report
    /// success.
    pub fn outward(
        (left, top, right, bottom): (f64, f64, f64, f64),
        margin: u32,
        (width, height): (u32, u32),
    ) -> Result<Self> {
        if ![left, top, right, bottom].iter().all(|v| v.is_finite()) {
            return Err(Error::Internal(
                "pdf flatten: a pixel rectangle with an edge that is not finite".to_owned(),
            ));
        }
        let margin = f64::from(margin);
        let rect = Self {
            left: clamp_whole(left.floor() - margin, width),
            top: clamp_whole(top.floor() - margin, height),
            right: clamp_whole(right.ceil() + margin, width),
            bottom: clamp_whole(bottom.ceil() + margin, height),
        };
        if rect.left >= rect.right || rect.top >= rect.bottom {
            return Err(Error::InvalidArgument(
                "pdf flatten: the region does not meet the page, so nothing would be covered"
                    .to_owned(),
            ));
        }
        Ok(rect)
    }
}

/// `value`, a whole number, clamped into `0..=max`.
fn clamp_whole(value: f64, max: u32) -> u32 {
    let clamped = value.clamp(0.0, f64::from(max));
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped into 0..=max, a u32, and already whole: the cast is exact"
    )]
    let whole = clamped as u32;
    whole
}

/// PDFium's page-to-device map for one render: user-space point to the whole device pixel it lands
/// on, with the render's own start, size and rotation.
///
/// A trait so the paint can be tested against fakes that lie -- one pixel off, half a pixel the
/// wrong way -- and so the native engine and (in a later slice) the web bridge each supply theirs.
pub trait PageToDevice {
    /// The device pixel `(x, y)` user-space point (`page_x`, `page_y`) maps to.
    ///
    /// # Errors
    ///
    /// Whatever the engine reports for a point it cannot map.
    fn page_to_device(&self, page_x: f64, page_y: f64) -> Result<(i32, i32)>;
}

/// A region conversion, display points to user space. Production passes
/// [`Region::to_content_space`]; a test passes one with a fault planted in it.
pub type ToUserSpace = fn(&Region, &PageFrame) -> Result<Rect>;

/// The rectangle the paint fills: `region` through burrow's conversion and the engine's transform,
/// widened by [`PAINT_MARGIN`] and clamped into a `width` x `height` bitmap.
///
/// # Errors
///
/// Whatever the conversion refuses (a region with no extent, a frame that cannot be read), whatever
/// the transform refuses, and [`Error::InvalidArgument`] for a region wholly off the page.
pub fn paint_rectangle(
    region: &Region,
    frame: &PageFrame,
    size: (u32, u32),
    to_device: &dyn PageToDevice,
) -> Result<PixelRect> {
    paint_rectangle_with(region, frame, size, to_device, Region::to_content_space)
}

/// [`paint_rectangle`] with the conversion named, so a fault can be planted in it.
///
/// # Errors
///
/// As [`paint_rectangle`].
pub fn paint_rectangle_with(
    region: &Region,
    frame: &PageFrame,
    size: (u32, u32),
    to_device: &dyn PageToDevice,
    to_user: ToUserSpace,
) -> Result<PixelRect> {
    let user = to_user(region, frame)?;
    // ALL FOUR CORNERS, because under a quarter turn the user-space rectangle's left edge becomes
    // the device's top or bottom: which corner lands where is the transform's to say.
    let corners = [
        to_device.page_to_device(user.left, user.bottom)?,
        to_device.page_to_device(user.left, user.top)?,
        to_device.page_to_device(user.right, user.bottom)?,
        to_device.page_to_device(user.right, user.top)?,
    ];
    let xs = corners.map(|(x, _)| f64::from(x));
    let ys = corners.map(|(_, y)| f64::from(y));
    let low = |v: [f64; 4]| v.iter().copied().fold(f64::INFINITY, f64::min);
    let high = |v: [f64; 4]| v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    PixelRect::outward((low(xs), low(ys), high(xs), high(ys)), PAINT_MARGIN, size)
}

/// Rule 6's ceilings on a *W* x *H* picture, exactly, on the product: the caller's `max_pixels`,
/// then redaction's own [`MAX_FLATTEN_PIXELS`], at `Stage::Pixels`. **Called before anything is
/// allocated for the picture**, on the render side as on the write side.
///
/// The refusal is `LimitExceeded` (`max_pixels`, or `redaction_flatten_pixels`) until the
/// integration slice gives rule 6 its id, `[page-too-large-to-flatten]`.
///
/// # Errors
///
/// [`Error::LimitExceeded`] above either ceiling.
pub fn check_ceilings(width: u32, height: u32, limits: &Limits) -> Result<u64> {
    let pixels = u64::from(width) * u64::from(height);
    Limits::check(Stage::Pixels, "max_pixels", pixels, limits.max_pixels)?;
    Limits::check(
        Stage::Pixels,
        "redaction_flatten_pixels",
        pixels,
        MAX_FLATTEN_PIXELS,
    )?;
    Ok(pixels)
}

/// PDFium's 4-byte pixels (`B, G, R, A` in memory) as the canonical RGB form: three bytes each,
/// rows top to bottom, no padding. Alpha is dropped: the bitmap was filled opaque before the
/// render, so it carries nothing.
///
/// `stride` is PDFium's bytes per row and is **bounded before it is used**: at least `width * 4`
/// and at most twice that, the same bound `pdfium::render` holds it to.
///
/// # Errors
///
/// [`Error::Internal`] for a stride outside that bound, a buffer shorter than `stride * height`,
/// or a size that does not fit in `usize`.
pub fn bgra_to_rgb(src: &[u8], stride: usize, width: u32, height: u32) -> Result<Vec<u8>> {
    let too_big = || Error::Internal("pdf flatten: a bitmap that does not fit in usize".to_owned());
    let wide = usize::try_from(width).map_err(|_| too_big())?;
    let rows = usize::try_from(height).map_err(|_| too_big())?;
    let row_bytes = wide.checked_mul(4).ok_or_else(too_big)?;
    if stride < row_bytes || stride > row_bytes.saturating_add(row_bytes) {
        return Err(Error::Internal(
            "pdf flatten: a stride the bitmap cannot have".to_owned(),
        ));
    }
    let span = stride.checked_mul(rows).ok_or_else(too_big)?;
    let pixels = src.get(..span).ok_or_else(|| {
        Error::Internal("pdf flatten: a bitmap buffer shorter than its rows".to_owned())
    })?;
    let mut rgb = Vec::with_capacity(
        wide.checked_mul(rows)
            .and_then(|p| p.checked_mul(3))
            .ok_or_else(too_big)?,
    );
    for row in pixels.chunks_exact(stride) {
        let row = row.get(..row_bytes).ok_or_else(too_big)?;
        let (whole, _) = row.as_chunks::<4>();
        for &[blue, green, red, _alpha] in whole {
            rgb.extend_from_slice(&[red, green, blue]);
        }
    }
    Ok(rgb)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::cast_possible_truncation,
    reason = "tests"
)]
pub(super) mod tests {
    use super::{
        PAINT_MARGIN, PageToDevice, PixelRect, bgra_to_rgb, check_ceilings, paint_rectangle,
    };
    use crate::pdfsyntax::geometry::Rect;
    use crate::pdfsyntax::region::{PageFrame, Region};
    use burrow_types::{Error, Limits, Result};

    /// An exact transform for a page of `box` at `rotate`, shown on `w` x `h` pixels, rounding to
    /// the nearest pixel as PDFium documents -- the fake the true-transform twins use.
    pub(crate) struct Ideal {
        pub frame: PageFrame,
        pub size: (u32, u32),
    }

    impl Ideal {
        pub(crate) fn exact(&self, x: f64, y: f64) -> (f64, f64) {
            let b = self.frame.display_box;
            let (bw, bh) = (b.right - b.left, b.top - b.bottom);
            let (ux, uy) = (x - b.left, y - b.bottom);
            // Display coordinates in points from the top-left, after the page's rotation.
            let (dx, dy, dw, dh) = match self.frame.rotate {
                0 => (ux, bh - uy, bw, bh),
                90 => (uy, ux, bh, bw),
                180 => (bw - ux, uy, bw, bh),
                _ => (bh - uy, bw - ux, bh, bw),
            };
            (
                dx * f64::from(self.size.0) / dw,
                dy * f64::from(self.size.1) / dh,
            )
        }
    }

    impl PageToDevice for Ideal {
        fn page_to_device(&self, x: f64, y: f64) -> Result<(i32, i32)> {
            let (dx, dy) = self.exact(x, y);
            Ok((dx.round() as i32, dy.round() as i32))
        }
    }

    pub(crate) fn letter(rotate: u16) -> PageFrame {
        PageFrame {
            display_box: Rect {
                left: 0.0,
                bottom: 0.0,
                right: 612.0,
                top: 792.0,
            },
            rotate,
            user_unit: 1.0,
        }
    }

    #[test]
    fn the_paint_covers_the_region_widened_by_two_pixels() {
        let frame = letter(0);
        let ideal = Ideal {
            frame,
            size: (1275, 1650),
        };
        // 72 pt = 150 px exactly, so the corners are whole and only the margin widens them.
        let region = Region {
            left: 72.0,
            top: 144.0,
            width: 72.0,
            height: 72.0,
        };
        let rect = paint_rectangle(&region, &frame, (1275, 1650), &ideal).unwrap();
        assert_eq!(
            rect,
            PixelRect {
                left: 150 - PAINT_MARGIN,
                top: 300 - PAINT_MARGIN,
                right: 300 + PAINT_MARGIN,
                bottom: 450 + PAINT_MARGIN,
            }
        );
    }

    #[test]
    fn a_rectangle_is_clamped_into_the_bitmap_and_one_wholly_off_it_refuses() {
        let partly = PixelRect::outward((-10.0, -5.0, 3.2, 4.0), 1, (10, 10)).unwrap();
        assert_eq!(
            partly,
            PixelRect {
                left: 0,
                top: 0,
                right: 5,
                bottom: 5
            }
        );
        for off in [(20.0, 0.0, 30.0, 5.0), (0.0, -30.0, 5.0, -20.0)] {
            assert!(matches!(
                PixelRect::outward(off, 2, (10, 10)),
                Err(Error::InvalidArgument(_))
            ));
        }
        assert!(matches!(
            PixelRect::outward((f64::NAN, 0.0, 1.0, 1.0), 1, (10, 10)),
            Err(Error::Internal(_))
        ));
    }

    #[test]
    fn the_ceilings_are_exact_on_the_product() {
        let mut web = Limits::DEFAULT;
        web.max_pixels = 4_194_304;
        assert_eq!(check_ceilings(2048, 2048, &web).unwrap(), 4_194_304);
        assert!(matches!(
            check_ceilings(2048, 2049, &web),
            Err(Error::LimitExceeded {
                limit: "max_pixels",
                ..
            })
        ));
        assert_eq!(
            check_ceilings(6235, 14351, &Limits::DEFAULT).unwrap(),
            89_478_485
        );
        assert!(matches!(
            check_ceilings(6235, 14352, &Limits::DEFAULT),
            Err(Error::LimitExceeded {
                limit: "redaction_flatten_pixels",
                ..
            })
        ));
    }

    #[test]
    fn bgra_becomes_rgb_row_by_row_and_padding_is_skipped() {
        // 2 x 2, stride 12 (one pixel of padding per row).
        let src = [
            1, 2, 3, 255, 4, 5, 6, 255, 99, 99, 99, 99, //
            7, 8, 9, 255, 10, 11, 12, 255, 99, 99, 99, 99,
        ];
        assert_eq!(
            bgra_to_rgb(&src, 12, 2, 2).unwrap(),
            vec![3, 2, 1, 6, 5, 4, 9, 8, 7, 12, 11, 10]
        );
        // Stride below a row, above twice a row, and a buffer one row short.
        assert!(bgra_to_rgb(&src, 7, 2, 2).is_err());
        assert!(bgra_to_rgb(&src, 17, 2, 2).is_err());
        assert!(bgra_to_rgb(&src[..12], 12, 2, 2).is_err());
    }

    proptest::proptest! {
        /// Under the true transform, at every rotation, the paint contains every pixel the region
        /// touches, plus rule 8's pixel, on every side that is inside the bitmap.
        #[test]
        fn the_true_paint_contains_the_touched_pixels_and_rule_8_s(
            rotate in proptest::sample::select(vec![0_u16, 90, 180, 270]),
            left in 0.0_f64..500.0, top in 0.0_f64..500.0,
            width in 0.1_f64..100.0, height in 0.1_f64..80.0,
        ) {
            let frame = letter(rotate);
            let (dw, dh) = frame.displayed_size();
            let size = (
                super::super::pixels_for(dw).unwrap(),
                super::super::pixels_for(dh).unwrap(),
            );
            let ideal = Ideal { frame, size };
            let region = Region { left, top, width, height };
            let rect = paint_rectangle(&region, &frame, size, &ideal).unwrap();
            let (sx, sy) = (f64::from(size.0) / dw, f64::from(size.1) / dh);
            let need = PixelRect::outward(
                (left * sx, top * sy, (left + width) * sx, (top + height) * sy),
                1,
                size,
            ).unwrap();
            proptest::prop_assert!(
                rect.left <= need.left && rect.top <= need.top
                    && rect.right >= need.right && rect.bottom >= need.bottom,
                "{rect:?} does not contain {need:?}"
            );
        }
    }
}

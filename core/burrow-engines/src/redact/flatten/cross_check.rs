//! ADR 0031 §1 step 6: the painted picture is held to burrow's own placement of the region.
//!
//! # Two sources, and this is the second
//!
//! The paint (`paint`) reaches its rectangle through burrow's display-to-user conversion and
//! PDFium's page-to-device transform. **This check goes through neither.** It takes the display
//! frame *d*w x *d*h that qpdf read, re-derives *W* and *H* from it, and maps the region from
//! display points to pixels by plain arithmetic -- *W*/*d*w across and *H*/*d*h down, from the
//! top-left -- outward to whole pixels and then by rule 8's one pixel. Every pixel of **that**
//! rectangle must be the paint, opaque black. A fault in the conversion, a transform that is a
//! pixel off, or rounding that went the wrong way moves the paint and not this rectangle, so the
//! check refuses.
//!
//! **It takes no [`PageFrame`](crate::pdfsyntax::region::PageFrame)**: no box origin and no
//! `/Rotate`, only the display size. That is what makes "compute the check through user space"
//! inexpressible here rather than merely not done -- the signature is the defence, and it is the
//! one mutation of ADR 0031's step-6 row this module cannot have a test for.
//!
//! # What it does not detect
//!
//! - **Anything outside its rectangle.** Rule 8's edge is a pixel edge: ink crossing it stays
//!   visible beyond, and no check can say whether that is what the person meant to cover.
//! - **Who painted a black pixel.** A page whose own pixels are black where the paint belongs
//!   passes. That is not a leak: the property held is that every pixel of the rectangle *is*
//!   black in the picture written, whoever made it so.
//! - **A wrong display frame.** The size it is handed is qpdf's; page identity across the two
//!   engines is §5's (S9b), not this check's.
//!
//! # The refusal
//!
//! [`Error::Unsupported`] with [`PAINT_DISAGREES`] as its message. The bracketed rule id,
//! `[paint-disagrees]`, its `RULE_IDS` row and its web message are the integration slice's (owner
//! decision 90, item 3), so this slice raises no new bracketed id.
//!
//! # Not reachable yet
//!
//! Nothing outside this crate's tests calls this module until the integration slice.

#![allow(
    dead_code,
    reason = "ADR 0031 S9a: no caller outside tests until the integration slice wires the check"
)]

use burrow_types::{Error, Limits, Result};

use super::paint::{PixelRect, check_ceilings};
use super::{Picture, pixels_for};
use crate::pdfsyntax::region::Region;

/// Rule 8's pixel: how far burrow's rectangle is widened past the pixels the region touches.
pub const CHECK_MARGIN: u32 = 1;

/// The paint-disagrees refusal's message, until the integration slice brackets its id.
pub const PAINT_DISAGREES: &str = "pdf flatten: the paint does not cover every pixel burrow's own \
     arithmetic places the region on, so the picture may show what the region was drawn over";

/// The refusal: the paint disagrees with burrow's rectangle.
#[must_use]
pub fn paint_disagrees() -> Error {
    Error::Unsupported(PAINT_DISAGREES.to_owned())
}

/// Whether `error` is the paint-disagrees refusal.
#[must_use]
pub fn is_paint_disagreement(error: &Error) -> bool {
    matches!(error, Error::Unsupported(message) if message == PAINT_DISAGREES)
}

/// Burrow's rectangle for `region` on a `display` frame shown at `size` pixels: display points
/// scaled by *W*/*d*w and *H*/*d*h, outward to whole pixels, widened by [`CHECK_MARGIN`], clamped.
///
/// # Errors
///
/// [`Error::InvalidArgument`] for a region that is not finite, has no positive extent, or does
/// not meet the page.
pub fn check_rectangle(
    display: (f64, f64),
    size: (u32, u32),
    region: &Region,
) -> Result<PixelRect> {
    let Region {
        left,
        top,
        width,
        height,
    } = *region;
    if ![left, top, width, height].iter().all(|v| v.is_finite()) || width <= 0.0 || height <= 0.0 {
        return Err(Error::InvalidArgument(
            "pdf flatten: a region with no finite, positive extent".to_owned(),
        ));
    }
    let across = f64::from(size.0) / display.0;
    let down = f64::from(size.1) / display.1;
    PixelRect::outward(
        (
            left * across,
            top * down,
            (left + width) * across,
            (top + height) * down,
        ),
        CHECK_MARGIN,
        size,
    )
}

/// Hold `picture` to page *N*'s display frame and its paint to burrow's rectangle for `region`.
///
/// In order: *W* and *H* re-derived from `display` by [`pixels_for`]; rule 6's ceilings on their
/// product; the picture's reported size equal to them; its buffer exactly *W*·*H*·3 bytes, by
/// checked arithmetic; then every pixel of [`check_rectangle`] opaque black (`0, 0, 0`).
///
/// # Errors
///
/// - [`Error::LimitExceeded`] above `max_pixels` or `MAX_FLATTEN_PIXELS`.
/// - [`Error::Internal`] for a picture of another size or another length: the drawing side is
///   burrow's own, so either is a bug, and either refuses.
/// - [`Error::InvalidArgument`] for a region [`check_rectangle`] refuses.
/// - [`paint_disagrees`] when any pixel of the rectangle is not the paint.
pub fn check_paint(
    display: (f64, f64),
    picture: &Picture<'_>,
    region: &Region,
    limits: &Limits,
) -> Result<()> {
    let (wide, high) = (pixels_for(display.0)?, pixels_for(display.1)?);
    let pixels = check_ceilings(wide, high, limits)?;
    if picture.width != wide || picture.height != high {
        return Err(Error::Internal(
            "pdf flatten: the painted picture is not the page's display frame at 150 dpi"
                .to_owned(),
        ));
    }
    let length = pixels
        .checked_mul(3)
        .and_then(|bytes| usize::try_from(bytes).ok());
    if length != Some(picture.rgb.len()) {
        return Err(Error::Internal(
            "pdf flatten: the painted picture's buffer is not three bytes per pixel".to_owned(),
        ));
    }
    let rect = check_rectangle(display, (wide, high), region)?;
    let fits = || Error::Internal("pdf flatten: a pixel offset that does not fit".to_owned());
    let row_bytes = usize::try_from(wide)
        .ok()
        .and_then(|w| w.checked_mul(3))
        .ok_or_else(fits)?;
    let from = usize::try_from(rect.left)
        .ok()
        .and_then(|x| x.checked_mul(3))
        .ok_or_else(fits)?;
    let to = usize::try_from(rect.right)
        .ok()
        .and_then(|x| x.checked_mul(3))
        .ok_or_else(fits)?;
    for row in rect.top..rect.bottom {
        let start = usize::try_from(row)
            .ok()
            .and_then(|r| r.checked_mul(row_bytes))
            .ok_or_else(fits)?;
        let span = picture.rgb.get(start + from..start + to).ok_or_else(fits)?;
        if span.iter().any(|&byte| byte != 0) {
            return Err(paint_disagrees());
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "tests"
)]
mod tests {
    use super::{CHECK_MARGIN, check_paint, check_rectangle, is_paint_disagreement};
    use crate::pdfsyntax::geometry::Rect;
    use crate::pdfsyntax::region::{PageFrame, Region};
    use crate::redact::flatten::Picture;
    use crate::redact::flatten::paint::tests::{Ideal, letter};
    use crate::redact::flatten::paint::{
        PageToDevice, PixelRect, paint_rectangle, paint_rectangle_with,
    };
    use burrow_types::{Error, Limits, Result};

    /// A white picture of `size` with `rect` painted black: what an honest render produces for a
    /// page with nothing on it.
    fn painted(size: (u32, u32), rect: PixelRect) -> Vec<u8> {
        let mut rgb = vec![255_u8; (size.0 * size.1 * 3) as usize];
        for y in rect.top..rect.bottom {
            for x in rect.left..rect.right {
                let at = ((y * size.0 + x) * 3) as usize;
                rgb[at..at + 3].fill(0);
            }
        }
        rgb
    }

    /// Paint `region` on `frame` through `to_device` (and `to_user`), then run the cross-check.
    fn paint_then_check(
        frame: &PageFrame,
        region: &Region,
        to_device: &dyn PageToDevice,
        to_user: super::super::paint::ToUserSpace,
    ) -> Result<()> {
        let display = frame.displayed_size();
        let size = (
            super::pixels_for(display.0).unwrap(),
            super::pixels_for(display.1).unwrap(),
        );
        let rect = paint_rectangle_with(region, frame, size, to_device, to_user)?;
        let rgb = painted(size, rect);
        check_paint(
            display,
            &Picture::new(size.0, size.1, &rgb),
            region,
            &Limits::DEFAULT,
        )
    }

    /// The transform shifted by `(dx, dy)` device pixels after rounding: one pixel off.
    struct Off {
        ideal: Ideal,
        by: (i32, i32),
    }
    impl PageToDevice for Off {
        fn page_to_device(&self, x: f64, y: f64) -> Result<(i32, i32)> {
            let (px, py) = self.ideal.page_to_device(x, y)?;
            Ok((px + self.by.0, py + self.by.1))
        }
    }

    /// Rounding half a pixel the wrong way: every value rounded as if it were half a pixel
    /// further along (`late`) or back (`!late`), so an edge can land a whole pixel off.
    struct HalfWrong {
        ideal: Ideal,
        late: bool,
    }
    impl PageToDevice for HalfWrong {
        fn page_to_device(&self, x: f64, y: f64) -> Result<(i32, i32)> {
            let (ex, ey) = self.ideal.exact(x, y);
            let shift = if self.late { 0.5 } else { -0.5 };
            Ok(((ex + shift).round() as i32, (ey + shift).round() as i32))
        }
    }

    /// Region conversion with a fault planted: the y flip forgotten on an unrotated page.
    fn unflipped(region: &Region, frame: &PageFrame) -> Result<Rect> {
        let mut rect = region.to_content_space(frame)?;
        let height = rect.top - rect.bottom;
        rect.bottom = region.top + frame.display_box.bottom;
        rect.top = rect.bottom + height;
        Ok(rect)
    }

    fn ideal_for(frame: PageFrame) -> Ideal {
        let (dw, dh) = frame.displayed_size();
        Ideal {
            frame,
            size: (
                super::pixels_for(dw).unwrap(),
                super::pixels_for(dh).unwrap(),
            ),
        }
    }

    /// A region off the page's horizontal centre line, with edges that are not whole pixels.
    const REGION: Region = Region {
        left: 100.3,
        top: 200.7,
        width: 50.2,
        height: 20.1,
    };

    /// ADR 0031 *Fixtures*, step 6 twin: the true transform, every rotation, agrees.
    #[test]
    fn the_true_transform_agrees_at_every_rotation() {
        for rotate in [0, 90, 180, 270] {
            let frame = letter(rotate);
            paint_then_check(&frame, &REGION, &ideal_for(frame), Region::to_content_space)
                .unwrap_or_else(|e| panic!("rotate {rotate}: {e}"));
        }
    }

    /// Step 6 fixture: a transform one pixel off, and two, in each direction. KILLS: deleting the
    /// pixel test.
    ///
    /// ONE PIXEL IS ABSORBED by the paint's rounding pixel on an edge where the rounding went the
    /// favourable way, and that is the margin doing its job. This region's edges sit where it
    /// cannot: left and top at a fraction of 0.6 of a pixel (which rounds up, so a shift right or
    /// down leaves the check's first column unpainted), right and bottom at 0.2 (which rounds down,
    /// so a shift left or up leaves its last one).
    #[test]
    fn a_transform_one_pixel_off_disagrees() {
        let frame = letter(0);
        // 0.48 pt is one pixel at 150 dpi on Letter (1275 / 612 = 150 / 72).
        let region = Region {
            left: 150.6 * 0.48,
            top: 150.6 * 0.48,
            width: 149.6 * 0.48,
            height: 149.6 * 0.48,
        };
        let mut caught = 0;
        for by in [
            (1, 0),
            (-1, 0),
            (0, 1),
            (0, -1),
            (2, 0),
            (-2, 0),
            (0, 2),
            (0, -2),
        ] {
            let liar = Off {
                ideal: ideal_for(frame),
                by,
            };
            let error = paint_then_check(&frame, &region, &liar, Region::to_content_space)
                .expect_err("a shifted paint must be refused");
            assert!(is_paint_disagreement(&error), "{by:?}: {error}");
            caught += 1;
        }
        assert_eq!(caught, 8);
        // Twin: the same region, the true transform.
        paint_then_check(&frame, &region, &ideal_for(frame), Region::to_content_space).unwrap();
    }

    /// Step 6 row "PDFium's rounding half a pixel the wrong way", measured rather than assumed:
    /// **the paint's rounding pixel absorbs it** -- that pixel exists for exactly this -- and the
    /// same paint without that pixel is refused. KILLS: removing the rounding pixel from
    /// `PAINT_MARGIN` (the absorbed half goes red), and deleting the pixel test (the refused half).
    #[test]
    fn rounding_half_a_pixel_the_wrong_way_is_absorbed_by_the_rounding_pixel_alone() {
        let frame = letter(0);
        let display = frame.displayed_size();
        let size = (1275, 1650);
        for late in [true, false] {
            let liar = HalfWrong {
                ideal: ideal_for(frame),
                late,
            };
            paint_then_check(&frame, &REGION, &liar, Region::to_content_space)
                .unwrap_or_else(|e| panic!("late {late}: {e}"));
            let mut bare = paint_rectangle(&REGION, &frame, size, &liar).unwrap();
            bare.left += 1;
            bare.top += 1;
            bare.right -= 1;
            bare.bottom -= 1;
            let rgb = painted(size, bare);
            let error = check_paint(
                display,
                &Picture::new(size.0, size.1, &rgb),
                &REGION,
                &Limits::DEFAULT,
            )
            .expect_err("no rounding pixel: refused");
            assert!(is_paint_disagreement(&error), "late {late}: {error}");
        }
    }

    /// Step 6 fixture: a fault planted in the paint side's display-to-user conversion. The check
    /// does not use the conversion, so it is not fooled. KILLS: deleting the pixel test.
    #[test]
    fn a_fault_in_the_display_to_user_conversion_disagrees() {
        let frame = letter(0);
        let error = paint_then_check(&frame, &REGION, &ideal_for(frame), unflipped)
            .expect_err("a mirrored paint must be refused");
        assert!(is_paint_disagreement(&error), "{error}");
    }

    /// Rule 8, check side: a paint exactly to the touched pixels, without rule 8's pixel, is
    /// refused. KILLS: removing `CHECK_MARGIN`.
    #[test]
    fn a_paint_without_rule_8_s_pixel_disagrees() {
        let frame = letter(0);
        let display = frame.displayed_size();
        let size = (1275, 1650);
        let touched = {
            let mut rect = check_rectangle(display, size, &REGION).unwrap();
            rect.left += CHECK_MARGIN;
            rect.top += CHECK_MARGIN;
            rect.right -= CHECK_MARGIN;
            rect.bottom -= CHECK_MARGIN;
            rect
        };
        let rgb = painted(size, touched);
        let error = check_paint(
            display,
            &Picture::new(size.0, size.1, &rgb),
            &REGION,
            &Limits::DEFAULT,
        )
        .expect_err("rule 8's pixel unpainted must be refused");
        assert!(is_paint_disagreement(&error), "{error}");
        // Twin: the same paint widened by rule 8's pixel passes.
        let rgb = painted(size, check_rectangle(display, size, &REGION).unwrap());
        check_paint(
            display,
            &Picture::new(size.0, size.1, &rgb),
            &REGION,
            &Limits::DEFAULT,
        )
        .unwrap();
    }

    /// Step 6's size and length: W+1, H+1, one byte short, one long, and the size alone wrong over
    /// a buffer of the right length; twin exact. KILLS: trusting the reported size.
    #[test]
    fn a_picture_of_the_wrong_size_or_length_is_refused() {
        let frame = letter(0);
        let display = frame.displayed_size();
        let rect = paint_rectangle(&REGION, &frame, (1275, 1650), &ideal_for(frame)).unwrap();
        let rgb = painted((1275, 1650), rect);
        let check = |w: u32, h: u32, rgb: &[u8]| {
            check_paint(display, &Picture::new(w, h, rgb), &REGION, &Limits::DEFAULT)
        };
        check(1275, 1650, &rgb).unwrap();
        let wider = painted((1276, 1650), rect);
        assert!(matches!(check(1276, 1650, &wider), Err(Error::Internal(_))));
        let taller = painted((1275, 1651), rect);
        assert!(matches!(
            check(1275, 1651, &taller),
            Err(Error::Internal(_))
        ));
        assert!(matches!(
            check(1275, 1650, &rgb[..rgb.len() - 1]),
            Err(Error::Internal(_))
        ));
        let mut long = rgb.clone();
        long.push(0);
        assert!(matches!(check(1275, 1650, &long), Err(Error::Internal(_))));
        // THE SIZE ON ITS OWN, with the buffer the frame's length, so the length check cannot
        // stand in for it: the dimensions swapped (a quarter turn reported the other way), and a
        // width one more with a height to match the product's neighbourhood. Without these the
        // size check could be deleted and every case above would still refuse, by length.
        assert!(matches!(check(1650, 1275, &rgb), Err(Error::Internal(_))));
        assert!(matches!(check(1276, 1650, &rgb), Err(Error::Internal(_))));
    }

    /// Rule 6 on the check side: re-checked, whatever the render did.
    #[test]
    fn the_check_re_applies_the_pixel_ceiling() {
        let mut web = Limits::DEFAULT;
        web.max_pixels = 4_194_304;
        // 983 x 983.5 pt: 2048 x 2049 pixels, one row past the web's ceiling. No buffer is needed:
        // the ceiling comes first.
        let refused = check_paint(
            (983.0, 983.5),
            &Picture::new(2048, 2049, &[]),
            &REGION,
            &web,
        );
        assert!(matches!(
            refused,
            Err(Error::LimitExceeded {
                limit: "max_pixels",
                ..
            })
        ));
    }

    #[test]
    fn a_region_that_is_not_finite_or_has_no_extent_or_misses_the_page_is_refused() {
        for region in [
            Region {
                left: f64::NAN,
                ..REGION
            },
            Region {
                width: 0.0,
                ..REGION
            },
            Region {
                height: -1.0,
                ..REGION
            },
            Region {
                left: 700.0,
                ..REGION
            },
        ] {
            assert!(matches!(
                check_rectangle((612.0, 792.0), (1275, 1650), &region),
                Err(Error::InvalidArgument(_))
            ));
        }
    }

    proptest::proptest! {
        /// The true transform always agrees, at any rotation and any region on the page.
        #[test]
        fn the_true_paint_always_passes_the_check(
            rotate in proptest::sample::select(vec![0_u16, 90, 180, 270]),
            left in 0.0_f64..500.0, top in 0.0_f64..500.0,
            width in 0.1_f64..90.0, height in 0.1_f64..90.0,
        ) {
            let frame = letter(rotate);
            let region = Region { left, top, width, height };
            proptest::prop_assert!(
                paint_then_check(&frame, &region, &ideal_for(frame), Region::to_content_space)
                    .is_ok()
            );
        }

        /// A transform two pixels off in any direction never passes.
        #[test]
        fn a_paint_two_pixels_off_never_passes(
            rotate in proptest::sample::select(vec![0_u16, 90, 180, 270]),
            by in proptest::sample::select(vec![(2, 0), (-2, 0), (0, 2), (0, -2)]),
            left in 10.0_f64..500.0, top in 10.0_f64..500.0,
            width in 0.1_f64..90.0, height in 0.1_f64..90.0,
        ) {
            let frame = letter(rotate);
            let region = Region { left, top, width, height };
            let liar = Off { ideal: ideal_for(frame), by };
            let outcome = paint_then_check(&frame, &region, &liar, Region::to_content_space);
            proptest::prop_assert!(matches!(&outcome, Err(e) if is_paint_disagreement(e)), "{outcome:?}");
        }
    }
}

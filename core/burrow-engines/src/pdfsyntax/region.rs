//! What a redaction region's numbers mean, and the one place they are converted.
//!
//! # Why this is a module and not four `f64`s
//!
//! "The rectangle the user selected" is not a well-defined thing in a PDF. A page has up to
//! five boxes, may declare a `/Rotate` that turns what the reader sees away from user space,
//! and may declare a `/UserUnit` that changes what a point means. Four numbers with no frame
//! attached can be read four different ways, and **three of them silently miss the secret**:
//! the region lands somewhere the text is not, no glyph intersects it, the operation removes
//! nothing and reports success.
//!
//! So the frame is part of the type. A caller cannot hand over a bare rectangle.
//!
//! # The frame this crate defines
//!
//! A [`Region`] is in **display coordinates**: what the person looking at the page saw when
//! they drew the box.
//!
//! | question | answer, and why |
//! |---|---|
//! | which box? | **`/CropBox`**, falling back to `/MediaBox` when absent. A viewer shows the crop box, so that is what the user selected within; measuring from `/MediaBox` on a page whose crop box has a non-zero origin offsets every coordinate by that origin |
//! | before or after `/Rotate`? | **after**. The user selected on the rotated page, because that is the one on screen |
//! | what is a unit? | **points, as displayed** — already multiplied by `/UserUnit`. A page with `/UserUnit 2` shows each point twice as large, so the number the user drew is in those larger units |
//! | origin | **top-left, y downwards**, as every viewer and every pointing device reports it |
//!
//! That last row is the one most likely to be got wrong silently, because PDF user space has
//! its origin at the **bottom** left with y upwards. A region converted without the flip lands
//! mirrored about the page's horizontal centre line — which on a two-column page or a form is
//! very often still *on* the page, still plausible, and over the wrong text.
//!
//! # Converted in one place
//!
//! [`Region::to_content_space`] is the only conversion. Everything downstream — the glyph
//! walk, the box intersection, the removal — works in content space and never sees a display
//! coordinate. A second conversion site is a second chance to disagree, and the disagreement
//! would be a redaction over the wrong part of the page.

use burrow_types::{Error, Result};

use super::geometry::{Matrix, Rect};

/// The page geometry a region is measured against, read from the page dictionary.
///
/// Every field is what the **file** says, so that the conversion below is a function of the
/// document rather than of whoever assembled this struct.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageFrame {
    /// The box the viewer displays: `/CropBox`, or `/MediaBox` where there is none.
    ///
    /// In unrotated user space, as the file writes it. A non-zero origin is ordinary — a
    /// press-ready page often crops inside its media box — and is exactly what a conversion
    /// that assumes `(0, 0)` gets wrong.
    pub display_box: Rect,
    /// `/Rotate`, normalised to 0, 90, 180 or 270.
    pub rotate: u16,
    /// `/UserUnit`, the number of points one unit represents. 1.0 unless the file says otherwise.
    pub user_unit: f64,
}

impl PageFrame {
    /// The displayed page's size in display units, after rotation and `/UserUnit`.
    #[must_use]
    pub fn displayed_size(&self) -> (f64, f64) {
        let width = (self.display_box.right - self.display_box.left) * self.user_unit;
        let height = (self.display_box.top - self.display_box.bottom) * self.user_unit;
        if self.rotate == 90 || self.rotate == 270 {
            (height, width)
        } else {
            (width, height)
        }
    }
}

/// A rectangle the user selected, in display coordinates.
///
/// See the module header for exactly what that means. The type exists so the meaning travels
/// with the numbers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Region {
    /// Distance from the **left** edge of the displayed page, in display units.
    pub left: f64,
    /// Distance from the **top** edge of the displayed page, in display units. Downwards.
    pub top: f64,
    /// Width, in display units.
    pub width: f64,
    /// Height, in display units.
    pub height: f64,
}

impl Region {
    /// This region as a rectangle in **content space** — the space glyph boxes are computed in.
    ///
    /// # The four conversions, in order
    ///
    /// 1. **`/UserUnit`** divides out, because the region is in displayed points and content
    ///    space is in unscaled ones.
    /// 2. **The y flip**, because display coordinates count down from the top and user space
    ///    counts up from the bottom.
    /// 3. **`/Rotate`** unwinds, because the user selected on the rotated page and the content
    ///    stream draws on the unrotated one.
    /// 4. **The display box's origin** shifts back in, because a `/CropBox` that does not start
    ///    at `(0, 0)` means display coordinate zero is that corner rather than the page's.
    ///
    /// # Errors
    ///
    /// [`Error::Malformed`] for a `/Rotate` that is not a right angle, a `/UserUnit` that is
    /// not positive and finite, or a box with no extent — each of which would otherwise produce
    /// a region somewhere arbitrary rather than a refusal.
    pub fn to_content_space(&self, frame: &PageFrame) -> Result<Rect> {
        if !frame.user_unit.is_finite() || frame.user_unit <= 0.0 {
            return Err(Error::Malformed(
                "pdf region [user-unit]: a /UserUnit that is not a positive finite number, so \
                 no region measured in displayed points converts"
                    .to_owned(),
            ));
        }
        if !matches!(frame.rotate, 0 | 90 | 180 | 270) {
            return Err(Error::Malformed(
                "pdf region [rotate]: a /Rotate that is not a right angle, so which way the \
                 page was displayed is not derivable"
                    .to_owned(),
            ));
        }
        let box_width = frame.display_box.right - frame.display_box.left;
        let box_height = frame.display_box.top - frame.display_box.bottom;
        if !(box_width > 0.0 && box_height > 0.0)
            || !box_width.is_finite()
            || !box_height.is_finite()
        {
            return Err(Error::Malformed(
                "pdf region [display-box]: a display box with no positive extent".to_owned(),
            ));
        }

        // (1) out of displayed points.
        let (left, top) = (self.left / frame.user_unit, self.top / frame.user_unit);
        let (width, height) = (self.width / frame.user_unit, self.height / frame.user_unit);

        // (2) and (3): the rotation decides which unrotated axis each display axis became, and
        // the y flip rides along with it. Written as one transform per case rather than as a
        // general matrix, because the four cases are the whole space and a reader can check
        // each corner by hand — which is not true of a composed rotation about a moving centre.
        let (displayed_width, displayed_height) = frame.displayed_size();
        let (dw, dh) = (
            displayed_width / frame.user_unit,
            displayed_height / frame.user_unit,
        );
        let (x0, y0, x1, y1) = match frame.rotate {
            // Unrotated: flip y only.
            0 => (left, dh - top - height, left + width, dh - top),
            // The page was turned 90° clockwise for display, so display-x came from
            // unrotated-y and display-y from unrotated-x.
            90 => (top, left, top + height, left + width),
            180 => (dw - left - width, top, dw - left, top + height),
            _ => (dh - top - height, dw - left - width, dh - top, dw - left),
        };

        // (4) back into the display box's own coordinates.
        Ok(Rect {
            left: x0 + frame.display_box.left,
            bottom: y0 + frame.display_box.bottom,
            right: x1 + frame.display_box.left,
            top: y1 + frame.display_box.bottom,
        })
    }
}

/// The transform a caller needs if it would rather compose than convert a rectangle.
///
/// Exposed so that nothing downstream is tempted to write its own: the matrix and
/// [`Region::to_content_space`] are the same conversion, and a caller that needed one and
/// found only the other is how a second implementation gets written.
#[must_use]
pub fn display_to_content(frame: &PageFrame) -> Matrix {
    let scale = 1.0 / frame.user_unit;
    Matrix::scale(scale, scale)
}

#[cfg(test)]
mod tests {
    use super::{PageFrame, Region};
    use crate::pdfsyntax::geometry::Rect;

    /// A6-ish page whose crop box starts well away from the origin.
    fn cropped() -> PageFrame {
        PageFrame {
            display_box: Rect {
                left: 100.0,
                bottom: 200.0,
                right: 400.0,
                top: 600.0,
            },
            rotate: 0,
            user_unit: 1.0,
        }
    }

    fn plain() -> PageFrame {
        PageFrame {
            display_box: Rect {
                left: 0.0,
                bottom: 0.0,
                right: 300.0,
                top: 400.0,
            },
            rotate: 0,
            user_unit: 1.0,
        }
    }

    /// The region 10 pt in from the top-left of the displayed page, 50 x 20.
    fn corner() -> Region {
        Region {
            left: 10.0,
            top: 10.0,
            width: 50.0,
            height: 20.0,
        }
    }

    #[test]
    fn the_y_axis_flips() {
        // DISPLAY COUNTS DOWN FROM THE TOP; user space counts up from the bottom. Without the
        // flip the region lands mirrored about the page's horizontal centre -- which on a form
        // or a two-column page is very often still on the page and over the wrong text, so
        // nothing about the output says it went wrong.
        let got = corner().to_content_space(&plain()).expect("converts");
        assert_eq!(
            got,
            Rect {
                left: 10.0,
                bottom: 370.0,
                right: 60.0,
                top: 390.0
            },
            "10 pt below the top of a 400 pt page is y 390..370 in user space"
        );
    }

    #[test]
    fn a_non_origin_crop_box_shifts_the_region() {
        // THE FIXTURE FOR THE CROP BOX. A press-ready page crops inside its media box
        // routinely. Measuring from /MediaBox instead offsets every coordinate by the crop
        // box's origin -- here by (100, 200), which is more than the region is wide.
        let got = corner().to_content_space(&cropped()).expect("converts");
        assert_eq!(
            got,
            Rect {
                left: 110.0,
                bottom: 570.0,
                right: 160.0,
                top: 590.0
            }
        );
        // AND THE MISS IS DEMONSTRATED, not asserted: the same region under a frame that
        // forgot the origin lands somewhere the first does not reach.
        let forgot_origin = PageFrame {
            display_box: Rect {
                left: 0.0,
                bottom: 0.0,
                right: 300.0,
                top: 400.0,
            },
            ..cropped()
        };
        let wrong = corner().to_content_space(&forgot_origin).expect("converts");
        assert!(
            wrong.right < got.left || wrong.top < got.bottom,
            "the two frames must disagree by more than the region's own size, or this fixture \
             does not demonstrate the miss: {got:?} against {wrong:?}"
        );
    }

    #[test]
    fn a_rotated_page_is_selected_on_as_displayed() {
        // THE FIXTURE FOR /Rotate. The user drew the box on the page as shown, which for
        // /Rotate 90 is turned a quarter turn from the content stream's own axes. A conversion
        // that ignored it puts the region on the wrong edge entirely.
        let rotated = PageFrame {
            rotate: 90,
            ..plain()
        };
        let got = corner().to_content_space(&rotated).expect("converts");
        let unrotated = corner().to_content_space(&plain()).expect("converts");
        assert_ne!(
            got, unrotated,
            "if rotation changed nothing the fixture asks nothing"
        );
        // The displayed page is 400 wide by 300 tall; the top-left corner of that is the
        // bottom-left of the unrotated page, so the region sits near the origin in y.
        assert_eq!(
            got,
            Rect {
                left: 10.0,
                bottom: 10.0,
                right: 30.0,
                top: 60.0
            }
        );
        // Width and height swap, which is the check that catches an axis mix-up rather than an
        // offset one.
        assert!(
            (got.right - got.left - corner().height).abs() < 1e-9
                && (got.top - got.bottom - corner().width).abs() < 1e-9,
            "a quarter turn swaps the region's extents: {got:?}"
        );
    }

    #[test]
    fn user_unit_divides_out() {
        // THE FIXTURE FOR /UserUnit. A page with /UserUnit 2 shows each point twice as large,
        // so a 50 pt selection covers 25 unscaled points of content. Treating the number as
        // unscaled makes every region twice the intended size and the origin twice as far in.
        let big = PageFrame {
            user_unit: 2.0,
            ..plain()
        };
        let got = corner().to_content_space(&big).expect("converts");
        // THE WHOLE RECTANGLE, not just its width. A first version asserted the extent only,
        // and a mutation that divided the extents but not the origin survived it -- the region
        // was the right size in the wrong place, which removes the wrong text rather than
        // none. Found by the mutation sweep, which is what it is for.
        assert_eq!(
            got,
            Rect {
                left: 5.0,
                bottom: 385.0,
                right: 30.0,
                top: 395.0
            },
            "a 50x20 selection 10 pt in, at /UserUnit 2, is 25x10 at 5 pt in"
        );
        let ignored = corner().to_content_space(&plain()).expect("converts");
        assert!(
            (ignored.right - ignored.left - 50.0).abs() < 1e-9
                && got.right - got.left < ignored.right - ignored.left,
            "ignoring /UserUnit must give a visibly different region, or the fixture asks nothing"
        );
    }

    #[test]
    fn a_frame_that_cannot_be_read_is_refused_rather_than_guessed() {
        for (label, frame) in [
            (
                "rotate",
                PageFrame {
                    rotate: 45,
                    ..plain()
                },
            ),
            (
                "user-unit",
                PageFrame {
                    user_unit: 0.0,
                    ..plain()
                },
            ),
            (
                "display-box",
                PageFrame {
                    display_box: Rect {
                        left: 10.0,
                        bottom: 10.0,
                        right: 10.0,
                        top: 10.0,
                    },
                    ..plain()
                },
            ),
        ] {
            let error = corner()
                .to_content_space(&frame)
                .expect_err("an unreadable frame must be refused");
            assert!(
                format!("{error:?}").contains(&format!("[{label}]")),
                "refused, but by a different rule: wanted `{label}`, got {error:?}"
            );
        }
    }

    #[test]
    fn every_rotation_keeps_the_region_inside_the_page() {
        // A cheap total check over all four cases: whatever the frame, a region inside the
        // displayed page converts to a rectangle inside the display box. A sign error in any
        // one arm puts it outside, which is how a redaction removes nothing at all.
        for rotate in [0, 90, 180, 270] {
            let frame = PageFrame {
                rotate,
                ..cropped()
            };
            let got = corner().to_content_space(&frame).expect("converts");
            let page = frame.display_box;
            assert!(
                got.left >= page.left - 1e-9
                    && got.right <= page.right + 1e-9
                    && got.bottom >= page.bottom - 1e-9
                    && got.top <= page.top + 1e-9,
                "rotate {rotate}: {got:?} escapes the display box {page:?}"
            );
        }
    }
}

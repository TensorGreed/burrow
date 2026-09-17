//! Property, golden and limit tests for `render`, against real PDFium.
//!
//! The unit tests beside the operation drive a fake engine: they pin the refusals, the
//! one-based conversion and the session's shape. These need the real rasteriser, and they are
//! the three invariants a fake cannot honestly assert:
//!
//! 1. **Rendering is deterministic.** The same page at the same size twice is the same bytes.
//!    Without it the whole strip is untestable and a conformance grid means nothing.
//! 2. **A raster never exceeds the box, and never has a zero dimension.** `Fit` promises a
//!    maximum, and the rounding in `fit_into` is where that promise is most easily lost.
//! 3. **The ink follows the page.** A render that produced a plausible rectangle of the right
//!    size for the wrong page would satisfy every other assertion here.
//!
//! The golden is the fourth: a committed 4x4 ink grid for a fixture whose ink is in exactly
//! one quadrant, which is the observable ADR 0027 §5 proposes for the differential corpus. It
//! lives here first so the grid's arithmetic is pinned before the two platforms compare it.

#![cfg(all(feature = "native-engines", target_os = "linux"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    // The grid arithmetic below divides a known-good pixel count into four bands and averages
    // a cell. The lint exists for attacker-controlled sizes in library code, which is where it
    // stays denied; here the remainder is deliberately discarded.
    clippy::integer_division,
    clippy::cast_possible_truncation
)]

#[path = "../../burrow-engines/testsupport/minimal_pdf.rs"]
mod minimal_pdf;

use std::sync::Arc;

use burrow_engines::OpenOptions;
use burrow_engines::pdfium::Pdfium;
use burrow_ops::{Fit, Rendered, render};
use burrow_types::{Clock, Error, Limits, ManualClock, Result, Stage};
use proptest::prelude::*;

fn options(limits: Limits) -> OpenOptions<'static> {
    OpenOptions::new(limits, Arc::new(ManualClock::new(0)) as Arc<dyn Clock>)
}

fn draw(bytes: Vec<u8>, pages: &[u64], fit: Fit, limits: Limits) -> Result<Vec<Rendered>> {
    render(
        &Pdfium::new(),
        bytes.into_boxed_slice(),
        pages,
        fit,
        &options(limits),
    )
}

/// A 4x4 grid of ink levels, 0 (white) to 3 (solid), from a rendered page.
///
/// **ADR 0027 §5's proposed conformance observable, computed here first.** Dimensions alone
/// cannot see a wrong page; a single `ink: bool` cannot see a rotation, which is the case
/// `/rotate-pdf` exists for. A 4x4 grid quantised to four levels sees both while staying
/// coarse enough that antialiasing differences between two builds of one rasteriser do not
/// move it.
///
/// Luminance is the plain average of R, G and B. Not a perceptual weighting: this is asking
/// "is there ink here", and a weighting would make the grid depend on the colour of the ink.
fn ink_grid(width: u32, height: u32, rgba: &[u8]) -> [u8; 16] {
    let mut grid = [0_u8; 16];
    let (w, h) = (width as usize, height as usize);
    for (cell, level) in grid.iter_mut().enumerate() {
        let (cx, cy) = (cell % 4, cell / 4);
        let (x0, x1) = (cx * w / 4, (cx + 1) * w / 4);
        let (y0, y1) = (cy * h / 4, (cy + 1) * h / 4);
        let mut total: u64 = 0;
        let mut count: u64 = 0;
        for y in y0..y1.max(y0 + 1) {
            for x in x0..x1.max(x0 + 1) {
                let at = (y * w + x) * 4;
                let luminance =
                    (u64::from(rgba[at]) + u64::from(rgba[at + 1]) + u64::from(rgba[at + 2])) / 3;
                total += luminance;
                count += 1;
            }
        }
        let mean = total.checked_div(count).unwrap_or(255);
        // Four levels, darkest first: 3 is solid ink, 0 is paper.
        *level = match mean {
            0..=63 => 3,
            64..=127 => 2,
            128..=191 => 1,
            _ => 0,
        };
    }
    grid
}

// --- the golden ----------------------------------------------------------------------

#[test]
fn the_ink_grid_of_the_one_quadrant_fixture_is_the_committed_one() {
    // `pdf_with_ink` is 200x400 points with a black rectangle over the TOP-LEFT quarter and
    // nothing else, so a correct render fills exactly the four cells of the grid's top-left
    // quadrant and leaves the other twelve at paper.
    //
    // THIS IS THE GOLDEN, and it is written out rather than computed from the fixture's
    // geometry: a golden derived from the same description as the thing it checks agrees with
    // it by construction. These sixteen numbers came from a run and were read.
    let strip = draw(
        minimal_pdf::pdf_with_ink(),
        &[1],
        Fit::box_of(64, 128),
        Limits::DEFAULT,
    )
    .expect("the ink fixture should render");

    let page = &strip[0].raster;
    assert_eq!((page.width, page.height), (64, 128));
    assert_eq!(
        ink_grid(page.width, page.height, &page.rgba),
        [
            3, 3, 0, 0, //
            3, 3, 0, 0, //
            0, 0, 0, 0, //
            0, 0, 0, 0,
        ],
        "the ink should fill exactly the top-left quadrant"
    );
}

#[test]
fn the_grid_moves_when_the_page_does_which_is_what_makes_it_an_observable() {
    // A grid that reported the same thing for a rotated page would be an observable that
    // cannot see the operation `/rotate-pdf` exists for. The fixture is rendered, then the
    // SAME bytes with `/Rotate 90` applied, and the two grids must differ.
    //
    // Asserted as "differs" rather than as a second golden: which way PDFium turns a page for
    // a given `/Rotate` is the engine's business and is already pinned by `rotate`'s own
    // tests. What matters here is that the readout is sensitive to it at all.
    let upright = draw(
        minimal_pdf::pdf_with_ink(),
        &[1],
        Fit::box_of(64, 128),
        Limits::DEFAULT,
    )
    .expect("upright");
    let turned = draw(
        burrow_ops::rotate(
            &burrow_engines::qpdf::Qpdf::new(),
            minimal_pdf::pdf_with_ink().into_boxed_slice(),
            burrow_ops::Pages::numbered(&[1]),
            90,
            &options(Limits::DEFAULT),
        )
        .expect("the fixture should rotate"),
        &[1],
        Fit::box_of(64, 128),
        Limits::DEFAULT,
    )
    .expect("turned");

    let a = ink_grid(
        upright[0].raster.width,
        upright[0].raster.height,
        &upright[0].raster.rgba,
    );
    let b = ink_grid(
        turned[0].raster.width,
        turned[0].raster.height,
        &turned[0].raster.rgba,
    );
    assert_ne!(a, b, "the ink grid cannot see a quarter turn");
}

// --- the limits ----------------------------------------------------------------------

#[test]
fn a_box_past_max_pixels_is_refused_at_the_pixels_stage_before_the_document_is_opened() {
    let limits = Limits::with(|l| l.max_pixels = 64 * 128 - 1);
    match draw(
        minimal_pdf::pdf_with_ink(),
        &[1],
        Fit::box_of(64, 128),
        limits,
    )
    .expect_err("one pixel past the ceiling")
    {
        Error::LimitExceeded {
            limit,
            stage,
            requested,
            allowed,
        } => {
            assert_eq!(limit, "max_pixels");
            assert_eq!(stage, Stage::Pixels);
            assert_eq!(requested, 8192);
            assert_eq!(allowed, 8191);
        }
        other => panic!("expected LimitExceeded at Stage::Pixels, got {other:?}"),
    }
}

#[test]
fn a_box_exactly_at_max_pixels_is_accepted() {
    // THE BOUNDARY, from the other side. A ceiling tested only from above passes for an
    // implementation that refuses everything.
    let limits = Limits::with(|l| l.max_pixels = 64 * 128);
    assert!(
        draw(
            minimal_pdf::pdf_with_ink(),
            &[1],
            Fit::box_of(64, 128),
            limits
        )
        .is_ok()
    );
}

// --- the properties ------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]

    /// Rendering the same page at the same size twice produces the same bytes.
    ///
    /// The invariant everything else here rests on. If it does not hold, a conformance grid is
    /// comparing noise and the strip's caching is unsound.
    #[test]
    fn rendering_is_deterministic(width in 8_u32..96, height in 8_u32..96) {
        let fit = Fit::box_of(width, height);
        let first = draw(minimal_pdf::pdf_with_ink(), &[1], fit, Limits::DEFAULT).unwrap();
        let second = draw(minimal_pdf::pdf_with_ink(), &[1], fit, Limits::DEFAULT).unwrap();
        prop_assert_eq!(first, second);
    }

    /// A raster never exceeds its box and never has a zero dimension.
    ///
    /// `Fit` promises a maximum, and the rounding in `fit_into` is where that promise is most
    /// easily lost: a 100.6-point page in a 100-pixel box rounds to 101.
    #[test]
    fn a_raster_fits_its_box_at_every_size(width in 1_u32..200, height in 1_u32..200) {
        let fit = Fit::box_of(width, height);
        let strip = draw(minimal_pdf::pdf_with_ink(), &[1], fit, Limits::DEFAULT).unwrap();
        let raster = &strip[0].raster;
        prop_assert!(raster.width <= width, "{} > {}", raster.width, width);
        prop_assert!(raster.height <= height, "{} > {}", raster.height, height);
        prop_assert!(raster.width >= 1 && raster.height >= 1);
        prop_assert_eq!(
            raster.rgba.len(),
            (raster.width as usize) * (raster.height as usize) * 4
        );
    }

    /// The page's proportions survive the fit, to within a pixel of rounding.
    ///
    /// The fixture is 200x400 points, so a correct fit is always twice as tall as it is wide
    /// unless the box binds the other way. A render that stretched a page to fill the box
    /// would pass every other assertion in this file.
    #[test]
    fn the_aspect_ratio_survives(width in 8_u32..64) {
        // A box far taller than the page needs, so the WIDTH is what binds and the height is
        // computed from the ratio.
        let fit = Fit::box_of(width, width * 8);
        let strip = draw(minimal_pdf::pdf_with_ink(), &[1], fit, Limits::DEFAULT).unwrap();
        let raster = &strip[0].raster;
        prop_assert_eq!(raster.width, width);
        // 200:400 is 1:2, so the height is twice the width, +/- one pixel of rounding.
        let expected = width * 2;
        prop_assert!(
            raster.height.abs_diff(expected) <= 1,
            "{}x{} is not 1:2",
            raster.width,
            raster.height
        );
    }

    /// Every page named comes back, once, in the order asked for.
    #[test]
    fn a_strip_answers_exactly_what_it_was_asked(order in prop::sample::subsequence(vec![1_u64, 2, 3, 4], 1..=4)) {
        let strip = draw(
            minimal_pdf::pdf_with_page_tree(4, minimal_pdf::RotationPlacement::Absent),
            &order,
            Fit::box_of(32, 64),
            Limits::DEFAULT,
        ).unwrap();
        prop_assert_eq!(strip.iter().map(|r| r.page).collect::<Vec<_>>(), order);
    }
}

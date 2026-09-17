//! Unit tests for the render operation, on a fake engine.
//!
//! What happens *inside* an engine — PDFium's bitmap, the stride, the swizzle, the page handle
//! that must never escape — is tested against real PDFium and against the fake bridge in
//! `burrow-engines`. What is tested here is the part that belongs to the operation: one-based
//! page numbers, the refusals, the aspect-ratio arithmetic, the ceiling on the request, and
//! that the engine is asked for exactly what the caller asked for.

use std::cell::RefCell;
use std::sync::Arc;

use burrow_engines::{DocumentEngine, OpenOptions, PageRenderer, Raster};
use burrow_types::{Clock, Deadline, Error, Limits, ManualClock, Result, Stage};

use super::{Fit, fit_into, render};

/// One render the fake was asked for, so a test can assert on the REQUEST.
///
/// A strip that quietly draws the wrong page at the wrong size returns `Ok` either way, and
/// the pixels a fake produces cannot tell anybody apart — so what is worth recording is what
/// was asked, not what came back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Asked {
    index: u64,
    width: u32,
    height: u32,
    /// The `max_pixels` the engine was handed for this call.
    ///
    /// Recorded because the ceilings a strip renders under are the ones it BEGAN with, and
    /// that is otherwise unobservable: the box check in `begin` already refuses anything the
    /// per-page check would, so a strip cannot be made to fail by raising the limit. What a
    /// test can see is which number reached the engine.
    max_pixels: u64,
}

/// A document the fake handed out. Plain numbers, so it is `Send + Sync` derived.
#[derive(Debug)]
struct FakeDocument {
    pages: u64,
}

struct FakeRenderer {
    pages: u64,
    /// Each page's size in points, by zero-based index. Short means "the last one repeats".
    sizes: Vec<(f32, f32)>,
    asked: RefCell<Vec<Asked>>,
    /// Milliseconds each `render` costs, so a budget can be spent inside the loop.
    elapse_ms: u64,
    /// Return a raster of this size instead of the one requested, to drive the check that the
    /// operation does not take the engine's word for its own output.
    lies_about_size: Option<(u32, u32)>,
    clock: Arc<ManualClock>,
}

impl FakeRenderer {
    fn with_pages(pages: u64) -> Self {
        Self {
            pages,
            sizes: vec![(200.0, 400.0)],
            asked: RefCell::new(Vec::new()),
            elapse_ms: 0,
            lies_about_size: None,
            clock: Arc::new(ManualClock::new(0)),
        }
    }

    fn sized(mut self, sizes: Vec<(f32, f32)>) -> Self {
        self.sizes = sizes;
        self
    }

    fn options(&self, limits: Limits) -> OpenOptions<'static> {
        OpenOptions::new(limits, Arc::clone(&self.clock) as Arc<dyn Clock>)
    }

    fn asked(&self) -> Vec<Asked> {
        self.asked.borrow().clone()
    }

    fn size_of(&self, index: u64) -> (f32, f32) {
        let at = usize::try_from(index).expect("a small index");
        *self
            .sizes
            .get(at)
            .or_else(|| self.sizes.last())
            .expect("at least one size")
    }
}

impl DocumentEngine for FakeRenderer {
    type Document = FakeDocument;

    fn name(&self) -> &'static str {
        "fake-renderer"
    }

    fn open(&self, _bytes: Box<[u8]>, _options: &OpenOptions<'_>) -> Result<Self::Document> {
        Ok(FakeDocument { pages: self.pages })
    }

    fn page_count(&self, doc: &Self::Document) -> Result<u64> {
        Ok(doc.pages)
    }
}

impl PageRenderer for FakeRenderer {
    fn page_size(&self, _doc: &Self::Document, index: u64) -> Result<(f32, f32)> {
        Ok(self.size_of(index))
    }

    fn render(
        &self,
        _doc: &Self::Document,
        index: u64,
        width: u32,
        height: u32,
        options: &OpenOptions<'_>,
        deadline: &Deadline,
    ) -> Result<Raster> {
        self.asked.borrow_mut().push(Asked {
            index,
            width,
            height,
            max_pixels: options.limits.max_pixels,
        });

        // The ceiling the real engines apply, applied here too, so a test that drives
        // `max_pixels` through the operation is driving the same refusal the engine raises.
        burrow_types::Limits::check(
            Stage::Pixels,
            "max_pixels",
            u64::from(width) * u64::from(height),
            options.limits.max_pixels,
        )?;

        self.clock.advance(self.elapse_ms);
        deadline.checkpoint(options.clock.as_ref())?;

        let (w, h) = self.lies_about_size.unwrap_or((width, height));
        let len = usize::try_from(u64::from(w) * u64::from(h) * 4).expect("a small raster");
        // Through `Raster::new`, like the real engines, so this fake cannot hold itself to a
        // weaker rule than the thing it stands in for -- a raster whose length disagreed with
        // its own dimensions would be unbuildable here too.
        Raster::new(
            w,
            h,
            // The page index in every byte, so a test can tell one page's pixels from
            // another's. The real engines cannot offer this and neither can a picture --
            // which is ADR 0027 §4's stated residue, and the reason this is a fake.
            vec![u8::try_from(index % 256).expect("a byte"); len],
        )
    }
}

fn a_pdf() -> Box<[u8]> {
    // The fake never parses it. Something non-empty, so nothing refuses it for being empty.
    vec![0x25, 0x50, 0x44, 0x46].into_boxed_slice()
}

// --- the happy path ------------------------------------------------------------------

#[test]
fn every_page_named_comes_back_in_the_order_it_was_asked_for() {
    let engine = FakeRenderer::with_pages(5);
    let out = render(
        &engine,
        a_pdf(),
        &[3, 1, 5],
        Fit::box_of(120, 160),
        &engine.options(Limits::default()),
    )
    .expect("three pages should render");

    assert_eq!(
        out.iter().map(|r| r.page).collect::<Vec<_>>(),
        vec![3, 1, 5]
    );
    // ONE-BASED IN, ZERO-BASED AT THE SEAM, converted once.
    assert_eq!(
        engine.asked().iter().map(|a| a.index).collect::<Vec<_>>(),
        vec![2, 0, 4]
    );
    // And the pixels follow their page, which is what the `page` field is for.
    assert_eq!(out[0].raster.rgba[0], 2);
    assert_eq!(out[1].raster.rgba[0], 0);
    assert_eq!(out[2].raster.rgba[0], 4);
}

#[test]
fn a_portrait_page_uses_the_full_height_of_the_box_and_less_than_its_width() {
    // 200 x 400 points into a 120 x 160 box: the height binds, so 80 x 160.
    let engine = FakeRenderer::with_pages(1);
    let out = render(
        &engine,
        a_pdf(),
        &[1],
        Fit::box_of(120, 160),
        &engine.options(Limits::default()),
    )
    .expect("one page should render");

    assert_eq!((out[0].raster.width, out[0].raster.height), (80, 160));
    assert_eq!(out[0].raster.rgba.len(), 80 * 160 * 4);
}

#[test]
fn a_landscape_page_uses_the_full_width_instead() {
    let engine = FakeRenderer::with_pages(1).sized(vec![(400.0, 200.0)]);
    let out = render(
        &engine,
        a_pdf(),
        &[1],
        Fit::box_of(120, 160),
        &engine.options(Limits::default()),
    )
    .expect("one page should render");

    assert_eq!((out[0].raster.width, out[0].raster.height), (120, 60));
}

#[test]
fn pages_of_different_shapes_each_get_their_own_size() {
    // The reason `Rendered` carries its own dimensions rather than the caller assuming the
    // box: a strip of mixed-orientation pages has a different size on nearly every tile.
    let engine =
        FakeRenderer::with_pages(3).sized(vec![(200.0, 400.0), (400.0, 200.0), (300.0, 300.0)]);
    let out = render(
        &engine,
        a_pdf(),
        &[1, 2, 3],
        Fit::box_of(120, 160),
        &engine.options(Limits::default()),
    )
    .expect("three pages should render");

    let sizes: Vec<_> = out
        .iter()
        .map(|r| (r.raster.width, r.raster.height))
        .collect();
    assert_eq!(sizes, vec![(80, 160), (120, 60), (120, 120)]);
}

// --- the arithmetic, on its own ------------------------------------------------------

#[test]
fn fitting_never_returns_a_zero_dimension() {
    // 3000 x 1 points into a 120 x 160 box scales the height to 0.04, which rounds to zero --
    // and a zero-pixel raster is not a small raster, it is a request no engine can satisfy.
    assert_eq!(
        fit_into(3000.0, 1.0, Fit::box_of(120, 160)).unwrap(),
        (120, 1)
    );
}

#[test]
fn fitting_never_exceeds_the_box_it_was_given() {
    // Rounding can push a dimension one past the box; `Fit` promises a maximum.
    for (w, h) in [(100.6_f32, 100.0_f32), (100.0, 100.6), (1.0, 1000.0)] {
        let (width, height) = fit_into(w, h, Fit::box_of(100, 100)).unwrap();
        assert!(
            width <= 100 && height <= 100,
            "{w}x{h} gave {width}x{height}"
        );
        assert!(width >= 1 && height >= 1);
    }
}

#[test]
fn a_page_with_no_usable_size_is_malformed_rather_than_divided_by() {
    for (w, h) in [
        (0.0_f32, 400.0_f32),
        (200.0, 0.0),
        (-1.0, 400.0),
        (f32::NAN, 400.0),
        (f32::INFINITY, 400.0),
    ] {
        assert!(
            matches!(
                fit_into(w, h, Fit::box_of(120, 160)),
                Err(Error::Malformed(_))
            ),
            "{w}x{h} should be malformed"
        );
    }
}

// --- the refusals --------------------------------------------------------------------

#[test]
fn naming_no_page_is_refused_before_the_document_is_opened() {
    let engine = FakeRenderer::with_pages(3);
    assert!(matches!(
        render(
            &engine,
            a_pdf(),
            &[],
            Fit::box_of(120, 160),
            &engine.options(Limits::default())
        ),
        Err(Error::InvalidArgument(_))
    ));
    assert!(engine.asked().is_empty());
}

#[test]
fn a_zero_sided_box_is_refused_before_the_document_is_opened() {
    let engine = FakeRenderer::with_pages(3);
    for fit in [Fit::box_of(0, 160), Fit::box_of(120, 0)] {
        assert!(matches!(
            render(
                &engine,
                a_pdf(),
                &[1],
                fit,
                &engine.options(Limits::default())
            ),
            Err(Error::InvalidArgument(_))
        ));
    }
    assert!(engine.asked().is_empty());
}

#[test]
fn page_zero_is_refused_because_page_numbers_start_at_one() {
    let engine = FakeRenderer::with_pages(3);
    assert!(matches!(
        render(
            &engine,
            a_pdf(),
            &[0],
            Fit::box_of(120, 160),
            &engine.options(Limits::default())
        ),
        Err(Error::InvalidArgument(_))
    ));
}

#[test]
fn a_page_past_the_end_is_refused_and_nothing_is_drawn_first() {
    // HALF A STRIP AND THEN A REFUSAL IS WORSE THAN A REFUSAL: the caller has to decide what
    // to do with the half. Every page number is validated before the first render.
    let engine = FakeRenderer::with_pages(3);
    assert!(matches!(
        render(
            &engine,
            a_pdf(),
            &[1, 2, 4],
            Fit::box_of(120, 160),
            &engine.options(Limits::default())
        ),
        Err(Error::InvalidArgument(_))
    ));
    assert!(
        engine.asked().is_empty(),
        "pages were drawn before the list was validated: {:?}",
        engine.asked()
    );
}

#[test]
fn naming_a_page_twice_is_refused() {
    let engine = FakeRenderer::with_pages(3);
    assert!(matches!(
        render(
            &engine,
            a_pdf(),
            &[2, 2],
            Fit::box_of(120, 160),
            &engine.options(Limits::default())
        ),
        Err(Error::InvalidArgument(_))
    ));
}

// --- the limits ----------------------------------------------------------------------

#[test]
fn a_box_past_max_pixels_is_refused_before_the_document_is_opened() {
    // The request, not the page. This is the check that tells a caller asking for something
    // impossible without an untrusted file having been parsed on their behalf.
    let engine = FakeRenderer::with_pages(3);
    let limits = Limits::with(|l| l.max_pixels = 120 * 160 - 1);
    match render(
        &engine,
        a_pdf(),
        &[1],
        Fit::box_of(120, 160),
        &engine.options(limits),
    )
    .expect_err("a box past the ceiling")
    {
        Error::LimitExceeded {
            limit,
            stage,
            requested,
            allowed,
        } => assert_eq!(
            (limit, stage, requested, allowed),
            ("max_pixels", Stage::Pixels, 19_200, 19_199)
        ),
        other => panic!("expected LimitExceeded at Stage::Pixels, got {other:?}"),
    }
    assert!(engine.asked().is_empty());
}

#[test]
fn naming_more_pages_than_max_pages_is_refused() {
    let engine = FakeRenderer::with_pages(10);
    let limits = Limits::with(|l| l.max_pages = 2);
    assert!(matches!(
        render(
            &engine,
            a_pdf(),
            &[1, 2, 3],
            Fit::box_of(120, 160),
            &engine.options(limits)
        ),
        Err(Error::LimitExceeded {
            stage: Stage::PageCount,
            ..
        })
    ));
}

#[test]
fn a_document_past_max_pages_is_refused_even_when_one_page_is_named() {
    let engine = FakeRenderer::with_pages(10);
    let limits = Limits::with(|l| l.max_pages = 2);
    assert!(matches!(
        render(
            &engine,
            a_pdf(),
            &[1],
            Fit::box_of(120, 160),
            &engine.options(limits)
        ),
        Err(Error::LimitExceeded {
            stage: Stage::PageCount,
            ..
        })
    ));
}

#[test]
fn the_deadline_is_checked_per_page_and_not_only_at_the_ends() {
    // A budget checked only at the ends is a budget a long enough list walks straight past.
    let mut engine = FakeRenderer::with_pages(10);
    engine.elapse_ms = 40;
    let limits = Limits::with(|l| l.max_duration_ms = 100);

    let error = render(
        &engine,
        a_pdf(),
        &[1, 2, 3, 4, 5],
        Fit::box_of(120, 160),
        &engine.options(limits),
    )
    .expect_err("five pages at 40 ms each should not fit in 100 ms");
    assert!(matches!(
        error,
        Error::LimitExceeded {
            stage: Stage::Deadline,
            ..
        }
    ));
    // And it stopped part way rather than drawing all five and then complaining.
    assert!(
        engine.asked().len() < 5,
        "the deadline fired only at the end: {:?}",
        engine.asked()
    );
}

// --- what the operation does not take the engine's word for ---------------------------

#[test]
fn an_engine_that_returns_a_raster_of_the_wrong_size_is_refused() {
    // The one shape a length check alone cannot see: a raster of the right LENGTH for the
    // wrong dimensions. 160x80 and 80x160 are both 51,200 pixels.
    let mut engine = FakeRenderer::with_pages(1);
    engine.lies_about_size = Some((160, 80));
    assert!(matches!(
        render(
            &engine,
            a_pdf(),
            &[1],
            Fit::box_of(120, 160),
            &engine.options(Limits::default())
        ),
        Err(Error::Internal(_))
    ));
}

// --- the session ---------------------------------------------------------------------

#[test]
fn a_strip_knows_how_many_pages_it_will_produce_before_it_draws_any() {
    let engine = FakeRenderer::with_pages(9);
    let options = engine.options(Limits::default());
    let strip = super::begin(
        &engine,
        a_pdf(),
        &[2, 4, 6],
        Fit::box_of(120, 160),
        &options,
    )
    .expect("a well-formed request");
    assert_eq!(strip.pages(), 3);
    assert!(
        engine.asked().is_empty(),
        "beginning a strip must draw nothing"
    );
}

#[test]
fn a_strip_draws_one_page_per_call_and_then_ends() {
    let engine = FakeRenderer::with_pages(9);
    let options = engine.options(Limits::default());
    let mut strip = super::begin(&engine, a_pdf(), &[2, 4], Fit::box_of(120, 160), &options)
        .expect("a well-formed request");

    assert_eq!(engine.asked().len(), 0);
    assert_eq!(strip.next(&engine, &options).unwrap().unwrap().page, 2);
    // ONE BITMAP AT A TIME: after the first call the engine has been asked exactly once.
    assert_eq!(engine.asked().len(), 1);
    assert_eq!(strip.next(&engine, &options).unwrap().unwrap().page, 4);
    assert_eq!(engine.asked().len(), 2);
    assert!(strip.next(&engine, &options).is_none());
    assert!(
        strip.next(&engine, &options).is_none(),
        "and it stays ended"
    );
}

#[test]
fn the_ceilings_a_strip_renders_under_are_the_ones_it_began_with() {
    // A caller that could hand in looser limits on the second page would have a ceiling that
    // holds only for as long as nobody tries -- the shape `DocumentCompressor::compress`
    // already refuses for the limits a document was opened under.
    //
    // It cannot be demonstrated by making the strip FAIL, and that is worth saying: a fitted
    // page never has more pixels than the box, and `begin` already checked the box, so the
    // per-page check can never fire on a strip that started. What a test can see is which
    // number reached the engine.
    let engine = FakeRenderer::with_pages(4);
    let begun = engine.options(Limits::with(|l| l.max_pixels = 120 * 160));
    let mut strip = super::begin(&engine, a_pdf(), &[1, 2], Fit::box_of(120, 160), &begun)
        .expect("the box is exactly at the ceiling");

    assert!(strip.next(&engine, &begun).unwrap().is_ok());

    // A second, far looser set handed in for the next page. It must not take effect.
    let loose = engine.options(Limits::with(|l| l.max_pixels = 1_000_000));
    assert!(strip.next(&engine, &loose).unwrap().is_ok());

    assert_eq!(
        engine
            .asked()
            .iter()
            .map(|a| a.max_pixels)
            .collect::<Vec<_>>(),
        vec![120 * 160, 120 * 160],
        "the second page was rendered under the limits handed in after the strip began"
    );
}

#[test]
fn a_failure_part_way_through_leaves_the_pages_already_drawn_untouched() {
    // THE OPPOSITE RULE TO `split`, deliberately. A split is a partition, so a subset of the
    // parts is not a partition of anything and the caller must discard. A strip is a set of
    // independent pictures: the ones that arrived are still true pictures of their pages.
    let mut engine = FakeRenderer::with_pages(5);
    engine.elapse_ms = 60;
    let options = engine.options(Limits::with(|l| l.max_duration_ms = 100));
    let mut strip = super::begin(
        &engine,
        a_pdf(),
        &[1, 2, 3],
        Fit::box_of(120, 160),
        &options,
    )
    .expect("a well-formed request");

    let mut kept = Vec::new();
    let mut ended = None;
    while let Some(result) = strip.next(&engine, &options) {
        match result {
            Ok(rendered) => kept.push(rendered),
            Err(error) => {
                ended = Some(error);
                break;
            }
        }
    }

    assert!(
        !kept.is_empty() && kept.len() < 3,
        "expected a partial strip, got {} of 3",
        kept.len()
    );
    // WHICH failure ended it, not merely that one did. `take_while(is_ok)` threw the error
    // away, so this passed for an `Internal` from an unrelated bug just as happily.
    assert!(
        matches!(
            ended,
            Some(Error::LimitExceeded {
                stage: Stage::Deadline,
                ..
            })
        ),
        "the strip ended for the wrong reason: {ended:?}"
    );
    // AND THE PAGES THAT ARRIVED ARE THE RIGHT PAGES. The length invariant is `Raster::new`'s
    // and holds by construction, so asserting it here measured nothing; the fake writes the
    // page index into every byte, which is the only thing that can tell one page's pixels from
    // another's.
    assert_eq!(kept[0].page, 1);
    assert_eq!(kept[0].raster.rgba[0], 0, "page 1 is index 0 at the seam");
}

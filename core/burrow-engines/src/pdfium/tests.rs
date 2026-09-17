//! Unit tests for the PDFium engine: the happy path, and one per error variant.
//!
//! The purely-arithmetic parts of the mapping live beside the code they test, in
//! `errors.rs` and `estimate.rs` — including the one that says a zero error code can
//! never read as success. What is here needs a real engine call.

use crate::minimal_pdf;

use std::sync::Arc;

use burrow_types::{Clock, Error, Limits, ManualClock, Password, Result, Stage};

use super::{Pdfium, PdfiumDocument};
use crate::{DocumentEngine, OpenOptions};

/// Limits generous enough that nothing here trips one by accident.
fn generous() -> Limits {
    Limits::default()
}

/// A stopped clock, so nothing here depends on how busy the machine is.
fn stopped() -> Arc<dyn Clock> {
    Arc::new(ManualClock::new(0))
}

fn open(bytes: Vec<u8>) -> Result<PdfiumDocument> {
    Pdfium::new().open(
        bytes.into_boxed_slice(),
        &OpenOptions::new(generous(), stopped()),
    )
}

fn open_with_password(bytes: Vec<u8>, password: &Password) -> Result<PdfiumDocument> {
    let mut options = OpenOptions::new(generous(), stopped());
    options.password = Some(password);
    Pdfium::new().open(bytes.into_boxed_slice(), &options)
}

#[test]
fn the_engine_names_itself() {
    assert_eq!(Pdfium::new().name(), "pdfium");
}

#[test]
fn a_valid_document_opens_and_reports_its_pages() {
    let doc = open(minimal_pdf::pdf_with_pages(3)).expect("a generated 3-page pdf should open");
    assert_eq!(doc.pages_at_open(), 3);
    assert_eq!(
        Pdfium::new()
            .page_count(&doc)
            .expect("page_count should succeed"),
        3
    );
}

#[test]
fn a_document_handle_is_send_and_sync_without_any_unsafe_impl() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<PdfiumDocument>();
}

#[test]
fn bytes_that_are_not_a_pdf_are_malformed() {
    assert!(matches!(
        open(minimal_pdf::not_a_pdf()),
        Err(Error::Malformed(_))
    ));
}

#[test]
fn a_truncated_pdf_is_malformed() {
    assert!(matches!(
        open(minimal_pdf::truncated_pdf()),
        Err(Error::Malformed(_))
    ));
}

#[test]
fn empty_input_is_malformed_rather_than_a_zero_page_success() {
    match open(Vec::new()) {
        Err(Error::Malformed(_)) => {}
        other => panic!("expected Malformed, got {other:?}"),
    }
}

#[test]
fn a_pdf_with_an_empty_page_tree_is_malformed_not_an_empty_success() {
    // The case ADR 0006 requirement 6 is really about: "opened fine, zero pages" must not
    // be a thing burrow can return.
    match open(minimal_pdf::pdf_with_no_pages()) {
        Err(Error::Malformed(_)) => {}
        other => panic!("expected Malformed for a zero-page document, got {other:?}"),
    }
}

#[test]
fn an_encrypted_document_with_no_password_needs_one() {
    match open(minimal_pdf::pdf_encrypted_with_unusable_credentials()) {
        Err(Error::PasswordRequired) => {}
        other => panic!("expected PasswordRequired, got {other:?}"),
    }
}

#[test]
fn an_encrypted_document_with_the_wrong_password_still_needs_one() {
    // Exercises the path that builds and passes the NUL-terminated password buffer.
    let password = Password::new(b"not the password");
    match open_with_password(
        minimal_pdf::pdf_encrypted_with_unusable_credentials(),
        &password,
    ) {
        Err(Error::PasswordRequired) => {}
        other => panic!("expected PasswordRequired, got {other:?}"),
    }
}

#[test]
fn a_password_containing_a_nul_is_rejected_without_echoing_it() {
    let password = Password::new(b"before\0after");
    let error = open_with_password(minimal_pdf::pdf_with_pages(1), &password)
        .expect_err("a NUL in the password should be rejected");
    assert!(
        matches!(error, Error::InvalidArgument(_)),
        "expected InvalidArgument, got {error:?}"
    );
    let rendered = error.to_string();
    for leaked in ["before", "after"] {
        assert!(
            !rendered.contains(leaked),
            "{leaked:?} leaked into {rendered:?}"
        );
    }
}

#[test]
fn an_oversized_input_is_rejected_before_the_engine_sees_it() {
    let limits = Limits::with(|l| l.max_input_bytes = 16);
    let bytes = minimal_pdf::pdf_with_pages(1);
    let requested = u64::try_from(bytes.len()).unwrap();
    match Pdfium::new().open(
        bytes.into_boxed_slice(),
        &OpenOptions::new(limits, stopped()),
    ) {
        Err(Error::LimitExceeded {
            limit,
            stage,
            requested: r,
            allowed,
        }) => {
            assert_eq!(limit, "max_input_bytes");
            assert_eq!(stage, Stage::InputSize);
            assert_eq!(r, requested);
            assert_eq!(allowed, 16);
        }
        other => panic!("expected LimitExceeded, got {other:?}"),
    }
}

#[test]
fn too_many_pages_is_rejected_and_no_handle_is_returned() {
    let limits = Limits::with(|l| l.max_pages = 2);
    match Pdfium::new().open(
        minimal_pdf::pdf_with_pages(5).into_boxed_slice(),
        &OpenOptions::new(limits, stopped()),
    ) {
        Err(Error::LimitExceeded {
            limit,
            stage,
            requested,
            allowed,
        }) => {
            assert_eq!(limit, "max_pages");
            assert_eq!(stage, Stage::PageCount);
            assert_eq!(requested, 5);
            assert_eq!(allowed, 2);
        }
        other => panic!("expected LimitExceeded, got {other:?}"),
    }
}

#[test]
fn a_tight_memory_estimate_rejects_before_loading() {
    let limits = Limits::with(|l| l.max_memory_bytes = 1);
    match Pdfium::new().open(
        minimal_pdf::pdf_with_pages(1).into_boxed_slice(),
        &OpenOptions::new(limits, stopped()),
    ) {
        Err(Error::LimitExceeded { limit, .. }) => assert_eq!(limit, "max_memory_bytes"),
        other => panic!("expected LimitExceeded, got {other:?}"),
    }
}

#[test]
fn a_spent_budget_stops_the_next_call_without_waiting() {
    let clock = Arc::new(ManualClock::new(0));
    let limits = Limits::with(|l| l.max_duration_ms = 10);

    let doc = Pdfium::new()
        .open(
            minimal_pdf::pdf_with_pages(2).into_boxed_slice(),
            &OpenOptions::new(limits, Arc::clone(&clock) as Arc<dyn Clock>),
        )
        .expect("the document should open inside its budget");

    clock.advance(11);

    match Pdfium::new().page_count(&doc) {
        Err(Error::LimitExceeded { limit, .. }) => assert_eq!(limit, "max_duration_ms"),
        other => panic!("expected LimitExceeded, got {other:?}"),
    }
}

#[test]
fn the_engine_is_not_poisoned_by_any_of_the_errors_above() {
    // Every failure here is a normal outcome. If one of them had poisoned the engine, the
    // whole suite after it would fail with `Internal` -- so assert it directly rather
    // than discovering it as a cascade.
    let _ = open(minimal_pdf::not_a_pdf());
    let _ = open(Vec::new());
    let _ = open(minimal_pdf::pdf_encrypted_with_unusable_credentials());
    assert!(
        !super::thread::is_poisoned(),
        "a malformed or encrypted file poisoned the engine"
    );
    assert!(open(minimal_pdf::pdf_with_pages(1)).is_ok());
}

// ---------------------------------------------------------------------------------
// Rendering (#57, ADR 0027). The first capability here that produces pixels.
// ---------------------------------------------------------------------------------

use crate::{PageRenderer, Raster};

/// A deadline over the stopped clock, as `PageRenderer::render` takes.
fn render_options(limits: Limits) -> OpenOptions<'static> {
    OpenOptions::new(limits, stopped())
}

fn render_at(doc: &PdfiumDocument, index: u64, w: u32, h: u32, limits: Limits) -> Result<Raster> {
    let options = render_options(limits);
    let deadline = burrow_types::Deadline::start(options.clock.as_ref(), &options.limits);
    PageRenderer::render(&Pdfium::new(), doc, index, w, h, &options, &deadline)
}

#[test]
fn a_page_renders_at_exactly_the_size_that_was_asked_for() {
    let doc = open(crate::minimal_pdf::pdf_with_ink()).expect("the ink fixture should open");
    let raster = render_at(&doc, 0, 40, 80, generous()).expect("a page should render");

    assert_eq!(raster.width, 40);
    assert_eq!(raster.height, 80);
    // THE LENGTH IS A DECISION, NOT A FACT THE FILE STATED, which is why it is asserted
    // rather than described. ADR 0027 section 4.
    assert_eq!(raster.rgba.len(), 40 * 80 * 4);
}

#[test]
fn the_ink_lands_where_the_page_puts_it_and_the_rest_is_opaque_white() {
    // The fixture is black over the top-left quarter and nothing elsewhere. Four probes,
    // one per quadrant, which is the smallest set that can tell apart a correct render, a
    // vertical flip, a horizontal mirror, and a quarter turn.
    let doc = open(crate::minimal_pdf::pdf_with_ink()).expect("the ink fixture should open");
    let raster = render_at(&doc, 0, 40, 80, generous()).expect("a page should render");

    let pixel = |x: usize, y: usize| -> [u8; 4] {
        let at = (y * 40 + x) * 4;
        [
            raster.rgba[at],
            raster.rgba[at + 1],
            raster.rgba[at + 2],
            raster.rgba[at + 3],
        ]
    };

    assert_eq!(pixel(10, 20), [0, 0, 0, 255], "top left should be inked");
    assert_eq!(pixel(30, 20), [255, 255, 255, 255], "top right should not");
    assert_eq!(
        pixel(10, 60),
        [255, 255, 255, 255],
        "bottom left should not"
    );
    assert_eq!(
        pixel(30, 60),
        [255, 255, 255, 255],
        "bottom right should not"
    );
}

#[test]
fn every_pixel_of_a_blank_page_is_opaque_white_rather_than_whatever_was_in_the_heap() {
    // This is the `FPDFBitmap_FillRect` test. A fresh bitmap's contents are UNDEFINED, so
    // without the fill this would be uninitialised engine heap in a picture handed to the
    // page -- and it would usually be zeros, which is why it needs asserting rather than
    // looking at. The alpha byte is the other half: `FPDFBitmap_BGRx` would leave it
    // undefined too, which is why `BITMAP_BGRA` is not `0`.
    let doc = open(crate::minimal_pdf::pdf_with_pages(1)).expect("a blank page should open");
    let raster = render_at(&doc, 0, 16, 16, generous()).expect("a blank page should render");
    assert!(
        raster
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [255, 255, 255, 255]),
        "a blank page should render as opaque white everywhere"
    );
}

#[test]
fn rendering_the_same_page_twice_produces_the_same_bytes() {
    let doc = open(crate::minimal_pdf::pdf_with_ink()).expect("the ink fixture should open");
    let first = render_at(&doc, 0, 24, 48, generous()).expect("first render");
    let second = render_at(&doc, 0, 24, 48, generous()).expect("second render");
    assert_eq!(first, second);
}

#[test]
fn a_render_one_pixel_past_max_pixels_is_refused_at_the_pixels_stage() {
    let doc = open(crate::minimal_pdf::pdf_with_ink()).expect("the ink fixture should open");

    // At the boundary: accepted.
    let at = Limits::with(|l| l.max_pixels = 40 * 80);
    assert!(render_at(&doc, 0, 40, 80, at).is_ok());

    // One past it: refused, and refused BEFORE anything was allocated.
    let past = Limits::with(|l| l.max_pixels = 40 * 80 - 1);
    match render_at(&doc, 0, 40, 80, past).expect_err("one pixel past the ceiling") {
        Error::LimitExceeded {
            limit,
            stage,
            requested,
            allowed,
        } => {
            assert_eq!(limit, "max_pixels");
            assert_eq!(stage, Stage::Pixels);
            assert_eq!(requested, 3200);
            assert_eq!(allowed, 3199);
        }
        other => panic!("expected LimitExceeded at Stage::Pixels, got {other:?}"),
    }
}

#[test]
fn a_page_past_the_end_is_an_invalid_argument_and_not_a_malformed_document() {
    let doc = open(crate::minimal_pdf::pdf_with_ink()).expect("the ink fixture should open");
    assert!(matches!(
        render_at(&doc, 1, 8, 8, generous()),
        Err(Error::InvalidArgument(_))
    ));
    assert!(matches!(
        PageRenderer::page_size(&Pdfium::new(), &doc, 1),
        Err(Error::InvalidArgument(_))
    ));
}

#[test]
fn a_zero_dimension_is_refused_before_the_engine_is_asked_for_a_page() {
    let doc = open(crate::minimal_pdf::pdf_with_ink()).expect("the ink fixture should open");
    assert!(matches!(
        render_at(&doc, 0, 0, 8, generous()),
        Err(Error::InvalidArgument(_))
    ));
}

#[test]
fn a_page_reports_its_size_in_points() {
    let doc = open(crate::minimal_pdf::pdf_with_ink()).expect("the ink fixture should open");
    let (width, height) =
        PageRenderer::page_size(&Pdfium::new(), &doc, 0).expect("a page should report its size");
    assert!((width - 200.0).abs() < 0.01, "width was {width}");
    assert!((height - 400.0).abs() < 0.01, "height was {height}");
}

#[test]
fn a_spent_budget_stops_a_render_before_it_starts() {
    let clock = Arc::new(ManualClock::new(0));
    let mut options = OpenOptions::new(
        Limits::with(|l| l.max_duration_ms = 10),
        Arc::clone(&clock) as Arc<dyn Clock>,
    );
    options.password = None;
    let doc = Pdfium::new()
        .open(
            crate::minimal_pdf::pdf_with_ink().into_boxed_slice(),
            &options,
        )
        .expect("the ink fixture should open");

    let deadline = burrow_types::Deadline::start(options.clock.as_ref(), &options.limits);
    clock.advance(11);
    assert!(matches!(
        PageRenderer::render(&Pdfium::new(), &doc, 0, 8, 8, &options, &deadline),
        Err(Error::LimitExceeded {
            stage: Stage::Deadline,
            ..
        })
    ));
}

#[test]
fn the_engine_is_still_usable_after_every_render_refusal_above() {
    // The same assertion `the_engine_is_not_poisoned_by_any_of_the_errors_above` makes for
    // the open path. A refused render is an ORDINARY outcome (ADR 0027) and must not cost
    // the engine -- on the web the equivalent is that it must not cost a worker.
    assert!(!super::thread::is_poisoned());
    let doc = open(crate::minimal_pdf::pdf_with_ink()).expect("the ink fixture should open");
    assert!(render_at(&doc, 0, 8, 8, generous()).is_ok());
}

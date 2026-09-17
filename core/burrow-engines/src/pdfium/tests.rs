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

// ---------------------------------------------------------------------------------
// THE SLICE-GRANULARITY MEASUREMENT (#103, ADR 0027's 2026-09-17 amendment).
//
// `#[ignore]`d: it needs fixtures that are generated rather than committed, and on the
// 10 M-path input it runs for minutes. Run it deliberately:
//
//   BURROW_SLICE_FIXTURES=<dir> cargo test -p burrow-engines --features native-engines \
//     --lib progressive_render_slice -- --ignored --nocapture
//
// The acceptance bar was written down BEFORE this existed, in the scratchpad's
// ACCEPTANCE.md, because a bar set after the numbers is not a bar.
// ---------------------------------------------------------------------------------

/// Render `bytes` progressively at `width` x `height`, returning each slice's duration and
/// the process RSS after it.
///
/// Returns `(durations_ms, rss_after_bytes, state)` where `state` is the terminal
/// `FPDF_RENDER_*` value.
#[cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
#[allow(
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    reason = "a measurement harness printing MiB from a resident-set reading"
)]
fn slice_profile(
    bytes: Vec<u8>,
    width: i32,
    height: i32,
) -> (Vec<f64>, Vec<Option<u64>>, i32, u64) {
    use std::time::Instant;

    let rss_at_entry = crate::rss::resident_bytes().unwrap_or(0);
    let doc = open(bytes).expect("the adversarial fixture should open");
    let id = doc.id;
    let rss_after_open = crate::rss::resident_bytes().unwrap_or(0);

    super::thread::submit(move |registry| {
        let handle = registry.handle(id)?;
        // SAFETY: the document is open in the registry; engine thread.
        let loading = std::time::Instant::now();
        let page = unsafe { super::ffi::load_page(handle, 0) };
        let load_ms = loading.elapsed().as_secs_f64() * 1000.0;
        assert!(!page.is_null(), "the fixture's page should load");
        let rss_after_load = crate::rss::resident_bytes().unwrap_or(0);

        // WHERE THE MEMORY GOES, BROKEN DOWN, because "the first slice grew 760 MiB" was an
        // artefact of one baseline spanning three phases. A 0.5 ms slice cannot allocate
        // 760 MiB, and noticing that is what sent this back for a second look.
        println!(
            "  phases    open +{:.1} MiB   load_page +{:.1} MiB ({load_ms:.0} ms)",
            (rss_after_open as i64 - rss_at_entry as i64) as f64 / 1_048_576.0,
            (rss_after_load as i64 - rss_after_open as i64) as f64 / 1_048_576.0,
        );

        // SAFETY: engine thread; dimensions are plain integers.
        let bitmap = unsafe { super::ffi::create_bitmap(width, height, super::ffi::BITMAP_BGRA) };
        assert!(!bitmap.is_null(), "a thumbnail bitmap should allocate");
        // SAFETY: the bitmap is live and the extent is its own.
        unsafe { super::ffi::fill_bitmap(bitmap, width, height, super::ffi::OPAQUE_WHITE) };

        // PINNED FOR THE WHOLE RENDER. PDFium keeps this pointer until `render_page_close`,
        // so it must not move; a `Box` gives it a stable address that outlives every call.
        let mut pause = Box::new(super::ffi::IfsdkPause::always());
        let pause_ptr: *mut super::ffi::IfsdkPause = &mut *pause;

        let mut durations = Vec::new();
        let mut rss: Vec<Option<u64>> = Vec::new();

        let started = Instant::now();
        // SAFETY: both handles are live, `pause` outlives the render, engine thread.
        let mut state =
            unsafe { super::ffi::render_page_start(bitmap, page, width, height, pause_ptr) };
        durations.push(started.elapsed().as_secs_f64() * 1000.0);
        rss.push(crate::rss::resident_bytes());

        // A CEILING ON THE LOOP, so a mechanism that never finishes is a failed measurement
        // rather than a hung test.
        let mut guard = 0_u32;
        while state == super::ffi::FPDF_RENDER_TOBECONTINUED && guard < 5_000_000 {
            guard += 1;
            let at = Instant::now();
            // SAFETY: the page has a render in progress, with this same `pause`.
            state = unsafe { super::ffi::render_page_continue(page, pause_ptr) };
            durations.push(at.elapsed().as_secs_f64() * 1000.0);
            rss.push(crate::rss::resident_bytes());
        }

        // SAFETY: a render was started on this page; once, before the page is closed.
        unsafe { super::ffi::render_page_close(page) };
        // SAFETY: the bitmap is live and this is its only destroy.
        unsafe { super::ffi::destroy_bitmap(bitmap) };
        // SAFETY: the page is live, this is its only close, the document is still open.
        unsafe { super::ffi::close_page(page) };

        drop(pause);
        Ok((durations, rss, state, rss_after_load))
    })
    .expect("the engine should answer")
}

#[test]
#[ignore = "needs generated fixtures and runs for minutes; see the module comment"]
#[cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
#[allow(
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    reason = "a measurement harness printing MiB and percentiles from integer samples"
)]
fn progressive_render_slice_granularity() {
    let dir = match std::env::var("BURROW_SLICE_FIXTURES") {
        Ok(d) => std::path::PathBuf::from(d),
        Err(_) => panic!("set BURROW_SLICE_FIXTURES to the directory holding paths-*m.pdf"),
    };

    // 240x320: the thumbnail size ADR 0027 chose, at DPR 2. The pathological case at the
    // SMALLEST output is the honest test -- if a slice is seconds long here, it is not the
    // raster that costs, it is the content stream.
    for name in ["paths-3m.pdf", "paths-10m.pdf"] {
        let path = dir.join(name);
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        println!("\n=== {name} ===");
        let whole = std::time::Instant::now();
        let (durations, rss, state, baseline) = slice_profile(bytes, 240, 320);
        let total_ms = whole.elapsed().as_secs_f64() * 1000.0;

        let mut sorted = durations.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).expect("no NaN"));
        let pick = |q: f64| sorted[((sorted.len() as f64 - 1.0) * q) as usize];
        // DELTAS BETWEEN CONSECUTIVE *SUCCESSFUL* READS ONLY.
        //
        // The first version used `resident_bytes().unwrap_or(0)`, so a procfs read that failed
        // became a zero -- and the delta after it was the whole resident set, which came out as
        // a "worst per-slice growth" of 759 MiB in a 0.5 ms slice. That is arithmetically
        // impossible and it is what made the measurement worth rechecking rather than
        // reporting. Failures are now counted and skipped.
        let failed = rss.iter().filter(|r| r.is_none()).count();
        let mut worst_growth: i64 = 0;
        let mut prev = baseline;
        for sample in rss.iter().flatten() {
            worst_growth = worst_growth.max(*sample as i64 - prev as i64);
            prev = *sample;
        }
        let peak = rss.iter().flatten().copied().max().unwrap_or(0);
        // THE FIRST SLICE ON ITS OWN, because it is the design-relevant one: if
        // `FPDF_RenderPageBitmap_Start` parses the content stream and builds the display list
        // before it yields, then the allocation happens BEFORE any checkpoint and progressive
        // render does not bound memory however fine the later slices are.
        let first_growth = rss
            .first()
            .and_then(|r| *r)
            .map_or(0, |r| r as i64 - baseline as i64);

        println!(
            "  terminal state      {state} ({})",
            if state == super::ffi::FPDF_RENDER_DONE {
                "DONE -- the page finished"
            } else {
                "NOT DONE -- the render did not complete, so the numbers below are partial"
            }
        );
        // THE NULL RESULT, NAMED. One slice means PDFium never yielded: the callback is not
        // consulted at a useful granularity, and that is a FAIL however fast the render was.
        // It is the outcome most likely to read as success, because everything still works.
        println!(
            "  slices              {}{}",
            durations.len(),
            if durations.len() <= 1 {
                "   <-- NEVER PAUSED: not a pause mechanism"
            } else {
                ""
            }
        );
        println!("  total               {total_ms:.0} ms");
        println!(
            "  slice ms  p50 {:.1}  p90 {:.1}  p99 {:.1}  MAX {:.1}",
            pick(0.50),
            pick(0.90),
            pick(0.99),
            sorted.last().copied().unwrap_or(0.0)
        );
        println!(
            "  rss       peak {:.0} MiB   worst per-slice growth {:.1} MiB   \
             first slice {:.1} MiB   failed reads {failed}",
            peak as f64 / 1_048_576.0,
            worst_growth as f64 / 1_048_576.0,
            first_growth as f64 / 1_048_576.0
        );
    }
}

#[test]
fn a_deadline_that_comes_due_mid_render_ends_it_without_finishing_the_page() {
    // THE POINT OF PROGRESSIVE RENDER, AND THE ONLY TEST THAT CAN SEE IT.
    //
    // With a single `FPDF_RenderPageBitmap` there is no checkpoint inside the call, so a
    // deadline could only be noticed AFTER the whole page was drawn -- on a hostile page that
    // is minutes, and the only thing that could end it was the caller terminating the instance.
    //
    // THE FIRST VERSION OF THIS TEST DID NOT MEASURE THAT. It asserted a `LimitExceeded` and an
    // elapsed time under ten seconds, and it PASSED with the loop's checkpoint deleted -- the
    // refusal simply came from the checkpoint after the render instead of the one inside it.
    // Found by making that deletion and re-running, which is the only thing that separates a
    // test of this mechanism from a test that a deadline exists.
    //
    // So it is SELF-CALIBRATING: the same page is rendered twice, once to completion and once
    // against a deadline, and the interrupted one has to come back in a fraction of the time.
    // No absolute duration appears, so nothing here depends on how fast the machine is.
    //
    // A REAL CLOCK, deliberately. A stopped `ManualClock` cannot make a deadline come due, and
    // a fake that advanced on a schedule would be a fixture converging on cue rather than a
    // render being interrupted.
    let bytes = crate::minimal_pdf::pdf_with_paths(400_000);
    let clock: Arc<dyn Clock> = Arc::new(burrow_types::SystemClock::new());

    // 1. The whole page, so the comparison has a denominator this machine produced.
    let generous_limits = Limits::with(|l| l.max_duration_ms = 600_000);
    let generous_options = OpenOptions::new(generous_limits, Arc::clone(&clock));
    let whole_doc = DocumentEngine::open(
        &Pdfium::new(),
        bytes.clone().into_boxed_slice(),
        &generous_options,
    )
    .expect("a many-path page should open");
    let whole_deadline = burrow_types::Deadline::start(clock.as_ref(), &generous_limits);
    let at = std::time::Instant::now();
    PageRenderer::render(
        &Pdfium::new(),
        &whole_doc,
        0,
        240,
        320,
        &generous_options,
        &whole_deadline,
    )
    .expect("the page should render given time");
    let full = at.elapsed();

    // 2. The same page, with a deadline that comes due almost immediately.
    //
    // OPENED UNDER THE GENEROUS BUDGET AND RENDERED UNDER THE TIGHT ONE. Opening under 50 ms
    // made this flake: a 400,000-path document takes longer than that to parse on a machine
    // running the rest of the suite in parallel, so the OPEN hit the deadline and the test
    // failed before it reached the thing it is about. The budget under test belongs to the
    // render, and `render` takes its own `Deadline`, so it can be exactly that.
    let limits = Limits::with(|l| l.max_duration_ms = 50);
    let options = OpenOptions::new(limits, Arc::clone(&clock));
    let doc = DocumentEngine::open(&Pdfium::new(), bytes.into_boxed_slice(), &generous_options)
        .expect("a many-path page should open");
    let deadline = burrow_types::Deadline::start(clock.as_ref(), &limits);
    let at = std::time::Instant::now();
    let outcome = PageRenderer::render(&Pdfium::new(), &doc, 0, 240, 320, &options, &deadline);
    let interrupted = at.elapsed();

    assert!(
        matches!(
            outcome,
            Err(Error::LimitExceeded {
                stage: Stage::Deadline,
                ..
            })
        ),
        "expected a deadline refusal, got {outcome:?}"
    );

    // HALF, which is generous: with the checkpoint inside the loop the interrupted run is
    // dominated by `FPDF_LoadPage` -- the part nothing can interrupt -- and comes back in
    // roughly a fifth of the full time. Without it the two are the same run, because the
    // refusal waits for the page to finish being drawn.
    assert!(
        interrupted * 2 < full,
        "interrupted in {interrupted:?} against a full render of {full:?}: the refusal waited \
         for the page to be drawn, so the checkpoint is not inside the render"
    );
}

#[test]
fn a_render_interrupted_by_a_deadline_leaves_the_engine_usable() {
    // A REFUSAL IS AN ORDINARY OUTCOME AND MUST NOT COST THE ENGINE. The progressive path has
    // two extra ways to get this wrong that the one-shot call did not: a `render_page_close`
    // skipped on the refusal path leaks PDFium's progressive context onto the page, and a
    // `pause` dropped while PDFium still holds the pointer is a dangling callback.
    //
    // IT INTERRUPTS A RENDER ITSELF RATHER THAN RELYING ON ANOTHER TEST HAVING RUN. The first
    // version asserted `!is_poisoned()` and rendered a fresh document, which passes with the
    // interrupting test deleted -- cargo does not order tests, so it was asserting that a
    // process which had done nothing was not poisoned. Found by code review.
    let bytes = crate::minimal_pdf::pdf_with_paths(400_000);
    let clock: Arc<dyn Clock> = Arc::new(burrow_types::SystemClock::new());
    // OPENED GENEROUSLY, RENDERED TIGHTLY. See the note in the test above: a 50 ms budget on
    // the open makes this a test of how busy the machine is.
    let open_options = OpenOptions::new(
        Limits::with(|l| l.max_duration_ms = 600_000),
        Arc::clone(&clock),
    );
    let limits = Limits::with(|l| l.max_duration_ms = 50);
    let options = OpenOptions::new(limits, Arc::clone(&clock));
    let doc = DocumentEngine::open(&Pdfium::new(), bytes.into_boxed_slice(), &open_options)
        .expect("a many-path page should open");
    let deadline = burrow_types::Deadline::start(clock.as_ref(), &limits);
    let interrupted = PageRenderer::render(&Pdfium::new(), &doc, 0, 240, 320, &options, &deadline);
    assert!(
        matches!(
            interrupted,
            Err(Error::LimitExceeded {
                stage: Stage::Deadline,
                ..
            })
        ),
        "the render should have been interrupted, got {interrupted:?}"
    );

    // AND THE ENGINE IS STILL THE ENGINE.
    assert!(!super::thread::is_poisoned());
    let fresh = open(crate::minimal_pdf::pdf_with_ink()).expect("the ink fixture should open");
    let raster = render_at(&fresh, 0, 40, 80, generous()).expect("a page should still render");
    let at = (20 * 40 + 10) * 4;
    assert_eq!(
        &raster.rgba[at..at + 4],
        &[0, 0, 0, 255],
        "the ink should still land where the page puts it"
    );
}

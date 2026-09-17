//! Fuzz the render path — PDFium's page load, the bitmap, the stride, and the pixel ceiling.
//!
//! ```text
//! # from fuzz/
//! LD_LIBRARY_PATH=../engines/vendor/native-$(uname -m)/lib ASAN_OPTIONS=detect_leaks=0 \
//!   cargo +nightly fuzz run render -- \
//!     -max_total_time=60 -timeout=10 -rss_limit_mb=4096
//! ```
//!
//! # What this reaches that no other target does
//!
//! Every other target here drives **qpdf**. `document_open` is the only other one that reaches
//! PDFium at all, and it stops at open-and-count. This is the first that goes past it:
//!
//! - **`FPDF_LoadPage` on an attacker-built page object.** A `/Type /Page` whose `/Contents`
//!   is a string, whose `/Resources` points at itself, whose `/MediaBox` is four nulls. Open
//!   and count never touch a page's own dictionary; loading one parses all of it.
//! - **The rasteriser, on file-controlled content streams**, now driven through PDFium's
//!   PROGRESSIVE API -- `FPDF_RenderPageBitmap_Start`, `_Continue` and `_Close`, with an
//!   `IFSDK_PAUSE` whose callback always pauses. That is new C++ surface this target reaches
//!   and nothing else does, including the terminal-state mapping and the pause interface's
//!   own lifetime. Behind it are PDFium's interpreter, its font code, its image decoders and
//!   its colour management — which
//!   is, by a wide margin, the largest body of C++ any input in this project reaches. HarfBuzz,
//!   ICU, lcms and OpenJPEG are all behind this one call (ADR 0010).
//! - **The stride and the copy out.** `bgra_to_rgba` reads `stride * height` bytes out of a
//!   buffer PDFium sized. A page that made the engine report a stride narrower than a row, or
//!   a bitmap shorter than its own geometry, is a read past the end of an engine allocation —
//!   and under ASan that is a finding rather than a plausible picture.
//! - **`max_pixels`, at a boundary the input chooses.** The box comes from the input's own
//!   bytes, so the ceiling is exercised from both sides rather than only from below.
//!
//! # What is asserted, and the one ceiling that is not exercised
//!
//! Errors are the expected outcome. A panic, a hang, an abort, or a **limit escape** is a bug:
//! a successful render must be exactly `width * height * 4` bytes and must not exceed the box.
//!
//! `max_duration_ms` is **not** exercised, for the reason every target here says: the clock is
//! a `ManualClock` that never advances, so no input can reach a deadline, and a clock that
//! moved on its own would turn every slow input into a `LimitExceeded` and hide what the input
//! was really doing. A hang is caught by libFuzzer's `-timeout`.
//!
//! **`max_pixels` is the one ceiling this target can drive from above**, and it is the reason
//! the box is taken from the input rather than fixed: a target that always asked for 120x160
//! would never reach the refusal, and the refusal is the newest code here.

#![no_main]

use std::sync::Arc;

use burrow_engines::OpenOptions;
use burrow_engines::pdfium::Pdfium;
use burrow_ops::{Fit, render};
use burrow_types::{Clock, Error, Limits, ManualClock};
use libfuzzer_sys::fuzz_target;

/// Tight limits, so a limit escape is a wrong answer rather than an uninteresting crash.
///
/// `max_pixels` is **512 x 512**, far below `Limits::DEFAULT`'s 256 Mpx. Two reasons, and the
/// second is the one that matters: a fuzzer must not spend its budget allocating 800 MB
/// bitmaps, and a ceiling the input can actually reach is a ceiling this target can test.
fn limits() -> Limits {
    Limits::with(|l| {
        l.max_input_bytes = 8 * 1024 * 1024;
        l.max_memory_bytes = 256 * 1024 * 1024;
        l.max_pages = 512;
        l.max_pixels = 512 * 512;
    })
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 4 {
        return;
    }

    // The box, from the input. 1..=1021 in each direction, so the product straddles the
    // 512x512 ceiling above: some requests are accepted and some are refused at
    // `Stage::Pixels`, which is the boundary worth steering.
    let box_width = 1 + u32::from(data[0]) * 4;
    let box_height = 1 + u32::from(data[1]) * 4;

    // ONE TO FOUR PAGES, never zero. Rendering is orders of magnitude more expensive than
    // anything else fuzzed here, so the count is small -- but it must have no dead value.
    //
    // IT WAS `data[2] % 5` WITH A `wanted == 0` RETURN, AND THAT MADE EVERY SEEDED INPUT A
    // NO-OP. `tools/seed-fuzz-corpus.py` writes a constant prefix byte of 5, and `5 % 5` is
    // zero -- so all twenty seeds returned before PDFium was opened, and the "runs clean for
    // 60s against a seeded corpus" line in the definition of done was hollow for this target.
    // It is the same failure CLAUDE.md records for the unseeded corpus, arriving through the
    // PARAMETER byte instead of through the document. Found by code review.
    //
    // The control is in the seeder: `LIVE_REQUESTS` decodes this expression the way this line
    // does and refuses a prefix byte that produces a request the target discards. A dead value
    // here is now a failing check rather than a green run.
    let wanted = 1 + usize::from(data[2] % 4);
    let document = &data[3..];
    if document.is_empty() {
        return;
    }

    // Page numbers taken modulo a small range, for the reason `rotate`'s target records: a
    // candidate PDF starts `%PDF`, so numbers derived raw from its bytes are all large and
    // every request is refused before an engine is touched. One case in eight is left raw to
    // keep the refusal path live.
    let raw = data[2] & 0x80 != 0;
    let mut pages: Vec<u64> = Vec::with_capacity(wanted);
    for i in 0..wanted {
        let byte = u64::from(document.get(i).copied().unwrap_or(1));
        pages.push(if raw { byte } else { 1 + byte % 8 });
    }

    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(0));
    let options = OpenOptions::new(limits(), clock);
    let engine = Pdfium::new();

    match render(
        &engine,
        document.to_vec().into_boxed_slice(),
        &pages,
        Fit::box_of(box_width, box_height),
        &options,
    ) {
        Ok(strip) => {
            assert_eq!(
                strip.len(),
                pages.len(),
                "a render returned a different number of pages than it was asked for"
            );
            for rendered in &strip {
                let (w, h) = (rendered.raster.width, rendered.raster.height);

                // THE LENGTH IS A DECISION, NOT A FACT THE FILE STATED, which is why it is the
                // assertion worth making. `Raster::new` checks it, and this is the input-driven
                // proof that the check is reached on every path rather than only in tests.
                assert_eq!(
                    rendered.raster.rgba.len(),
                    (w as usize) * (h as usize) * 4,
                    "a raster is not the size it claims to be"
                );

                // THE CEILING DID NOT ESCAPE. `Fit` promises a maximum, and a page fitted into
                // a box may never exceed it -- rounding included, which is the case the
                // clamping in `fit_into` exists for.
                assert!(w <= box_width && h <= box_height, "a raster exceeded its box");
                assert!(w >= 1 && h >= 1, "a raster has a zero dimension");
                assert!(
                    u64::from(w) * u64::from(h) <= limits().max_pixels,
                    "a raster escaped max_pixels"
                );
                assert!(
                    rendered.page >= 1 && rendered.page <= 8 || raw,
                    "a page number came back that was never asked for"
                );
            }
        }
        // EVERY TYPED FAILURE IS AN ACCEPTABLE OUTCOME. `Internal` included: it is the variant
        // an engine contradicting itself produces, and this target's job is to find an input
        // that makes PDFium do something worse than say so.
        Err(
            Error::Malformed(_)
            | Error::Unsupported(_)
            | Error::PasswordRequired
            | Error::LimitExceeded { .. }
            | Error::InvalidArgument(_)
            | Error::Io(_)
            | Error::Internal(_),
        ) => {}
        Err(other) => panic!("unexpected error variant: {other:?}"),
    }
});

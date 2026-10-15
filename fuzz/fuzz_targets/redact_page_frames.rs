//! Fuzz `page_frames`, the read-only entry point the redaction page asks first (#136).
//!
//! ```bash
//! # from fuzz/
//! LD_LIBRARY_PATH=../engines/vendor/native-$(uname -m)/lib ASAN_OPTIONS=detect_leaks=0 \
//!   cargo +nightly fuzz run redact_page_frames -- \
//!     -max_total_time=60 -timeout=10 -rss_limit_mb=2048
//! ```
//!
//! # Why a target of its own
//!
//! It takes a whole document from the web worker and reads every page's dictionary and the page
//! tree above it -- the frame reader, the `/Resources` climb and, natively, PDFium's page size for
//! an inherited `/MediaBox` -- over every page, where the redaction targets read one page of a
//! generated document. The input is the document, so the seeds are whole fixtures
//! (`tools/seed-fuzz-corpus.py`, prefix 0).
//!
//! # The claims
//!
//! 1. **No panic, no hang, no limit escaped**, through `burrow_ops::redact::page_frames`, which
//!    also holds the engine's answer to its shape.
//! 2. **One reading per page**, never more than `max_pages`.
//! 3. **Every frame is positive, finite and at most `MAX_FRAME_SIDE` a side**, and every refusal
//!    names the rule its own error carries -- asserted here again, so a regression in the
//!    operation's own check is still caught.
//! 4. **The web engine is not here.** Its Rust is the same body over a JS bridge; the native-backed
//!    differential (`qpdf::page_frames_tests`) holds the two together over the corpus.

#![no_main]

use std::sync::{Arc, Once};

use burrow_engines::OpenOptions;
use burrow_engines::qpdf::Qpdf;
use burrow_engines::redact::frames::{FrameRule, MAX_FRAME_SIDE, PageFrameReading};
use burrow_types::{Limits, ManualClock};
use libfuzzer_sys::fuzz_target;

/// Tight limits, so a limit escape shows up as a wrong answer rather than as an out-of-memory.
fn limits() -> Limits {
    Limits::with(|l| {
        l.max_input_bytes = 8 * 1024 * 1024;
        l.max_memory_bytes = 256 * 1024 * 1024;
        l.max_pages = 512;
        l.max_duration_ms = 5_000;
    })
}

/// Make any panic fatal, so libFuzzer sees it: PDFium's engine thread catches panics, which is
/// right for production and would hide one here (see `document_open`).
fn make_panics_fatal() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            previous(info);
            std::process::abort();
        }));
    });
}

fuzz_target!(|data: &[u8]| {
    make_panics_fatal();
    let limits = limits();
    let options = OpenOptions::new(limits, Arc::new(ManualClock::new(0)));
    let Ok(readings) = burrow_ops::redact::page_frames(&Qpdf::new(), data, &options) else {
        // Any typed error is a legal outcome; that one was returned rather than a panic is the
        // property.
        return;
    };
    assert!(!readings.is_empty(), "a document that opened has a page");
    assert!(
        u64::try_from(readings.len()).is_ok_and(|n| n <= limits.max_pages),
        "{} readings past a ceiling of {}",
        readings.len(),
        limits.max_pages
    );
    for reading in &readings {
        match reading {
            PageFrameReading::Frame { width, height } => assert!(
                [*width, *height]
                    .iter()
                    .all(|side| side.is_finite() && *side > 0.0 && *side <= MAX_FRAME_SIDE),
                "a frame that is not a positive finite size: {width} x {height}"
            ),
            PageFrameReading::Refused { rule, error } => assert_eq!(
                FrameRule::of(error),
                Some(*rule),
                "a refusal that does not name its rule"
            ),
            _ => panic!("a reading this target does not know"),
        }
    }
});

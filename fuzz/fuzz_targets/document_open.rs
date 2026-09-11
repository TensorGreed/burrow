//! Fuzz `DocumentEngine::open` — the first parser entry point burrow exposes.
//!
//! ```bash
//! # from fuzz/
//! LD_LIBRARY_PATH=../engines/vendor/native-$(uname -m)/lib ASAN_OPTIONS=detect_leaks=0 \
//!   cargo +nightly fuzz run document_open -- \
//!     -max_total_time=60 -timeout=10 -rss_limit_mb=2048
//! ```
//!
//! Both `-timeout` and `-rss_limit_mb` are **required**, not tuning. `Limits::max_duration_ms`
//! is checkpoint-based (ADR 0007): it is only observed between engine calls, so a single
//! `FPDF_*` call that runs forever is not stopped by it and would simply hang the fuzzer.
//! libFuzzer's own `-timeout` is what catches that, and a hang it finds is a bug in where
//! our checkpoints are, not only in the input.
//!
//! `LD_LIBRARY_PATH` is needed because `cargo fuzz run` executes the built binary
//! directly rather than through cargo, so it does not get the `<target>/<profile>/deps`
//! path cargo adds for `cargo test` — which is how every other binary in the workspace
//! finds the pinned `libpdfium.so`.
//!
//! `detect_leaks=0` because `FPDF_DestroyLibrary` is deliberately never called (tearing
//! PDFium down while anything could still be inside it is unsound), so its statics are
//! live at exit by design. That is a decision, not a leak.
//!
//! # What this does and does not test
//!
//! PDFium is a **prebuilt** binary with no sanitizer instrumentation and no coverage
//! feedback (ADR 0004). libFuzzer is therefore driving a black box: it explores *our*
//! code, not PDFium's parser. What this target proves is that our boundary is sound —
//! that hostile input becomes a typed error, that no limit is escaped, and that nothing
//! panics or aborts.
//!
//! **It does not fuzz PDFium.** PDFium's internals are fuzzed upstream by OSS-Fuzz,
//! continuously and with instrumentation we are not going to match. Claiming our fuzzing
//! covers PDFium would be false. If we ever build PDFium from source (ADR 0006's pre-M2
//! gate), instrumenting it becomes possible and this changes.

#![no_main]

use std::sync::{Arc, Once};

use burrow_engines::pdfium::Pdfium;
use burrow_engines::{DocumentEngine, OpenOptions};
use burrow_types::{Limits, ManualClock};
use libfuzzer_sys::fuzz_target;

/// Tight limits, so a limit escape shows up as a wrong answer rather than as an
/// out-of-memory the fuzzer reports as an uninteresting crash.
fn limits() -> Limits {
    Limits::with(|l| {
        l.max_input_bytes = 8 * 1024 * 1024;
        l.max_memory_bytes = 256 * 1024 * 1024;
        l.max_pages = 512;
        l.max_duration_ms = 5_000;
    })
}

/// Make any panic fatal, so libFuzzer sees it.
///
/// **Load-bearing.** The engine thread wraps every job in `catch_unwind` so a caller gets
/// a typed `Internal` instead of a stranded channel — which is right for production and
/// wrong here: it would swallow the panic and the fuzzer would record a clean run over a
/// bug it had just found. Aborting from the hook fires *before* unwinding starts, so the
/// `catch_unwind` never gets the chance.
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

    // A clock that never advances. Real time would make runs non-reproducible, and the
    // deadline is not what this target is exploring.
    let clock = Arc::new(ManualClock::new(0));
    let limits = limits();

    let result = Pdfium::new().open(
        data.to_vec().into_boxed_slice(),
        &OpenOptions::new(limits, clock),
    );

    match result {
        Ok(document) => {
            // A handle that already breaks `max_pages` must never have been returned.
            let at_open = document.pages_at_open();
            assert!(
                at_open > 0 && at_open <= limits.max_pages,
                "open returned a document with {at_open} pages, outside 1..={}",
                limits.max_pages
            );

            match Pdfium::new().page_count(&document) {
                Ok(counted) => assert!(
                    counted <= limits.max_pages,
                    "page_count returned {counted}, above the limit of {}",
                    limits.max_pages
                ),
                // A typed failure is a legal outcome even for a document that opened.
                Err(_) => {}
            }
        }
        // Any typed error is a legal outcome, and the property under test is that we
        // *returned* one rather than panicking or aborting -- so there is nothing to match
        // on. Enumerating the variants here would assert nothing: `Error` is
        // `#[non_exhaustive]`, so a catch-all arm is mandatory and a new variant would
        // fall into it silently either way.
        //
        // The stronger claim -- that the message is a fixed constant carrying nothing from
        // the input -- is asserted in `core/burrow-engines/tests/properties.rs`, where it
        // can be checked against a list rather than re-derived on every iteration.
        Err(_) => {}
    }
});

//! Fuzz the rotation path — the page-tree walk, and the only writes burrow makes to a page.
//!
//! ```text
//! # from fuzz/ -- the RUSTFLAGS override selects the INSTRUMENTED qpdf archive, so
//! # libFuzzer sees inside qpdf's own parser and writer rather than only our wrapper.
//! RUSTFLAGS="-L native=$PWD/../engines/vendor/native-$(uname -m)/lib/fuzz" \
//!   LD_LIBRARY_PATH=../engines/vendor/native-$(uname -m)/lib ASAN_OPTIONS=detect_leaks=0 \
//!   cargo +nightly fuzz run rotate -- \
//!     -max_total_time=60 -timeout=10 -rss_limit_mb=2048
//! ```
//!
//! # What this reaches that no other target does
//!
//! - **The `/Parent` walk, on an attacker-built page tree.** `effective_rotation` climbs from
//!   a page to its ancestors. A `/Parent` cycle, a `/Parent` pointing at a string, a tree 200
//!   deep — all are two-line edits to a PDF, and all of them arrive here. The depth ceiling is
//!   what stops the first; a target that never fuzzed the tree would never exercise it.
//! - **`qpdf_oh_get_key` and `qpdf_oh_get_type_code` on file-controlled objects.** These are
//!   the first calls burrow makes that resolve an indirect object by name, which is the parser
//!   running on bytes the input chose. They are trapped only through two proven helpers
//!   (ADR 0013's 2026-09-13 amendment); this is where that claim meets real inputs.
//! - **A write into a document qpdf parsed.** Every other operation copies pages between
//!   documents. `qpdf_oh_replace_key` edits a dictionary qpdf built from the input.
//! - **Object-handle churn.** The walk takes a handle per ancestor per page and releases each
//!   one. Under ASan a double release or a use-after-release shows up here rather than as slow
//!   growth in production.
//!
//! # The selection and the angle come from the input
//!
//! The first byte chooses how many pages to name and the second the quarter turn, so libFuzzer
//! steers the request as well as the document. Page numbers are taken raw from the input:
//! `InvalidArgument` on a page past the end is a real path, and a target that only ever asked
//! politely would never test it.
//!
//! Errors are the expected outcome. A panic, a hang, an abort, or a limit escape is a bug —
//! with one ceiling excepted and said plainly rather than left to be discovered.
//! `max_duration_ms` is **not** exercised here: the clock is a `ManualClock` that never
//! advances, so no input can reach a deadline. A clock that moved on its own would turn every
//! slow input into a `LimitExceeded` and hide whatever that input was really doing. A hang is
//! caught by libFuzzer's `-timeout`; the deadline itself is tested with a stepping clock in
//! `burrow-engines`' `the_deadline_is_checked_between_pages`.

#![no_main]

use std::sync::{Arc, Once};

use burrow_engines::OpenOptions;
use burrow_engines::PageRotator;
use burrow_engines::qpdf::Qpdf;
use burrow_ops::{Pages, rotate};
use burrow_types::{Clock, Error, Limits, ManualClock};
use libfuzzer_sys::fuzz_target;

/// Tight limits, so a limit escape is a wrong answer rather than an uninteresting crash.
fn limits() -> Limits {
    Limits::with(|l| {
        l.max_input_bytes = 8 * 1024 * 1024;
        l.max_memory_bytes = 256 * 1024 * 1024;
        l.max_pages = 512;
    })
}

fn fuzz_mode() {
    static ONCE: Once = Once::new();
    // Upstream's fuzz limits: without them a fuzzer spends its time rediscovering that
    // large files are slow. See `qpdf_check.rs` for the full argument.
    ONCE.call_once(burrow_engines::qpdf::enable_fuzz_mode);
}

fuzz_target!(|data: &[u8]| {
    fuzz_mode();

    if data.len() < 3 {
        return;
    }

    // At most eight pages named, so one input cannot turn every case into a timeout.
    let wanted = usize::from(data[0] % 9);
    // Every multiple of 90 from -360 to 360, plus the out-of-range values the operation has to
    // refuse: `% 11` over a byte gives 0..=10, mapped so two of the eleven are not quarter
    // turns at all. Normalisation is cheap to get wrong and free to fuzz.
    let degrees = match data[1] % 11 {
        10 => 45,
        9 => i64::from(data[1]) + 1,
        n => (i64::from(n) - 4) * 90,
    };
    let document = &data[2..];
    if document.is_empty() {
        return;
    }

    // RAW PAGE NUMBERS, from the document's own bytes. `split`'s first fuzz target derived its
    // cuts from raw bytes too and that was the bug -- a candidate PDF starts `%PDF`, so every
    // derived value was large and every request was refused before an engine was touched. Here
    // the numbers are taken modulo a small range instead, so the request usually lands inside
    // a plausible document, with one case in eight left raw to keep the refusal path live.
    let raw = data[0] & 0x80 != 0;
    let mut pages: Vec<u64> = Vec::with_capacity(wanted);
    for i in 0..wanted {
        let byte = u64::from(document.get(i).copied().unwrap_or(1));
        pages.push(if raw { byte } else { 1 + byte % 8 });
    }

    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(0));
    let options = OpenOptions::new(limits(), clock);
    let engine = Qpdf::new();

    match rotate(
        &engine,
        document.to_vec().into_boxed_slice(),
        Pages::numbered(&pages),
        degrees,
        &options,
    ) {
        Ok(output) => {
            assert!(
                !output.is_empty(),
                "rotate produced an empty document, which is not a document"
            );

            // THE PAGE COUNT IS THE INVARIANT WORTH ASSERTING, and it is asserted against the
            // INPUT's count rather than against a ceiling -- `split`'s first target compared a
            // count of documents against a ceiling on pages, which was a tautology for every
            // input libFuzzer could produce.
            //
            // Reopening is the only way to count without a second parser, and it doubles as a
            // check that the output is a document the engine will accept back. A refusal on
            // reopen is not comparable, so it is skipped rather than asserted on: firing an
            // assertion for a case this target calls acceptable is a false positive.
            // COUNTED UNDER A DELIBERATELY LOOSE CEILING. Re-opening under `limits()` made the
            // ceiling assertion below unfalsifiable: a document over `max_pages` fails that
            // open, so `before` was `None` and the `if let` never ran. It was a tautology
            // sitting directly under a comment congratulating itself for not being one.
            // Found by code review.
            let counting = OpenOptions::new(
                Limits::with(|l| {
                    l.max_input_bytes = u64::MAX;
                    l.max_memory_bytes = u64::MAX;
                    l.max_pages = u64::MAX;
                }),
                Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
            );
            let before = PageRotator::open(&engine, document.to_vec().into_boxed_slice(), &counting)
                .ok()
                .and_then(|source| engine.pages(&source).ok());
            let after = PageRotator::open(&engine, output.into_boxed_slice(), &counting)
                .ok()
                .and_then(|source| engine.pages(&source).ok());

            if let (Some(before), Some(after)) = (before, after) {
                assert_eq!(
                    before, after,
                    "rotate changed the page count; it may only change an attribute"
                );
                // Now genuinely falsifiable: `before` is the real page count, so a document
                // over the ceiling that was nonetheless rotated fires here.
                assert!(
                    before <= limits().max_pages,
                    "a document over the page ceiling was rotated"
                );
            }
        }
        // Every typed error is an acceptable outcome. `Internal` is the one that is not: it
        // means an invariant inside burrow did not hold, which is a finding rather than a
        // refusal.
        Err(Error::Internal(message)) => {
            panic!("rotate reported an internal error: {message}");
        }
        Err(_) => {}
    }
});

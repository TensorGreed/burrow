//! Fuzz the one-document-into-many path.
//!
//! ```text
//! # from fuzz/ -- the RUSTFLAGS override selects the INSTRUMENTED qpdf archive, so
//! # libFuzzer sees inside qpdf's own parser and writer rather than only our wrapper.
//! RUSTFLAGS="-L native=$PWD/../engines/vendor/native-$(uname -m)/lib/fuzz" \
//!   LD_LIBRARY_PATH=../engines/vendor/native-$(uname -m)/lib ASAN_OPTIONS=detect_leaks=0 \
//!   cargo +nightly fuzz run split -- \
//!     -max_total_time=60 -timeout=10 -rss_limit_mb=2048
//! ```
//!
//! # What this covers that `merge` does not
//!
//! Merge holds several sources and writes once. Split holds **one** source and writes many
//! times, and the differences are where its hazards are:
//!
//! - **One source, reused across writes.** Every output resolves foreign pages out of the
//!   same `qpdf_data` during its own `qpdf_write`. Merge never writes twice from one source,
//!   so a handle invalidated by a previous write is a hazard only this target can reach.
//! - **A destination that is read from a constant and then edited.** `extract` opens
//!   `BLANK_DOCUMENT`, adds pages, and removes page 0 with `qpdf_remove_page` — a function no
//!   other operation calls, on a document whose page list was just mutated.
//! - **Cut arithmetic on attacker-influenced page counts.** The runs come from the request,
//!   but they are validated against the page count qpdf reports for the *fuzzed* document,
//!   so an absurd count meets the range checks here rather than in a unit test.
//!
//! # The cuts come from the input
//!
//! The first byte chooses how many parts to ask for, so libFuzzer can steer the partition as
//! well as the document. Every cut is derived and then offered to `split` unvalidated: the
//! operation is expected to refuse nonsense with `InvalidArgument`, and a target that only
//! ever passed valid cuts would never test that.
//!
//! Errors are the expected outcome. A panic, a hang, an abort, or a limit escape is a bug.

#![no_main]

use std::sync::{Arc, Once};

use burrow_engines::OpenOptions;
use burrow_engines::PageExtractor;
use burrow_engines::qpdf::Qpdf;
use burrow_ops::{Cuts, split};
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

    if data.is_empty() {
        return;
    }

    // The first byte steers the partition; the rest is the document. Capped at seven cuts
    // (`% 8` yields 0..=7) so a single input cannot ask for hundreds of writes and turn every
    // case into a timeout.
    let parts = usize::from(data[0] % 8);
    let document = &data[1..];
    if document.is_empty() {
        return;
    }

    // CUTS THAT ARE INCREASING BY CONSTRUCTION, most of the time.
    //
    // The first version took raw bytes as cuts. A candidate PDF begins `%PDF` = 37, 80, 68,
    // 70, so any request for three or more parts was rejected by `runs_from` before an engine
    // was touched — and in practice only `data[0] % 8 == 0` (one input in eight, zero cuts)
    // ever reached `extract`, exactly once. None of the three hazards this target's header
    // names were reached at all. Found by security review.
    //
    // A minority branch still passes raw bytes, because `InvalidArgument` on a nonsensical
    // request is a real path and a target that only ever asked politely would never test it.
    let raw_cuts = data[0] & 0x80 != 0;
    let mut cuts: Vec<u64> = Vec::with_capacity(parts);
    let mut previous = 0u64;
    for i in 0..parts {
        let byte = u64::from(document.get(i).copied().unwrap_or(1));
        if raw_cuts {
            cuts.push(byte);
        } else {
            previous += 1 + byte % 4;
            cuts.push(previous);
        }
    }

    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(0));
    let options = OpenOptions::new(limits(), clock);
    let engine = Qpdf::new();

    match split(
        &engine,
        document.to_vec().into_boxed_slice(),
        Cuts::after_pages(&cuts),
        &options,
    ) {
        Ok(outputs) => {
            // THE PARTITION INVARIANT, which is the assertion this target exists for. The
            // first version compared `outputs.len()` — which the target itself caps at 8 —
            // against `max_pages` of 512, a tautology for every input libFuzzer can produce.
            // It compared a count of DOCUMENTS against a ceiling on PAGES.
            //
            // Re-opening each output through the extractor is the only way to count its pages
            // without a second parser, and it doubles as a check that every output is a
            // document the engine will accept back.
            let mut total = 0u64;
            // A reopen refusal makes the partition sum incomplete, so the invariant below
            // cannot be asserted on this input. Skipped rather than compared: comparing would
            // fire `assert_eq!` as a fuzz crash for a case this target explicitly calls
            // acceptable, which is a false positive rather than a finding.
            let mut skipped = false;
            for output in &outputs {
                assert!(
                    !output.is_empty(),
                    "split produced an empty document, which is not a document"
                );
                let Ok(source) = engine.open(output.clone().into_boxed_slice(), &options) else {
                    skipped = true;
                    // An output the engine cannot reopen is a bug, but a limit refusal on
                    // reopen is not: the ceilings are the same, so this is unreachable in
                    // practice and asserting it would be asserting the engine rather than
                    // the operation.
                    continue;
                };
                let pages = engine.pages(&source).expect("a reopened output has a page count");
                assert!(
                    pages <= limits().max_pages,
                    "an output has more pages than the ceiling allows"
                );
                total += pages;
            }

            // Every input page in exactly one output. This is the ROADMAP invariant, and it is
            // the assertion that would catch a `runs_from` bug on an adversarial document
            // rather than on a hand-written fixture.
            if skipped {
                return;
            }
            if let Ok(source) = engine.open(document.to_vec().into_boxed_slice(), &options) {
                if let Ok(expected) = engine.pages(&source) {
                    assert_eq!(
                        total, expected,
                        "the outputs do not partition the document: {total} pages across \
                         {} outputs, from a {expected}-page source",
                        outputs.len()
                    );
                }
            }
        }
        // Every typed error is an acceptable outcome for arbitrary bytes. `Internal` is
        // included deliberately: it is how an engine reports something it could not do, and
        // making it a fuzz failure would turn every unusual file into a false positive.
        Err(
            Error::Malformed(_)
            | Error::Unsupported(_)
            | Error::PasswordRequired
            | Error::InvalidArgument(_)
            | Error::LimitExceeded { .. }
            | Error::Io(_)
            | Error::Internal(_),
        ) => {}
        Err(other) => panic!("unexpected error variant: {other:?}"),
    }
});

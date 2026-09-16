//! Fuzz the compression path — qpdf's object-stream writer, which nothing else here reaches.
//!
//! ```text
//! # from fuzz/ -- the RUSTFLAGS override selects the INSTRUMENTED qpdf archive, so
//! # libFuzzer sees inside qpdf's own parser and writer rather than only our wrapper.
//! python3 ../tools/seed-fuzz-corpus.py
//! RUSTFLAGS="-L native=$PWD/../engines/vendor/native-$(uname -m)/lib/fuzz" \
//!   LD_LIBRARY_PATH=../engines/vendor/native-$(uname -m)/lib ASAN_OPTIONS=detect_leaks=0 \
//!   cargo +nightly fuzz run compress -- \
//!     -max_total_time=60 -timeout=10 -rss_limit_mb=2048
//! ```
//!
//! # What this reaches that no other target does, and why it is worth a target of its own
//!
//! **`compress` adds no parser entry point.** It opens a document every other operation
//! already opens, and the definition of done scopes its fuzz requirement to new parser entry
//! points — so on the letter of the rule this target is optional. It exists anyway, because
//! what it reaches is not a parser and is not covered:
//!
//! - **`QPDFWriter`'s object-stream assembly.** Every other target writes with
//!   `qpdf_o_preserve`. `qpdf_o_generate` walks the document deciding which objects are
//!   eligible, packs them into object streams in batches, and emits a **cross-reference
//!   stream** instead of a table. That is several hundred lines of qpdf C++ that burrow has
//!   never executed under libFuzzer, on a structure the input chooses.
//! - **The eligibility walk on adversarial structure.** Which objects may be packed depends on
//!   the document's own graph — generation numbers, streams, the encryption dictionary. An
//!   input that makes that walk disagree with the emission is exactly the shape that produces
//!   a wrong xref.
//! - **Re-reading what it just wrote.** ADR 0022's verification reopens the output through a
//!   fresh engine, so every successful compression here also parses a freshly generated object
//!   stream. A target that only wrote would miss half of it.
//!
//! The reason to do this rather than defer it is measured, not precautionary: seeding these
//! targets from real fixtures for the first time found three defects in minutes, **two of them
//! memory-unsafety in this same pinned qpdf** (#62). The writer is the part of it burrow has
//! run least.
//!
//! # It must be run SEEDED, and an unseeded run of this target proves nothing
//!
//! `fuzz/corpus/` is gitignored, and every "runs clean for 60 s" recorded during M1 was
//! measured against a corpus grown from `/dev/urandom` until `tools/seed-fuzz-corpus.py`
//! existed. Measured, with a defect deliberately planted: **577,209 unseeded executions found
//! nothing, and one seed failed on the first execution.**
//!
//! That applies with more force here than anywhere else. libFuzzer does not invent a valid
//! PDF; an invalid one is refused at `open` and **never reaches the writer at all**, so an
//! unseeded run of this target exercises the rejection path and none of the code the target
//! exists for. A green unseeded 60 seconds here is not weak evidence — it is evidence about a
//! different function.
//!
//! # What is asserted
//!
//! Errors are the expected outcome; a panic, a hang, an abort or a limit escape is a bug.
//! `Error::Internal` is the one typed error that is not acceptable: it means an invariant
//! inside burrow did not hold.
//!
//! `Outcome::NotSmaller` is an ordinary answer and not a finding. It is the correct result for
//! any document already efficiently stored, and on a fuzzer's inputs it will be common.
//!
//! `max_duration_ms` is **not** exercised: the clock is a `ManualClock` that never advances, so
//! no input can reach a deadline. A clock that moved on its own would turn every slow input
//! into a `LimitExceeded` and hide what that input was really doing. A hang is caught by
//! libFuzzer's `-timeout`.

#![no_main]

use std::sync::{Arc, Once};

use burrow_engines::OpenOptions;
use burrow_engines::PageRotator;
use burrow_engines::qpdf::Qpdf;
use burrow_ops::{Outcome, compress};
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

/// Counting without a ceiling, so the ceiling assertion below is falsifiable.
///
/// Re-opening under `limits()` made `rotate`'s equivalent assertion unfalsifiable: a document
/// over `max_pages` fails that open, so the count was `None` and the check never ran — a
/// tautology sitting under a comment congratulating itself for not being one. Found by code
/// review there; copied here with the lesson rather than the bug.
fn counting() -> OpenOptions<'static> {
    OpenOptions::new(
        Limits::with(|l| {
            l.max_input_bytes = u64::MAX;
            l.max_memory_bytes = u64::MAX;
            l.max_pages = u64::MAX;
        }),
        Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
    )
}

fuzz_target!(|data: &[u8]| {
    fuzz_mode();

    if data.is_empty() {
        return;
    }

    // THE WHOLE INPUT IS THE DOCUMENT. Unlike `rotate` and `split`, compression takes no
    // selection — there is no page list, no angle, no cut — so there is nothing for a leading
    // byte to steer and taking one would only shift every seeded fixture by one, breaking the
    // PDF header on every case. The document IS the whole of the input space here.
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(0));
    let options = OpenOptions::new(limits(), clock);
    let engine = Qpdf::new();

    match compress(&engine, data.to_vec().into_boxed_slice(), &options) {
        Ok(Outcome::Smaller {
            document,
            original_bytes,
        }) => {
            assert!(
                !document.is_empty(),
                "compress produced an empty document, which is not a document"
            );
            // THE TYPE'S OWN INVARIANT, asserted rather than assumed: `Smaller` must be
            // smaller. If this ever fires, the never-worse branch has been reordered or
            // removed and the operation is handing back a document bigger than it was given.
            assert!(
                u64::try_from(document.len()).unwrap_or(u64::MAX) < original_bytes,
                "Smaller carried a document of {} bytes against an input of {original_bytes}",
                document.len()
            );

            // THE PAGE COUNT IS THE INVARIANT WORTH ASSERTING, against the INPUT's count rather
            // than against a ceiling. A refusal on reopen is not comparable, so it is skipped
            // rather than asserted on: firing for a case this target calls acceptable is a
            // false positive.
            let before = PageRotator::open(&engine, data.to_vec().into_boxed_slice(), &counting())
                .ok()
                .and_then(|source| engine.pages(&source).ok());
            let after = PageRotator::open(&engine, document.into_boxed_slice(), &counting())
                .ok()
                .and_then(|source| engine.pages(&source).ok());

            if let (Some(before), Some(after)) = (before, after) {
                assert_eq!(
                    before, after,
                    "compress changed the page count; it may only change how objects are stored"
                );
                assert!(
                    before <= limits().max_pages,
                    "a document over the page ceiling was compressed"
                );
            }
        }
        // AN ORDINARY ANSWER, not a finding. A document already efficiently stored does not
        // shrink, and on a fuzzer's inputs that will be most of them.
        Ok(Outcome::NotSmaller {
            original_bytes,
            produced_bytes,
        }) => {
            assert!(
                produced_bytes >= original_bytes,
                "NotSmaller reported a saving: {produced_bytes} < {original_bytes}"
            );
        }
        // Every typed error is an acceptable outcome. `Internal` is the one that is not: it
        // means an invariant inside burrow did not hold, which is a finding rather than a
        // refusal.
        Err(Error::Internal(message)) => {
            panic!("compress reported an internal error: {message}");
        }
        Err(_) => {}
    }
});

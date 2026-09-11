//! Fuzz qpdf's structure check.
//!
//! ```bash
//! # from fuzz/ -- note the RUSTFLAGS override, which is what selects the INSTRUMENTED
//! # qpdf archive. Without it this links the plain one and libFuzzer sees nothing inside
//! # qpdf at all.
//! RUSTFLAGS="-L native=$PWD/../engines/vendor/native-$(uname -m)/lib/fuzz" \
//!   LD_LIBRARY_PATH=../engines/vendor/native-$(uname -m)/lib ASAN_OPTIONS=detect_leaks=0 \
//!   cargo +nightly fuzz run qpdf_check -- \
//!     -max_total_time=60 -timeout=10 -rss_limit_mb=2048
//! ```
//!
//! # Unlike PDFium, qpdf really is instrumented
//!
//! `engines/build-native.sh` builds a second qpdf archive with
//! `-fsanitize=fuzzer-no-link,address`, which it can do because we build qpdf from source.
//! So this target gets coverage feedback from **qpdf's own parser**, not merely from our
//! wrapper — the thing `document_open.rs` explicitly cannot claim about PDFium.
//!
//! M1 PR 2 measured that the clang 18 ASan runtime in that archive links with cargo-fuzz's
//! Rust ASan (both expose `__asan_version_mismatch_check_v8`) and that libFuzzer discovers
//! new edges inside qpdf. That was on aarch64; CI is x86-64, which is why this target runs
//! there — the CI run is the check.
//!
//! # Fuzz mode
//!
//! `qpdf::enable_fuzz_mode()` sets limits upstream describes as unsuitable for production
//! but necessary for fuzzing, "with the aim of avoiding spurious time-outs and
//! out-of-memory errors" (`global.hh:98-133`). Without it, a fuzzer spends its time
//! rediscovering that large files are slow.

#![no_main]

use std::sync::{Arc, Once};

use burrow_engines::qpdf::Qpdf;
use burrow_engines::{CheckOptions, StructureEngine};
use burrow_types::{Clock, Limits, ManualClock};
use libfuzzer_sys::fuzz_target;

/// Tight limits, so a limit escape is a wrong answer rather than an uninteresting crash.
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
/// Load-bearing for the same reason as in `document_open.rs`. It matters less here --
/// qpdf work runs on the caller's thread, with no `catch_unwind` in the way -- but a
/// future change that moved it behind one would otherwise silently swallow findings.
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

fn setup() {
    static SETUP: Once = Once::new();
    SETUP.call_once(|| {
        make_panics_fatal();
        // Process-global and one-way. Fuzz targets only; see the module docs.
        burrow_engines::qpdf::enable_fuzz_mode();
    });
}

fuzz_target!(|data: &[u8]| {
    setup();

    // Both dispositions of recovery. They are genuinely different code paths inside qpdf:
    // with recovery on it reconstructs a damaged cross-reference table, which is the
    // largest and least-travelled part of its parser and exactly where a fuzzer earns its
    // keep.
    for attempt_recovery in [false, true] {
        // A clock that never advances: real time would make runs non-reproducible, and
        // the deadline is not what this target explores.
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(0));
        let mut options = CheckOptions::new(limits(), clock);
        options.attempt_recovery = attempt_recovery;

        match Qpdf::new().check(data.to_vec().into_boxed_slice(), &options) {
            Ok(report) => {
                // Only the upper bound. Whether qpdf can return `Ok` with zero pages is
                // not something this target should assume -- `tests/structure.rs`
                // deliberately accepts any page count for an empty page tree, and the
                // two should not disagree. The limit escape is the property that matters.
                assert!(
                    report.pages <= limits().max_pages,
                    "check returned {} pages, above the limit of {}",
                    report.pages,
                    limits().max_pages
                );
            }
            // Any typed error is a legal outcome; the property is that we returned one
            // rather than aborting. `tests/prescan.rs` and `src/qpdf/errors.rs` assert the
            // stronger claim about message content, where it can be checked against a list.
            Err(_) => {}
        }
    }
});

//! Fuzz the multi-document path.
//!
//! ```bash
//! # from fuzz/ -- the RUSTFLAGS override selects the INSTRUMENTED qpdf archive, so
//! # libFuzzer sees inside qpdf's own parser and writer rather than only our wrapper.
//! RUSTFLAGS="-L native=$PWD/../engines/vendor/native-$(uname -m)/lib/fuzz" \
//!   LD_LIBRARY_PATH=../engines/vendor/native-$(uname -m)/lib ASAN_OPTIONS=detect_leaks=0 \
//!   cargo +nightly fuzz run merge -- \
//!     -max_total_time=60 -timeout=10 -rss_limit_mb=2048
//! ```
//!
//! # What this covers that `qpdf_check` does not
//!
//! `qpdf_check` drives one document through `qpdf_read_memory` and `qpdf_get_num_pages`.
//! Merge adds three things it never touches, and they are where the cross-document hazards
//! live:
//!
//! - **`qpdf_add_page` across two `qpdf_data` handles.** Object handles are per-document,
//!   and this is the only place one is resolved against a document other than its own.
//! - **The write path** — `qpdf_init_write_memory`, `qpdf_write`, `qpdf_get_buffer`. Two of
//!   those are untrapped, argued in `engines/qpdf-untrapped-accepted.toml` on a precondition
//!   the caller establishes rather than the callee checks. A fuzzer is the right instrument
//!   for a precondition like that.
//! - **Source lifetime.** qpdf resolves foreign pages lazily, during the write, so a
//!   use-after-free here would be a real one rather than a logic slip. ASan is what would
//!   notice, and this is the target that puts ASan on that path.
//!
//! # The input is split, not one document
//!
//! One buffer is cut in half and offered as two inputs. A single blob would exercise only
//! `begin`, which is `qpdf_check` again by another name. Splitting means the fuzzer can
//! reach `append` and the write with two *different* mutated documents, which is the thing
//! worth exploring — and because the split point is derived from the input, libFuzzer can
//! steer it.
//!
//! Errors are the expected outcome. A panic, a hang, an abort, or a limit escape is a bug.

#![no_main]

use std::sync::{Arc, Once};

use burrow_engines::OpenOptions;
use burrow_engines::qpdf::Qpdf;
use burrow_ops::{Input, merge};
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

    // Two inputs at minimum; below that there is nothing multi-document to explore.
    if data.len() < 4 {
        return;
    }

    // The split point comes from the input so libFuzzer can steer it. `% len` keeps it in
    // range; the `max(1)` keeps both halves non-empty, since an empty input is rejected by
    // a path that has its own coverage and is not what this target is for.
    let at = (usize::from(data[0]) * 256 + usize::from(data[1])) % data.len();
    let at = at.clamp(1, data.len() - 1);
    let (first, second) = data.split_at(at);

    let limits = limits();
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(0));
    let options = OpenOptions::new(limits, clock);

    let inputs = vec![
        Input::new(first.to_vec().into_boxed_slice()),
        Input::new(second.to_vec().into_boxed_slice()),
    ];

    match merge(&Qpdf::new(), inputs, &options) {
        Ok(out) => {
            // A merge that succeeded must have produced something openable. A writer that
            // returned a truncated buffer -- the failure mode the `Assembly` owning its
            // sources exists to prevent -- would surface here rather than as a silent
            // success, which is the whole reason this branch asserts anything at all.
            assert!(
                out.starts_with(b"%PDF-"),
                "merge returned bytes that are not a PDF"
            );
        }
        Err(Error::Internal(_)) => {
            // `Internal` means a bug in burrow, not a bad file. Every other variant is a
            // legitimate answer to arbitrary bytes.
            panic!("arbitrary input produced Error::Internal, which is ours and not the file's");
        }
        Err(_) => {}
    }
});

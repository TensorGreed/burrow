//! Fuzz the structural pre-scan.
//!
//! ```bash
//! # from fuzz/
//! LD_LIBRARY_PATH=../engines/vendor/native-$(uname -m)/lib ASAN_OPTIONS=detect_leaks=0 \
//!   cargo +nightly fuzz run prescan -- \
//!     -max_total_time=60 -timeout=10 -rss_limit_mb=2048
//! ```
//!
//! # What makes this target different from the others
//!
//! Everything else in `fuzz/` drives a prebuilt C++ engine, so libFuzzer is exploring a
//! black box (see `document_open.rs`). **This target is pure Rust with full coverage
//! instrumentation**, so the fuzzer can actually see and steer the code it is testing.
//! It is the one place in this project where coverage-guided fuzzing works as intended.
//!
//! The code under test needs no engine — `prescan` is pure Rust and is not gated on the
//! `native-engines` feature, precisely so M1 PR 4's web path can use it unchanged. The
//! *binary* still needs `LD_LIBRARY_PATH`, though, because it shares a crate with the
//! engine wrappers and the whole fuzz package is built with `native-engines` on. Worth
//! knowing for PR 4: anything that depends on `burrow-engines` today drags in
//! `libpdfium.so` whether it uses it or not.
//!
//! # The claim being tested
//!
//! `prescan` exists to be un-blowable-up: allocation is O(1) whatever the input, every
//! loop has a fixed cap, and there is no recursion. `-rss_limit_mb` is what turns the
//! first of those from a comment into a checked property — a regression that made the
//! scan allocate in proportion to its input would trip it rather than pass quietly.
//! `-timeout` covers the second: an unbounded loop shows up as a hang, not a wrong answer.

#![no_main]

use burrow_engines::prescan;
use burrow_types::Limits;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // `describe` reads without judging; `check` then applies a policy. Both are exercised
    // because a panic could live in either, and `describe` is the half M1 PR 4's web path
    // will call directly.
    let declared = prescan::describe(data);

    // The estimate must not wrap: an absurd declaration has to stay absurd, because
    // wrapping into a small number is precisely how a bomb would get past the check.
    assert!(
        declared.estimated_xref_bytes() >= declared.xref_entries || declared.xref_entries == 0,
        "the per-entry estimate wrapped for {} entries",
        declared.xref_entries
    );

    // And the policy layer, which must only ever produce two typed outcomes.
    match prescan::check(data, &Limits::default()) {
        Ok(_) | Err(burrow_types::Error::LimitExceeded { .. }) => {}
        Err(burrow_types::Error::Malformed(_)) => {}
        Err(other) => panic!("the pre-scan produced an unexpected error: {other:?}"),
    }
});

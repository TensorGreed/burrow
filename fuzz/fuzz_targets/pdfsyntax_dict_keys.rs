//! Fuzz the dictionary-key reader `split`'s page-key allowlist is built on.
//!
//! ```bash
//! # from fuzz/
//! LD_LIBRARY_PATH=../engines/vendor/native-$(uname -m)/lib ASAN_OPTIONS=detect_leaks=0 \
//!   cargo +nightly fuzz run pdfsyntax_dict_keys -- \
//!     -max_total_time=60 -timeout=10 -rss_limit_mb=2048
//! ```
//!
//! # Why this one carries more weight than its size suggests
//!
//! The input is `qpdf_oh_unparse`'s output for a page dictionary, and the answer decides **which
//! keys survive into a split output**. The two failure directions are not symmetric and the
//! dangerous one is the quiet one:
//!
//! - a key reported that is not there — the caller removes a key that does not exist, which qpdf
//!   ignores;
//! - a key **not** reported that is there — the caller never removes it, so `/B`, `/AA` or
//!   anything else nobody enumerated rides into the output. A leak, out of a check that returned
//!   success.
//!
//! So the module refuses rather than returning a short list, and this target asserts that: every
//! `Ok` is a complete answer or there is no `Ok`. A crash is not the failure being hunted here —
//! a plausible short list is.
//!
//! Seeds come from `tools/seed-fuzz-corpus.py`, which carves balanced `<<` … `>>` spans out of the
//! committed fixtures. A whole PDF is a *useless* seed for this target — it does not begin with
//! `<<`, so it is rejected on the first token and teaches libFuzzer nothing — which is exactly the
//! shape of mistake `fuzz/README.md` records, and why the seeder carves rather than copies.

#![no_main]

use burrow_engines::pdfsyntax::dict::{MAX_KEYS, top_level_keys};
use burrow_types::Error;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    match top_level_keys(data) {
        Ok(keys) => {
            assert!(
                keys.len() <= MAX_KEYS,
                "the key list grew past its own cap instead of refusing: {}",
                keys.len()
            );

            // A COMPLETE ANSWER, OR NO ANSWER. `Ok` means every top-level key is in the list, so
            // the closing `>>` must really have been reached -- and the cheapest independent
            // witness of that is that the input contains one at all. A reader that returned
            // early on an unclosed dictionary would produce a plausible short list here, which
            // is the failure this target exists for and the one a crash detector cannot see.
            assert!(
                data.windows(2).any(|w| w == b">>"),
                "a dictionary with no closing '>>' was read as complete, and a short key list \
                 means keys nobody saw are never removed"
            );
        }
        Err(Error::Malformed(_) | Error::Unsupported(_)) => {}
        Err(other) => panic!("the key reader produced an unexpected error: {other:?}"),
    }
});

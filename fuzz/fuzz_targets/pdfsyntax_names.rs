//! Fuzz the resource-name scan `split`'s pruning filters a `/Resources` dictionary with.
//!
//! ```bash
//! # from fuzz/
//! LD_LIBRARY_PATH=../engines/vendor/native-$(uname -m)/lib ASAN_OPTIONS=detect_leaks=0 \
//!   cargo +nightly fuzz run pdfsyntax_names -- \
//!     -max_total_time=60 -timeout=10 -rss_limit_mb=2048
//! ```
//!
//! # What this target is actually for
//!
//! Like `prescan`, this is **pure Rust with full coverage instrumentation**, so libFuzzer can
//! steer the code rather than poke at a black box. Unlike `prescan`, the input is not a document:
//! it is a **decoded content stream**, which is what `qpdf_oh_get_page_content_data` hands back
//! after inflating whatever was in the file. So the bytes here are as attacker-controlled as a
//! PDF's are, with none of a PDF's structure to constrain them — and by the time they reach this
//! function, qpdf's parser has already agreed to produce them.
//!
//! Seeds come from `tools/seed-fuzz-corpus.py`, which carves real `stream`…`endstream` bodies out
//! of the committed fixtures. Unseeded, this target explores a tokeniser's rejection paths and
//! almost nothing else — the lesson `fuzz/README.md` records and `CLAUDE.md`'s definition of done
//! requires seeding for.
//!
//! # The claims being tested
//!
//! 1. **No panic, hang, or unbounded allocation.** `-rss_limit_mb` is what makes the last of those
//!    a checked property rather than a comment: the set is capped at `MAX_NAMES`, and a regression
//!    that dropped the cap would allocate in proportion to its input and trip it.
//! 2. **Only two error kinds escape.** Anything else means a failure mode nobody classified,
//!    reaching a caller that matches on the kind.
//! 3. **The bounds the module promises actually hold on the returned set** — asserted here rather
//!    than trusted, because they are what stop the set from being the allocation.

#![no_main]

use burrow_engines::pdfsyntax::names::{MAX_NAME_LENGTH, MAX_NAMES, names_in_content};
use burrow_types::Error;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    match names_in_content(data) {
        Ok(names) => {
            // THE CAPS ARE PROPERTIES OF THE ANSWER, not of the code path that built it. A
            // truncated set is the failure this module's rustdoc calls the one that deletes a
            // resource the page draws with, so the ceiling has to be a refusal -- meaning an
            // `Ok` above either ceiling is the bug, not merely a large answer.
            assert!(
                names.len() <= MAX_NAMES,
                "the name set grew past its own cap instead of refusing: {}",
                names.len()
            );
            for name in &names {
                assert!(
                    name.len() <= MAX_NAME_LENGTH,
                    "a name longer than the recorded maximum came back instead of a refusal: {}",
                    name.len()
                );
            }
        }
        // `Malformed` -- the bytes could not be tokenised. `Unsupported` -- past a ceiling.
        Err(Error::Malformed(_) | Error::Unsupported(_)) => {}
        Err(other) => panic!("the name scan produced an unexpected error: {other:?}"),
    }
});

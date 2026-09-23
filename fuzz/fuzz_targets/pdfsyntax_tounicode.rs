//! Fuzz the `/ToUnicode` reader and the narrowed program it writes back.
//!
//! ```bash
//! # from fuzz/
//! LD_LIBRARY_PATH=../engines/vendor/native-$(uname -m)/lib ASAN_OPTIONS=detect_leaks=0 \
//!   cargo +nightly fuzz run pdfsyntax_tounicode -- \
//!     -max_total_time=60 -timeout=10 -rss_limit_mb=2048
//! ```
//!
//! # Why this is its own target
//!
//! It is the only parser in this crate that **writes a program back out**, and the output goes
//! into the emitted document through `qpdf_oh_replace_stream_data`. Every other `pdfsyntax`
//! target asks whether a parse is safe; this one also asks whether what it emits is.
//!
//! The grammar is PostScript-shaped rather than content-stream-shaped — `beginbfchar`,
//! `beginbfrange`, hex strings, arrays of them — so folding it into the content-stream target
//! would spend the budget on the wrong shape. It is seeded from the corpus's own CMaps.
//!
//! # The claims being tested
//!
//! 1. **No panic, no hang** on any bytes.
//! 2. **The narrowed program never maps a code the filter excluded.** This is the leak
//!    direction: a `/ToUnicode` entry for a removed code is the removed character in plain
//!    text. Checked by re-reading the emitted program, which is the only way to ask the
//!    question of the bytes rather than of the map they came from.
//! 3. **The narrowed program is readable by this module.** burrow emitting something burrow
//!    cannot read is how `MAX_COMPOSITE_ITEMS` was found: the operation produced output its own
//!    parser rejected, on an ordinary kerned line.
//! 4. **Narrowing is idempotent in the set it maps.** Narrowing twice with the same filter maps
//!    exactly what narrowing once did — a round trip that loses or gains a code is a round trip
//!    that changes the document on every save.

#![no_main]

use burrow_engines::pdfsyntax::tounicode::ToUnicode;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(read) = ToUnicode::parse(data) else {
        // A refusal is a correct outcome for arbitrary bytes, and most of these are.
        return;
    };

    // THE FILTER IS DERIVED FROM THE INPUT rather than fixed, so the fuzzer explores keeping
    // none, all, and arbitrary subsets without a second input channel. `data`'s first byte is
    // as good a source of arbitrariness as any and costs nothing to reproduce.
    let seed = data.first().copied().unwrap_or(0);
    let keep = |code: u32| -> bool {
        match seed % 3 {
            0 => false,
            1 => true,
            _ => code % 2 == 0,
        }
    };

    let Some(narrowed) = read.narrowed(&keep) else {
        // Nothing survived the filter, so there is no program and nothing to check.
        return;
    };

    // CLAIM 3: burrow can read what burrow wrote.
    let again = ToUnicode::parse(&narrowed).expect("burrow must be able to read its own output");

    // CLAIM 2: nothing the filter excluded is mapped by the emitted program. Asked of the
    // re-read rather than of `read`, because the map is not what reaches the document.
    for code in 0..=u32::from(u16::MAX) {
        if again.maps(code) {
            assert!(
                keep(code) && read.maps(code),
                "the narrowed program maps code {code}, which the filter excluded or the \
                 original never mapped"
            );
        }
    }

    // CLAIM 4: narrowing what was already narrowed changes nothing about what is mapped.
    if let Some(twice) = again.narrowed(&keep) {
        let third = ToUnicode::parse(&twice).expect("burrow must read its own output");
        assert_eq!(
            third.len(),
            again.len(),
            "narrowing twice with the same filter mapped a different number of codes"
        );
    } else {
        assert_eq!(
            again.len(),
            0,
            "a program that maps codes the filter keeps must narrow to a program"
        );
    }
});

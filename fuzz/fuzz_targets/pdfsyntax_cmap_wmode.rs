//! Fuzz the writing-mode derivation — the check that keeps vertical text from being boxed
//! horizontally, which is the direction that misses it.
//!
//! ```bash
//! # from fuzz/
//! LD_LIBRARY_PATH=../engines/vendor/native-$(uname -m)/lib ASAN_OPTIONS=detect_leaks=0 \
//!   cargo +nightly fuzz run pdfsyntax_cmap_wmode -- \
//!     -max_total_time=60 -timeout=10 -rss_limit_mb=2048
//! ```
//!
//! # Why this is its own target
//!
//! The input is a CMap program, which is PostScript-shaped rather than content-stream-shaped:
//! `def`, `usecmap`, `begincmap`, comments, and names the producer chooses. Folding it into the
//! content-stream target would mean libFuzzer spending its budget on the wrong grammar.
//!
//! # The claims being tested
//!
//! 1. **No panic or hang** on any bytes, as either a predefined name or an embedded program.
//! 2. **Never `Ok(Horizontal)` for a program that declares `WMode 1` at top level.** This is
//!    the leak direction and the only asymmetric claim here: refusing too much costs a document,
//!    reporting horizontal for a vertical CMap costs the secret.
//! 3. **The dictionary's `WMode` is honoured or the disagreement is refused** — never silently
//!    overridden in the horizontal direction.

#![no_main]

use burrow_engines::pdfsyntax::geometry::{CMap, WritingMode, writing_mode_of};
use libfuzzer_sys::fuzz_target;

/// Whether a program declares `WMode 1` in a place this target is confident about.
///
/// Deliberately conservative: it looks only for the exact token run at the start of a line or
/// after white space, with no comment or string anywhere in the input. An oracle that tried to
/// match the real scanner would be the scanner written twice, and would assert nothing.
fn plainly_vertical(program: &[u8]) -> bool {
    if program.contains(&b'%') || program.contains(&b'(') || program.contains(&b'<') {
        return false;
    }
    let text = String::from_utf8_lossy(program);
    let mut seen = text.match_indices("/WMode ");
    let Some((_, _)) = seen.next() else {
        return false;
    };
    // Exactly one declaration, and it is 1.
    seen.next().is_none() && text.contains("/WMode 1") && !text.contains("usecmap")
}

fuzz_target!(|data: &[u8]| {
    let Some((&mode, body)) = data.split_first() else {
        return;
    };

    // (1) as a predefined name.
    let _ = writing_mode_of(&CMap::Predefined(body));

    // (1) and (2) as an embedded program.
    let dictionary_wmode = match mode % 4 {
        0 => None,
        1 => Some(0),
        2 => Some(1),
        other => Some(i64::from(other) * 7),
    };
    let cmap = CMap::Embedded {
        dictionary_wmode,
        program: body,
    };
    match writing_mode_of(&cmap) {
        Ok(WritingMode::Horizontal) => {
            // (2) THE ASYMMETRIC CLAIM. Refusing is always allowed; reporting horizontal for a
            // program that plainly says otherwise is the leak.
            assert!(
                !(plainly_vertical(body) && dictionary_wmode != Some(0)),
                "a program declaring 'WMode 1' was read as horizontal: {:?}",
                String::from_utf8_lossy(body)
            );
            // (3) and it did not silently override a dictionary that said vertical.
            assert_ne!(
                dictionary_wmode,
                Some(1),
                "a dictionary declaring 'WMode 1' was overridden to horizontal"
            );
        }
        Ok(WritingMode::Vertical) | Err(_) => {}
    }
});

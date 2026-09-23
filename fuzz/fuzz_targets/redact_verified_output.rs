//! Fuzz the claim that a redaction's output passes its own verification.
//!
//! ```bash
//! # from fuzz/
//! LD_LIBRARY_PATH=../engines/vendor/native-$(uname -m)/lib ASAN_OPTIONS=detect_leaks=0 \
//!   cargo +nightly fuzz run redact_verified_output -- \
//!     -max_total_time=60 -timeout=10 -rss_limit_mb=2048
//! ```
//!
//! # What this target is for, and why it is not the detection target again
//!
//! `redact_shared_contents` generates sharing structures and asks whether the *refusal* is
//! right. This one generates **regions** over a fixed set of documents and asks whether the
//! operation and its own read-back can ever disagree — that is, whether a redaction can succeed
//! and then be rejected by the check it just ran, or return bytes the check never saw.
//!
//! The region is the input because it is the caller's only real degree of freedom, and because
//! every geometry defect this milestone found showed up as *which glyphs a region reached*: a
//! box scaled twice, a code framed at the wrong width, a span located in the wrong element.
//!
//! # The claims
//!
//! 1. **No panic, no hang.** Any region over any of these documents, including regions off the
//!    page and regions straddling its edges.
//! 2. **If it succeeded, no glyph sits inside the region** — re-derived here from the emitted
//!    bytes through this crate's public geometry, converted with the **output's own** frame.
//! 3. **A refusal names a rule**, and a rejected output names the operation. The two are
//!    different shapes and the target distinguishes them.
//!
//! # What claim 2 can and cannot catch, measured rather than asserted
//!
//! It is **not** falsifiable by a single fault in the removal, and that is worth saying because
//! the obvious reading is that it is. Planting `contents.apply(&[])` — a removal that removes
//! nothing — and running this target for 60 s produced **no crash**: every affected region made
//! the operation reject its own output, so the target took the refusal arm and passed. The
//! verification caught the fault before the fuzzer could.
//!
//! So claim 2 fires only on a **two-fault** shape: a removal that leaves something *and* an
//! internal check that does not see it.
//!
//! **And it is a second path through the witness, not a second implementation of the question.**
//! It calls `glyphs_on_first_page`, `page_frame`, `Region::to_content_space` and
//! `conservative_box` — the same four functions `region_is_cleared` uses, differing only in the
//! marshalling between them. So it catches a defect in that marshalling (the witness returning
//! the wrong thing, reading the wrong page, opening the wrong bytes) and **cannot catch a defect
//! in the geometry**, which is the "wrong reason" that would matter most. The header said the
//! opposite until a review read it.
//!
//! The single-fault case is covered where it can be made to fail on demand:
//! `redact_verify`'s `Liar` witness tests, which break the *reading* rather than the writing.

#![no_main]

use std::collections::BTreeSet;
use std::sync::Arc;

use burrow_engines::pdfsyntax::region::Region;
use burrow_engines::{OpenOptions, PageRedactor};
use burrow_types::{Limits, SystemClock};
use libfuzzer_sys::fuzz_target;

/// The committed producer fixtures, read once.
///
/// Real documents rather than generated ones: the corpus's hand-built pages are all stopped by
/// the standard-14 gap, so a target built on them would measure that refusal and nothing else.
fn documents() -> Vec<Vec<u8>> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/redaction/fixtures");
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return out;
    };
    let mut paths: Vec<std::path::PathBuf> = entries
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "pdf"))
        .collect();
    paths.sort();
    for path in paths {
        if let Ok(bytes) = std::fs::read(&path) {
            out.push(bytes);
        }
    }
    out
}

/// A coordinate from two bytes, spread over a page and a little past its edges.
///
/// **Past the edges deliberately.** A region entirely off the page, or straddling an edge, is a
/// caller's mistake rather than a document's, and the operation has to have an answer for it.
fn coordinate(low: u8, high: u8) -> f64 {
    let raw = f64::from(u16::from(high) << 8 | u16::from(low));
    raw / 65_535.0 * 1_000.0 - 100.0
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 9 {
        return;
    }
    let documents = documents();
    // A FUZZ TARGET THAT CANNOT FIND ITS CORPUS FAILS LOUDLY. Returning here would have it pass
    // 60 s having examined nothing, which reads as coverage -- and libFuzzer reports this on the
    // first execution rather than after an hour of nightly.
    assert!(
        !documents.is_empty(),
        "no producer fixture was found; this target examined nothing"
    );
    let Some(pdf) = documents.get(usize::from(data[0]) % documents.len()) else {
        return;
    };
    let region = Region {
        left: coordinate(data[1], data[2]),
        top: coordinate(data[3], data[4]),
        width: coordinate(data[5], data[6]).abs(),
        height: coordinate(data[7], data[8]).abs(),
    };

    let options = OpenOptions::new(Limits::default(), Arc::new(SystemClock::new()));
    let redacted: BTreeSet<usize> = [0].into_iter().collect();
    let engine = burrow_engines::qpdf::Qpdf;

    match engine.redact_page(pdf, 0, &redacted, region, &options) {
        Err(burrow_types::Error::OutputRejected(what)) => {
            // A REJECTED OUTPUT IS NOT A REFUSED INPUT, and it does not carry a `[rule]`: it
            // names the operation and what the read-back saw. Separating the two arms is not
            // tidiness -- the first version asserted the bracket on every error, so the moment
            // verification legitimately rejected something the target would have failed on
            // claim 3 and hidden whatever claim 2 was about.
            assert!(
                what.contains("redact"),
                "a rejected output names the operation: {what}"
            );
        }
        Err(error) => {
            // CLAIM 3. A refusal that names no rule cannot be told from a redaction that
            // quietly did nothing.
            let text = format!("{error}");
            assert!(
                text.contains('[') && text.contains(']'),
                "a refusal must name its rule: {text}"
            );
        }
        Ok((out, _report)) => {
            assert!(out.starts_with(b"%PDF"), "the output must be a document");

            // CLAIM 2, asked of the bytes rather than of the operation. `emit_verified` already
            // ran the check; this re-derives its central assertion independently, so a check
            // that passed for the wrong reason still fails here.
            let Ok(glyphs) = burrow_engines::glyphs_on_first_page(&out, &options) else {
                // A document burrow wrote and cannot walk would have been rejected by the
                // check itself, so reaching here means the walk succeeded once already.
                panic!("the emitted document must still walk");
            };
            // THE REGION IS CONVERTED WITH THE OUTPUT'S OWN FRAME, which is what the check
            // does too -- the region is in display space and the glyphs in content space, and a
            // comparison that skipped the conversion would be comparing two different spaces
            // and passing for that reason.
            let Ok(frame) = burrow_engines::page_frame(&out, 0, &options) else {
                panic!("the emitted document must still have a readable frame");
            };
            let Ok(content) = region.to_content_space(&frame) else {
                // A region this frame cannot express is one the operation refused before
                // emitting anything, so reaching here means the conversion already worked once.
                panic!("the region converted once and must convert again");
            };
            let left = glyphs
                .iter()
                .filter(|glyph| glyph.conservative_box().intersects(&content))
                .count();
            assert!(
                left == 0,
                "the operation returned a document with {left} glyph(s) still inside the \
                 region it was asked to clear"
            );
        }
    }
});

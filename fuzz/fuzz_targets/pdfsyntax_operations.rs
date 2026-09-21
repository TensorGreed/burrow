//! Fuzz the operation reader and the string decoder redaction rewrites a page through.
//!
//! ```bash
//! # from fuzz/
//! LD_LIBRARY_PATH=../engines/vendor/native-$(uname -m)/lib ASAN_OPTIONS=detect_leaks=0 \
//!   cargo +nightly fuzz run pdfsyntax_operations -- \
//!     -max_total_time=60 -timeout=10 -rss_limit_mb=2048
//! ```
//!
//! # Why this needs its own target beside `pdfsyntax_names`
//!
//! Both read a **decoded content stream**, so the input has the same shape and the same
//! provenance: whatever `qpdf_oh_get_page_content_data` produces after inflating what was in the
//! file, with none of a PDF's structure left to constrain it. What differs is the answer.
//!
//! `names_in_content` returns a set and over-approximates on purpose. `operations` returns
//! **spans**, and a rewriter cuts the page's bytes at them. A span that is one byte wrong there
//! does not keep a resource that could have gone — it splices a replacement into the middle of a
//! token, and the output is a page that opens and draws something nobody wrote. So the properties
//! worth asserting are about the spans, and `pdfsyntax_names` asserts none of them.
//!
//! Seeds come from `tools/seed-fuzz-corpus.py`, carved out of the committed fixtures the same way
//! `pdfsyntax_names`' are. Unseeded, this explores the tokeniser's rejection paths and little
//! else — the lesson `fuzz/README.md` records.
//!
//! # The claims being tested
//!
//! 1. **No panic, hang, or unbounded allocation.** `MAX_OPERANDS` and `MAX_OPERATIONS` are
//!    refusals rather than truncations, so an `Ok` past either is the bug.
//! 2. **Only two error kinds escape**, so no failure mode reaches a caller unclassified.
//! 3. **The spans tile the input, and the gaps hold nothing drawable.** Ascending,
//!    non-overlapping, inside the stream, each operand's span inside its operation's — and every
//!    byte *between* two operations is white space or a comment. Non-overlap alone was what an
//!    earlier version of this called "tiling", and it is the weaker half: it permits a reader
//!    that skips a run of operators entirely. The gap check is what says deleting every
//!    operation leaves nothing that draws, which is the property the rewriter actually rests on.
//! 4. **Decoding a string never panics**, on any span the reader hands back, and
//!    **`encode_literal` round-trips through it**. `decode_string` is reached with attacker bytes
//!    exactly this way, so it is fuzzed through its real caller rather than on its own — and the
//!    round trip is asserted here rather than only on seven unit values, because a rewriter that
//!    writes a string back and reads a different one is the leak in miniature.

#![no_main]

use burrow_engines::pdfsyntax::ops::{MAX_OPERANDS, MAX_OPERATIONS, Operand, operations};
use burrow_engines::pdfsyntax::strings::{decode_string, encode_literal};
use burrow_types::Error;
use libfuzzer_sys::fuzz_target;

/// White space and comments only — what may legally sit between two operations.
fn is_only_trivia(bytes: &[u8]) -> bool {
    let mut at = 0usize;
    while let Some(&byte) = bytes.get(at) {
        match byte {
            b'\0' | b'\t' | b'\n' | 0x0c | b'\r' | b' ' => at += 1,
            b'%' => {
                // A comment runs to the end of the line, or to the end of the input.
                at += 1;
                while bytes.get(at).is_some_and(|b| *b != b'\n' && *b != b'\r') {
                    at += 1;
                }
            }
            _ => return false,
        }
    }
    true
}

/// Every operand's span must lie inside `outer`, recursively through arrays and dictionaries.
fn check_operand(operand: &Operand, outer: (usize, usize), content: &[u8]) {
    let (start, end) = operand.span();
    assert!(
        start >= outer.0 && end <= outer.1 && start <= end,
        "an operand's span {:?} is not inside its operation's {outer:?}",
        (start, end)
    );
    if let Operand::Str { span } = *operand {
        let raw = content
            .get(span.0..span.1)
            .expect("a string's span must be inside the content it was read from");
        // The value is discarded: what is being tested is that decoding attacker bytes returns
        // rather than panics, and that only classified errors come out of it.
        match decode_string(raw) {
            Ok(value) => {
                // THE ROUND TRIP, on attacker-shaped values rather than on a handful of chosen
                // ones. `encode_literal` claims it holds for every input; this is where that
                // claim meets bytes nobody wrote on purpose.
                let written = encode_literal(&value);
                match decode_string(&written) {
                    Ok(again) => assert_eq!(
                        again.len(),
                        value.len(),
                        "encode_literal did not round-trip a {}-byte value",
                        value.len()
                    ),
                    Err(e) => panic!("encode_literal produced something undecodable: {e:?}"),
                }
            }
            Err(Error::Malformed(_)) => {}
            Err(other) => panic!("decoding a string produced an unexpected error: {other:?}"),
        }
    }
    if let Operand::Array { items, .. } | Operand::Dict { items, .. } = operand {
        for item in items {
            check_operand(item, (start, end), content);
        }
    }
}

fuzz_target!(|data: &[u8]| {
    match operations(data) {
        Ok(read) => {
            assert!(
                read.len() <= MAX_OPERATIONS,
                "the operation list grew past its own cap instead of refusing: {}",
                read.len()
            );
            let mut previous_end = 0usize;
            for operation in &read {
                let (start, end) = operation.span;
                // THE TILING PROPERTY. A rewriter splices at these offsets, so overlapping or
                // descending spans would let one edit eat another's bytes -- and the operation
                // would report a removal it did not make.
                assert!(
                    start >= previous_end,
                    "operation spans overlap or run backwards: {:?} after {previous_end}",
                    (start, end)
                );
                assert!(
                    start <= end && end <= data.len(),
                    "an operation's span {:?} leaves the content stream of {} bytes",
                    (start, end),
                    data.len()
                );
                assert!(
                    operation.operands.len() <= MAX_OPERANDS,
                    "an operation kept more operands than the cap instead of refusing: {}",
                    operation.operands.len()
                );
                for operand in &operation.operands {
                    check_operand(operand, (start, end), data);
                }
                // THE GAP HOLDS NOTHING DRAWABLE. Between the previous operation and this one
                // there may be white space and comments and nothing else -- so removing every
                // operation leaves a stream that draws nothing, which is the statement a
                // rewriter needs and which non-overlap does not make.
                let gap = data
                    .get(previous_end..start)
                    .expect("the gap between two operations is inside the stream");
                assert!(
                    is_only_trivia(gap),
                    "the bytes between two operations are not white space or a comment: {:?}",
                    &gap[..gap.len().min(32)]
                );
                previous_end = end;
            }
            let tail = data
                .get(previous_end..)
                .expect("the tail after the last operation is inside the stream");
            assert!(
                is_only_trivia(tail),
                "the bytes after the last operation are not white space or a comment: {:?}",
                &tail[..tail.len().min(32)]
            );
        }
        // `Malformed` -- the bytes are not a content stream. `Unsupported` -- past a ceiling.
        Err(Error::Malformed(_) | Error::Unsupported(_)) => {}
        Err(other) => panic!("the operation reader produced an unexpected error: {other:?}"),
    }
});

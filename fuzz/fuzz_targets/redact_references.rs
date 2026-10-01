//! Fuzz the reference lexer redaction asks qpdf about (#227).
//!
//! ```bash
//! # from fuzz/
//! LD_LIBRARY_PATH=../engines/vendor/native-$(uname -m)/lib ASAN_OPTIONS=detect_leaks=0 \
//!   cargo +nightly fuzz run redact_references -- \
//!     -max_total_time=60 -timeout=10 -rss_limit_mb=2048
//! ```
//!
//! # What a wrong answer costs, and which one this hunts
//!
//! Every reference the lexer reports is asked of qpdf, and one qpdf resolves to null refuses the
//! redaction. So the two failure directions are not symmetric:
//!
//! - a reference reported that is not there -- one more lookup, and at worst a refusal;
//! - a reference **not** reported that qpdf parses -- never asked about. If qpdf resolves it to
//!   null and PDFium follows it, the redaction measures a page the viewer does not show and
//!   returns `Ok`. That is the leak.
//!
//! The oracle is narrower than "nothing is missed", and says so. Whatever the input holds, an
//! object written after it must still be read, or the scan must say it stopped (`irregular`), which
//! refuses. Every `obj` starts its own scan, so this holds unless the work budget is spent -- it
//! checks that scans are independent and that running out is reported, not that the lexer agrees
//! with qpdf. Agreement with qpdf is held by the engine tests and the corpus, against qpdf itself;
//! a differential against the engine is not something this pure-Rust target can do.
//!
//! The whole input is the document, so the seeds are whole fixtures (`tools/seed-fuzz-corpus.py`,
//! prefix 0). The same bytes are also read as a decoded object stream, with `/N` and `/First`
//! derived from the input's own leading integer pairs, so an object-stream body -- the seeder writes
//! some -- is read as one, members and all.

#![no_main]

use burrow_engines::pdfsyntax::references::{
    MAX_DISTINCT_REFERENCES, MAX_STREAM_OBJECTS, in_file, in_object_stream,
};
use libfuzzer_sys::fuzz_target;

/// The object appended after every input, and the reference it holds.
const TAIL: &[u8] = b"\n999 0 obj [7 3 R] endobj\n";
const TAIL_REFERENCE: (i32, i32) = (7, 3);

fuzz_target!(|data: &[u8]| {
    let found = in_file(data, &mut || Ok(())).expect("a checkpoint that never fails");
    assert!(found.references.len() <= MAX_DISTINCT_REFERENCES);
    assert!(found.stream_objects.len() <= MAX_STREAM_OBJECTS);
    assert!(
        found.references.iter().all(|(n, g)| *n >= 0 && *g >= 0),
        "a plain reference with a negative number"
    );

    // NOTHING BEFORE AN OBJECT HIDES IT.
    let mut with_tail = data.to_vec();
    with_tail.extend_from_slice(TAIL);
    let tailed = in_file(&with_tail, &mut || Ok(())).expect("a checkpoint that never fails");
    assert!(
        tailed.irregular.is_some() || tailed.references.contains(&TAIL_REFERENCE),
        "an object written after the input was not read, and the scan did not say it stopped: \
         a reference qpdf parses would never be asked about"
    );

    let (count, first) = header_shape(data);
    let members = in_object_stream(data, count, first, &mut || Ok(()))
        .expect("a checkpoint that never fails");
    assert!(members.references.len() <= MAX_DISTINCT_REFERENCES);
    assert!(
        members.stream_objects.is_empty(),
        "an object stream reported a stream object, which cannot be a member"
    );
    // WHAT EACH READING MAY REPORT. Declarations come from headers in the file, which an object
    // stream's members do not have; members come only from an object stream's header.
    assert!(members.declarations.is_empty() && !members.unreadable_header);
    assert!(found.members.is_empty());
});

/// `/N` and `/First` as an object-stream body's own header implies them: the leading run of
/// integers, taken in pairs, and the offset of the first byte after it.
fn header_shape(data: &[u8]) -> (i64, i64) {
    let space = |b: &u8| matches!(*b, b'\0' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r' | b' ');
    let mut at = 0;
    let mut integers = 0_i64;
    loop {
        // White space and comments, as the lexer and qpdf skip them in the header.
        loop {
            while data.get(at).is_some_and(space) {
                at += 1;
            }
            if data.get(at) != Some(&b'%') {
                break;
            }
            while data.get(at).is_some_and(|b| *b != b'\r' && *b != b'\n') {
                at += 1;
            }
        }
        let start = at;
        while data.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
        if at == start || data.get(at).is_some_and(|b| !space(b)) {
            at = start;
            break;
        }
        integers += 1;
    }
    (integers >> 1, i64::try_from(at).unwrap_or(0))
}

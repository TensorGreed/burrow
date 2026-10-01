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
//! So the oracle is about the second. Whatever the input holds, an object written after it must
//! still be read: the lexer starts a scan at every `obj`, and nothing before one -- an unclosed
//! string, a comment, binary data -- may swallow it. Either the appended reference is found, or
//! the scan says it stopped (`irregular`), which refuses. A plausible answer without it is the
//! failure a crash detector cannot see.
//!
//! The whole input is the document, so the seeds are whole fixtures (`tools/seed-fuzz-corpus.py`,
//! prefix 0). The same bytes are also read as a decoded object stream, with `/N` and `/First`
//! taken from the input's length and first byte, because members are the other place qpdf parses.

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

    let count = i64::try_from(data.len() % 64).unwrap_or(0);
    let first = i64::from(data.first().copied().unwrap_or(0));
    let members = in_object_stream(data, count, first, &mut || Ok(()))
        .expect("a checkpoint that never fails");
    assert!(members.references.len() <= MAX_DISTINCT_REFERENCES);
    assert!(
        members.stream_objects.is_empty(),
        "an object stream reported a stream object, which cannot be a member"
    );
});

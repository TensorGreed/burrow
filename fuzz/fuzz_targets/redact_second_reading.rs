//! Fuzz the second reading's recording decoder, the bytes the render worker hands the redaction
//! worker on the web (#206).
//!
//! ```bash
//! # from fuzz/
//! cargo +nightly fuzz run redact_second_reading -- -max_total_time=60 -timeout=10 -rss_limit_mb=2048
//! ```
//!
//! # The claims
//!
//! Through `decode_answer`, the one function the redaction worker hands a reply to: a recording or
//! a typed failure.
//!
//! 1. **No panic, no hang, no allocation past the ceiling**: a reply past `MAX_ANSWER_LEN` and a
//!    count past `MAX_CHARS` are refused before anything is allocated for them.
//! 2. **A decoded answer re-encodes to exactly the bytes it came from**: the layout is fixed and
//!    exact, so two encodings of one answer would be a second way to say the same thing, and a
//!    reply the redaction module accepted would no longer be one the render module could have
//!    written.
//! 3. **A decoded recording never holds more characters than a reading may**.
//!
//! Pure Rust, no engine: seeded with hand-written recordings (`tools/seed-fuzz-corpus.py`).

#![no_main]

use burrow_engines::redact_read_back::MAX_CHARS;
use burrow_engines::redact_read_back::recorded::{
    Answer, MAX_ANSWER_LEN, decode_answer, encode_answer,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(answer) = decode_answer(data) else {
        return;
    };
    assert!(
        data.len() <= MAX_ANSWER_LEN,
        "an answer past its ceiling decoded"
    );
    if let Answer::Read(recorded) = &answer {
        assert!(
            recorded.page.chars.len() <= MAX_CHARS,
            "a recording past the ceiling decoded"
        );
    }
    let again = encode_answer(&answer).expect("a decoded answer re-encodes");
    assert_eq!(again, data, "a decoded answer re-encodes to other bytes");
});

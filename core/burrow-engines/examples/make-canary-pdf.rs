//! Write a canary PDF to stdout, for the web console-silence test.
//!
//! ```bash
//! cargo run -q -p burrow-engines --example make-canary-pdf -- BURROW-CANARY-abc123 > out.pdf
//! ```
//!
//! # Why this exists rather than a TypeScript generator
//!
//! `apps/web/e2e/console-silence.spec.ts` needs the same fixture
//! `core/burrow-engines/tests/secret_leak.rs` uses: a PDF with a canary in every place an
//! engine message might quote it — a name object, a string, a stream's contents, a dictionary
//! key, and a broken token immediately beside the canary so the parser fails at exactly that
//! offset.
//!
//! Getting that right is the whole value of the fixture, and reimplementing it in TypeScript
//! would be a second generator that drifts from the first. `minimal_pdf.rs`'s own header says
//! why it is included by `#[path]` rather than copied: "so there is one generator rather than
//! two that drift." The web is a third consumer of the same rule.
//!
//! The canary is a **parameter**, not a constant, because it must be unique per run: a fixed
//! one committed to the repository could match unrelated text, and a matcher that can report a
//! leak that is not there is as useless as one that cannot report a real one.

// Only `expect_used`, and only for the one `write_all` below. `core/CLAUDE.md` exempts tests
// and `build.rs`, not examples, so the exemption list here is kept to what is actually used
// rather than copied wholesale from the fixture generator next door.
#![allow(clippy::expect_used)]

#[path = "../testsupport/minimal_pdf.rs"]
mod minimal_pdf;

use std::io::Write as _;

fn main() {
    let canary = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: make-canary-pdf <canary>");
        std::process::exit(2);
    });

    // A canary reaches a PDF name object (`/{canary}`), so it must be a legal one: no
    // whitespace and no delimiter. Rejected rather than sanitised, because silently altering
    // it would mean the test searched for a string the file does not contain.
    if canary.is_empty()
        || canary
            .bytes()
            .any(|b| !b.is_ascii_alphanumeric() && b != b'-' && b != b'_')
    {
        eprintln!("the canary must be non-empty and [A-Za-z0-9_-] only");
        std::process::exit(2);
    }

    let bytes = minimal_pdf::pdf_with_canary(&canary);
    std::io::stdout()
        .write_all(&bytes)
        .expect("stdout should be writable");
}

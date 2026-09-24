//! Redact one region of one page of each document listed on stdin, and report each outcome.
//!
//! ```text
//! cargo run -p burrow-ops --features native-engines --release --example redact-batch < jobs.tsv
//! ```
//!
//! Each input line is `input<TAB>page<TAB>left<TAB>top<TAB>width<TAB>height<TAB>output`, the
//! region in display coordinates as [`burrow_engines::pdfsyntax::region::Region`] defines them.
//! Each output line is `OK<TAB>input` after writing the redacted document to `output`, or
//! `REFUSED<TAB>input<TAB>message` with the operation's typed error.
//!
//! # Why
//!
//! `tools/check-redaction-corpus.py --after` (#176) compares every placement's `expect_after` in
//! `tests/redaction/manifest.toml` against a real run, using the **same witnesses** that
//! established the canary was present before. Those witnesses are Python. This is the run: the
//! public operation, `burrow_ops::redact::page`, with its verification, and nothing else. One
//! process for the whole corpus rather than one `cargo run` per document.
//!
//! **Not production code.** It reports and exits; it decides nothing.

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "an example that reports to a script"
)]

#[cfg(all(feature = "native-engines", target_os = "linux"))]
fn main() {
    use std::collections::BTreeSet;
    use std::io::BufRead;
    use std::sync::Arc;

    use burrow_engines::OpenOptions;
    use burrow_engines::pdfsyntax::region::Region;
    use burrow_types::{Limits, SystemClock};

    for line in std::io::stdin().lock().lines() {
        let line = line.expect("stdin is readable");
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        let [input, page, left, top, width, height, output] = fields[..] else {
            eprintln!(
                "redact-batch: a line with {} fields, not 7: {line}",
                fields.len()
            );
            std::process::exit(2);
        };
        let number = |text: &str| -> f64 { text.parse().expect("a number") };
        let page: usize = page.parse().expect("a page index");
        let region = Region {
            left: number(left),
            top: number(top),
            width: number(width),
            height: number(height),
        };
        let bytes = std::fs::read(input).expect("the input is readable");
        let options = OpenOptions::new(Limits::default(), Arc::new(SystemClock::new()));
        let redacted: BTreeSet<usize> = [page].into_iter().collect();
        match burrow_ops::redact::page(
            &burrow_engines::qpdf::Qpdf,
            &bytes,
            page,
            &redacted,
            region,
            &options,
        ) {
            Ok(done) => {
                std::fs::write(output, &done.document).expect("the output is writable");
                println!("OK\t{input}");
            }
            // THE MESSAGE ON ONE LINE: a refusal names its rule in brackets, and the checker
            // reads the rule from there.
            Err(error) => println!(
                "REFUSED\t{input}\t{}",
                format!("{error:?}").replace('\n', " ")
            ),
        }
    }
}

#[cfg(not(all(feature = "native-engines", target_os = "linux")))]
fn main() {
    eprintln!("redact-batch needs the native engines on Linux.");
    std::process::exit(2);
}

//! Run every shipped operation over a document and report the typed outcome of each.
//!
//! ```text
//! cargo run -p burrow-ops --features native-engines --release \
//!   --example run-operations -- <file.pdf> [more.pdf ...]
//! ```
//!
//! # Why
//!
//! The redaction corpus (#135) put the first real-world documents in this repository since the
//! embedded-font defect (#112) — a LibreOffice export, a pdfTeX document and a tesseract OCR of
//! a scan. #112 is the reason that matters: an ordinary course PDF found in minutes what
//! twenty-two synthetic conformance cases never touched, because none of them had an embedded
//! subset font.
//!
//! So before any of these become conformance cases, run the five SHIPPED operations over them
//! and look at what comes back. This prints one line per (document, operation) with the typed
//! outcome, which is the observable `tests/conformance/expectations.json` records.
//!
//! **Not production code.** It reports and exits; it decides nothing.

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "an example that reports to a person"
)]

#[cfg(all(feature = "native-engines", target_os = "linux"))]
fn main() {
    use std::sync::Arc;

    use burrow_engines::qpdf::Qpdf;
    use burrow_engines::{OpenOptions, PageExtractor};
    use burrow_ops::compress::compress;
    use burrow_ops::merge::{Input, merge};
    use burrow_ops::reorder::reorder;
    use burrow_ops::rotate::{Pages, rotate};
    use burrow_ops::split::{Cuts, split};
    use burrow_types::{Limits, SystemClock};

    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        eprintln!("usage: run-operations <file.pdf> [more.pdf ...]");
        std::process::exit(2);
    }

    let engine = Qpdf::new();
    let opts = || OpenOptions::new(Limits::DEFAULT, Arc::new(SystemClock::new()));

    // `Err` is printed in full. A truncated error is how a real defect reads as noise.
    let say = |_name: &str, op: &str, r: Result<String, burrow_types::Error>| match r {
        Ok(detail) => println!("  {op:<10} ok      {detail}"),
        Err(e) => println!("  {op:<10} ERROR   {e}"),
    };

    for file in &files {
        let Ok(raw) = std::fs::read(file) else {
            println!("{file}\n  could not be read");
            continue;
        };
        println!("\n{file}  ({} bytes)", raw.len());

        let pages =
            match <Qpdf as PageExtractor>::open(&engine, raw.clone().into_boxed_slice(), &opts()) {
                Ok(doc) => match <Qpdf as PageExtractor>::pages(&engine, &doc) {
                    Ok(n) => {
                        println!("  {:<10} ok      {n} page(s)", "open");
                        n
                    }
                    Err(e) => {
                        println!("  {:<10} ERROR   {e}", "page_count");
                        continue;
                    }
                },
                Err(e) => {
                    println!("  {:<10} ERROR   {e}", "open");
                    continue;
                }
            };

        let all: Vec<u64> = (1..=pages).collect();
        say(
            file,
            "rotate",
            rotate(
                &engine,
                raw.clone().into_boxed_slice(),
                Pages::numbered(&all),
                90,
                &opts(),
            )
            .map(|v| format!("turned {} page(s), {} bytes out", all.len(), v.len())),
        );

        let order: Vec<u64> = (1..=pages).rev().collect();
        say(
            file,
            "reorder",
            reorder(&engine, raw.clone().into_boxed_slice(), &order, &opts())
                .map(|v| format!("reversed {} page(s), {} bytes out", order.len(), v.len())),
        );

        // Cut after page 1 where there is more than one page; a one-page document can only be
        // split one way, which still exercises the build route and the pruning.
        let cuts: Vec<u64> = if pages > 1 { vec![1] } else { vec![] };
        say(
            file,
            "split",
            split(
                &engine,
                raw.clone().into_boxed_slice(),
                Cuts::after_pages(&cuts),
                &opts(),
            )
            .map(|parts| {
                let sizes: Vec<String> = parts.iter().map(|p| p.len().to_string()).collect();
                format!("{} part(s): {} bytes", parts.len(), sizes.join(" + "))
            }),
        );

        say(
            file,
            "compress",
            compress(&engine, raw.clone().into_boxed_slice(), &opts()).map(|o| format!("{o:?}")),
        );

        // Merged with itself: one input is not a merge, and a second copy is the smallest
        // honest two-input case.
        let a = raw.clone().into_boxed_slice();
        let b = raw.clone().into_boxed_slice();
        say(
            file,
            "merge",
            merge(&engine, vec![Input::new(a), Input::new(b)], &opts())
                .map(|v| format!("{} bytes out", v.len())),
        );
    }
}

#[cfg(not(all(feature = "native-engines", target_os = "linux")))]
fn main() {
    eprintln!("run-operations needs the native engines on Linux.");
    std::process::exit(2);
}

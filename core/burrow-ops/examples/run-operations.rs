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
//!
//! # One operation, one file: the mode `tools/check-fixtures-survive.sh` runs (#260)
//!
//! ```text
//! run-operations --op <open|check|rotate|reorder|split|compress|merge|redact|render> <file>
//! ```
//!
//! Runs that one operation and exits **0** on success and **1** on a typed refusal, printing the
//! outcome. Anything else -- death by a signal, an abort, a hang the caller times out -- is
//! what the gate exists to catch, and it can only catch it because each pair runs in a process
//! of its own. The nine are every shipped operation: `open` is PDFium's open and page count,
//! `check` is qpdf's structure check with recovery off AND on -- both run, whatever the first
//! returns -- `redact` is page 1 against a whole-page region, `render` is page 1 into a 200 px
//! box, and the rest write the whole document.

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

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--op") {
        let (Some(op), Some(file)) = (args.get(1), args.get(2)) else {
            eprintln!("usage: run-operations --op <operation> <file>");
            std::process::exit(2);
        };
        std::process::exit(one_operation(op, file));
    }
    let files = args;
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

/// Run `op` over `file` once, print the outcome, and return the exit code: 0 for `Ok`, 1 for a
/// typed error, 2 for a usage mistake.
#[cfg(all(feature = "native-engines", target_os = "linux"))]
fn one_operation(op: &str, file: &str) -> i32 {
    use std::sync::Arc;

    use burrow_engines::pdfium::Pdfium;
    use burrow_engines::qpdf::Qpdf;
    use burrow_engines::{CheckOptions, DocumentEngine, OpenOptions, StructureEngine};
    use burrow_ops::compress::compress;
    use burrow_ops::merge::{Input, merge};
    use burrow_ops::reorder::reorder;
    use burrow_ops::rotate::{Pages, rotate};
    use burrow_ops::split::{Cuts, split};
    use burrow_types::{Clock, Limits, SystemClock};

    let Ok(raw) = std::fs::read(file) else {
        eprintln!("{file}: could not be read");
        return 2;
    };
    let clock = || -> Arc<dyn Clock> { Arc::new(SystemClock::new()) };
    let opts = || OpenOptions::new(Limits::DEFAULT, clock());
    let boxed = || raw.clone().into_boxed_slice();
    let qpdf = Qpdf::new();
    // THE REAL WRITE PATH, NOT A REFUSAL OF A BAD ARGUMENT: reorder by the reverse of the actual
    // count, rotate every page, cut after page 1 only where there is a page 2. A count that
    // cannot be had is itself the outcome for those three.
    // THE ARGUMENT COUNT, via the rotator's open -- the same unguarded qpdf open rotate, reorder
    // and split's operations use, so it agrees with them on the count. NOT via split's
    // PageExtractor open, which carries the repaired-input guard: counting there reported split's
    // refusal as rotate's and reorder's, which are not guarded. NOT via PDFium either, which
    // disagrees with qpdf on a page count for a document like `five-pages-or-six` and would build
    // a page list the qpdf operation then rejects.
    let pages = || -> Result<u64, burrow_types::Error> {
        use burrow_engines::PageRotator;
        let source = <Qpdf as PageRotator>::open(&qpdf, boxed(), &opts())?;
        <Qpdf as PageRotator>::pages(&qpdf, &source)
    };
    // A DETAIL LINE THE GATE PINS, deterministic and free of byte sizes: what each operation
    // was asked (the arguments it was actually given) and what came out (the output re-opened and
    // counted). An outcome bit alone let an operation be narrowed -- one page rotated, an identity
    // order, no cut -- and still pass (second security review).
    let reopened = |bytes: &[u8]| -> String {
        use burrow_engines::PageExtractor;
        match <Qpdf as PageExtractor>::open(&qpdf, bytes.to_vec().into_boxed_slice(), &opts())
            .and_then(|document| <Qpdf as PageExtractor>::pages(&qpdf, &document))
        {
            Ok(count) => format!("{count} page(s)"),
            Err(error) => format!("unreadable ({error})"),
        }
    };
    // WHAT THE OUTPUT SHOWS, PAGE BY PAGE: each page's effective rotation (qpdf, as ADR 0022's
    // read-back reads it) and its size (PDFium). Printing the arguments the runner meant to pass
    // let a narrowing at the call site -- one page rotated, an identity order, an angle of 0 --
    // through, because the line named the variable rather than what the operation did (third
    // security review). Pages alike in both cannot be told apart, and neither can any reordering
    // of them.
    let shape = |bytes: &[u8]| -> String {
        use burrow_engines::{OutputReader, PageRenderer};
        let options = opts();
        let deadline = burrow_types::Deadline::start(options.clock.as_ref(), &options.limits);
        let rotations = qpdf
            .open_output(bytes, &options)
            .and_then(|read| qpdf.rotations(&read, &options, &deadline));
        let pdfium = Pdfium::new();
        let sizes = pdfium
            .open(bytes.to_vec().into_boxed_slice(), &options)
            .and_then(|document| {
                let count = pdfium.page_count(&document)?;
                (0..count)
                    .map(|index| {
                        pdfium
                            .page_size(&document, index)
                            .map(|(w, h)| format!("{w:.0}x{h:.0}"))
                    })
                    .collect::<Result<Vec<_>, _>>()
            });
        match (rotations, sizes) {
            (Ok(rotations), Ok(sizes)) => format!("rotations {rotations:?} sizes {sizes:?}"),
            (Err(error), _) | (_, Err(error)) => format!("unreadable ({error})"),
        }
    };
    let outcome: Result<String, burrow_types::Error> = match op {
        "open" => {
            let pdfium = Pdfium::new();
            pdfium
                .open(boxed(), &opts())
                .and_then(|document| pdfium.page_count(&document))
                .map(|pages| format!("{pages} page(s)"))
        }
        "check" => {
            // BOTH ATTEMPTS RUN, AND EACH IS REPORTED. Collecting straight into a `Result`
            // stopped at the first `Err`, so recovery-on never ran on the damaged files it exists
            // for; and one bit for the pair hid what recovery-on did (code reviews).
            let mut lines = Vec::new();
            let mut all_ok = true;
            for recovery in [false, true] {
                let mut options = CheckOptions::new(Limits::DEFAULT, clock());
                options.attempt_recovery = recovery;
                let label = if recovery { "recover" } else { "strict" };
                match qpdf.check(boxed(), &options) {
                    Ok(report) => lines.push(format!("{label} ok {} page(s)", report.pages)),
                    Err(error) => {
                        all_ok = false;
                        lines.push(format!("{label} refused {error}"));
                    }
                }
            }
            let detail = lines.join("; ");
            if all_ok {
                Ok(detail)
            } else {
                println!("check refused {detail}");
                return 1;
            }
        }
        "redact" => {
            use burrow_engines::pdfsyntax::region::Region;
            let whole = Region {
                left: 0.0,
                top: 0.0,
                width: 1.0e5,
                height: 1.0e5,
            };
            let redacted: std::collections::BTreeSet<usize> = [0].into_iter().collect();
            let page = 0;
            burrow_ops::redact::page(&qpdf, &raw, page, &redacted, whole, &opts()).map(|done| {
                let cut = done.report.fonts.iter().filter(|font| font.cut).count();
                // WHAT IS LEFT ON THE REDACTED PAGE, from the output itself: the report's counts do
                // not depend on where the region is, and a region moved off the page at the call
                // site passed with them alone. "none" or "some" rather than a count, because
                // anti-aliasing is free to differ between architectures and a count would pin it.
                let ink = match burrow_ops::render::render(
                    &Pdfium::new(),
                    done.document.clone().into_boxed_slice(),
                    &[1],
                    burrow_ops::render::Fit::box_of(200, 200),
                    &opts(),
                ) {
                    Ok(pages) => {
                        let dark = pages.first().map_or(0, |page| {
                            page.raster
                                .rgba
                                .as_chunks::<4>()
                                .0
                                .iter()
                                .filter(|[r, g, b, _]| (*r).min(*g).min(*b) < 128)
                                .count()
                        });
                        if dark == 0 { "none" } else { "some" }
                    }
                    Err(_) => "unrenderable",
                };
                format!(
                    "page index {page} of {redacted:?}, region {} {} {}x{}: fonts cut {cut} of {}, \
                     carried text dropped {}, ink left on the page {ink}; output {}",
                    whole.left,
                    whole.top,
                    whole.width,
                    whole.height,
                    done.report.fonts.len(),
                    done.report.dropped_carried_text,
                    reopened(&done.document)
                )
            })
        }
        "render" => {
            let (width, height) = (200, 200);
            let pages = [1];
            burrow_ops::render::render(
                &Pdfium::new(),
                boxed(),
                &pages,
                burrow_ops::render::Fit::box_of(width, height),
                &opts(),
            )
            .map(|rendered| {
                let sizes: Vec<String> = rendered
                    .iter()
                    .map(|r| format!("page {} at {}x{}", r.page, r.raster.width, r.raster.height))
                    .collect();
                format!("{pages:?} into {width}x{height}: {}", sizes.join(", "))
            })
        }
        "rotate" => pages().and_then(|count| {
            let all: Vec<u64> = (1..=count).collect();
            rotate(&qpdf, boxed(), Pages::numbered(&all), 90, &opts())
                .map(|out| format!("every page by 90: output {}", shape(&out)))
        }),
        "reorder" => pages().and_then(|count| {
            let order: Vec<u64> = (1..=count).rev().collect();
            reorder(&qpdf, boxed(), &order, &opts())
                .map(|out| format!("reversed: input {} output {}", shape(&raw), shape(&out)))
        }),
        "split" => pages().and_then(|count| {
            let cuts: Vec<u64> = if count > 1 { vec![1] } else { vec![] };
            split(&qpdf, boxed(), Cuts::after_pages(&cuts), &opts()).map(|parts| {
                let each: Vec<String> = parts.iter().map(|part| reopened(part)).collect();
                format!("cuts {cuts:?}: parts {}", each.join(" + "))
            })
        }),
        "compress" => compress(&qpdf, boxed(), &opts()).map(|outcome| match outcome {
            burrow_ops::compress::Outcome::Smaller { document, .. } => {
                format!("smaller: output {}", reopened(&document))
            }
            burrow_ops::compress::Outcome::NotSmaller { .. } => "not smaller".to_owned(),
        }),
        "merge" => merge(
            &qpdf,
            vec![Input::new(boxed()), Input::new(boxed())],
            &opts(),
        )
        .map(|out| format!("two copies: output {}", reopened(&out))),
        _ => {
            eprintln!("unknown operation {op}");
            return 2;
        }
    };
    match outcome {
        Ok(detail) => {
            println!("{op} ok {detail}");
            0
        }
        Err(error) => {
            println!("{op} refused {error}");
            1
        }
    }
}

#[cfg(not(all(feature = "native-engines", target_os = "linux")))]
fn main() {
    eprintln!("run-operations needs the native engines on Linux.");
    std::process::exit(2);
}

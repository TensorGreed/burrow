//! What ADR 0022's verification costs, measured rather than assumed.
//!
//! The decision it informs is the one in ADR 0022's first requirement: the witness is a
//! **fresh** engine, not the instance that produced the bytes. That is a second full parse of
//! the output, and "a second parse" is the kind of claim that gets rounded to "too expensive"
//! or "free" depending on who is arguing. So it is measured.
//!
//! Run it, from the repository root:
//!
//! ```text
//! cargo run -p burrow-ops --features native-engines --release \
//!   --example measure-verification -- tests/conformance/fixtures/pages-137.pdf
//! ```
//!
//! Three numbers per document, each the median of `RUNS` iterations:
//!
//!   * **operation** — open, promise, edit, write. Everything except the read-back.
//!   * **verification** — `fresh()`, `open_output`, `page_count`, `rotations`. What ADR 0022
//!     adds.
//!   * **promise** — the per-page `effective_rotation` sweep the operation does *before* the
//!     edit. It is part of the operation's own time above, and it is broken out because it is
//!     the other thing ADR 0022 added and it is the one that scales with page count.
//!
//! **Not a test and not production code**, the same way `burrow-engines`' `measure-merge.rs`
//! and `measure-split.rs` are not: it times entry points directly so the numbers are
//! attributable to a phase rather than to an operation as a whole.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::indexing_slicing,
    clippy::integer_division,
    clippy::missing_docs_in_private_items
)]
#![cfg_attr(
    not(all(feature = "native-engines", target_os = "linux")),
    allow(unused)
)]

/// The one generator, included by path rather than copied -- `core/CLAUDE.md`'s rule.
#[cfg(all(feature = "native-engines", target_os = "linux"))]
#[path = "../../burrow-engines/testsupport/minimal_pdf.rs"]
mod minimal_pdf;

#[cfg(all(feature = "native-engines", target_os = "linux"))]
mod measure {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use burrow_engines::qpdf::Qpdf;
    use burrow_engines::{OpenOptions, OutputReader, PageRotator};
    use burrow_types::{Clock, Deadline, Limits, ManualClock};

    /// Enough that one scheduler hiccup does not become the answer, few enough that the whole
    /// run stays under a few seconds on the largest fixture in the repository.
    const RUNS: usize = 11;

    /// A clock for the deadline the sweeps are handed. Measuring, not limiting: the budget is
    /// `Limits::DEFAULT`'s, and nothing here is meant to hit it.
    fn clock() -> Arc<dyn Clock> {
        Arc::new(ManualClock::new(0)) as Arc<dyn Clock>
    }

    fn options() -> OpenOptions<'static> {
        OpenOptions::new(
            Limits::DEFAULT,
            Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
        )
    }

    fn median(mut times: Vec<Duration>) -> Duration {
        times.sort_unstable();
        times[times.len() / 2]
    }

    /// A document of `pages` pages, for measuring the sweeps at a realistic ceiling.
    pub fn generated(pages: usize) -> Vec<u8> {
        super::minimal_pdf::pdf_with_pages(pages)
    }

    /// The same page count, reached through a page tree `depth` nodes deep.
    ///
    /// THE SHAPE THE COMMITTED FIXTURES DO NOT HAVE, and the one the promise sweep is
    /// sensitive to: `effective_rotation` walks `/Parent` to the root per page, so the sweep
    /// costs pages x depth while the write costs pages. Both numbers are attacker-chosen, and
    /// on a flat tree the sweep looks like a rounding error -- which is how it came to sit
    /// outside every deadline.
    ///
    /// Object numbering: 1 catalogue, 2..=depth+1 the chain of `/Pages` nodes, then one
    /// object per page hanging off the last of them.
    pub fn deep(pages: usize, depth: usize) -> Vec<u8> {
        let mut out: Vec<u8> = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
        let mut offsets: Vec<usize> = Vec::new();
        let leaf = depth + 1;
        let page_obj = |i: usize| leaf + 1 + i;

        let object = |out: &mut Vec<u8>, offsets: &mut Vec<usize>, body: String| {
            offsets.push(out.len());
            out.extend_from_slice(body.as_bytes());
        };

        object(
            &mut out,
            &mut offsets,
            "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".to_owned(),
        );

        for node in 2..=leaf {
            let kids = if node == leaf {
                (0..pages)
                    .map(|i| format!("{} 0 R", page_obj(i)))
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                format!("{} 0 R", node + 1)
            };
            let parent = if node == 2 {
                String::new()
            } else {
                format!(" /Parent {} 0 R", node - 1)
            };
            object(
                &mut out,
                &mut offsets,
                format!(
                    "{node} 0 obj\n<< /Type /Pages{parent} /Kids [{kids}] /Count {pages} \
                     /MediaBox [0 0 612 792] >>\nendobj\n"
                ),
            );
        }

        for i in 0..pages {
            object(
                &mut out,
                &mut offsets,
                format!(
                    "{} 0 obj\n<< /Type /Page /Parent {leaf} 0 R /Resources << >> \
                     /MediaBox [0 0 612 792] >>\nendobj\n",
                    page_obj(i)
                ),
            );
        }

        let xref = out.len();
        let count = offsets.len() + 1;
        out.extend_from_slice(format!("xref\n0 {count}\n0000000000 65535 f \n").as_bytes());
        for at in &offsets {
            out.extend_from_slice(format!("{at:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!("trailer\n<< /Size {count} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n")
                .as_bytes(),
        );
        out
    }

    pub fn run(path: &std::path::Path) {
        let bytes = std::fs::read(path).expect("fixture");
        let engine = Qpdf::new();

        // One rotation of one page: the smallest edit the operation offers, so as much of the
        // measured time as possible is parse and write rather than work.
        let mut operation = Vec::new();
        let mut promise = Vec::new();
        let mut verification = Vec::new();
        let mut freshness = Vec::new();
        let mut through_self = Vec::new();
        let mut pages = 0;
        let mut output = Vec::new();

        for _ in 0..RUNS {
            let started = Instant::now();
            let source = engine
                .open(bytes.clone().into_boxed_slice(), &options())
                .expect("open");
            let total = PageRotator::pages(&engine, &source).expect("pages");
            pages = total;

            let promised = Instant::now();
            let ticking = clock();
            let budget = Deadline::start(ticking.as_ref(), &Limits::DEFAULT);
            let _rotations =
                PageRotator::rotations(&engine, &source, &options(), &budget).expect("sweep");
            promise.push(promised.elapsed());

            output = engine
                .rotate(
                    &source,
                    &[0],
                    burrow_types::Rotation::Clockwise90,
                    &options(),
                )
                .expect("rotate");
            operation.push(started.elapsed());

            let checked = Instant::now();
            let witness = engine.fresh();
            freshness.push(checked.elapsed());
            let read = witness.open_output(&output, &options()).expect("read back");
            let _ = witness.page_count(&read).expect("page count");
            let _ =
                OutputReader::rotations(&witness, &read, &options(), &budget).expect("rotations");
            verification.push(checked.elapsed());

            // THE ALTERNATIVE ADR 0022 REJECTED, timed beside it: reading the bytes back
            // through the instance that wrote them. Same parse, same work; the only thing
            // saved is one `qpdf_init`.
            let reused = Instant::now();
            let read = engine.open_output(&output, &options()).expect("read back");
            let _ = engine.page_count(&read).expect("page count");
            let _ =
                OutputReader::rotations(&engine, &read, &options(), &budget).expect("rotations");
            through_self.push(reused.elapsed());
        }

        let op = median(operation);
        let pr = median(promise);
        let ve = median(verification);
        println!(
            "{:<44} {pages:>4} pages {:>9} bytes in {:>9} out",
            path.display(),
            bytes.len(),
            output.len(),
        );
        println!("  operation (open, promise, edit, write) {op:>12.2?}");
        println!(
            "  of which the promise sweep             {pr:>12.2?}  {:>5.1}%",
            ratio(pr, op)
        );
        println!(
            "  verification (fresh, open, read back)   {ve:>12.2?}  {:>5.1}%",
            ratio(ve, op)
        );
        // BROKEN OUT BECAUSE IT IS THE DECISION. The freshness is not what the verification
        // costs -- the second parse is, and a verifier reading through the writing instance
        // would pay that too. What `fresh()` itself adds is this line.
        let se = median(through_self);
        println!(
            "  the same, through the WRITING instance {se:>12.2?}  {:>5.1}%",
            ratio(se, op)
        );
        let fr = median(freshness);
        println!(
            "  of which `fresh()` itself              {fr:>12.2?}  {:>5.1}%",
            ratio(fr, op)
        );
    }

    fn ratio(part: Duration, whole: Duration) -> f64 {
        if whole.is_zero() {
            return 0.0;
        }
        part.as_secs_f64() / whole.as_secs_f64() * 100.0
    }
}

fn main() {
    #[cfg(all(feature = "native-engines", target_os = "linux"))]
    {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if args.is_empty() {
            eprintln!("usage: measure-verification <pdf> [<pdf> ...]");
            eprintln!("   or: measure-verification --generated <pages>");
            return;
        }
        if args.first().map(String::as_str) == Some("--deep") {
            let pages = args.get(1).and_then(|n| n.parse().ok()).unwrap_or(10_000);
            let depth = args.get(2).and_then(|n| n.parse().ok()).unwrap_or(60);
            let path = std::env::temp_dir().join(format!("burrow-measure-{pages}x{depth}.pdf"));
            std::fs::write(&path, measure::deep(pages, depth)).expect("write fixture");
            measure::run(&path);
            return;
        }
        if args.first().map(String::as_str) == Some("--generated") {
            // THE SHAPE THE COMMITTED FIXTURES DO NOT HAVE. `pages-137.pdf` is 16 kB, so the
            // parse and the write dominate and the per-page sweep looks like a rounding
            // error. At the default `max_pages` it is the other way round, and the honest
            // table has both rows. Security review measured that difference and it is why
            // this switch exists.
            let pages = args.get(1).and_then(|n| n.parse().ok()).unwrap_or(10_000);
            let path = std::env::temp_dir().join(format!("burrow-measure-{pages}.pdf"));
            std::fs::write(&path, measure::generated(pages)).expect("write fixture");
            measure::run(&path);
            return;
        }
        for arg in args {
            measure::run(std::path::Path::new(&arg));
        }
    }
    #[cfg(not(all(feature = "native-engines", target_os = "linux")))]
    {
        eprintln!("measure-verification needs --features native-engines on linux");
    }
}

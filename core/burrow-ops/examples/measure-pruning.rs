//! What `split`'s pruning costs, measured rather than assumed.
//!
//! [ADR 0019](../../../docs/adr/0019-how-split-builds-its-outputs.md) §2b, issue #54. Closing the
//! leak added a pass over every page of every output that reads the page's keys as syntax, walks
//! its annotations, and **decodes its content streams** to find out which resources it uses. The
//! last of those is the one worth measuring: it is the only part of any operation in this
//! repository that inflates a stream in order to decide something, and "it decompresses every
//! page" is the kind of claim that gets rounded to "too expensive" or "free" depending on who is
//! arguing.
//!
//! **Redaction inherits whatever this costs** (ADR 0019's section on what transfers to M2), which
//! is why it is recorded rather than noticed.
//!
//! Run it, from the repository root:
//!
//! ```text
//! cargo run -p burrow-ops --features native-engines --release \
//!   --example measure-pruning -- tests/conformance/fixtures/pages-137.pdf
//! cargo run … --example measure-pruning -- --generated 10000
//! cargo run … --example measure-pruning -- --deep 10000 60
//! ```
//!
//! Two numbers per document, each the median of `RUNS` iterations:
//!
//!   * **whole split** — open, the sharing sweep, the promise sweep, copy, prune, write, and the
//!     verification read-back of every part. Since split gained ADR 0022's promise this is the
//!     composed number a caller actually waits through, which is why ADR 0019's cost table could
//!     replace a projection with a measurement — and why it had to: the projection was right to
//!     1% on a flat page tree and 54% too high on a deep one.
//!   * **open + the sharing sweep** — `PageExtractor::open`, which is where `annots_sharing`
//!     runs. Broken out because it is the one part of the prune that scales with the *source's*
//!     page count rather than an output's, and because it happens once per split rather than
//!     once per part.
//!
//! # How the before-and-after is obtained, since it is not in this binary
//!
//! There is no "pruning off" switch and there deliberately is not one: a flag that turns the leak
//! back on is a flag somebody can turn on, and ADR 0019 §2 is not a preference. So the cost of
//! the rule is measured the way any change's cost is — run this on the branch, run it on the
//! commit before, subtract. Both numbers and the two commits go in the ADR, rather than a
//! difference nobody can reproduce.
//!
//! What this binary can say on its own is the shape: how the total scales with page count and
//! with page-tree depth, which is the question `measure-verification` had to answer for the
//! promise sweep and got a surprising answer to.
//!
//! # Why the deep mode is here too
//!
//! `measure-verification` found that the promise sweep is 16% of the operation on a flat tree and
//! **84%** on a 10,000-page document with a 60-deep one, because it walks `/Parent` per page. The
//! prune does not walk `/Parent` — `qpdf_add_page` flattens the tree before it runs (ADR 0021) —
//! so the prediction is that depth costs it nothing. A prediction is not a measurement, and the
//! mode costs one flag.
//!
//! **Not a test and not production code**, like `measure-verification` and
//! `burrow-engines`' `measure-merge.rs` and `measure-split.rs`: it times phases so the numbers are
//! attributable to one.

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
    allow(unused_imports, dead_code)
)]

/// The synthetic documents, shared with `measure-verification` rather than copied.
///
/// Two generators agree until the day they do not, and both examples' numbers land in ADRs beside
/// each other — so a divergence would be two measurements of different documents presented as
/// comparable.
#[path = "../../burrow-engines/testsupport/measure_fixtures.rs"]
mod measure_fixtures;

#[cfg(all(feature = "native-engines", target_os = "linux"))]
mod measure {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use burrow_engines::qpdf::Qpdf;
    use burrow_engines::{OpenOptions, PageExtractor};
    use burrow_ops::{Cuts, split};
    use burrow_types::{Clock, Limits, ManualClock};

    pub const RUNS: usize = 11;

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

    /// Time one document, at the cuts a caller would realistically ask for.
    ///
    /// **Five parts, not one.** A one-way split prunes every page once, which is the same total
    /// work as no split at all and hides the thing worth knowing: the prune runs per *output*, so
    /// a document cut five ways has its pages pruned once each across five destinations. Splitting
    /// into one part would measure a case nobody asks for — the same mistake ADR 0019 records
    /// about its own first fidelity run, where one-page fixtures made a split the identity.
    pub fn report(label: &str, bytes: &[u8]) {
        let engine = Qpdf::new();
        let opts = options();

        let source = engine
            .open(bytes.to_vec().into_boxed_slice(), &opts)
            .expect("the document must open");
        let pages = engine.pages(&source).expect("page count");
        drop(source);

        // Cut into five roughly equal parts, or as many as the document has pages for.
        let parts = pages.clamp(1, 5);
        let step = pages / parts;
        let cuts: Vec<u64> = (1..parts).map(|n| n * step).collect();

        let mut whole = Vec::with_capacity(RUNS);
        let mut sharing = Vec::with_capacity(RUNS);
        let mut bytes_out = 0usize;
        for _ in 0..RUNS {
            let started = Instant::now();
            let outputs = split(
                &engine,
                bytes.to_vec().into_boxed_slice(),
                Cuts::after_pages(&cuts),
                &opts,
            )
            .expect("the document must split");
            whole.push(started.elapsed());
            bytes_out = outputs.iter().map(Vec::len).sum();

            // THE SHARING SWEEP ON ITS OWN, timed as `open` does it: it is the whole difference
            // between `PageExtractor::open` before #54 and after, and it is the part that scales
            // with the SOURCE's page count rather than an output's.
            let started = Instant::now();
            let source = engine
                .open(bytes.to_vec().into_boxed_slice(), &opts)
                .expect("open");
            sharing.push(started.elapsed());
            drop(source);
        }

        let whole = median(whole);
        let sharing = median(sharing);
        println!(
            "{label}   {pages} pages  {} bytes in  {bytes_out} out, {parts} part(s)",
            bytes.len()
        );
        println!("  whole split (open, prune, write, verify)  {whole:>12.3?}");
        println!(
            "  of which open + the sharing sweep        {sharing:>12.3?}   {:>5.1}%",
            percent(sharing, whole)
        );
    }

    /// Peak resident set, from the kernel rather than from an estimate.
    ///
    /// `VmHWM` is the high-water mark: the largest the process has ever been, which is what a
    /// memory question about a streaming operation is actually asking. A sample taken at the end
    /// would miss the peak by construction, since the point of streaming is that the peak is
    /// transient.
    fn peak_rss_bytes() -> u64 {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|status| {
                status
                    .lines()
                    .find(|line| line.starts_with("VmHWM:"))
                    .and_then(|line| line.split_whitespace().nth(1).map(str::to_owned))
            })
            .and_then(|kib| kib.parse::<u64>().ok())
            .map_or(0, |kib| kib * 1024)
    }

    /// What a ONE-WAY split costs each way round, which is the case streaming is worst at.
    ///
    /// **The question this answers is whether a single-part special case is worth writing.**
    /// Streaming holds the source open while each part is produced and verified; the batching
    /// path it replaced could drop the source before any read-back. For many parts streaming
    /// wins easily -- the parts together are about the size of the source, and only one is
    /// resident. For ONE part there is nothing to win and one document to lose.
    ///
    /// Both paths are built from the public API so neither is a reconstruction: the streaming one
    /// is `split_begin` + `next_part`, and the batching one is `open` -> `extract` -> `drop` ->
    /// `verify`, which is what `split` did before ADR 0023.
    ///
    /// # ONE PATH PER PROCESS, and the first version of this got it wrong
    ///
    /// `VmHWM` is a high-water mark: it never falls. Running both paths in one process meant the
    /// second one measured the growth the FIRST had already caused, and reported roughly zero --
    /// so the batching path looked free and the streaming path looked like the whole cost. The
    /// numbers were confident and meaningless, which is the shape this repository keeps catching.
    ///
    /// So the caller names the path and runs the binary twice.
    pub fn report_memory(label: &str, bytes: &[u8], which: &str) {
        use burrow_engines::PageExtractor;

        let engine = Qpdf::new();
        let opts = options();

        if which != "streaming" && which != "batching" {
            // A MISTYPED ARGUMENT USED TO PRINT A NUMBER. Neither arm matched, nothing ran, and
            // the harness reported `peak RSS growth 4096 bytes` -- which reads exactly like
            // "this path is free". This is the harness behind ADR 0023's memory table; a
            // confident small number out of it is worse than a crash. Found by code review.
            eprintln!("measure-pruning: --memory takes `streaming` or `batching`, not {which:?}");
            std::process::exit(2);
        }

        let before = peak_rss_bytes();
        if which == "streaming" {
            let mut session = burrow_ops::split_begin(
                &engine,
                bytes.to_vec().into_boxed_slice(),
                Cuts::after_pages(&[]),
                &opts,
            )
            .expect("one-way split");
            while let Some(part) = session.next_part(&engine, &opts).expect("a part") {
                core::hint::black_box(&part);
            }
        }
        if which == "batching" {
            let source = engine
                .open(bytes.to_vec().into_boxed_slice(), &opts)
                .expect("open");
            let pages = engine.pages(&source).expect("pages");
            let deadline = burrow_types::Deadline::start(opts.clock.as_ref(), &opts.limits);
            let rotations =
                PageExtractor::rotations(&engine, &source, &opts, &deadline).expect("sweep");
            let output = engine.extract(&source, 0, pages, &opts).expect("extract");
            drop(source);
            burrow_ops::verify::output(
                &engine,
                &output,
                &burrow_ops::verify::Expected::Split { rotations },
                &opts,
                &deadline,
            )
            .expect("verify");
            core::hint::black_box(&output);
        }
        let grew = peak_rss_bytes().saturating_sub(before);
        println!("{label}  {which:<10} peak RSS growth {grew:>10} bytes");
    }

    fn percent(part: Duration, whole: Duration) -> f64 {
        if whole.as_nanos() == 0 {
            return 0.0;
        }
        (part.as_secs_f64() / whole.as_secs_f64()) * 100.0
    }
}

#[cfg(all(feature = "native-engines", target_os = "linux"))]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--memory") => {
            let pages: usize = args.get(1).and_then(|n| n.parse().ok()).unwrap_or(10_000);
            let which = args.get(2).map_or("streaming", String::as_str);
            let bytes = measure_fixtures::generated(pages);
            measure::report_memory(&format!("--memory {pages}"), &bytes, which);
        }
        Some("--deep") => {
            let pages: usize = args.get(1).and_then(|n| n.parse().ok()).unwrap_or(10_000);
            let depth: usize = args.get(2).and_then(|n| n.parse().ok()).unwrap_or(60);
            let bytes = measure_fixtures::deep(pages, depth);
            measure::report(&format!("--deep {pages} {depth}"), &bytes);
        }
        Some("--generated") => {
            let pages: usize = args.get(1).and_then(|n| n.parse().ok()).unwrap_or(10_000);
            let bytes = measure_fixtures::generated(pages);
            measure::report(&format!("--generated {pages}"), &bytes);
        }
        Some(_) => {
            for path in &args {
                let bytes = std::fs::read(path).expect("read the fixture");
                measure::report(path, &bytes);
            }
        }
        None => {
            println!(
                "usage: measure-pruning <pdf>... | --generated <pages> | --deep <pages> <depth> \
| --memory <pages>"
            );
        }
    }
}

#[cfg(not(all(feature = "native-engines", target_os = "linux")))]
fn main() {
    println!("measure-pruning needs --features native-engines on linux");
}

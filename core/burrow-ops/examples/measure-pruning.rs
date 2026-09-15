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
                "usage: measure-pruning <pdf>... | --generated <pages> | --deep <pages> <depth>"
            );
        }
    }
}

#[cfg(not(all(feature = "native-engines", target_os = "linux")))]
fn main() {
    println!("measure-pruning needs --features native-engines on linux");
}

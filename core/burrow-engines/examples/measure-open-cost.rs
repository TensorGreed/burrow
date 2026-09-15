//! What an open actually costs each engine, against what `crate::estimate` predicts.
//!
//! **This exists because a decision needed a number.** `estimate::estimated_open_bytes` is
//! `input_len + input_len/4 + 16 MiB`, and both constants were measured against PDFium
//! ([spike 0001](../../../docs/spikes/0001-wasm-engines.md) Finding 4). The qpdf paths never
//! called it, which is issue #26 — and `web/qpdf.rs` carried a comment arguing they should
//! not, because "applying them to a structural check would reject files qpdf handles
//! comfortably".
//!
//! That argument is either right or wrong, and nobody had measured which. This does.
//!
//! Run it, from the repository root:
//!
//! ```text
//! cargo run -p burrow-engines --features native-engines --release \
//!   --example measure-open-cost -- qpdf tests/conformance/fixtures/pages-10.pdf
//! ```
//!
//! # ONE OPEN PER PROCESS, and the first version of this was meaningless without it
//!
//! Resident set size is a high-water mark: it never falls. So a process that opens the same
//! file five times reports the first open's cost and then four zeroes, and a process that
//! opens it with two engines attributes everything to whichever ran first. The first version
//! did both, and reported 0 bytes for a 20 MB document — confidently, and meaninglessly.
//! This is the same mistake ADR 0023 records against the split memory figure, made again in
//! the same session, which is why it is written here rather than remembered.
//!
//! So: this binary measures **one engine opening one file, once**, and exits. Sweeping is the
//! caller's job:
//!
//! ```text
//! for f in tests/conformance/fixtures/*.pdf; do
//!   for e in qpdf pdfium; do
//!     cargo run -q -p burrow-engines --features native-engines --release \
//!       --example measure-open-cost -- "$e" "$f"
//!   done
//! done
//! ```

// UNUSED WITHOUT THE ENGINES, and that is the point of the gate below rather than a
// reason to drop them: `cargo clippy --workspace --all-targets` builds examples, so this
// file must lint clean both with the feature and without it.
#![cfg_attr(
    not(all(feature = "native-engines", burrow_native_engines, target_os = "linux")),
    allow(unused_imports, dead_code)
)]

use std::path::{Path, PathBuf};

use burrow_engines::{CheckOptions, DocumentEngine, OpenOptions, StructureEngine};
use burrow_types::{Clock, Limits, SystemClock};

/// The estimate, restated. An example is its own crate, so `crate::estimate` is not in
/// scope — and restating it here is the point of the comparison anyway: if this drifts from
/// `estimate.rs`, the two are measuring different things and the column heading is a lie.
/// `estimate.rs`'s own unit tests pin the formula; this pins that a reader can see it.
fn estimated_open_bytes(input_len: u64) -> u64 {
    input_len
        .saturating_add(input_len.div_euclid(4))
        .saturating_add(16 * 1024 * 1024)
}

/// Peak resident set, from `VmHWM`.
///
/// **`VmHWM`, not `VmRSS`, and the difference is the whole measurement.** `VmRSS` is the
/// current resident set and it *falls* when memory is freed --- measured here, 521,724 kB
/// down to 9,720 kB across one `free`. So a reading taken after an engine closed its document
/// reports what is left rather than what it took, and two engines sampled at different points
/// are not comparable. `VmHWM` is a genuine high-water mark, so the sampling point stops
/// mattering and both branches below can be written the same way.
///
/// Code review caught this: the first version read `VmRSS`, held the PDFium document open
/// across its reading, and let the qpdf branch sample after `StructureEngine::check` had
/// already dropped the document. The conclusion survived, because the under-measured engine
/// was the one that read higher --- but that was luck, not method.
fn peak_resident_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kb * 1024);
        }
    }
    None
}

fn main() {
    #[cfg(not(all(feature = "native-engines", burrow_native_engines, target_os = "linux")))]
    {
        eprintln!("measure-open-cost needs --features native-engines on linux");
    }
    #[cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
    measure();
}

/// The measurement proper. Split out so the `cfg` above gates one call rather than the body,
/// which is the shape `measure-split.rs` and `measure-merge.rs` already use --- and the reason
/// is that `cargo clippy --workspace --all-targets` and `cargo test --workspace` both BUILD
/// examples, so an example that only compiles with `--all-features` fails the Definition of
/// done on an ordinary run. Found by code review.
#[cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
fn measure() {
    let mut args = std::env::args().skip(1);
    let (Some(engine), Some(file)) = (args.next(), args.next()) else {
        eprintln!("usage: measure-open-cost <qpdf|pdfium> <file.pdf>");
        std::process::exit(2);
    };
    let path = PathBuf::from(&file);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("{}: {error}", path.display());
            std::process::exit(2);
        }
    };

    // A ceiling nothing here can reach, so the measurement is of cost rather than of
    // refusal. The whole question is what the cost IS.
    let limits = Limits::with(|l| {
        l.max_memory_bytes = u64::MAX;
        l.max_input_bytes = u64::MAX;
    });
    let clock: std::sync::Arc<dyn Clock> = std::sync::Arc::new(SystemClock::new());

    // BEFORE THE BYTES ARE HANDED OVER, and after they are already resident: `bytes` is read
    // above, so the input buffer itself is not counted as engine cost.
    let before = peak_resident_bytes().unwrap_or(0);
    let ok = match engine.as_str() {
        "qpdf" => {
            let qpdf = burrow_engines::qpdf::Qpdf::new();
            let options = CheckOptions::new(limits, clock);
            StructureEngine::check(&qpdf, bytes.clone().into_boxed_slice(), &options).is_ok()
        }
        "pdfium" => {
            let pdfium = burrow_engines::pdfium::Pdfium::new();
            let options = OpenOptions::new(limits, clock);
            // SYMMETRIC WITH THE qpdf BRANCH: opened, then dropped, and the peak read after.
            // With `VmHWM` that is the same measurement either way, which is the point of
            // reading a high-water mark rather than the current set.
            DocumentEngine::open(&pdfium, bytes.clone().into_boxed_slice(), &options).is_ok()
        }
        other => {
            eprintln!("unknown engine {other:?}: expected qpdf or pdfium");
            std::process::exit(2);
        }
    };
    let grew = peak_resident_bytes().unwrap_or(0).saturating_sub(before);
    let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    report(&engine, &path, len, grew, ok);
}

/// One line, naming what was examined rather than printing a bare number.
fn report(engine: &str, path: &Path, len: u64, grew: u64, ok: bool) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    println!(
        "{engine:<7} {name:<26} bytes={len:<10} rss_growth={grew:<10} \
estimate={:<10} outcome={}",
        estimated_open_bytes(len),
        if ok { "ok" } else { "refused" },
    );
}

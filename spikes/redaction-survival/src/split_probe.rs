//! Finding 3 — what the machinery that already exists does with these fixtures.
//!
//! `split`'s pruning (`core/burrow-engines/src/prune/`), its closure harness and ADR 0022's
//! read-back are the nearest things burrow has to a redaction. This runs the SHIPPED `split`
//! over spike 0006's fixtures as a ONE-WAY split -- no cuts, so every page is included -- and
//! reports what came back and whether each channel's canary is still in it.
//!
//! A one-way split is the right probe precisely because it excludes nothing. Every difference
//! between input and output is then the pruning policy and the writer, not the subsetting: it
//! isolates "what does this machinery remove from a page it is KEEPING", which is redaction's
//! question and not `split`'s.
//!
//! ```text
//! cargo run --release --manifest-path spikes/redaction-survival/Cargo.toml \
//!   --bin what-split-already-does
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;
// ---------------------------------------------------------------------------
// The record. Every table goes to stdout AND to a file the repository commits.
//
// WHY THE BINARY WRITES IT RATHER THAN A SHELL REDIRECT. The evidence this spike's report
// cites has to be an artifact somebody can diff, not a run they can repeat -- two wrong cost
// tables and a directory of no-op "redacted" PDFs in this very spike are the argument. A `>`
// in a documented command goes stale silently the first time somebody runs the binary without
// it; a file the binary writes cannot disagree with the run that produced it.
//
// It does NOT go in `results/`: `tools/check-no-generated-files.sh` refuses any tracked path
// matching `(^|/)results/`, and its own probe for that pattern is a spike results file from
// PR #37. That gate is correct and this is not the thing it is guarding against, so the
// record gets a directory whose name says what it is instead of an exception to the gate.
// ---------------------------------------------------------------------------

use std::cell::RefCell;

thread_local! {
    static RECORD: RefCell<String> = const { RefCell::new(String::new()) };
}

macro_rules! out {
    () => { out!("") };
    ($($arg:tt)*) => {{
        let line = format!($($arg)*);
        println!("{line}");
        RECORD.with(|r| {
            let mut r = r.borrow_mut();
            r.push_str(&line);
            r.push('\n');
        });
    }};
}

/// Write the accumulated record beside the sources, where git can see it.
fn write_record(path: &Path) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    RECORD.with(|r| std::fs::write(path, r.borrow().as_bytes())).map_err(|e| e.to_string())
}


use burrow_engines::OpenOptions;
use burrow_engines::qpdf::Qpdf;
use burrow_ops::split::{Cuts, split};
use burrow_types::{Limits, SystemClock};

#[path = "scan.rs"]
mod scan;

const CHANNELS: &[(u32, &str)] = &[
    (1, "plain-tj"),
    (2, "kerned-tj"),
    (3, "positioned-runs"),
    (4, "differences-encoding"),
    (5, "cid-with-tounicode"),
    (6, "cid-without-tounicode"),
    (7, "form-xobject"),
    (8, "type3-glyph"),
    (9, "actualtext"),
    (10, "structure-tree"),
    (11, "annotation"),
    (12, "acroform-field"),
    (13, "optional-content"),
    (14, "thumbnail"),
    (15, "metadata"),
    (16, "attachment"),
    (17, "incremental-update"),
    (18, "vector-outlines"),
    (19, "covered-by-a-rectangle"),
    (20, "image-pixels"),
    (21, "cid-lying-tounicode"),
    (22, "split-content-streams"),
    (24, "page-level-metadata"),
];

fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixtures = here.join("results/fixtures");
    let scratch = here.join("results/scratch");
    let _ = std::fs::create_dir_all(&scratch);
    let repo = PathBuf::from(env!("SPIKE_REPO_ROOT"));
    let arch = std::env::consts::ARCH;
    let qpdf_cli = repo
        .join("engines/vendor/src")
        .join(format!("build-qpdf-plain-{arch}"))
        .join("qpdf/qpdf");

    let engine = Qpdf::new();
    out!("## Finding 3 — the shipped `split`, run one-way over the same fixtures");
    out!();
    // THE BEFORE COLUMN IS WHAT MAKES THE AFTER COLUMN READABLE. Eight of these channels put
    // the secret somewhere I2 cannot see it in ANY state, so a "no" in the after column means
    // "the instrument is blind here", not "split removed it". Without this column the table
    // would credit the pruning with eight removals it did not perform.
    out!(
        "| # | channel | one-way split | page canary before | page canary after | carrier canary before | carrier canary after | what split did |"
    );
    out!("|--:|---|---|:--:|:--:|:--:|:--:|---|");

    let mut refused = 0usize;
    let mut carried = 0usize;
    let mut removed = 0usize;
    for (number, slug) in CHANNELS {
        let path = fixtures.join(format!("{number:02}-{slug}.pdf"));
        let Ok(bytes) = std::fs::read(&path) else {
            out!("| {number} | {slug} | fixture missing | — | — | — | — | — |");
            continue;
        };
        // The round-trip of the same fixture, so "before" and "after" differ by the split and
        // nothing else -- the same reason the main harness has three states rather than two.
        let roundtrip = plain_write(&qpdf_cli, &bytes, &scratch).unwrap_or_else(|_| bytes.clone());
        let options = OpenOptions::new(Limits::DEFAULT, Arc::new(SystemClock::new()));
        let outcome = split(
            &engine,
            bytes.into_boxed_slice(),
            Cuts::after_pages(&[]),
            &options,
        );
        match outcome {
            Ok(parts) if parts.len() == 1 => {
                let out = &parts[0];
                let needle = format!("BURROW-SECRET-{number:02}");
                let carrier = ((9..=16).contains(number) || *number == 24)
                    .then(|| format!("BURROW-CARRIER-{number:02}"));
                let Ok(before_bytes) = scan::expand(&qpdf_cli, &roundtrip, &scratch) else {
                    out!("| {number} | {slug} | ok | I2 FAILED | — | — | — | — |");
                    continue;
                };
                let Ok(after_bytes) = scan::expand(&qpdf_cli, out, &scratch) else {
                    out!("| {number} | {slug} | ok | — | I2 FAILED | — | — | — |");
                    continue;
                };
                let pb = scan::raw(&before_bytes, &needle);
                let pa = scan::raw(&after_bytes, &needle);
                let cb = carrier.as_deref().map(|c| scan::raw(&before_bytes, c));
                let ca = carrier.as_deref().map(|c| scan::raw(&after_bytes, c));
                if !scan::raw(&after_bytes, "KEEP-THIS-LINE") {
                    out!("| {number} | {slug} | **LOST THE KEEP LINE** | | | | | |");
                    continue;
                }
                // Attribution, per place, rather than one OR over both. `before` is what makes
                // `after` mean anything: I2 cannot see eight of these channels in any state.
                let mut notes: Vec<&str> = Vec::new();
                for (b, a, what) in [
                    (Some(pb), Some(pa), "the page copy"),
                    (cb, ca, "the carrier"),
                ] {
                    match (b, a) {
                        (Some(true), Some(true)) => {
                            carried += 1;
                            notes.push(match what {
                                "the page copy" => "**carried the page copy**",
                                _ => "**carried the carrier**",
                            });
                        }
                        (Some(true), Some(false)) => {
                            removed += 1;
                            notes.push(match what {
                                "the page copy" => "**removed the page copy**",
                                _ => "**removed the carrier**",
                            });
                        }
                        (Some(false), _) => notes.push("I2 blind"),
                        (Some(true), None) | (None, _) => {}
                    }
                }
                let cell = |v: Option<bool>| match v {
                    Some(true) => "yes",
                    Some(false) => "no",
                    None => "—",
                };
                out!(
                    "| {number} | {slug} | ok, {} bytes | {} | {} | {} | {} | {} |",
                    out.len(),
                    cell(Some(pb)),
                    cell(Some(pa)),
                    cell(cb),
                    cell(ca),
                    notes.join("; "),
                );
            }
            Ok(parts) => out!(
                "| {number} | {slug} | {} parts (unexpected) | — | — | — | — | — |",
                parts.len()
            ),
            Err(e) => {
                refused += 1;
                out!(
                    "| {number} | {slug} | **refused**: {e} | — | — | — | — | **refused the document** |"
                );
            }
        }
    }
    out!();
    out!();
    out!(
        "examined {} of {} channels: **{refused} refused outright**, **{removed} had the canary \
         removed** by machinery that excluded no page, **{carried} carried through**, counting \
         the page copy and the carrier separately. The rest are places I2 cannot see in any \
         state, and are marked as such rather than counted as removals.",
        CHANNELS.len(),
        CHANNELS.len()
    );
    let record = here.join("measurements").join("run-split-probe.txt");
    write_record(&record)?;
    eprintln!("record written to {}", record.display());
    Ok(())
}

/// The fixture as qpdf writes it with NO edit and NO pruning.
///
/// Through the CLI rather than through `split`, deliberately: using a one-way split as the
/// "before" state would compare the pruning against itself, which is the same
/// instance-agreeing-with-itself mistake ADR 0022's `fresh()` exists to stop.
fn plain_write(qpdf: &std::path::Path, bytes: &[u8], scratch: &std::path::Path) -> Result<Vec<u8>, String> {
    let input = scratch.join("plain-in.pdf");
    let output = scratch.join("plain-out.pdf");
    std::fs::write(&input, bytes).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&output);
    let status = std::process::Command::new(qpdf)
        .arg(&input)
        .arg(&output)
        .output()
        .map_err(|e| e.to_string())?;
    let code = status.status.code().unwrap_or(-1);
    if code != 0 && code != 3 {
        return Err(format!("qpdf exited {code}"));
    }
    std::fs::read(&output).map_err(|e| e.to_string())
}

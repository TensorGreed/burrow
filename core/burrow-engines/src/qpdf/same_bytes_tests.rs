//! R10, on the real engines: every buffer redaction's verification opens is the one it returns.
//!
//! ADR 0006's R10, as amended 2026-09-27 (#206): the bytes verification examines are PROVEN
//! identical to the bytes that are emitted. `redact::tests` holds the engine half with a fake whose
//! writes drift; this holds the wiring with the real ones -- qpdf's walk and PDFium's second
//! reading -- by recording, inside each engine's open, the sha256 of the buffer the engine is
//! handed (`hooks::record_open`), under the name of the verifier that asked for it
//! (`hooks::opening`), on the caller's thread. An open nobody named is recorded too, as
//! `Opened::Unnamed`, so a run is held to its whole sequence of opens and not to the named ones
//! alone.
//!
//! Engine-internal readings are only observable from here: `cfg(test)` hooks in this crate are not
//! compiled into the copy `burrow-ops` links, so `redaction_outcomes.rs` can see the operation's
//! frame check and not these.
//!
//! # What it runs
//!
//! Every case `tests/redaction/outcomes.tsv` records as `OK`, read from the file, with the region
//! `redaction_outcomes.rs` names by the row's label (the manifest's own, for `manifest`). Each is
//! run through `Qpdf::redact_page` under the golden file's own clock and limits, so its input must
//! be the digest the file's `INPUT` line pins and its output the digest the row pins: a region
//! read differently here would fail by name rather than measure another case.
//!
//! # What it asserts, per case
//!
//! Exactly four opens through the instrumented engine opens, in this order: the operation's own
//! open of the INPUT (unnamed: `redact::redact_page` opens its input once, through
//! `open_for_redaction`, before any step runs), the walk of the OUTPUT, the second reading's page
//! count of the INPUT, and its page read of the OUTPUT -- each digest equal to the bytes returned,
//! or to the input's. A verifier that opened its buffer and then another would show a fifth open,
//! unnamed, and fail. Then totals against the expected counts, printed.
//!
//! # What it cannot say
//!
//! What either engine does with the buffer once it holds it: the digest is of the copy Rust hands
//! over -- the boxed slice qpdf and PDFium are given -- and nothing inside the engine. Opens
//! outside the three instrumented ones (`Qpdf::open_for_redaction`, `PdfiumReading::open`, the
//! web engine's `open_for_redaction`) are not recorded: the renderer's size read of an inherited
//! `/MediaBox` opens the input through `Pdfium::open`, which no verifier uses. And only the
//! corpus's `OK` cases: a path none of them reaches is held by the fakes' structural tests in
//! `redact::tests`, not by real digests.
//!
//! **Nothing records through the web engine.** Its open carries the same recorder, but no test
//! here runs `WebRedactor`, and deleting that recorder leaves the suite green (measured in R10's
//! round 2). The web walk is held by the call site it shares with the native one,
//! `Witness::open_output` (M2web; M5 natively), and by the browser differential -- not by a
//! digest.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use burrow_types::{Clock, Limits, ManualClock};

use super::Qpdf;
use crate::PageRedactor;
use crate::pdfsyntax::region::Region;
use crate::redact::hooks::{Opened, start_recording_opens, take_opens};

/// The golden file the cases are read from, relative to the repository.
const GOLDEN: &str = "tests/redaction/outcomes.tsv";

/// The manifest the `manifest` label's regions come from, relative to the repository.
const MANIFEST: &str = "tests/redaction/manifest.toml";

/// One `OK` row of the golden file: `(file, page, covered, region label, output digest)`.
struct Row {
    file: String,
    page: usize,
    covered: BTreeSet<usize>,
    label: String,
    output: String,
}

/// Every `OK` row of the golden file, and every `INPUT` line's digest by file.
fn ok_rows(golden: &str) -> (Vec<Row>, BTreeMap<String, String>) {
    let mut rows = Vec::new();
    let mut inputs = BTreeMap::new();
    for line in golden.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        match fields.as_slice() {
            ["INPUT", file, sha] => {
                inputs.insert((*file).to_owned(), (*sha).to_owned());
            }
            ["CASE", file, page, covered, label, outcome] => {
                let Some(rest) = outcome.strip_prefix("OK ") else {
                    continue;
                };
                rows.push(Row {
                    file: (*file).to_owned(),
                    page: page.parse().expect("a page"),
                    covered: covered
                        .split(',')
                        .map(|n| n.parse().expect("a page"))
                        .collect(),
                    label: (*label).to_owned(),
                    output: rest.split(' ').next().unwrap_or_default().to_owned(),
                });
            }
            _ => {}
        }
    }
    (rows, inputs)
}

/// The manifest's `region` for each file, keyed as the golden file names documents, read as
/// `redaction_outcomes.rs` reads them: each `region` precedes its `file` in a `[[fixture]]`.
fn manifest_regions() -> BTreeMap<String, Region> {
    let text = std::fs::read_to_string(root().join(MANIFEST)).expect("the manifest");
    let value_of = |line: &str, key: &str| -> Option<String> {
        let (name, value) = line.split_once('=')?;
        (name.trim() == key).then(|| value.trim().to_owned())
    };
    let mut out = BTreeMap::new();
    let mut region: Option<Region> = None;
    for line in text.lines().map(str::trim) {
        if line == "[[fixture]]" {
            region = None;
        } else if let Some(value) = value_of(line, "region") {
            let numbers: Vec<f64> = value
                .trim_start_matches('[')
                .trim_end_matches(']')
                .split(',')
                .map(|n| n.trim().parse().expect("a region number"))
                .collect();
            let [left, top, width, height] = numbers[..] else {
                panic!("a manifest region that is not four numbers: {line}");
            };
            region = Some(Region {
                left,
                top,
                width,
                height,
            });
        } else if let Some(file) = value_of(line, "file")
            && let Some(found) = region.take()
        {
            out.insert(format!("tests/redaction/{}", file.trim_matches('"')), found);
        }
    }
    out
}

/// The regions `redaction_outcomes.rs` names by these labels. Kept equal to it by the golden
/// digest: a region that differed would produce different bytes and fail the digest below.
fn region(label: &str, file: &str, manifest: &BTreeMap<String, Region>) -> Region {
    match label {
        "manifest" => *manifest
            .get(file)
            .unwrap_or_else(|| panic!("{MANIFEST} declares no region for {file}")),
        "pdfbuild" => Region {
            left: 30.0,
            top: 68.0,
            width: 340.0,
            height: 44.0,
        },
        "whole" => Region {
            left: 0.0,
            top: 0.0,
            width: 5000.0,
            height: 5000.0,
        },
        "band" => Region {
            left: 0.0,
            top: 300.0,
            width: 5000.0,
            height: 120.0,
        },
        other => panic!("no region labelled {other}"),
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn digest(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn options() -> crate::OpenOptions<'static> {
    crate::OpenOptions::new(
        Limits::default(),
        Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
    )
}

#[test]
fn every_buffer_redaction_verifies_is_the_one_it_returns() {
    let golden = std::fs::read_to_string(root().join(GOLDEN)).expect("the golden file");
    let (rows, inputs) = ok_rows(&golden);
    let manifest = manifest_regions();
    let mut tally: BTreeMap<Opened, usize> = BTreeMap::new();
    let mut fixtures = BTreeSet::new();
    for row in &rows {
        let file = row.file.as_str();
        let input = std::fs::read(root().join(file))
            .unwrap_or_else(|error| panic!("{file}: {error}; run tools/check-redaction-corpus.sh"));
        assert_eq!(
            Some(&digest(&input)),
            inputs.get(file),
            "{file}: the input is not the one the golden file pins"
        );
        fixtures.insert(file);

        start_recording_opens();
        let result = Qpdf.redact_page(
            &input,
            row.page,
            &row.covered,
            region(&row.label, file, &manifest),
            &options(),
        );
        let opens = take_opens();
        let case = format!("{file} page {} {}", row.page, row.label);
        let (output, _) = result.unwrap_or_else(|error| panic!("{case}: {error:?}"));

        let returned = digest(&output);
        assert_eq!(
            returned, row.output,
            "{case}: the output is not the one the golden file pins"
        );
        let taken = digest(&input);
        assert_ne!(returned, taken, "{case}: the output is the input");
        let expected = vec![
            (Opened::Unnamed, taken.clone()),
            (Opened::Walk, returned.clone()),
            (Opened::PageCount, taken),
            (Opened::ReadPage, returned),
        ];
        // NAMED BY METHOD, so a failure says which reading opened what.
        let named = |opens: &[(Opened, String)], output: &str, input: &str| -> Vec<String> {
            opens
                .iter()
                .map(|(which, sha)| {
                    let what = if sha == output {
                        "the output"
                    } else if sha == input {
                        "the input"
                    } else {
                        "neither the input nor the output"
                    };
                    format!("{which:?} opened {what}")
                })
                .collect()
        };
        assert_eq!(
            opens,
            expected,
            "{case}: the verifiers opened [{}]; expected [{}]",
            named(&opens, &expected[1].1, &expected[0].1).join(", "),
            named(&expected, &expected[1].1, &expected[0].1).join(", "),
        );
        for (which, _) in &opens {
            *tally.entry(*which).or_default() += 1;
        }
    }

    // THE COUNTS, AGAINST WHAT THEY SHOULD BE: one of each -- the input's unnamed open included --
    // per `OK` row the FILE records, counted
    // from the file rather than from the loop, so a run over fewer reads as fewer.
    let recorded = golden
        .lines()
        .filter(|line| line.starts_with("CASE\t") && line.contains("\tOK "))
        .count();
    let count = |which| tally.get(&which).copied().unwrap_or(0);
    println!(
        "R10 engine readings: {} OK case(s) over {} fixture(s); {} unnamed (input) opens, {} walk \
         opens, {} read_page opens, {} page_count opens; expected {recorded} of each \
         (outcomes.tsv's OK rows)",
        rows.len(),
        fixtures.len(),
        count(Opened::Unnamed),
        count(Opened::Walk),
        count(Opened::ReadPage),
        count(Opened::PageCount),
    );
    assert!(recorded > 0, "{GOLDEN} records no OK case");
    assert_eq!(rows.len(), recorded, "OK rows read vs OK rows in the file");
    for which in [
        Opened::Unnamed,
        Opened::Walk,
        Opened::ReadPage,
        Opened::PageCount,
    ] {
        assert_eq!(
            count(which),
            recorded,
            "{which:?} opened {} buffer(s) over {recorded} OK cases",
            count(which),
        );
    }
}

#[test]
fn a_refusal_before_verification_opens_only_its_input() {
    // THE TWIN: a page past the end is refused at the constructor, before any output exists, so
    // the only open is the operation's own, unnamed, of its input -- and a recorder that named
    // every open, or a verifier that ran anyway, would say otherwise.
    let input = std::fs::read(root().join("tests/redaction/fixtures/producer-latex.pdf"))
        .expect("the fixture");
    start_recording_opens();
    let result = Qpdf.redact_page(
        &input,
        999,
        &BTreeSet::from([999]),
        region("whole", "", &BTreeMap::new()),
        &options(),
    );
    let opens = take_opens();
    let Err(error) = result else {
        panic!("page 999 does not exist");
    };
    assert!(
        format!("{error:?}").contains("[page-out-of-range]"),
        "refused for another reason: {error:?}"
    );
    assert_eq!(
        opens,
        vec![(Opened::Unnamed, digest(&input))],
        "a refusal before verification opened more than its input"
    );
}

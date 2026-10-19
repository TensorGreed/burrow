//! R10, on the real engines: every buffer redaction's verification opens is the one it returns.
//!
//! ADR 0006's R10, as amended 2026-09-27 (#206): the bytes verification examines are PROVEN
//! identical to the bytes that are emitted. `redact::tests` holds the engine half with a fake whose
//! writes drift; this holds the wiring with the real ones -- qpdf's walk and PDFium's second
//! reading -- by recording, at each OPEN, the sha256 of the buffer it was handed
//! (`hooks::record_open`), by method and on the caller's thread.
//!
//! Engine-internal readings are only observable from here: `cfg(test)` hooks in this crate are not
//! compiled into the copy `burrow-ops` links, so `redaction_outcomes.rs` can see the operation's
//! frame check and not these.
//!
//! # What it runs
//!
//! A fixed set of cases from `tests/redaction/outcomes.tsv`, each of which the file records as
//! `OK`: two producer documents, a Type 3 glyph, a form XObject, a multi-page document covered
//! whole, plain text and a CID font. Each is run through `Qpdf::redact_page` under the golden
//! file's own clock and limits, so its output must also be the digest the file pins.
//!
//! # What it asserts, per case
//!
//! Exactly three opens, in this order: the walk of the OUTPUT, the second reading's page count of
//! the INPUT, and its page read of the OUTPUT -- each digest equal to the bytes returned, or to the
//! input's. Then totals against the expected counts, printed.
//!
//! # What it cannot say
//!
//! What PDFium reads internally once `PdfiumReading::open` has copied the buffer: the digest is of
//! what Rust hands over. And only these cases: a path no case here reaches is held by the fakes'
//! structural tests in `redact::tests`, not by real digests.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use burrow_types::{Clock, Limits, ManualClock};

use super::Qpdf;
use crate::PageRedactor;
use crate::pdfsyntax::region::Region;
use crate::redact::hooks::{Opened, start_recording_opens, take_opens};

/// The golden file the cases are read from, relative to the repository.
const GOLDEN: &str = "tests/redaction/outcomes.tsv";

/// `(file, page, covered, region label)`, each an `OK` row of the golden file.
const CASES: &[(&str, usize, &str, &str)] = &[
    (
        "tests/redaction/fixtures/producer-latex.pdf",
        0,
        "0",
        "pdfbuild",
    ),
    (
        "tests/redaction/fixtures/producer-writer-untagged.pdf",
        0,
        "0",
        "whole",
    ),
    (
        "tests/conformance/fixtures/type3-glyph-draws-an-xobject.pdf",
        1,
        "1",
        "band",
    ),
    (
        "tests/redaction/generated/07-form-xobject.pdf",
        0,
        "0",
        "whole",
    ),
    (
        "tests/conformance/fixtures/pages-10.pdf",
        0,
        "0,1,2,3,4,5,6,7,8,9",
        "whole",
    ),
    (
        "tests/redaction/generated/01-plain-tj.pdf",
        0,
        "0",
        "pdfbuild",
    ),
    (
        "tests/redaction/generated/21-cid-lying-tounicode.pdf",
        0,
        "0",
        "band",
    ),
];

/// The regions `redaction_outcomes.rs` names by these labels. Kept equal to it by the golden
/// digest: a region that differed would produce different bytes and fail the digest below.
fn region(label: &str) -> Region {
    match label {
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

/// The digest the golden file pins for this case, which must be an `OK` row.
fn golden_digest(golden: &str, file: &str, page: usize, covered: &str, label: &str) -> String {
    let key = format!("CASE\t{file}\t{page}\t{covered}\t{label}\t");
    let line = golden
        .lines()
        .find(|line| line.starts_with(&key))
        .unwrap_or_else(|| panic!("{GOLDEN} has no case {key:?}"));
    let outcome = &line[key.len()..];
    let Some(rest) = outcome.strip_prefix("OK ") else {
        panic!("{GOLDEN} records {key:?} as {outcome:?}, not OK");
    };
    rest.split(' ').next().unwrap_or_default().to_owned()
}

#[test]
fn every_buffer_redaction_verifies_is_the_one_it_returns() {
    let golden = std::fs::read_to_string(root().join(GOLDEN)).expect("the golden file");
    let mut tally: std::collections::BTreeMap<Opened, usize> = std::collections::BTreeMap::new();
    for &(file, page, covered, label) in CASES {
        let input = std::fs::read(root().join(file))
            .unwrap_or_else(|error| panic!("{file}: {error}; run tools/check-redaction-corpus.sh"));
        let pages: BTreeSet<usize> = covered
            .split(',')
            .map(|n| n.parse().expect("a page"))
            .collect();

        start_recording_opens();
        let result = Qpdf.redact_page(&input, page, &pages, region(label), &options());
        let opens = take_opens();
        let (output, _) = result.unwrap_or_else(|error| panic!("{file}: {error:?}"));

        assert_eq!(
            digest(&output),
            golden_digest(&golden, file, page, covered, label),
            "{file}: the output is not the one the golden file pins"
        );
        let returned = digest(&output);
        let taken = digest(&input);
        assert_ne!(returned, taken, "{file}: the output is the input");
        let expected = vec![
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
            "{file}: the verifiers opened [{}]; expected [{}]",
            named(&opens, &expected[0].1, &expected[1].1).join(", "),
            named(&expected, &expected[0].1, &expected[1].1).join(", "),
        );
        for (which, _) in &opens {
            *tally.entry(*which).or_default() += 1;
        }
    }

    // THE COUNTS, AGAINST WHAT THEY SHOULD BE: one of each per case.
    let count = |which| tally.get(&which).copied().unwrap_or(0);
    println!(
        "R10 engine readings: {} fixtures; {} walk opens, {} read_page opens, {} page_count opens; \
         expected {n}, {n}, {n}",
        CASES.len(),
        count(Opened::Walk),
        count(Opened::ReadPage),
        count(Opened::PageCount),
        n = CASES.len()
    );
    assert_eq!(CASES.len(), 7, "the fixed set is seven cases");
    for which in [Opened::Walk, Opened::ReadPage, Opened::PageCount] {
        assert_eq!(
            count(which),
            CASES.len(),
            "{which:?} opened {} buffer(s) over {} cases",
            count(which),
            CASES.len()
        );
    }
}

#[test]
fn a_refusal_before_verification_opens_nothing() {
    // THE TWIN: the recorder records only what a verifier opened. A page past the end is refused
    // at the constructor, before any output exists, so nothing is opened -- and a recorder that
    // recorded every open in the operation would say otherwise.
    let (file, _, _, _) = CASES[0];
    let input = std::fs::read(root().join(file)).expect("the fixture");
    start_recording_opens();
    let result = Qpdf.redact_page(
        &input,
        999,
        &BTreeSet::from([999]),
        region("whole"),
        &options(),
    );
    let opens = take_opens();
    assert!(result.is_err(), "page 999 does not exist");
    assert!(opens.is_empty(), "a refusal opened {opens:?}");
}

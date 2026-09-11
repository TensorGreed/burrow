//! Golden-file tests: the committed corpus, against the committed expectations.
//!
//! The expectations live in `tests/conformance/expectations.json` at the repository root,
//! not in this file, because M1 PR 4 runs the **same** file through the web
//! implementation and asserts identical outcomes (ROADMAP M1 item 12). Restating the
//! expected results here in Rust would guarantee the two lists drift.
//!
//! Regenerate the fixtures with:
//!
//! ```bash
//! cargo run -p burrow-engines --example make-conformance-fixtures
//! ```

#![cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "../testsupport/expectations.rs"]
mod expectations;

mod support;

use std::sync::Arc;

use burrow_engines::pdfium::Pdfium;
use burrow_engines::{DocumentEngine, OpenOptions};
use burrow_types::{Limits, ManualClock, Password};
use expectations::{ErrorKind, Expect, Expectations, conformance_dir, sha256_hex};

/// The schema version this test understands. A newer file must fail, not be guessed at.
const SUPPORTED_SCHEMA: u32 = 1;

fn load() -> Expectations {
    let path = conformance_dir().join("expectations.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let expectations: Expectations =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("parsing {}: {e}", path.display()));
    assert_eq!(
        expectations.schema,
        SUPPORTED_SCHEMA,
        "{} uses schema {}, which this test does not understand",
        path.display(),
        expectations.schema
    );
    expectations
}

#[test]
fn the_expectations_file_describes_a_real_corpus() {
    let expectations = load();
    assert!(
        !expectations.cases.is_empty(),
        "an empty corpus would pass every assertion below while proving nothing"
    );
    // Both kinds must be present, or a regression that broke one of them would still show
    // the suite green.
    assert!(
        expectations
            .cases
            .iter()
            .any(|c| matches!(c.expect, Expect::Ok { .. })),
        "no case expects a successful open"
    );
    assert!(
        expectations
            .cases
            .iter()
            .any(|c| matches!(c.expect, Expect::Err(_))),
        "no case expects a typed failure"
    );
}

#[test]
fn every_fixture_matches_its_recorded_digest() {
    let dir = conformance_dir();
    for case in load().cases {
        let path = dir.join(&case.file);
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|e| panic!("case {:?}: reading {}: {e}", case.name, path.display()));
        assert_eq!(
            sha256_hex(&bytes),
            case.sha256,
            "case {:?}: {} does not match its recorded digest. If the change was \
             intended, regenerate with `cargo run -p burrow-engines --example \
             make-conformance-fixtures` and review the expectations alongside it.",
            case.name,
            path.display()
        );
    }
}

#[test]
fn every_fixture_produces_the_outcome_the_corpus_records() {
    let dir = conformance_dir();

    for case in load().cases {
        let bytes = std::fs::read(dir.join(&case.file))
            .unwrap_or_else(|e| panic!("case {:?}: {e}", case.name));

        let password = case.password.as_ref().map(|p| Password::new(p.as_bytes()));
        let mut options = OpenOptions::new(Limits::default(), Arc::new(ManualClock::new(0)));
        options.password = password.as_ref();

        let result = Pdfium::new().open(bytes.into_boxed_slice(), &options);

        match (&case.expect, result) {
            (Expect::Ok { page_count }, Ok(doc)) => {
                assert_eq!(
                    doc.pages_at_open(),
                    *page_count,
                    "case {:?}: wrong page count at open",
                    case.name
                );
                assert_eq!(
                    Pdfium::new().page_count(&doc).unwrap(),
                    *page_count,
                    "case {:?}: page_count disagreed with open",
                    case.name
                );
            }
            (Expect::Err(expected), Err(actual)) => {
                let got = ErrorKind::of(&actual).unwrap_or_else(|| {
                    panic!(
                        "case {:?}: {actual:?} is an error variant the conformance schema \
                         does not know about",
                        case.name
                    )
                });
                assert_eq!(
                    got, *expected,
                    "case {:?}: expected {expected:?}, got {got:?}",
                    case.name
                );
            }
            (Expect::Ok { page_count }, Err(actual)) => panic!(
                "case {:?}: expected {page_count} pages, got {actual:?}",
                case.name
            ),
            (Expect::Err(expected), Ok(doc)) => panic!(
                "case {:?}: expected {expected:?}, but the document opened with {} pages",
                case.name,
                doc.pages_at_open()
            ),
        }
    }
}

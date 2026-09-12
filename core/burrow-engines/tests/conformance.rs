//! Golden-file tests: the committed corpus, against the committed expectations.
//!
//! The expectations live in `tests/conformance/expectations.json` at the repository root, not
//! in this file, because `apps/web/e2e/conformance.spec.ts` runs the **same** corpus through
//! the web implementations and asserts identical outcomes (ROADMAP M1 item 12). Restating the
//! expected results here in Rust would guarantee the two lists drift.
//!
//! Regenerate the fixtures with:
//!
//! ```bash
//! cargo run -p burrow-engines --example make-conformance-fixtures
//! ```
//!
//! # This file is half of a differential harness
//!
//! It does two jobs. It asserts every case against the golden file, which is what catches a
//! behaviour change. And it **writes what it saw** to `.native-outcomes.json`, which the web
//! side downloads and diffs against its own record — catching what the golden file cannot: a
//! divergence in something nobody thought to put in the schema.
//!
//! Both are needed, and the reason is worth stating. Asserting both sides against one file is
//! transitive — if native matches and web matches then native matches web — but only as
//! complete as the schema. The direct diff has no such limit.

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

use std::collections::BTreeSet;
use std::sync::Arc;

use burrow_engines::pdfium::Pdfium;
use burrow_engines::qpdf::Qpdf;
use burrow_engines::{CheckOptions, DocumentEngine, OpenOptions, StructureEngine};
use burrow_types::{Limits, ManualClock, Password};
use expectations::{
    Case, Expectations, MILESTONES, Operation, Outcome, OutcomeRecord, Platform, RecordedOutcome,
    conformance_dir, milestone_index, outcome_of, sha256_hex,
};

/// The schema version this test understands. A newer file must fail, not be guessed at.
const SUPPORTED_SCHEMA: u32 = 2;

/// Where this run's answers are written, for the web side to diff against.
///
/// Gitignored: it is the output of a run, not a record of intent. `expectations.json` is the
/// committed contract; this is evidence that one implementation met it.
///
/// **Not a dotfile**, and it was one until CI said otherwise: `actions/upload-artifact` skips
/// hidden files unless told not to, so the record was written, never uploaded, and the web job
/// was skipped for want of it. A leading dot to mean "generated" fights the tooling for no
/// benefit — `.gitignore` is what makes it generated.
const RECORD: &str = "native-outcomes.json";

fn raw_expectations() -> String {
    let path = conformance_dir().join("expectations.json");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

fn load() -> Expectations {
    let text = raw_expectations();
    let expectations: Expectations =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("parsing expectations.json: {e}"));
    assert_eq!(
        expectations.schema, SUPPORTED_SCHEMA,
        "expectations.json uses schema {}, which this test does not understand",
        expectations.schema
    );
    expectations
}

/// Run one case through one operation, against the ceilings the case names.
///
/// **A `ManualClock` that never advances.** Nothing here may produce a duration failure: a
/// conformance outcome that depended on how fast the machine was would be a different answer
/// on every runner, and a differential harness that is not reproducible gets ignored.
fn run(case: &Case, operation: Operation, bytes: Vec<u8>) -> Outcome {
    let limits = case.limits.map_or_else(Limits::default, |l| l.to_limits());
    let clock = Arc::new(ManualClock::new(0));
    let password = case.password.as_ref().map(|p| Password::new(p.as_bytes()));

    let result = match operation {
        Operation::PageCount => {
            let mut options = OpenOptions::new(limits, clock);
            options.password = password.as_ref();
            Pdfium::new()
                .open(bytes.into_boxed_slice(), &options)
                .map(|document| document.pages_at_open())
        }
        Operation::StructureCheck => {
            let mut options = CheckOptions::new(limits, clock);
            options.password = password.as_ref();
            options.attempt_recovery = case.attempt_recovery;
            Qpdf::new()
                .check(bytes.into_boxed_slice(), &options)
                .map(|report| report.pages)
        }
    };
    outcome_of(&result)
}

/// What the case expects of `platform` for `operation`, honouring any recorded difference.
fn expected_for(case: &Case, platform: Platform, operation: Operation) -> &Outcome {
    if let Some(recorded) = case
        .platform_expectations
        .iter()
        .find(|p| p.platform == platform && p.operation == operation)
    {
        return &recorded.expect;
    }
    match operation {
        Operation::PageCount => &case.expect.page_count,
        Operation::StructureCheck => &case.expect.structure_check,
    }
}

// =====================================================================================
// Defences: the corpus describes itself honestly
// =====================================================================================

#[test]
fn the_expectations_file_describes_a_real_corpus() {
    let expectations = load();
    assert!(
        !expectations.cases.is_empty(),
        "an empty corpus would pass every assertion below while proving nothing"
    );
    // Every kind must be present, or a regression that broke one of them would still show the
    // suite green. The list grew in PR 4b: a corpus of only well-formed files and simple
    // damage was what let the adversarial fixtures live in one test each for three PRs.
    let mut saw_ok = false;
    let mut saw_failure = false;
    let mut saw_limit = false;
    let mut saw_disagreement = false;
    for case in &expectations.cases {
        for outcome in [&case.expect.page_count, &case.expect.structure_check] {
            match outcome {
                Outcome::Ok { .. } => saw_ok = true,
                Outcome::Err(failure) => {
                    saw_failure = true;
                    if failure.stage.is_some() {
                        saw_limit = true;
                    }
                }
            }
        }
        if case.expect.page_count != case.expect.structure_check {
            saw_disagreement = true;
        }
    }
    // THE ADVERSARIAL FIXTURES ARE NAMED, not counted.
    //
    // Every check above is satisfied by two well-chosen cases, and deleting a fixture together
    // with its case passes the coverage test too -- so "the corpus contains what we think it
    // does" was a property nothing held. ADR 0016 claims every adversarial file this project
    // has found runs on every PR; this is what makes that a check rather than a sentence.
    let named: BTreeSet<&str> = expectations.cases.iter().map(|c| c.file.as_str()).collect();
    for required in [
        "fixtures/xref-bomb.pdf",
        "fixtures/bomb-hidden-decoy-key.pdf",
        "fixtures/bomb-hidden-padded-dictionary.pdf",
        "fixtures/bomb-hidden-trailing-junk.pdf",
        "fixtures/objstm-bomb.pdf",
        "fixtures/object-number-above-int-max.pdf",
        "fixtures/truncated-mid-object.pdf",
        "fixtures/trailer-removed.pdf",
        "fixtures/canary.pdf",
    ] {
        assert!(
            named.contains(required),
            "{required} is no longer in the corpus. Every adversarial file this project has \
             found runs through both implementations on every PR -- removing one is a \
             deliberate act, not a tidy-up."
        );
    }

    assert!(saw_ok, "no case expects a successful open");
    assert!(saw_failure, "no case expects a typed failure");
    assert!(
        saw_limit,
        "no case expects a limit failure, so no stage is ever compared"
    );
    assert!(
        saw_disagreement,
        "no case expects the two engines to differ -- per-operation outcomes would then be \
         untested, and the files qpdf reads and PDFium refuses are the reason they exist"
    );
}

/// Every fixture has a case, and every case names a fixture that exists.
///
/// Both directions. An orphaned fixture is a file nothing runs; an orphaned case is an
/// expectation about nothing. Either one means the corpus is smaller than it looks.
#[test]
fn the_corpus_and_the_expectations_cover_each_other() {
    let dir = conformance_dir();
    let expectations = load();

    let named: BTreeSet<String> = expectations
        .cases
        .iter()
        .map(|c| c.file.trim_start_matches("fixtures/").to_owned())
        .collect();

    let mut on_disk = BTreeSet::new();
    for entry in std::fs::read_dir(dir.join("fixtures")).expect("fixtures/ should exist") {
        let entry = entry.expect("a readable directory entry");
        if entry.file_type().expect("a file type").is_file() {
            on_disk.insert(entry.file_name().to_string_lossy().into_owned());
        }
    }

    let unused: Vec<_> = on_disk.difference(&named).collect();
    assert!(
        unused.is_empty(),
        "these fixtures are on disk and no case runs them: {unused:?}"
    );
    let missing: Vec<_> = named.difference(&on_disk).collect();
    assert!(
        missing.is_empty(),
        "these cases name fixtures that do not exist: {missing:?}"
    );
}

#[test]
fn case_names_are_unique() {
    // The records are keyed by (case, operation). Two cases sharing a name would make one of
    // them invisible to the comparison, which would look exactly like agreement.
    let expectations = load();
    let mut seen = BTreeSet::new();
    for case in &expectations.cases {
        assert!(
            seen.insert(case.name.clone()),
            "duplicate case {:?}",
            case.name
        );
    }
}

/// A limit failure must record which check produced it, and an incomparable number must not
/// be recorded at all.
#[test]
fn the_schema_records_a_route_for_every_limit_failure() {
    for case in load().cases {
        // `platform_expectations` are included, and they are not an afterthought: the corpus's
        // ONLY `measured` outcome today lives in one. Checking `expect` alone let a
        // `requested` be added to that entry -- the exact field this rule exists to keep out
        // of a measured failure -- with every test still green, and the mismatch would then
        // have surfaced much later as an unexplained browser-job divergence.
        let platform_outcomes = case
            .platform_expectations
            .iter()
            .map(|p| (p.operation.as_str(), &p.expect));
        for (operation, outcome) in [
            ("page_count", &case.expect.page_count),
            ("structure_check", &case.expect.structure_check),
        ]
        .into_iter()
        .chain(platform_outcomes)
        {
            let Outcome::Err(failure) = outcome else {
                continue;
            };
            if failure.kind != expectations::ErrorKind::LimitExceeded {
                assert!(
                    failure.stage.is_none(),
                    "case {:?} ({operation}): a non-limit failure recorded a stage",
                    case.name
                );
                continue;
            }
            assert!(
                failure.limit.is_some() && failure.stage.is_some(),
                "case {:?} ({operation}): a limit failure must name both the limit and the \
                 stage, or a rejection could move between the pre-scan and the measured check \
                 with nothing noticing",
                case.name
            );
            // `measured` is the resident set on native and `HEAPU8.byteLength` on the web
            // (ADR 0007). Recording a number the two implementations cannot agree on would
            // guarantee a false divergence.
            if failure.stage.as_deref() == Some("measured") {
                assert!(
                    failure.requested.is_none(),
                    "case {:?} ({operation}): `requested` is not comparable across platforms \
                     at the measured stage and must be omitted",
                    case.name
                );
            } else {
                assert!(
                    failure.requested.is_some(),
                    "case {:?} ({operation}): every deterministic stage computes `requested` \
                     from the file, so omitting it drops the evidence of which check fired",
                    case.name
                );
            }
        }
    }
}

/// The most `known_gap` entries this corpus may carry.
///
/// A `known_gap` is green CI and an open defect at the same time. That trade is reasonable --
/// the alternative is a skipped test, and a skipped test records "untested", which is the
/// wrong memory to leave for M2 -- but it is a trade that gets easier to make every time it
/// is made. Without a ceiling, `known_gap` becomes where an inconvenient failure goes, and a
/// corpus of documented gaps asserts nothing while staying green.
///
/// Raising this number is a deliberate act in a diff a reviewer sees, which is the point.
/// It is the same shape as the licence allowlist: the constraint is worth having precisely
/// because widening it cannot be done quietly.
const MAX_KNOWN_GAPS: usize = 2;

/// A recorded gap needs somewhere to lead, and a deadline to lead there by.
#[test]
fn every_known_gap_names_an_issue_and_a_milestone() {
    let expectations = load();
    let current = milestone_index(&expectations.current_milestone).unwrap_or_else(|| {
        panic!(
            "current_milestone {:?} is not one of {:?}",
            expectations.current_milestone, MILESTONES
        )
    });

    for case in &expectations.cases {
        let Some(gap) = &case.known_gap else { continue };
        assert!(
            gap.issue.starts_with("https://"),
            "case {:?}: a known gap must link to the issue tracking it, got {:?}",
            case.name,
            gap.issue
        );
        assert!(
            gap.reason.len() > 40,
            "case {:?}: a known gap needs a reason, not a label",
            case.name
        );

        let target = milestone_index(&gap.milestone).unwrap_or_else(|| {
            panic!(
                "case {:?}: milestone {:?} is not one of {:?}",
                case.name, gap.milestone, MILESTONES
            )
        });
        assert!(
            target > current,
            "case {:?}: this gap was due by {} and we are in {}. {} is still open. \
             Fix it, or re-target it in a commit that says why the date moved -- do not \
             delete the case, which would turn a documented defect into an undocumented one",
            case.name,
            gap.milestone,
            expectations.current_milestone,
            gap.issue
        );
    }
}

/// There is a ceiling on how many defects the corpus may document rather than fix.
#[test]
fn known_gaps_are_under_the_ceiling() {
    let expectations = load();
    let gaps: Vec<&str> = expectations
        .cases
        .iter()
        .filter(|c| c.known_gap.is_some())
        .map(|c| c.name.as_str())
        .collect();

    assert!(
        gaps.len() <= MAX_KNOWN_GAPS,
        "{} known gaps, ceiling is {MAX_KNOWN_GAPS}: {}. \
         Close one before recording another, or raise MAX_KNOWN_GAPS deliberately and say \
         in the commit what changed about the trade",
        gaps.len(),
        gaps.join(", ")
    );
}

/// A recorded platform difference needs a reason that is a mechanism.
#[test]
fn every_platform_expectation_gives_a_reason() {
    for case in load().cases {
        for recorded in &case.platform_expectations {
            assert!(
                recorded.reason.len() > 40,
                "case {:?}: a platform expectation must say WHY the platforms differ",
                case.name
            );
        }
    }
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
            "case {:?}: {} does not match its recorded digest. If the change was intended, \
             regenerate with `cargo run -p burrow-engines --example \
             make-conformance-fixtures` and review the expectations alongside it.",
            case.name,
            path.display()
        );
    }
}

// =====================================================================================
// The run
// =====================================================================================

#[test]
fn every_fixture_produces_the_outcome_the_corpus_records() {
    let dir = conformance_dir();
    let expectations = load();
    let mut results = Vec::new();
    let mut compared = 0usize;
    let mut gaps = Vec::new();

    for case in &expectations.cases {
        let bytes = std::fs::read(dir.join(&case.file))
            .unwrap_or_else(|e| panic!("case {:?}: {e}", case.name));

        for operation in Operation::ALL {
            let actual = run(case, operation, bytes.clone());
            let expected = expected_for(case, Platform::Native, operation);

            // The failure message carries the case name, the digest and the two TYPED
            // outcomes. Never the fixture's bytes, never an engine's message, never the
            // canary — `tests/secret_leak.rs`'s discipline applies to CI output too, and a
            // conformance failure is exactly when someone pastes a log into an issue.
            assert_eq!(
                &actual,
                expected,
                "case {:?} ({}) diverged from the corpus\n  fixture: {} sha256 {}\n  \
                 expected: {expected:?}\n  actual:   {actual:?}",
                case.name,
                operation.as_str(),
                case.file,
                case.sha256,
            );

            results.push(RecordedOutcome {
                case: case.name.clone(),
                operation,
                outcome: actual,
            });
            compared += 1;
        }

        if let Some(gap) = &case.known_gap {
            gaps.push(format!("  {} -> {}", case.name, gap.issue));
        }
    }

    // THE COUNT. A harness that ran nothing would satisfy every assertion above, because
    // there would be no assertion left to fail. This is the one check that cannot pass
    // vacuously by construction.
    let expected_count = expectations.cases.len() * Operation::ALL.len();
    assert_eq!(
        compared, expected_count,
        "the harness made {compared} comparisons and the corpus describes {expected_count}"
    );
    assert!(compared > 0, "no comparisons were made");

    if !gaps.is_empty() {
        // Printed, not hidden. A known gap is green CI and an open defect at the same time,
        // and the only thing keeping it visible is that it says so on every run.
        println!("\nknown gaps recorded in this corpus ({}):", gaps.len());
        for gap in &gaps {
            println!("{gap}");
        }
    }

    let record = OutcomeRecord {
        platform: Platform::Native,
        runner: "native".to_owned(),
        expectations_sha256: sha256_hex(raw_expectations().as_bytes()),
        results,
    };
    let path = conformance_dir().join(RECORD);
    std::fs::write(
        &path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&record).expect("the record should serialise")
        ),
    )
    .unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
    println!("wrote {} ({compared} outcomes)", path.display());
}

// WHERE THE "BOTH PATHS RAN" CHECK LIVES, AND WHY NOT HERE
//
// An earlier version of this file had a second test that re-read the record and asserted one
// entry per case and operation. It was theatre twice over: the loop above builds the record by
// iterating cases × operations, so the property holds by construction, and as a separate
// `#[test]` it raced the one that writes the file (libtest runs tests in parallel, so it read
// a record that did not exist yet).
//
// The check is real on the other side of the harness, where the two records come from
// different processes and one of them can genuinely be short. `apps/web/e2e/compare.ts` is
// where it belongs, and it is asserted there against both records at once.

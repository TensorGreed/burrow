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

// Gated on THIS crate's `native-engines`, which forwards to the engine crate's. The
// `burrow_native_engines` cfg is set by `burrow-engines`' build script and is not visible
// here; the feature is the gate.
#![cfg(all(feature = "native-engines", target_os = "linux"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "../../burrow-engines/testsupport/expectations.rs"]
mod expectations;

use std::collections::BTreeSet;
use std::sync::Arc;

use burrow_engines::pdfium::Pdfium;
use burrow_engines::qpdf::Qpdf;
use burrow_engines::{CheckOptions, DocumentEngine, OpenOptions, StructureEngine};
use burrow_types::{Limits, ManualClock, Password};
use expectations::{
    Case, Expectations, MILESTONES, Operation, Outcome, OutcomeRecord, Platform, RecordedOutcome,
    conformance_dir, milestone_index, outcome_of, outcome_with_rotations, sha256_hex,
};

/// The schema version this test understands. A newer file must fail, not be guessed at.
const SUPPORTED_SCHEMA: u32 = 3;

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
fn run(case: &Case, operation: Operation, inputs: &[Vec<u8>]) -> Outcome {
    let limits = case.limits.map_or_else(Limits::default, |l| l.to_limits());
    let clock = Arc::new(ManualClock::new(0));
    let password = case.password.as_ref().map(|p| Password::new(p.as_bytes()));

    // The single-document operations take the FIRST input. A multi-input case that also
    // declared `page_count` would be asserting something about only one of its files, which
    // is why no case does -- and why this reads `first` explicitly rather than assuming
    // there is exactly one.
    let first = || {
        inputs
            .first()
            .unwrap_or_else(|| panic!("case {:?}: no inputs", case.name))
            .clone()
            .into_boxed_slice()
    };

    // ROTATE RECORDS MORE THAN A COUNT, so it returns early rather than joining the
    // page-count path below. A rotation cannot change the page count -- that is one of its
    // invariants -- so a case asserting only `page_count` would pass against an
    // implementation that did nothing at all.
    if operation == Operation::Rotate {
        let mut options = OpenOptions::new(limits, clock);
        options.password = password.as_ref();

        let engine = Qpdf::new();
        let outcome = burrow_engines::PageRotator::open(&engine, first(), &options)
            .and_then(|source| {
                let total = burrow_engines::PageRotator::pages(&engine, &source)?;
                // EVERY PAGE, BY 90. Fixed, as `Operation::Rotate` records: the corpus asks
                // whether the two implementations agree, and every-page-by-90 asks that as
                // well as any other selection.
                let numbers: Vec<u64> = (1..=total).collect();
                drop(source);
                burrow_ops::rotate(
                    &engine,
                    first(),
                    burrow_ops::Pages::numbered(&numbers),
                    90,
                    &options,
                )
            })
            .and_then(|out| {
                // READ BACK OUT OF THE EMITTED BYTES, never from what the operation meant to
                // do. The rotations are the readout that tells a real rotation from a no-op.
                let options = OpenOptions::new(limits, Arc::new(ManualClock::new(0)));
                let engine = Qpdf::new();
                let source =
                    burrow_engines::PageRotator::open(&engine, out.into_boxed_slice(), &options)?;
                let total = burrow_engines::PageRotator::pages(&engine, &source)?;
                let mut rotations = Vec::with_capacity(usize::try_from(total).unwrap_or(0));
                for page in 0..total {
                    rotations.push(
                        burrow_engines::PageRotator::effective_rotation(&engine, &source, page)?
                            .degrees(),
                    );
                }
                Ok((total, rotations))
            });

        return match outcome {
            Ok((total, rotations)) => outcome_with_rotations(&Ok(total), Some(rotations)),
            Err(error) => outcome_of(&Err(error)),
        };
    }

    if operation == Operation::Reorder {
        let mut options = OpenOptions::new(limits, clock);
        options.password = password.as_ref();

        let engine = Qpdf::new();
        let outcome = burrow_engines::PageReorderer::open(&engine, first(), &options)
            .and_then(|source| {
                let total = burrow_engines::PageReorderer::pages(&engine, &source)?;
                // REVERSE EVERY PAGE. Fixed, as `Operation::Reorder` records, and the same
                // shape as rotate's every-page-by-90: neither side can name a permutation
                // without first knowing how many pages there are, so both read the count and
                // reverse it.
                let order: Vec<u64> = (1..=total).rev().collect();
                drop(source);
                burrow_ops::reorder(&engine, first(), &order, &options)
            })
            .and_then(|out| {
                // READ BACK OUT OF THE EMITTED BYTES. The rotations vector is the observable
                // -- see `Operation::Reorder` for why it rather than something reorder-shaped:
                // a reversal must come back reversed, and on a fixture whose pages differ that
                // fails for any permutation except the one asked for.
                let options = OpenOptions::new(limits, Arc::new(ManualClock::new(0)));
                let engine = Qpdf::new();
                let source =
                    burrow_engines::PageRotator::open(&engine, out.into_boxed_slice(), &options)?;
                let total = burrow_engines::PageRotator::pages(&engine, &source)?;
                let mut rotations = Vec::with_capacity(usize::try_from(total).unwrap_or(0));
                for page in 0..total {
                    rotations.push(
                        burrow_engines::PageRotator::effective_rotation(&engine, &source, page)?
                            .degrees(),
                    );
                }
                Ok((total, rotations))
            });

        return match outcome {
            Ok((total, rotations)) => outcome_with_rotations(&Ok(total), Some(rotations)),
            Err(error) => outcome_of(&Err(error)),
        };
    }

    if operation == Operation::Split {
        // CUT AFTER THE FIRST PAGE, fixed like rotate's angle and reorder's reversal.
        //
        // The recorded outcome is the number of PARTS, not a page count -- and on the fixture
        // this operation exists for it is a refusal rather than either. `Operation::Split`'s
        // rustdoc has the argument: the optional-content refusal lives inside the shared
        // pruning policy, so a path that skips pruning succeeds where this expects
        // `Unsupported`, and that is the one failure a single shared policy can still have.
        let mut options = OpenOptions::new(limits, clock);
        options.password = password.as_ref();
        let result = burrow_ops::split(
            &Qpdf::new(),
            first(),
            burrow_ops::Cuts::after_pages(&[1]),
            &options,
        )
        .and_then(|parts| {
            u64::try_from(parts.len())
                .map_err(|_| burrow_types::Error::Internal("part count does not fit".to_owned()))
        });
        return outcome_of(&result);
    }

    let result = match operation {
        Operation::PageCount => {
            let mut options = OpenOptions::new(limits, clock);
            options.password = password.as_ref();
            Pdfium::new()
                .open(first(), &options)
                .map(|document| document.pages_at_open())
        }
        Operation::StructureCheck => {
            let mut options = CheckOptions::new(limits, clock);
            options.password = password.as_ref();
            options.attempt_recovery = case.attempt_recovery;
            Qpdf::new()
                .check(first(), &options)
                .map(|report| report.pages)
        }
        Operation::Merge => {
            let mut options = OpenOptions::new(limits, clock);
            options.password = password.as_ref();
            let documents = inputs
                .iter()
                .map(|b| burrow_ops::Input::new(b.clone().into_boxed_slice()))
                .collect();
            // The RECORDED outcome is the merged document's page count, read back through
            // the assembler rather than summed from the inputs. A sum is a claim about what
            // the engine did; reading the output is what it actually produced, and if the
            // two ever disagree the reading is the true one.
            burrow_ops::merge(&Qpdf::new(), documents, &options).and_then(|out| {
                let clock = Arc::new(ManualClock::new(0));
                let options = OpenOptions::new(limits, clock);
                let engine = Qpdf::new();
                let assembly = burrow_engines::PageAssembler::begin(
                    &engine,
                    out.into_boxed_slice(),
                    &options,
                )?;
                burrow_engines::PageAssembler::pages(&engine, &assembly)
            })
        }
        // Handled above: rotate records rotations as well as a count, so it returns early
        // rather than joining this path. Matched explicitly rather than with `_` so adding an
        // operation still fails to compile here -- which is how `Operation::Rotate` was
        // caught needing a runner at all.
        Operation::Rotate => unreachable!("rotate returns before this match"),
        Operation::Reorder => unreachable!("reorder returns before this match"),
        Operation::Split => unreachable!("split returns before this match"),
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
    case.expect.get(&operation).unwrap_or_else(|| {
        panic!(
            "{}: no expectation for {}. Since schema 3 a case declares the operations it is \
             about, so asking for one it does not declare is a bug in the caller rather than \
             a missing entry.",
            case.name,
            operation.as_str()
        )
    })
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
        for outcome in case.expect.values() {
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
        // A disagreement between the two single-document engines. Read from the map rather
        // than from two fields, and `and_then` rather than indexing: a case that declares
        // only one of them has no disagreement to report, which is different from having
        // none.
        if let (Some(p), Some(q)) = (
            case.expect.get(&Operation::PageCount),
            case.expect.get(&Operation::StructureCheck),
        ) && p != q
        {
            saw_disagreement = true;
        }
    }
    // THE ADVERSARIAL FIXTURES ARE NAMED, not counted.
    //
    // Every check above is satisfied by two well-chosen cases, and deleting a fixture together
    // with its case passes the coverage test too -- so "the corpus contains what we think it
    // does" was a property nothing held. ADR 0016 claims every adversarial file this project
    // has found runs on every PR; this is what makes that a check rather than a sentence.
    let named: BTreeSet<&str> = expectations
        .cases
        .iter()
        .flat_map(|c| c.inputs.iter().map(|i| i.file.as_str()))
        .collect();
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
fn the_corpus_is_not_shrinking() {
    // A CORPUS THAT GETS SMALLER IS INDISTINGUISHABLE FROM ONE THAT DID NOT, and that is not
    // hypothetical: `merge-refuses-when-the-total-passes-max-input-bytes` and
    // `…-max-pages` were hand-written into `expectations.json` and never added to
    // `make-conformance-fixtures.rs`, so the first regeneration -- rotate's, adding two cases
    // -- deleted them. The count stayed at 29 and every test still passed. Found by code
    // review, and it is exactly `CLAUDE.md`'s "a check that silently examines nothing is worse
    // than no check": the suite reported OK on a corpus two cases poorer.
    //
    // A FLOOR RATHER THAN AN EXACT COUNT. Adding a case must not need this line edited --
    // that would make the gate an obstacle and it would be widened or deleted. Removing one
    // deliberately does, which is the point: a deletion should be a decision somebody wrote
    // down, not a side effect of regenerating a file.
    const FEWEST_CASES: usize = 36;
    const FEWEST_COMPARISONS: usize = 59;

    let expectations = load();
    let comparisons: usize = expectations.cases.iter().map(|c| c.expect.len()).sum();

    assert!(
        expectations.cases.len() >= FEWEST_CASES,
        "the corpus has {} cases, fewer than the {FEWEST_CASES} recorded here. If a case was \
         removed on purpose, lower the floor in the same commit and say why; if it was not, \
         something deleted it -- check make-conformance-fixtures.rs against expectations.json.",
        expectations.cases.len()
    );
    assert!(
        comparisons >= FEWEST_COMPARISONS,
        "the corpus declares {comparisons} case x operation pairs, fewer than the \
         {FEWEST_COMPARISONS} recorded here. A case can lose an operation without losing \
         itself, which the case count alone would not see."
    );
    println!(
        "  corpus: {} cases, {comparisons} comparisons",
        expectations.cases.len()
    );
}

#[test]
fn the_corpus_and_the_expectations_cover_each_other() {
    let dir = conformance_dir();
    let expectations = load();

    let named: BTreeSet<String> = expectations
        .cases
        .iter()
        .flat_map(|c| {
            c.inputs
                .iter()
                .map(|i| i.file.trim_start_matches("fixtures/").to_owned())
        })
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
        for (operation, outcome) in case
            .expect
            .iter()
            .map(|(op, outcome)| (op.as_str(), outcome))
            .chain(platform_outcomes)
        {
            let Outcome::Err(failure) = outcome else {
                continue;
            };
            // `InputFailed` CAN carry limit detail, and only `InputFailed`. The wrapper names
            // which input failed and never what was wrong with it, so a per-input ceiling is
            // recorded as the wrapper plus the inner limit's detail -- the alternative, which
            // this corpus held until the merge page was built, was a bare `InputFailed` with
            // the limit, the stage and both numbers discarded.
            let may_carry_detail = matches!(
                failure.kind,
                expectations::ErrorKind::LimitExceeded | expectations::ErrorKind::InputFailed
            );
            if !may_carry_detail {
                assert!(
                    failure.stage.is_none() && failure.limit.is_none(),
                    "case {:?} ({operation}): a failure that cannot be a limit recorded limit \
                     detail",
                    case.name
                );
                continue;
            }
            // An `InputFailed` wrapping something that is not a limit has no detail to record,
            // and that is the ordinary case -- a malformed or encrypted input. Nothing to
            // check beyond the pairing rule below.
            if failure.kind == expectations::ErrorKind::InputFailed
                && failure.limit.is_none()
                && failure.stage.is_none()
            {
                assert!(
                    failure.requested.is_none() && failure.allowed.is_none(),
                    "case {:?} ({operation}): numbers were recorded for a failure that named \
                     no limit",
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
        for input in &case.inputs {
            let path = dir.join(&input.file);
            let bytes = std::fs::read(&path).unwrap_or_else(|e| {
                panic!("case {:?}: reading {}: {e}", case.name, path.display())
            });
            assert_eq!(
                sha256_hex(&bytes),
                input.sha256,
                "case {:?}: {} does not match its recorded digest. If the change was \
                 intended, regenerate with `cargo run -p burrow-engines --example \
                 make-conformance-fixtures` and review the expectations alongside it.",
                case.name,
                path.display()
            );
        }
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
        let inputs: Vec<Vec<u8>> = case
            .inputs
            .iter()
            .map(|i| {
                std::fs::read(dir.join(&i.file))
                    .unwrap_or_else(|e| panic!("case {:?}: {e}", case.name))
            })
            .collect();

        // ONLY the operations this case declares. Schema 3 lets a case be about one
        // operation, so running all of them would demand an expectation nobody wrote --
        // and inventing one is how a corpus stops describing what anyone decided.
        for operation in case.expect.keys().copied() {
            let actual = run(case, operation, &inputs);
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
                case.inputs
                    .iter()
                    .map(|i| i.file.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                case.inputs
                    .iter()
                    .map(|i| i.sha256.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
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
    //
    // DERIVED FROM WHAT THE CORPUS DECLARES, not from `cases * ALL`. Under schema 2 every
    // case ran every operation and the product was right; schema 3 lets a case be about one
    // operation, and keeping the product would have demanded 84 comparisons from a corpus
    // that describes 50. Summing the declarations keeps this a measurement of the corpus
    // rather than an assumption about its shape.
    let expected_count: usize = expectations.cases.iter().map(|c| c.expect.len()).sum();
    assert_eq!(
        compared, expected_count,
        "the harness made {compared} comparisons and the corpus describes {expected_count}"
    );
    assert!(compared > 0, "no comparisons were made");

    // AND EVERY OPERATION MUST APPEAR SOMEWHERE. The sum above is satisfied by a corpus
    // that quietly stopped declaring one -- delete every `merge` key and both sides fall to
    // 44 together. This is what stops an operation dropping out of the corpus entirely.
    for operation in Operation::ALL {
        assert!(
            expectations
                .cases
                .iter()
                .any(|c| c.expect.contains_key(&operation)),
            "no case declares {}, so that operation is not in the corpus at all",
            operation.as_str()
        );
    }

    // And no case may declare nothing: a fixture asserting no outcome is a fixture in the
    // corpus for decoration.
    for case in &expectations.cases {
        assert!(
            !case.expect.is_empty(),
            "case {:?} declares no operations",
            case.name
        );
    }

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

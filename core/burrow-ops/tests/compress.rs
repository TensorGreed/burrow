//! Property, golden and limit tests for `compress`, against real qpdf.
//!
//! # The invariant, from `docs/ROADMAP.md`
//!
//! > Output is never larger than the input; page count and page dimensions are unchanged; text
//! > remains extractable.
//!
//! The first clause is carried by the **type** rather than by a check: `compress` returns
//! [`Outcome`], and the only variant carrying a document is the one where it is smaller. There
//! is no code path that returns a larger document, so "never larger" is not something a test
//! has to catch — what a test has to catch is the two halves being inconsistent, which is what
//! `the_outcome_and_the_bytes_never_disagree` asserts on every generated case.
//!
//! The rest is read back out of the emitted bytes. A compressed container puts the page tree
//! inside an object stream, so every raw-byte reader in this repository goes blind on it — the
//! output is expanded through `qpdf --qdf` first, which is the same route the leak and closure
//! harnesses take and the reason they can afford it.
//!
//! **Text extractability is not tested here and does not need to be.** It holds by
//! construction: the engine sets one storage lever and touches no content stream, which
//! `compress_keeps_everything.rs` asserts directly by finding each page's operator run in the
//! decompressed output. A test here would be asserting the same thing one layer further away.

#![cfg(all(feature = "native-engines", target_os = "linux"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "../../burrow-engines/testsupport/minimal_pdf.rs"]
mod minimal_pdf;

#[path = "../../burrow-engines/testsupport/pdf_reading.rs"]
mod pdf_reading;

use std::sync::Arc;

use burrow_engines::OpenOptions;
use burrow_engines::qpdf::Qpdf;
use burrow_ops::{Outcome, compress};
use burrow_types::{Clock, Error, Limits, ManualClock, Stage};
use minimal_pdf::RotationPlacement;
use pdf_reading::{expanded, page_order, page_widths};
use proptest::prelude::*;

fn options(limits: Limits) -> OpenOptions<'static> {
    OpenOptions::new(limits, Arc::new(ManualClock::new(0)) as Arc<dyn Clock>)
}

fn run(bytes: Vec<u8>) -> Outcome {
    compress(
        &Qpdf::new(),
        bytes.into_boxed_slice(),
        &options(Limits::DEFAULT),
    )
    .expect("a well-formed document compresses")
}

/// The compressed document, insisting there is one.
///
/// # This used to fall back to the input, and that made three assertions vacuous
///
/// The first version returned the input on `NotSmaller`, so a fixture that stopped shrinking
/// would have had those tests comparing the input against itself — passing while asserting
/// nothing. The file's non-vacuity guard uses a **40-page** fixture and the tests below use a
/// **6-page** one, so the guard would have stayed green through it. Found by security review.
///
/// Panicking with a named reason is what `compress_keeps_everything.rs` already does, and it is
/// the right shape: these tests are about what compression preserves, and there is nothing to
/// examine if it did not produce anything.
fn compressed_document(outcome: &Outcome) -> Vec<u8> {
    match outcome {
        Outcome::Smaller { document, .. } => document.clone(),
        Outcome::NotSmaller {
            original_bytes,
            produced_bytes,
        } => panic!(
            "this fixture no longer shrinks ({original_bytes} -> {produced_bytes}), so the \
             assertion below would compare the input against itself and pass while examining \
             nothing"
        ),
    }
}

// ------------------------------------------------------------------ golden

#[test]
fn a_document_with_many_small_objects_gets_smaller() {
    // THE NON-VACUITY CASE. Everything else in this file is satisfied by a `compress` that
    // never shrinks anything and always answers `NotSmaller` -- including every page-count and
    // dimension assertion, which is the whole reason this file needs one test that insists on
    // the other branch.
    let input = minimal_pdf::pdf_with_page_tree(40, RotationPlacement::Absent);
    let outcome = run(input.clone());
    match outcome {
        Outcome::Smaller {
            ref document,
            original_bytes,
        } => {
            assert_eq!(original_bytes, u64::try_from(input.len()).unwrap());
            assert!(
                document.len() < input.len(),
                "{} -> {}",
                input.len(),
                document.len()
            );
        }
        Outcome::NotSmaller { .. } => panic!(
            "a 40-page document of small objects did not shrink: {outcome:?}. Either the lever \
             stopped being applied or the fixture stopped being compressible; both need looking \
             at before any other assertion here means anything."
        ),
    }
}

#[test]
fn an_already_compressed_document_is_reported_rather_than_returned() {
    // COMPRESS'S OWN OUTPUT, fed back. This is the `NotSmaller` branch reached by a real engine
    // rather than by a fake, and it is the case a person re-running the tool actually hits.
    let input = minimal_pdf::pdf_with_page_tree(40, RotationPlacement::Absent);
    let once = run(input);
    let Outcome::Smaller { document, .. } = once else {
        panic!("the fixture must shrink on the first pass for this test to mean anything");
    };

    let twice = run(document.clone());
    match twice {
        Outcome::NotSmaller {
            original_bytes,
            produced_bytes,
        } => {
            assert_eq!(original_bytes, u64::try_from(document.len()).unwrap());
            assert!(
                produced_bytes >= original_bytes,
                "NotSmaller reported a saving: {produced_bytes} < {original_bytes}"
            );
        }
        Outcome::Smaller { .. } => panic!(
            "compressing compress's own output shrank it again: {twice:?}. That is not wrong in \
             principle, but it means this test is no longer exercising the never-worse branch \
             and something else must."
        ),
    }
}

#[test]
fn the_page_count_and_the_widths_are_unchanged() {
    let input = minimal_pdf::pdf_with_page_tree(6, RotationPlacement::Absent);
    let before = page_widths(&expanded(&input));
    assert_eq!(before.len(), 6, "the fixture's widths must be readable");

    let outcome = run(input.clone());
    let after = page_widths(&expanded(&compressed_document(&outcome)));

    assert_eq!(
        after, before,
        "compression changed a page's dimensions, which it may not do"
    );
}

#[test]
fn the_page_order_is_unchanged() {
    // THE FIXTURE'S PAGES ARE DISTINGUISHABLE -- each has its own `/MediaBox` width -- which is
    // what makes this an order assertion rather than a count. Read through `expanded`, because
    // a compressed container has no page tree in its raw bytes.
    let input = minimal_pdf::pdf_with_page_tree(6, RotationPlacement::Absent);
    assert_eq!(page_order(&expanded(&input)), vec![1, 2, 3, 4, 5, 6]);

    let outcome = run(input.clone());
    assert_eq!(
        page_order(&expanded(&compressed_document(&outcome))),
        vec![1, 2, 3, 4, 5, 6],
        "compression reordered or reattributed a page"
    );
}

// ------------------------------------------------------------------ limits

#[test]
fn an_unreadable_document_is_refused_rather_than_compressed() {
    let refused = compress(
        &Qpdf::new(),
        minimal_pdf::not_a_pdf().into_boxed_slice(),
        &options(Limits::DEFAULT),
    );
    assert!(matches!(refused, Err(Error::Malformed(_))), "{refused:?}");
}

#[test]
fn a_document_over_the_page_ceiling_is_refused_with_both_numbers() {
    let limits = Limits::with(|l| l.max_pages = 5);
    let refused = compress(
        &Qpdf::new(),
        minimal_pdf::pdf_with_page_tree(6, RotationPlacement::Absent).into_boxed_slice(),
        &options(limits),
    );
    match refused {
        Err(Error::LimitExceeded {
            limit,
            stage,
            requested,
            allowed,
            ..
        }) => {
            assert_eq!(limit, "max_pages");
            assert_eq!(stage, Stage::PageCount);
            assert_eq!(requested, 6);
            assert_eq!(allowed, 5);
        }
        other => panic!("expected a page-count refusal, got {other:?}"),
    }
}

#[test]
fn an_oversized_input_is_refused_before_the_engine_sees_it() {
    let input = minimal_pdf::pdf_with_page_tree(6, RotationPlacement::Absent);
    let limits = Limits::with(|l| l.max_input_bytes = 10);
    let refused = compress(&Qpdf::new(), input.into_boxed_slice(), &options(limits));
    assert!(
        matches!(
            refused,
            Err(Error::LimitExceeded { limit, stage, .. })
                if limit == "max_input_bytes" && stage == Stage::InputSize
        ),
        "{refused:?}"
    );
}

// ------------------------------------------------------------------ property

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// The outcome and the bytes never disagree — the ROADMAP's "never larger", as a property.
    ///
    /// `Smaller` must carry a document strictly under the input's length, and `NotSmaller` must
    /// carry a produced size at or above it and no document at all. Between them there is no
    /// shape in which a caller receives a document bigger than the one it supplied.
    // FROM TWO, because `pdf_with_page_tree` builds a two-level tree and refuses one page.
    #[test]
    fn the_outcome_and_the_bytes_never_disagree(pages in 2usize..24) {
        let input = minimal_pdf::pdf_with_page_tree(pages, RotationPlacement::Absent);
        let outcome = compress(
            &Qpdf::new(),
            input.clone().into_boxed_slice(),
            &options(Limits::DEFAULT),
        ).unwrap();

        prop_assert_eq!(outcome.original_bytes(), u64::try_from(input.len()).unwrap());
        match &outcome {
            Outcome::Smaller { document, .. } => {
                prop_assert!(document.len() < input.len());
                prop_assert_eq!(outcome.produced_bytes(), u64::try_from(document.len()).unwrap());
                prop_assert!(outcome.document().is_some());
            }
            Outcome::NotSmaller { produced_bytes, original_bytes } => {
                prop_assert!(produced_bytes >= original_bytes);
                prop_assert!(outcome.document().is_none());
            }
        }
    }

    /// Whatever the outcome, every page is still there and still its own size.
    #[test]
    fn no_page_is_lost_or_resized_at_any_length(pages in 2usize..16) {
        let input = minimal_pdf::pdf_with_page_tree(pages, RotationPlacement::Absent);
        let before = page_widths(&expanded(&input));
        prop_assert_eq!(before.len(), pages);

        let outcome = compress(
            &Qpdf::new(),
            input.clone().into_boxed_slice(),
            &options(Limits::DEFAULT),
        ).unwrap();

        // INSISTED ON, not fallen back from. Measured: every case in 2..16 takes the
        // `Smaller` branch, so this never rejects today -- and if that changes, proptest says
        // so instead of the assertion quietly comparing the input with itself.
        prop_assert!(
            matches!(outcome, Outcome::Smaller { .. }),
            "a {}-page fixture stopped shrinking: {:?}",
            pages,
            outcome
        );
        let after = page_widths(&expanded(&compressed_document(&outcome)));
        prop_assert_eq!(after, before);
    }

    /// Compressing twice is stable: the second pass never claims a further saving.
    ///
    /// Not idempotence of the bytes -- qpdf rewrites the file it is asked to write, so no
    /// operation here preserves a signature. What is asserted is the useful half: once a
    /// document has been through `compress`, running it again reports `NotSmaller` rather than
    /// finding more to remove, so a person clicking twice is told the truth the second time.
    #[test]
    fn a_second_pass_finds_nothing_more(pages in 2usize..16) {
        let input = minimal_pdf::pdf_with_page_tree(pages, RotationPlacement::Absent);
        let first = compress(
            &Qpdf::new(),
            input.into_boxed_slice(),
            &options(Limits::DEFAULT),
        ).unwrap();

        // REPORTED, NOT SKIPPED. This was `if let` with no `else`, so a generated case that
        // came back `NotSmaller` was dropped on the floor and counted as a pass -- a check
        // that examined nothing while reporting success. `prop_assume!` makes proptest count
        // the rejection and say so. Measured: every case in 2..16 takes the `Smaller` branch
        // today, so the rejection count should be zero and a non-zero one is worth reading.
        prop_assume!(matches!(first, Outcome::Smaller { .. }));
        if let Outcome::Smaller { document, .. } = first {
            let second = compress(
                &Qpdf::new(),
                document.into_boxed_slice(),
                &options(Limits::DEFAULT),
            ).unwrap();
            prop_assert!(
                matches!(second, Outcome::NotSmaller { .. }),
                "a second pass found more to remove: {:?}", second
            );
        }
    }
}

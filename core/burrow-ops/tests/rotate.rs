//! Property, golden and limit tests for `rotate`, against real qpdf.
//!
//! The ROADMAP's invariants for this operation are two: **four 90° rotations return to the
//! original**, and **rotation is recorded, not re-rasterised**. The first is here; the second
//! is `rotate_keeps_everything.rs`, where the content streams are compared byte for byte,
//! because it needs the decompressing harness.

#![cfg(all(feature = "native-engines", target_os = "linux"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "../../burrow-engines/testsupport/minimal_pdf.rs"]
mod minimal_pdf;

use std::sync::Arc;

use burrow_engines::qpdf::Qpdf;
use burrow_engines::{OpenOptions, PageRotator};
use burrow_ops::{Pages, rotate};
use burrow_types::{Clock, Error, Limits, ManualClock, Result, Rotation, Stage};
use minimal_pdf::RotationPlacement;
use proptest::prelude::*;

fn options(limits: Limits) -> OpenOptions<'static> {
    OpenOptions::new(limits, Arc::new(ManualClock::new(0)) as Arc<dyn Clock>)
}

fn turn(bytes: Vec<u8>, pages: &[u64], degrees: i64) -> Result<Vec<u8>> {
    rotate(
        &Qpdf::new(),
        bytes.into_boxed_slice(),
        Pages::numbered(pages),
        degrees,
        &options(Limits::DEFAULT),
    )
}

/// Every page's effective rotation, read back out of emitted bytes.
fn rotations(bytes: &[u8]) -> Vec<i64> {
    let engine = Qpdf::new();
    let source = PageRotator::open(
        &engine,
        bytes.to_vec().into_boxed_slice(),
        &options(Limits::DEFAULT),
    )
    .expect("the output must be a document the engine can read back");
    let pages = engine.pages(&source).unwrap();
    (0..pages)
        .map(|page| {
            engine
                .effective_rotation(&source, page)
                .expect("every page must have a readable rotation")
                .degrees()
        })
        .collect()
}

#[test]
fn four_ninety_degree_rotations_return_to_the_original() {
    // THE ROADMAP INVARIANT. Asserted on the rotation read back from the emitted bytes at each
    // step, not only at the end: a version that reset to 0 on the first turn and stayed there
    // would also "return to the original".
    let original = minimal_pdf::pdf_with_page_tree(4, RotationPlacement::OnTheRoot(90));
    let before = rotations(&original);
    assert_eq!(before, vec![90, 90, 90, 90]);

    let mut current = original;
    let expected = [180, 270, 0, 90];
    for (step, want) in expected.into_iter().enumerate() {
        current = turn(current, &[1], 90).unwrap();
        assert_eq!(
            rotations(&current)[0],
            want,
            "after {} quarter turns page 1 should be at {want}",
            step + 1
        );
        // And no other page moved, at any step.
        assert_eq!(&rotations(&current)[1..], &[90, 90, 90]);
    }
}

#[test]
fn the_page_count_is_unchanged_and_no_page_is_reordered() {
    // Rotation touches an attribute. A page appearing, vanishing or moving is the failure this
    // rules out -- and the page bodies are distinct, so "in order" is checkable rather than
    // assumed.
    let original = minimal_pdf::pdf_with_page_tree(6, RotationPlacement::Absent);
    let output = turn(original, &[2, 4], 270).unwrap();
    // The rotations read back IN PAGE ORDER are the fingerprint: only pages 2 and 4 moved, and
    // they are still in positions 2 and 4. A permutation would show here.
    assert_eq!(rotations(&output), vec![0, 270, 0, 270, 0, 0]);
}

#[test]
fn the_output_is_deterministic() {
    // `qpdf_set_deterministic_ID` is what makes this true; without it every run differs in
    // `/ID` and no golden assertion about this operation could exist.
    let bytes = minimal_pdf::pdf_with_page_tree(3, RotationPlacement::Absent);
    let first = turn(bytes.clone(), &[1], 90).unwrap();
    let second = turn(bytes, &[1], 90).unwrap();
    assert_eq!(first, second);
}

#[test]
fn rotating_by_zero_changes_no_page_s_rotation() {
    let bytes = minimal_pdf::pdf_with_page_tree(4, RotationPlacement::OnTheSecondBranch(180));
    let before = rotations(&bytes);
    let after = rotations(&turn(bytes, &[1, 2, 3, 4], 0).unwrap());
    assert_eq!(before, after);
}

#[test]
fn a_document_over_the_page_ceiling_is_refused() {
    let bytes = minimal_pdf::pdf_with_page_tree(6, RotationPlacement::Absent);
    let limits = Limits::with(|l| l.max_pages = 5);
    let refused = rotate(
        &Qpdf::new(),
        bytes.into_boxed_slice(),
        Pages::numbered(&[1]),
        90,
        &options(limits),
    );
    match refused {
        Err(Error::LimitExceeded { limit, stage, .. }) => {
            assert_eq!(limit, "max_pages");
            assert_eq!(stage, Stage::PageCount);
        }
        other => panic!("expected a page-count refusal, got {other:?}"),
    }
}

#[test]
fn a_document_exactly_at_the_page_ceiling_is_allowed() {
    let bytes = minimal_pdf::pdf_with_page_tree(5, RotationPlacement::Absent);
    let limits = Limits::with(|l| l.max_pages = 5);
    assert!(
        rotate(
            &Qpdf::new(),
            bytes.into_boxed_slice(),
            Pages::numbered(&[1]),
            90,
            &options(limits),
        )
        .is_ok()
    );
}

#[test]
fn an_input_over_the_size_ceiling_is_refused() {
    let bytes = minimal_pdf::pdf_with_page_tree(4, RotationPlacement::Absent);
    let limits = Limits::with(|l| l.max_input_bytes = 16);
    match rotate(
        &Qpdf::new(),
        bytes.into_boxed_slice(),
        Pages::numbered(&[1]),
        90,
        &options(limits),
    ) {
        Err(Error::LimitExceeded { limit, stage, .. }) => {
            assert_eq!(limit, "max_input_bytes");
            assert_eq!(stage, Stage::InputSize);
        }
        other => panic!("expected a size refusal, got {other:?}"),
    }
}

#[test]
fn an_unreadable_document_is_refused_rather_than_rotated() {
    let refused = turn(minimal_pdf::not_a_pdf(), &[1], 90);
    assert!(matches!(refused, Err(Error::Malformed(_))));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]

    /// Any quarter turn, on any page, moves that page and nothing else.
    ///
    /// The generated inputs are the two things a caller actually varies: which page, and by
    /// how much. The document keeps its two-level tree with an inherited rotation, because
    /// that is the structure where "nothing else moved" can be false.
    #[test]
    fn turning_one_page_moves_that_page_and_only_that_page(
        page in 1u64..5,
        quarters in 0i64..4,
    ) {
        let degrees = quarters * 90;
        let bytes = minimal_pdf::pdf_with_page_tree(4, RotationPlacement::OnTheRoot(90));
        let output = turn(bytes, &[page], degrees).unwrap();
        let after = rotations(&output);

        let expected = Rotation::from_degrees(90 + degrees).unwrap().degrees();
        for (index, rotation) in after.iter().enumerate() {
            let page_number = u64::try_from(index).unwrap() + 1;
            if page_number == page {
                prop_assert_eq!(*rotation, expected);
            } else {
                prop_assert_eq!(*rotation, 90, "page {} was not asked to move", page_number);
            }
        }
    }

    /// Negative and over-360 arguments reach the same document as their reduced form.
    #[test]
    fn equivalent_arguments_produce_identical_documents(quarters in -8i64..8) {
        let degrees = quarters * 90;
        let reduced = Rotation::from_degrees(degrees).unwrap().degrees();
        let bytes = minimal_pdf::pdf_with_page_tree(3, RotationPlacement::Absent);

        let from_raw = turn(bytes.clone(), &[2], degrees).unwrap();
        let from_reduced = turn(bytes, &[2], reduced).unwrap();
        // BYTE FOR BYTE, not "the same rotation". Normalisation that happened to pick the
        // right quarter turn while writing a different value -- `/Rotate -90` instead of
        // `/Rotate 270` -- would pass a rotation comparison and produce a document that reads
        // differently in a viewer that does not normalise.
        prop_assert_eq!(from_raw, from_reduced);
    }
}

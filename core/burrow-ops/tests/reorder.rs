//! Property, golden and limit tests for `reorder`, against real qpdf.
//!
//! # The invariant, from `docs/ROADMAP.md`
//!
//! > The output is a permutation of the input — no page lost, added, or duplicated — and the
//! > identity permutation is a no-op.
//!
//! A page count cannot see any of that: a reorder that moved nothing, one that moved the wrong
//! pages, and one that moved the right ones all return the same number of pages. So every
//! assertion here reads the **order** back out of the emitted bytes, with each page made
//! identifiable by its `/MediaBox` width.
//!
//! The reader is `testsupport/pdf_reading.rs` rather than one written here, and that is the
//! point of the module: `merge`, `split` and the first version of the engine tests each wrote
//! their own and each rediscovered that qpdf flates content streams and spaces its arrays.
//!
//! What is *not* here: the page tree is flattened by any real permutation and left alone by
//! the identity (ADR 0021). That is measured in `burrow-engines`, where the engine's own
//! behaviour belongs, and stated in `reorder_keeps_everything.rs` as a loss the closure
//! harness names.

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
use burrow_ops::reorder;
use burrow_types::{Clock, Error, Limits, ManualClock, Result, Stage};
use minimal_pdf::RotationPlacement;
use pdf_reading::page_order;
use proptest::prelude::*;

fn options(limits: Limits) -> OpenOptions<'static> {
    OpenOptions::new(limits, Arc::new(ManualClock::new(0)) as Arc<dyn Clock>)
}

/// A document of `pages` pages, each a different width so its identity is observable.
fn document(pages: usize) -> Vec<u8> {
    minimal_pdf::pdf_with_page_tree(pages, RotationPlacement::Absent)
}

fn put(bytes: Vec<u8>, order: &[u64]) -> Result<Vec<u8>> {
    reorder(
        &Qpdf::new(),
        bytes.into_boxed_slice(),
        order,
        &options(Limits::DEFAULT),
    )
}

#[test]
fn the_fixture_s_pages_are_distinguishable_in_the_first_place() {
    // THE BASELINE, MEASURED. Every assertion below compares orders, so a fixture whose pages
    // were interchangeable -- or a reader that found nothing -- would make all of them
    // vacuous rather than failing. `rotate`'s e2e learnt this the same way.
    assert_eq!(page_order(&document(6)), vec![1, 2, 3, 4, 5, 6]);
}

#[test]
fn a_reversal_reverses_the_pages() {
    let output = put(document(6), &[6, 5, 4, 3, 2, 1]).unwrap();
    assert_eq!(page_order(&output), vec![6, 5, 4, 3, 2, 1]);
}

#[test]
fn the_identity_permutation_is_a_no_op() {
    // THE ROADMAP'S SECOND INVARIANT, asserted on the ORDER rather than on the bytes. The
    // output is not byte-identical to the input -- qpdf rewrites the file it was asked to
    // write, and no operation here preserves a signature (ADR 0021) -- so "no-op" means the
    // pages are where they were, which is what a person asked for.
    let original = document(5);
    let output = put(original.clone(), &[1, 2, 3, 4, 5]).unwrap();
    assert_eq!(page_order(&output), page_order(&original));
    assert_eq!(page_order(&output), vec![1, 2, 3, 4, 5]);
}

#[test]
fn moving_one_page_leaves_the_rest_in_their_relative_order() {
    // The commonest real request: drag page 4 to the front. Everything else must stay in
    // sequence -- an implementation that rebuilt the tree from a set would pass a page-count
    // check and fail this.
    let output = put(document(6), &[4, 1, 2, 3, 5, 6]).unwrap();
    assert_eq!(page_order(&output), vec![4, 1, 2, 3, 5, 6]);
}

#[test]
fn a_swap_of_two_adjacent_pages_moves_only_those_two() {
    let output = put(document(5), &[1, 3, 2, 4, 5]).unwrap();
    assert_eq!(page_order(&output), vec![1, 3, 2, 4, 5]);
}

#[test]
fn reordering_is_reversible() {
    // Applying a permutation and then its inverse returns the original sequence. Not the
    // original bytes -- see the identity test above for why that is a different claim.
    let original = document(6);
    let once = put(original.clone(), &[3, 6, 1, 5, 2, 4]).unwrap();
    assert_eq!(page_order(&once), vec![3, 6, 1, 5, 2, 4]);
    // The inverse: page `i` of the intermediate goes back to where it came from.
    let back = put(once, &[3, 5, 1, 6, 4, 2]).unwrap();
    assert_eq!(page_order(&back), page_order(&original));
}

#[test]
fn a_one_page_document_can_only_be_reordered_into_itself() {
    // The degenerate case, on the FLAT generator -- `pdf_with_page_tree` builds a two-level
    // tree and refuses to make one for a single page. Its pages are all US-Letter, so the
    // assertion is on the count rather than on the widths: with one page there is nothing an
    // order can distinguish anyway, and what matters is that the operation neither refuses
    // nor loses it.
    let output = put(minimal_pdf::pdf_with_pages(1), &[1]).unwrap();
    assert_eq!(page_order(&output).len(), 1);
}

#[test]
fn the_output_is_deterministic() {
    // `qpdf_set_deterministic_ID` is what makes this true; without it every run differs in
    // `/ID` and no golden assertion about this operation could exist.
    let bytes = document(4);
    let first = put(bytes.clone(), &[2, 1, 4, 3]).unwrap();
    let second = put(bytes, &[2, 1, 4, 3]).unwrap();
    assert_eq!(first, second);
}

#[test]
fn an_inherited_rotation_survives_the_reordering() {
    // THE MEASUREMENT ADR 0021 RESTS ON, at the operation layer. qpdf flattens the page tree
    // to move a page, and `pushInheritedAttributesToPage` runs first -- so a `/Rotate` that
    // lived only on the root ends up on every page and what each page DISPLAYS is unchanged.
    //
    // If a qpdf bump ever flattened without pushing, reordering would silently unrotate an
    // entire scanned document. That is the failure this asserts rather than assumes.
    let bytes = minimal_pdf::pdf_with_page_tree(6, RotationPlacement::OnTheRoot(90));
    let output = put(bytes, &[6, 5, 4, 3, 2, 1]).unwrap();
    assert_eq!(page_order(&output), vec![6, 5, 4, 3, 2, 1]);

    let text = String::from_utf8_lossy(&output);
    let rotations = text.matches("/Rotate").count();
    assert_eq!(
        rotations, 6,
        "the inherited rotation should have been pushed onto every page before the tree was \
         flattened; {rotations} page(s) carry one"
    );
}

#[test]
fn a_page_named_twice_is_refused_rather_than_duplicated() {
    // The invariant's teeth, against the real engine rather than only the fake: `[1, 1, 3]`
    // has the right length and is not a permutation.
    assert!(matches!(
        put(document(3), &[1, 1, 3]),
        Err(Error::InvalidArgument(_))
    ));
}

#[test]
fn an_order_of_the_wrong_length_is_refused() {
    assert!(matches!(
        put(document(4), &[1, 2, 3]),
        Err(Error::InvalidArgument(_))
    ));
    assert!(matches!(
        put(document(4), &[1, 2, 3, 4, 5]),
        Err(Error::InvalidArgument(_))
    ));
}

#[test]
fn page_zero_is_refused() {
    // `[0, 2, 3]`, not `[0, 1, 2]` -- the latter coerces to a DUPLICATE and is refused by a
    // different rule, so it cannot fail if the zero check is removed. This one coerces to the
    // identity, which nothing else refuses. See `src/reorder/tests.rs` for the mutation that
    // found it.
    assert!(matches!(
        put(document(3), &[0, 2, 3]),
        Err(Error::InvalidArgument(_))
    ));
}

#[test]
fn a_document_over_the_page_ceiling_is_refused() {
    let refused = reorder(
        &Qpdf::new(),
        document(6).into_boxed_slice(),
        &[1, 2, 3, 4, 5, 6],
        &options(Limits::with(|l| l.max_pages = 5)),
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
    // The other half of the boundary, so the case above measures a ceiling rather than a
    // check that always fires.
    assert!(
        reorder(
            &Qpdf::new(),
            document(5).into_boxed_slice(),
            &[5, 4, 3, 2, 1],
            &options(Limits::with(|l| l.max_pages = 5)),
        )
        .is_ok()
    );
}

#[test]
fn an_input_over_the_size_ceiling_is_refused() {
    match reorder(
        &Qpdf::new(),
        document(4).into_boxed_slice(),
        &[1, 2, 3, 4],
        &options(Limits::with(|l| l.max_input_bytes = 16)),
    ) {
        Err(Error::LimitExceeded { limit, stage, .. }) => {
            assert_eq!(limit, "max_input_bytes");
            assert_eq!(stage, Stage::InputSize);
        }
        other => panic!("expected a size refusal, got {other:?}"),
    }
}

#[test]
fn an_unreadable_document_is_refused_rather_than_reordered() {
    assert!(matches!(
        put(minimal_pdf::not_a_pdf(), &[1]),
        Err(Error::Malformed(_))
    ));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// Any permutation puts the pages in exactly the order it named.
    ///
    /// THE ROADMAP INVARIANT, generated rather than enumerated. The order read back is
    /// compared against the request in full, so a reorder that lost a page, duplicated one,
    /// or got two of them the wrong way round fails with a visible difference rather than a
    /// count that happens to match.
    #[test]
    fn any_permutation_is_carried_out_exactly(order in Just((1u64..=5).collect::<Vec<u64>>())
        .prop_shuffle())
    {
        let output = reorder(
            &Qpdf::new(),
            document(5).into_boxed_slice(),
            &order,
            &options(Limits::DEFAULT),
        ).unwrap();
        prop_assert_eq!(page_order(&output), order);
    }

    /// Every page survives, exactly once, whatever the permutation.
    ///
    /// The weaker half of the invariant, asserted separately because it is the half that
    /// still holds if the reader ever stopped understanding the tree's ORDER: a sorted
    /// comparison catches loss and duplication even then.
    #[test]
    fn no_page_is_lost_added_or_duplicated(order in Just((1u64..=6).collect::<Vec<u64>>())
        .prop_shuffle())
    {
        let output = reorder(
            &Qpdf::new(),
            document(6).into_boxed_slice(),
            &order,
            &options(Limits::DEFAULT),
        ).unwrap();
        let mut seen = page_order(&output);
        seen.sort_unstable();
        prop_assert_eq!(seen, vec![1, 2, 3, 4, 5, 6]);
    }
}

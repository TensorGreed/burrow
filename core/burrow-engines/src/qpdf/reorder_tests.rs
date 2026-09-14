//! Unit tests for the qpdf reordering engine.
//!
//! Every assertion reads the **emitted bytes** back through the engine rather than trusting
//! what the operation meant to do. A reorder that moved nothing and a reorder that moved
//! everything both return `Ok` with the same page count; the order is the only thing that
//! separates them, and it is only observable in the output.

use std::sync::Arc;

use burrow_types::{Clock, Error, Limits, ManualClock, Permutation, Result};

use super::Qpdf;
// The stepping clock lives in `rotate_tests` and is `pub(super)` so both suites share one
// definition -- a second copy would be a second thing to keep honest.
use super::rotate_tests::SteppingClock;
use crate::minimal_pdf::{self, RotationPlacement};
// THE OUTPUT READER IS SHARED, and it did not start that way. This file's first version had
// its own: it read the `(page N)` marker out of each content stream (qpdf flates those, so
// they are in the fixture and not in the output), then read `/MediaBox [0 0 ` by position
// (qpdf writes `[ 0 0 101 792 ]`, so it read the y-origin), then took the first `/Kids` array
// (right for a flattened tree, wrong for the one the identity permutation leaves behind).
// Six failures against an operation that was already working. Both of the first two lessons
// were recorded in `split.rs` in prose at the time. `pdf_reading` is the same lessons as
// functions, so the next operation inherits them instead of rediscovering them.
use crate::pdf_reading::page_order as order_of;
use crate::{OpenOptions, PageReorderer, PageRotator};

fn stopped() -> Arc<dyn Clock> {
    Arc::new(ManualClock::new(0))
}

fn options() -> OpenOptions<'static> {
    OpenOptions::new(Limits::default(), stopped())
}

fn open(bytes: Vec<u8>) -> Result<<Qpdf as PageReorderer>::Source> {
    PageReorderer::open(&Qpdf::new(), bytes.into_boxed_slice(), &options())
}

/// Reorder and hand back the emitted bytes.
fn reordered(bytes: Vec<u8>, order: &[u64]) -> Result<Vec<u8>> {
    let pages = u64::try_from(order.len()).expect("a test order fits in u64");
    let source = open(bytes)?;
    let permutation = Permutation::of(order.to_vec(), pages)?;
    Qpdf::new().reorder(&source, &permutation, &options())
}

#[test]
fn the_widths_distinguish_the_pages_in_the_first_place() {
    // THE BASELINE, MEASURED. Every assertion below compares marker orders, so a fixture whose
    // pages were indistinguishable -- or a reader that found nothing -- would make all of them
    // vacuous. `rotate`'s e2e learnt this the same way.
    let fixture = minimal_pdf::pdf_with_page_tree(6, RotationPlacement::Absent);
    assert_eq!(order_of(&fixture), vec![1, 2, 3, 4, 5, 6]);
}

#[test]
fn a_reversal_comes_back_reversed() {
    let out = reordered(
        minimal_pdf::pdf_with_page_tree(6, RotationPlacement::Absent),
        &[5, 4, 3, 2, 1, 0],
    )
    .unwrap();
    assert_eq!(order_of(&out), vec![6, 5, 4, 3, 2, 1]);
}

#[test]
fn a_single_page_moved_to_the_front_takes_only_itself() {
    // The common request -- "this page belongs first" -- and the one where an algorithm that
    // shuffles more than it was asked to shows up.
    let out = reordered(
        minimal_pdf::pdf_with_page_tree(6, RotationPlacement::Absent),
        &[3, 0, 1, 2, 4, 5],
    )
    .unwrap();
    assert_eq!(order_of(&out), vec![4, 1, 2, 3, 5, 6]);
}

#[test]
fn a_swap_of_two_middle_pages_leaves_the_rest_alone() {
    let out = reordered(
        minimal_pdf::pdf_with_page_tree(6, RotationPlacement::Absent),
        &[0, 1, 3, 2, 4, 5],
    )
    .unwrap();
    assert_eq!(order_of(&out), vec![1, 2, 4, 3, 5, 6]);
}

#[test]
fn the_identity_leaves_the_order_alone() {
    // The ROADMAP's second invariant. The operation short-circuits it, so this also pins that
    // the short-circuit produces a readable document rather than skipping the write.
    let out = reordered(
        minimal_pdf::pdf_with_page_tree(4, RotationPlacement::Absent),
        &[0, 1, 2, 3],
    )
    .unwrap();
    assert_eq!(order_of(&out), vec![1, 2, 3, 4]);
}

#[test]
fn every_page_appears_exactly_once_however_it_is_permuted() {
    // THE INVARIANT ITSELF: no page lost, added or duplicated. Asserted over several orders,
    // because a permutation that dropped a page would still produce a document.
    for order in [
        vec![0, 1, 2, 3, 4, 5],
        vec![5, 4, 3, 2, 1, 0],
        vec![2, 0, 4, 1, 5, 3],
        vec![1, 0, 2, 3, 4, 5],
    ] {
        let out = reordered(
            minimal_pdf::pdf_with_page_tree(6, RotationPlacement::Absent),
            &order,
        )
        .unwrap();
        let mut seen = order_of(&out);
        seen.sort_unstable();
        assert_eq!(seen, vec![1, 2, 3, 4, 5, 6], "order {order:?} lost a page");
    }
}

#[test]
fn reordering_twice_composes() {
    // Reversing a reversal is the identity. A cheap check that the operation is a permutation
    // rather than something that merely produces permutation-shaped output.
    let once = reordered(
        minimal_pdf::pdf_with_page_tree(5, RotationPlacement::Absent),
        &[4, 3, 2, 1, 0],
    )
    .unwrap();
    let twice = reordered(once, &[4, 3, 2, 1, 0]).unwrap();
    assert_eq!(order_of(&twice), vec![1, 2, 3, 4, 5]);
}

#[test]
fn an_inherited_rotation_survives_the_reordering() {
    // THE FIDELITY QUESTION ADR 0021 MEASURED, as a test rather than a paragraph. Moving a page
    // flattens the page tree; qpdf pushes inherited attributes down BEFORE it does, so every
    // page still displays turned. Had it flattened without pushing, reordering would silently
    // unrotate an entire scanned document -- and the page count, the page order and the object
    // count would all still look right.
    //
    // This also guards a qpdf bump: the behaviour is upstream's, not ours.
    let out = reordered(
        minimal_pdf::pdf_with_page_tree(6, RotationPlacement::OnTheRoot(90)),
        &[5, 4, 3, 2, 1, 0],
    )
    .unwrap();

    let engine = Qpdf::new();
    let source = PageRotator::open(&engine, out.into_boxed_slice(), &options()).unwrap();
    for page in 0..6 {
        assert_eq!(
            engine.effective_rotation(&source, page).unwrap().degrees(),
            90,
            "page {page} lost the rotation it inherited"
        );
    }
}

#[test]
fn the_page_tree_is_flattened_and_the_test_says_so() {
    // NOT AN ASSERTION THAT IT SHOULD BE -- an assertion that it IS, so the claim in ADR 0021
    // and on `/reorder-pdf` stays true of the code. If a qpdf bump stopped flattening, this
    // fails and the prose gets corrected rather than quietly becoming wrong.
    let before = minimal_pdf::pdf_with_page_tree(6, RotationPlacement::OnTheRoot(90));
    let after = reordered(before.clone(), &[5, 4, 3, 2, 1, 0]).unwrap();

    let count = |bytes: &[u8], needle: &str| String::from_utf8_lossy(bytes).matches(needle).count();
    assert_eq!(
        count(&before, "/Type /Pages"),
        3,
        "the fixture is two levels"
    );
    assert_eq!(
        count(&after, "/Type /Pages"),
        1,
        "the page tree was not flattened, so ADR 0021's measurement no longer holds"
    );
    // And the inherited value is now explicit on every page rather than on the root.
    assert_eq!(count(&before, "/Rotate 90"), 1);
    assert_eq!(count(&after, "/Rotate 90"), 6);
}

#[test]
fn a_permutation_for_a_different_document_is_refused() {
    let source = open(minimal_pdf::pdf_with_page_tree(
        4,
        RotationPlacement::Absent,
    ))
    .unwrap();
    // Valid on its own terms -- three pages, each named once -- and not for this document.
    let wrong = Permutation::of(vec![2, 0, 1], 3).unwrap();
    match Qpdf::new().reorder(&source, &wrong, &options()) {
        Err(Error::InvalidArgument(message)) => {
            assert!(message.contains('3') && message.contains('4'), "{message}");
        }
        other => panic!("expected a refusal naming both sizes, got {other:?}"),
    }
}

#[test]
fn an_unreadable_document_is_refused_rather_than_reordered() {
    assert!(matches!(
        open(minimal_pdf::not_a_pdf()),
        Err(Error::Malformed(_))
    ));
}

#[test]
fn a_document_over_the_page_ceiling_is_refused_at_open() {
    let limits = Limits::with(|l| l.max_pages = 5);
    let refused = PageReorderer::open(
        &Qpdf::new(),
        minimal_pdf::pdf_with_page_tree(6, RotationPlacement::Absent).into_boxed_slice(),
        &OpenOptions::new(limits, stopped()),
    );
    assert!(matches!(refused, Err(Error::LimitExceeded { .. })));
}

#[test]
fn every_object_handle_the_reordering_takes_is_released() {
    // One handle per page is held for the whole permutation, by design -- an index means
    // something different after each move, so the loop works with identities. They must all go
    // back: qpdf's handle cache only grows, and `max_memory_bytes` samples at operation
    // boundaries and would not see it.
    let source = open(minimal_pdf::pdf_with_page_tree(
        40,
        RotationPlacement::Absent,
    ))
    .unwrap();
    let baseline = super::handle::live();

    let order: Vec<u64> = (0..40).rev().collect();
    let permutation = Permutation::of(order, 40).unwrap();
    let out = Qpdf::new()
        .reorder(&source, &permutation, &options())
        .unwrap();
    assert!(!out.is_empty());

    assert_eq!(
        super::handle::live(),
        baseline,
        "the reordering left object handles alive in qpdf's cache"
    );
}

#[test]
fn the_deadline_is_checked_between_pages() {
    // `max_duration_ms` on the engine path, as `rotate` has it. The clock advances a second per
    // reading, so a 40-page permutation under a 5-second budget cannot finish -- and the
    // refusal must come from inside `reorder`, because the ops layer's checkpoints sit either
    // side of the whole call.
    let limits = Limits::with(|l| l.max_duration_ms = 5_000);
    let source = PageReorderer::open(
        &Qpdf::new(),
        minimal_pdf::pdf_with_page_tree(40, RotationPlacement::Absent).into_boxed_slice(),
        &OpenOptions::new(limits, stopped()),
    )
    .unwrap();

    let order: Vec<u64> = (0..40).rev().collect();
    let permutation = Permutation::of(order, 40).unwrap();
    let stepping: Arc<dyn Clock> = Arc::new(super::rotate_tests::SteppingClock::new(1_000));
    match Qpdf::new().reorder(&source, &permutation, &OpenOptions::new(limits, stepping)) {
        Err(Error::LimitExceeded { limit, .. }) => assert_eq!(limit, "max_duration_ms"),
        other => panic!("expected a deadline refusal, got {other:?}"),
    }
}

#[test]
fn a_deadline_refusal_part_way_through_poisons_the_document() {
    // A permutation moves a page by taking it OUT and putting it back. A refusal between those
    // two -- here a deadline, which is the reachable one -- leaves the document with a page
    // outside the tree. `reorder` takes `&self` and `&Source`, so nothing in the type system
    // stops a caller trying again and writing out a document that is silently short a page.
    //
    // Found by security review on the web module; fixed and tested on both, because it is a
    // property of the trait contract rather than of either implementation.
    let bytes = minimal_pdf::pdf_with_page_tree(6, RotationPlacement::Absent);
    // A clock that advances far enough on each read to blow the budget partway through.
    let clock: Arc<dyn Clock> = Arc::new(SteppingClock::new(400));
    let limits = Limits::with(|l| l.max_duration_ms = 1_000);
    let options = OpenOptions::new(limits, Arc::clone(&clock));

    let source = PageReorderer::open(&Qpdf::new(), bytes.into_boxed_slice(), &options)
        .expect("the document opens");
    let permutation = Permutation::of(vec![5, 4, 3, 2, 1, 0], 6).expect("a valid permutation");

    let first = Qpdf::new().reorder(&source, &permutation, &options);
    assert!(
        matches!(first, Err(Error::LimitExceeded { .. })),
        "the stepping clock must blow the deadline partway through, got {first:?}"
    );

    // THE SECOND ATTEMPT IS THE POINT. Without the poison flag this can return `Ok` with a
    // document that is missing whichever page was out of the tree when the deadline fired.
    let again = Qpdf::new().reorder(&source, &permutation, &options);
    assert!(
        matches!(again, Err(Error::Internal(_))),
        "a source left part-way through a failed reordering must refuse to be written, got \
         {again:?}"
    );
}

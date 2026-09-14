//! The structural property on `reorder`, in its inverted form: **nothing a reader can see is
//! lost** — and the one thing that *is* lost, stated by name.
//!
//! `reorder` is not a subsetting operation. Every input page appears in the output, so
//! ADR 0019 §2's rule — an output carries nothing derived from what it excluded — has nothing
//! to bite on, and the obligation runs the other way, as it does for `rotate`.
//!
//! # The third object kind, and why it is not a weakened assertion
//!
//! [ADR 0021](../../../docs/adr/0021-how-reorder-permutes-a-page-tree.md) measured what qpdf
//! does to move a page: `Pages::erase` calls `findPage`, which "ensures flat /Pages", and
//! `flattenPagesTree` begins with `pushInheritedAttributesToPage(true, true)`. So the
//! intermediate `/Pages` nodes are gone from the output, and everything they were carrying for
//! their pages has been pushed down onto the pages first.
//!
//! That is a real loss. The fixture therefore declares a **third kind** — `page-tree`, for
//! structure whose only job is to hold other pages — and this file requires `content` and
//! `navigation` whole while asserting that the `page-tree` objects are the ones that went.
//!
//! **The harness is not weakened to accommodate it.** `assert_nothing_lost` still requires
//! every object it is handed. The alternative — relaxing it to a subset check — would have
//! quietly relaxed it for `rotate` and `compress` too, which is why ADR 0021 records this as
//! the harder of the two routes and takes it anyway.

#![cfg(all(feature = "native-engines", target_os = "linux"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

/// The shared harness, included by path rather than copied.
#[path = "../../burrow-engines/testsupport/object_closure.rs"]
mod object_closure;

use std::sync::Arc;

use burrow_engines::OpenOptions;
use burrow_engines::qpdf::Qpdf;
use burrow_ops::reorder;
use burrow_types::{Clock, Limits, ManualClock};
use object_closure::{Marked, assert_nothing_lost, expanded, marked_document, owned_by};

/// What a reader can observe, and what `reorder` may therefore never lose.
const OBSERVABLE: &[&str] = &["content", "navigation"];

/// The structure that exists only to hold pages, which qpdf flattens away.
const SCAFFOLDING: &[&str] = &["page-tree"];

fn options() -> OpenOptions<'static> {
    OpenOptions::new(
        Limits::DEFAULT,
        Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
    )
}

fn reorder_marked(order: &[u64]) -> Vec<u8> {
    reorder(
        &Qpdf::new(),
        marked_document().bytes.into_boxed_slice(),
        order,
        &options(),
    )
    .expect("the marked document must reorder")
}

fn all_pages(marked: &Marked) -> Vec<u64> {
    (1..=marked.pages).collect()
}

#[test]
fn the_fixture_declares_all_three_kinds() {
    // THE BASELINE. Every assertion below is about a kind, so a fixture that stopped declaring
    // one would make them vacuous rather than failing -- `assert_nothing_lost` refuses an
    // empty requirement, but a test asking about a kind nobody declares would simply find
    // nothing missing and report success.
    let marked = marked_document();
    let pages = all_pages(&marked);
    for kind in OBSERVABLE.iter().chain(SCAFFOLDING) {
        let declared = owned_by(&marked, &pages, &[kind]);
        assert!(
            !declared.is_empty(),
            "the fixture declares no {kind:?} objects owned by any page, so every assertion \
             about that kind measures nothing"
        );
    }
    // And the tree really is two levels deep, which is what makes a flattening observable at
    // all. A flat tree flattens into itself and the loss below could never be measured.
    assert_eq!(
        owned_by(&marked, &pages, SCAFFOLDING).len(),
        2,
        "the marked fixture should have two intermediate /Pages nodes"
    );
}

#[test]
fn reversing_the_pages_loses_nothing_a_reader_can_see() {
    let marked = marked_document();
    let pages = all_pages(&marked);
    let must = owned_by(&marked, &pages, OBSERVABLE);

    // Every page moves. The outline entries, the form field, the shared annotation array's
    // members, the article thread and the resource only page 4 draws all have to come through.
    let order: Vec<u64> = pages.iter().rev().copied().collect();
    let output = reorder_marked(&order);
    assert_nothing_lost(&marked, &output, &must, "reverse every page");
}

#[test]
fn moving_one_page_loses_nothing_a_reader_can_see_either() {
    let marked = marked_document();
    let pages = all_pages(&marked);
    let must = owned_by(&marked, &pages, OBSERVABLE);

    // The commonest real request, and a different code path from the reversal: most pages are
    // already where they belong, so most iterations take the "already in place" branch.
    let mut order = pages.clone();
    let moved = order.remove(3);
    order.insert(0, moved);
    let output = reorder_marked(&order);
    assert_nothing_lost(&marked, &output, &must, "move page 4 to the front");
}

#[test]
fn the_identity_permutation_loses_nothing_at_all_including_the_page_tree() {
    // THE ASYMMETRY ADR 0021 RECORDS. Nothing MOVES for the identity, so nothing calls into
    // qpdf's page machinery and the two-level tree survives intact -- which makes this the one
    // permutation for which the third kind is required rather than exempt.
    //
    // Note what does the work: it is "no page moved", not the `is_identity` short-circuit in
    // `reorder.rs`. That branch is an optimisation and this test passes with it removed; code
    // review measured it, and the comment here used to credit the wrong thing.
    //
    // Asserted rather than described, because it is surprising: the output's structure depends
    // on whether anything actually moved.
    let marked = marked_document();
    let pages = all_pages(&marked);
    let everything = owned_by(&marked, &pages, &["content", "navigation", "page-tree"]);

    let output = reorder_marked(&pages);
    assert_nothing_lost(&marked, &output, &everything, "the identity permutation");
}

#[test]
fn a_real_permutation_loses_the_page_tree_scaffolding_and_says_so() {
    // THE STATED LOSS, measured rather than assumed. If qpdf ever stopped flattening, this
    // fails -- and the right response would be to delete this test and require the third kind
    // everywhere, not to widen it. A test that accepted either outcome would be no test.
    let marked = marked_document();
    let pages = all_pages(&marked);
    let scaffolding = owned_by(&marked, &pages, SCAFFOLDING);

    let order: Vec<u64> = pages.iter().rev().copied().collect();
    let output = reorder_marked(&order);

    // Decompress first, so a missing `qpdf` CLI fails loudly here rather than arriving as a
    // scan that found nothing and reported no loss. `rotate_keeps_everything.rs` records why.
    let _ = expanded(&output);

    let survivors = object_closure::survivors(&marked, &output);
    let still_there: Vec<u64> = scaffolding
        .iter()
        .copied()
        .filter(|n| survivors.contains(n))
        .collect();
    assert!(
        still_there.is_empty(),
        "reorder is documented (ADR 0021) as flattening the page tree, but the intermediate \
         /Pages node(s) {still_there:?} survived. Either qpdf stopped flattening — in which \
         case the third object kind is no longer needed and this test should go — or the \
         permutation did not actually move anything."
    );
    println!(
        "  reverse every page: {} page-tree object(s) flattened away, as ADR 0021 records",
        scaffolding.len()
    );
}

#[test]
fn the_nothing_lost_assertion_can_fail_on_this_fixture() {
    // THE CONTROL. `assert_nothing_lost` passing says nothing unless it is capable of failing
    // on this document, with this harness, in this test binary. `split`'s first leak test was
    // green on a fixture where the leak it looked for could not occur.
    //
    // The deliberate loser is a REORDER required to keep the page tree as well -- which is
    // exactly the requirement the three tests above are careful not to make, so this control
    // also proves that the exemption is doing work rather than being redundant.
    let marked = marked_document();
    let pages = all_pages(&marked);
    let everything = owned_by(&marked, &pages, &["content", "navigation", "page-tree"]);
    assert!(
        !everything.is_empty(),
        "the fixture declares no objects at all, so nothing above measures anything"
    );

    let order: Vec<u64> = pages.iter().rev().copied().collect();
    let output = reorder_marked(&order);
    let _ = expanded(&output);

    let outcome = std::panic::catch_unwind(|| {
        assert_nothing_lost(
            &marked,
            &output,
            &everything,
            "control: a reorder required to keep its page tree",
        );
    });
    let message = match outcome {
        Ok(()) => panic!(
            "the control did not fail: a real permutation flattens the page tree (ADR 0021), \
             so requiring the page-tree objects must be detected. If this passes, \
             assert_nothing_lost is not measuring loss and the tests above prove nothing."
        ),
        Err(payload) => payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_default(),
    };
    // AND IT FAILED FOR THE RIGHT REASON. Any panic satisfies `is_err`; only this one means
    // the harness detected loss.
    assert!(
        message.contains("had to survive"),
        "the control panicked, but not because objects were lost: {message}"
    );
}

#[test]
fn a_damaged_document_is_refused_rather_than_losing_a_page_silently() {
    // ISSUE #61, TAKEN. The document opens as five pages and writes as four; ADR 0022's
    // verification reads the output back through a fresh engine, sees four where five were
    // promised, and refuses. That meets #61's own stated bar -- "a refusal would not block;
    // losing the page quietly does" -- and it is separable from the recovery-posture question
    // of WHICH of qpdf's two readings is right, which is still open on the issue.
    //
    // Found by the `reorder` fuzz target once its corpus was seeded with the conformance
    // fixtures. It is not reorder's: a rotation by zero degrees on the same bytes loses the
    // same page, which is why the fix is the shared step and not a guard here.
    let bytes = damaged();
    let engine = Qpdf::new();
    let source =
        burrow_engines::PageReorderer::open(&engine, bytes.clone().into_boxed_slice(), &options())
            .expect("the document opens -- that is the whole point of this input class");
    let opened = burrow_engines::PageReorderer::pages(&engine, &source).expect("a page count");
    assert_eq!(
        opened, 5,
        "the document declares 6 pages in /Count and yields 5 that resolve"
    );

    // The IDENTITY permutation: no page moves, so this is a plain write of what was opened.
    let order: Vec<u64> = (1..=opened).collect();
    let err = reorder(&engine, bytes.into_boxed_slice(), &order, &options())
        .expect_err("a document short of a page must not reach the caller");

    match err {
        burrow_types::Error::OutputRejected(why) => {
            // The MESSAGE, not only the variant. A refusal that does not say which two numbers
            // disagreed sends someone back to the engine to find out.
            assert!(
                why.contains('5') && why.contains('4'),
                "the refusal must name both counts: {why}"
            );
        }
        other => panic!("expected OutputRejected, got {other:?}"),
    }
}

#[test]
#[ignore = "issue #61: the ENGINE still loses the page. The operation now refuses the output \
            (ADR 0022), so the defect is no longer reachable by a caller -- but it is not \
            fixed, and this pins the behaviour so CI goes red the day it moves."]
fn a_damaged_document_loses_pages_on_write() {
    // THE DEFECT BENEATH THE REFUSAL, asserted as it is rather than as it should be. The test
    // above proves nobody receives the short document; this one proves the short document is
    // still what qpdf writes, at the engine seam where verification does not run.
    //
    // It fails in EITHER direction -- a qpdf that keeps all five pages and a qpdf that errors
    // both go red here -- which is what makes it a pin rather than a note. Whichever happens,
    // #61's recovery-posture decision has moved and needs re-measuring.
    let bytes = damaged();
    let engine = Qpdf::new();
    let source = burrow_engines::PageReorderer::open(&engine, bytes.into_boxed_slice(), &options())
        .expect("the document opens");
    let opened = burrow_engines::PageReorderer::pages(&engine, &source).expect("a page count");
    assert_eq!(opened, 5, "five pages resolve out of the six declared");

    let order = burrow_types::Permutation::of((0..opened).collect(), opened).expect("identity");
    let written = burrow_engines::PageReorderer::reorder(&engine, &source, &order, &options())
        .expect("and it writes without an error, which is the defect");

    let reopened =
        burrow_engines::PageReorderer::open(&engine, written.into_boxed_slice(), &options())
            .expect("the output is a valid document");
    let after = burrow_engines::PageReorderer::pages(&engine, &reopened).expect("a page count");

    assert_eq!(
        after, 4,
        "issue #61: writing this document loses a page silently at the engine seam. If this \
         fails with {opened}, the loss is fixed -- close #61, delete this test and move the \
         fixture into the conformance corpus; if it fails some other way, re-measure."
    );
    assert!(
        after < opened,
        "the point of the case: fewer pages came out than went in, with no error anywhere"
    );
}

/// The committed reproduction for #61, read once by both tests above.
fn damaged() -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/damaged/page-loss-on-write.pdf");
    std::fs::read(&path).expect("the committed reproduction must be readable")
}

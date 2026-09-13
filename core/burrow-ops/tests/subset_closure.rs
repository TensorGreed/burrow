//! The structural property, on `split`: an output carries no object belonging only to pages it
//! excluded.
//!
//! **This is the test ADR 0019 §2 needs and the canary version never was.** The canary test
//! plants markers by feature and confirms the enumeration it was given; this asserts a property
//! over *every* object in the source, so it fails for categories nobody named.
//!
//! The harness is shared (`core/burrow-engines/testsupport/object_closure.rs`) rather than
//! written here, because the next three operations need it too — and they need it pointing the
//! other way. `rotate`, `reorder` and `compress` are not subsetting operations: every page
//! survives, so *every* marked object should, and `assert_closed` with all pages included says
//! exactly that.
//!
//! The six named channels from ADR 0019 §2a stay as regression cases on top, in
//! `split_no_leak.rs`. They catch what this cannot: a leak **inside** an object that
//! legitimately survives, such as a form field owned by two pages carrying the value somebody
//! typed on the excluded one.

#![cfg(all(feature = "native-engines", target_os = "linux"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

/// The shared harness, included by path rather than copied.
///
/// Same reason `minimal_pdf.rs` is included this way from three places: two copies agree until
/// the day they do not, and this one is about to be inherited by three more operations.
#[path = "../../burrow-engines/testsupport/object_closure.rs"]
mod object_closure;

use std::sync::Arc;

use burrow_engines::OpenOptions;
use burrow_engines::qpdf::Qpdf;
use burrow_ops::{Cuts, split};
use burrow_types::{Clock, Limits, ManualClock};
use object_closure::{assert_closed, assert_nothing_lost, marked_document, owned_by, trespassers};

fn options() -> OpenOptions<'static> {
    OpenOptions::new(
        Limits::DEFAULT,
        Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
    )
}

fn split_marked(cuts: &[u64]) -> Vec<Vec<u8>> {
    split(
        &Qpdf::new(),
        marked_document().bytes.into_boxed_slice(),
        Cuts::after_pages(cuts),
        &options(),
    )
    .expect("the marked document must split")
}

#[test]
#[ignore = "ADR 0019 §2a: the build route still carries objects from excluded pages. This is \
            the structural test that says so — it is ignored rather than deleted so the gap \
            is visible in the suite. See issue #54."]
fn every_surviving_object_belongs_to_a_page_the_output_contains() {
    let marked = marked_document();
    let outputs = split_marked(&[1, 3]);
    // Pages 1 | 2-3 | 4-5.
    for (output, included) in outputs.iter().zip([vec![1u64], vec![2, 3], vec![4, 5]]) {
        assert_closed(&marked, output, &included, "split");
    }
}

#[test]
fn the_closure_property_is_violated_exactly_where_the_adr_records_it() {
    // THE GAP, PINNED STRUCTURALLY. While the rule is unmet, this is the assertion that keeps
    // the record and the code honest with each other: it fails if a channel closes without
    // ADR 0019 §2a being updated, and it fails if the scan goes blind — two things that look
    // identical from a green run.
    let marked = marked_document();
    let outputs = split_marked(&[1, 3]);

    let leaking = trespassers(&marked, &outputs[0], &[1]);
    assert!(
        !leaking.is_empty(),
        "the page-1-only output carried nothing it should not have. Either the pruning in \
         issue #54 has landed — in which case un-ignore the test above, delete this one and \
         amend ADR 0019 §2a — or the harness has stopped finding markers."
    );

    // The harness must be *finding* things, not merely failing to. Without this, a manifest
    // that had stopped listing objects would satisfy the assertion above by accident.
    let survived = object_closure::survivors(&marked, &outputs[0]);
    assert!(
        survived.len() > leaking.len(),
        "every surviving object is a trespasser, which means the scan is matching everything \
         rather than the markers it was given"
    );
}

#[test]
fn a_one_way_split_loses_no_page_content() {
    // THE INVERSE ASSERTION, and the shape `rotate`, `reorder` and `compress` will use.
    //
    // The first version of this called `assert_closed` with every page included and claimed to
    // be "the control". It could not fail: with every page included the trespasser list is
    // mathematically empty whatever the operation did. Code review measured it by handing the
    // page-1-only output to `assert_closed` while claiming all five pages — a must-lose-nothing
    // operation losing four pages in five, accepted.
    //
    // `assert_nothing_lost` names what has to be there instead. A one-way split legitimately
    // drops the catalog furniture ADR 0019 §1 drops, so the required set is what belongs to the
    // pages rather than everything marked.
    let marked = marked_document();
    let outputs = split_marked(&[]);
    assert_eq!(outputs.len(), 1);

    let all_pages: Vec<u64> = (1..=marked.pages).collect();
    assert_nothing_lost(
        &marked,
        &outputs[0],
        // CONTENT ONLY. A split may drop navigation — ADR 0019 §1 drops outlines and the page
        // says so — and requiring it here would fail a correct implementation, which is how
        // this test first failed: 18 of 23, the five missing being outline entries.
        &owned_by(&marked, &all_pages, &["content"]),
        "one-way split",
    );
}

#[test]
fn losing_a_page_is_caught_by_the_inverse_assertion() {
    // THE CONTROL FOR THE CONTROL. Without it, `assert_nothing_lost` passing would say nothing
    // about whether it can fail -- which is exactly what went wrong with the assertion it
    // replaced. A two-way split's first part must NOT satisfy a requirement built from all
    // five pages, because four of them are not in it.
    let marked = marked_document();
    let outputs = split_marked(&[1]);
    let all_pages: Vec<u64> = (1..=marked.pages).collect();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert_nothing_lost(
            &marked,
            &outputs[0],
            &owned_by(&marked, &all_pages, &["content"]),
            "deliberately wrong claim",
        );
    }));
    assert!(
        result.is_err(),
        "a one-page output satisfied a requirement naming all five pages' objects, so \
         assert_nothing_lost cannot detect loss and every use of it is decoration"
    );
}

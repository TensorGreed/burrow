//! The structural property on `rotate`, in its inverted form: **nothing is lost**.
//!
//! `rotate` is not a subsetting operation. Every input page appears in the output, so ADR 0019
//! §2's rule — an output carries nothing derived from what it excluded — has nothing to bite
//! on, and the obligation runs the other way. The same shared harness says so: `assert_closed`
//! asks whether anything trespassed and is vacuous when every page is included;
//! `assert_nothing_lost` asks whether anything vanished, which is the question here.
//!
//! Both `content` and `navigation` are required. A subsetting operation may drop the
//! catalog-level furniture (ADR 0019 §1 does, and `/split-pdf` says so); a rotation may not,
//! because nothing it did could justify losing an outline.
//!
//! # And content streams come out byte-identical
//!
//! The closure harness cannot see this. Rotation changes an attribute on the page dictionary,
//! so a page's content stream must survive **unchanged**: an implementation that round-tripped
//! content through a filter would keep every object, keep every page, set every `/Rotate`
//! correctly, and quietly recompress somebody's scan. The comparison is over the decompressed
//! stream bodies of the original and the output, per page.

#![cfg(all(feature = "native-engines", target_os = "linux"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

/// The shared harness, included by path rather than copied — as `subset_closure.rs` does.
#[path = "../../burrow-engines/testsupport/object_closure.rs"]
mod object_closure;

/// The generated fixtures, likewise.
#[path = "../../burrow-engines/testsupport/minimal_pdf.rs"]
mod minimal_pdf;

use std::sync::Arc;

use burrow_engines::OpenOptions;
use burrow_engines::qpdf::Qpdf;
use burrow_ops::{Pages, rotate};
use burrow_types::{Clock, Limits, ManualClock};
use minimal_pdf::RotationPlacement;
use object_closure::{assert_nothing_lost, expanded, marked_document, owned_by};

fn options() -> OpenOptions<'static> {
    OpenOptions::new(
        Limits::DEFAULT,
        Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
    )
}

fn rotate_marked(pages: &[u64], degrees: i64) -> Vec<u8> {
    rotate(
        &Qpdf::new(),
        marked_document().bytes.into_boxed_slice(),
        Pages::numbered(pages),
        degrees,
        &options(),
    )
    .expect("the marked document must rotate")
}

#[test]
fn rotating_one_page_loses_no_object_from_any_page() {
    let marked = marked_document();
    let all: Vec<u64> = (1..=marked.pages).collect();
    let must = owned_by(&marked, &all, &["content", "navigation"]);

    let output = rotate_marked(&[1], 90);
    assert_nothing_lost(&marked, &output, &must, "rotate page 1 by 90");
}

#[test]
fn rotating_every_page_loses_no_object_either() {
    let marked = marked_document();
    let all: Vec<u64> = (1..=marked.pages).collect();
    let must = owned_by(&marked, &all, &["content", "navigation"]);

    let output = rotate_marked(&all, 180);
    assert_nothing_lost(&marked, &output, &must, "rotate every page by 180");
}

#[test]
fn the_nothing_lost_assertion_can_fail_on_this_fixture() {
    // THE CONTROL. `assert_nothing_lost` passing says nothing unless it is capable of failing
    // on this document, with this harness, in this test binary -- `split`'s first leak test
    // was green on a fixture where the leak it looked for could not occur.
    //
    // A one-way `split` is the deliberate loser here: ADR 0019 §1 drops the catalog-level
    // furniture, so the `navigation` objects go. Requiring them of a split must fail; if this
    // ever passes, the harness has stopped detecting loss and the two tests above are
    // decoration.
    let marked = marked_document();
    let all: Vec<u64> = (1..=marked.pages).collect();
    let must = owned_by(&marked, &all, &["content", "navigation"]);
    assert!(
        !must.is_empty(),
        "the fixture declares no objects at all, so nothing above measures anything"
    );

    let split_output = burrow_ops::split(
        &Qpdf::new(),
        marked.bytes.clone().into_boxed_slice(),
        burrow_ops::Cuts::after_pages(&[]),
        &options(),
    )
    .expect("the marked document must split")
    .remove(0);

    // DECOMPRESS FIRST, OUTSIDE THE CONTROL. `catch_unwind` treats every panic alike, and the
    // harness panics when the `qpdf` CLI is missing -- so on a machine without it this control
    // passed while measuring nothing at all, which is the exact failure mode it exists to rule
    // out. Doing the expansion here means a missing CLI fails loudly instead of arriving as a
    // green control.
    let _ = expanded(&split_output);

    let outcome = std::panic::catch_unwind(|| {
        assert_nothing_lost(&marked, &split_output, &must, "control: a one-way split");
    });
    let message = match outcome {
        Ok(()) => panic!(
            "the control did not fail: a one-way split drops navigation objects (ADR 0019 §1), \
             so requiring them must be detected. If this passes, assert_nothing_lost is not \
             measuring loss and the tests above prove nothing."
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
fn every_page_s_content_stream_comes_out_byte_identical() {
    // Rotation sets an attribute. Re-encoding content would keep every object, keep every
    // page, set every `/Rotate` correctly -- and resample the document. Compared decompressed,
    // through the same `qpdf --qdf` expansion the harness uses, so the comparison is about the
    // content rather than about which filter each side happened to choose.
    let original = minimal_pdf::pdf_with_page_tree(6, RotationPlacement::OnTheRoot(90));

    let output = rotate(
        &Qpdf::new(),
        original.clone().into_boxed_slice(),
        Pages::numbered(&[2, 5]),
        90,
        &options(),
    )
    .expect("the fixture must rotate");

    let before = expanded(&original);
    let after = expanded(&output);

    for page in 1..=6 {
        let needle = format!("(page {page})");
        assert!(
            contains(&before, needle.as_bytes()),
            "the fixture's own content stream for page {page} is not in the expanded input, so \
             this test is comparing nothing"
        );
        assert!(
            contains(&after, needle.as_bytes()),
            "page {page}'s content stream did not survive the rotation byte for byte"
        );
    }

    // And the whole stream body, not just the marker inside it: a re-encode that preserved the
    // text while changing the operators around it would pass the check above.
    for page in 1..=6 {
        let body = format!("BT /F1 12 Tf 72 720 Td (page {page}) Tj ET");
        assert!(
            contains(&after, body.as_bytes()),
            "page {page}'s content stream was rewritten rather than carried through"
        );
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

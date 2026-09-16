//! The structural property on `compress`, in its inverted form: **nothing is lost**.
//!
//! `compress` is not a subsetting operation. Every input page appears in the output, so
//! ADR 0019 §2's rule — an output carries nothing derived from what it excluded — has nothing
//! to bite on, and the obligation runs the other way. The same shared harness says so:
//! `assert_closed` asks whether anything trespassed and is vacuous when every page is included;
//! `assert_nothing_lost` asks whether anything vanished, which is the question here.
//!
//! Both `content` and `navigation` are required. A subsetting operation may drop the
//! catalog-level furniture (ADR 0019 §1 does); a re-encoding may not, because nothing about
//! storing objects differently could justify losing an outline.
//!
//! # This is where compress's real check lives
//!
//! `verify::Expected::Compressed`'s rustdoc says at length that a page count and a rotation
//! vector see almost nothing of what compression touches: a content stream re-encoded, an image
//! resampled, a font dropped — all invisible to the runtime check. ADR 0022 rejected
//! decompressing every output in production, so the residue is carried here, as a test that can
//! afford `qpdf --qdf` and a marked fixture.
//!
//! **The claim about content streams is a substring search, not a whole-body comparison.** Each
//! page's operator run is looked for in the decompressed output; a re-encode that preserved
//! those bytes and changed everything around them would pass. That is narrower than "byte for
//! byte", and it is the claim the code makes — the same wording `rotate_keeps_everything.rs`
//! settled on after code review caught the overclaim there.

#![cfg(all(feature = "native-engines", target_os = "linux"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

/// The shared harness, included by path rather than copied — as `rotate_keeps_everything.rs` does.
#[path = "../../burrow-engines/testsupport/object_closure.rs"]
mod object_closure;

/// The generated fixtures, likewise.
#[path = "../../burrow-engines/testsupport/minimal_pdf.rs"]
mod minimal_pdf;

use std::sync::Arc;

use burrow_engines::OpenOptions;
use burrow_engines::qpdf::Qpdf;
use burrow_ops::{Outcome, compress};
use burrow_types::{Clock, Limits, ManualClock};
use minimal_pdf::RotationPlacement;
use object_closure::{assert_nothing_lost, expanded, marked_document, owned_by};

fn options() -> OpenOptions<'static> {
    OpenOptions::new(
        Limits::DEFAULT,
        Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
    )
}

/// Compress, and insist on getting a document rather than a `NotSmaller`.
///
/// If the marked fixture ever stops shrinking, these tests must fail loudly rather than
/// quietly asserting nothing — a `NotSmaller` here would leave every `assert_nothing_lost`
/// below with no output to examine.
fn compress_marked(bytes: Vec<u8>) -> Vec<u8> {
    match compress(&Qpdf::new(), bytes.into_boxed_slice(), &options())
        .expect("the marked document must compress")
    {
        Outcome::Smaller { document, .. } => document,
        Outcome::NotSmaller {
            original_bytes,
            produced_bytes,
        } => panic!(
            "the fixture no longer shrinks ({original_bytes} -> {produced_bytes}), so every \
             assertion in this file would have nothing to examine"
        ),
    }
}

#[test]
fn compressing_loses_no_object_from_any_page() {
    let marked = marked_document();
    let all: Vec<u64> = (1..=marked.pages).collect();
    let must = owned_by(&marked, &all, &["content", "navigation"]);

    let output = compress_marked(marked_document().bytes);
    assert_nothing_lost(&marked, &output, &must, "compress the marked document");
}

#[test]
fn the_nothing_lost_assertion_can_fail_on_this_fixture() {
    // THE CONTROL. `assert_nothing_lost` passing says nothing unless it is capable of failing
    // on this document, with this harness, in this test binary -- `split`'s first leak test was
    // green on a fixture where the leak it looked for could not occur.
    //
    // The deliberate loser is an output built from a different, smaller document: every marked
    // object required of it is genuinely absent, so requiring them must be detected. If this
    // ever passes, the harness has stopped detecting loss and the test above is decoration.
    let marked = marked_document();
    let all: Vec<u64> = (1..=marked.pages).collect();
    let must = owned_by(&marked, &all, &["content", "navigation"]);

    let unrelated = compress_marked(minimal_pdf::pdf_with_page_tree(
        3,
        RotationPlacement::Absent,
    ));

    let panicked = std::panic::catch_unwind(|| {
        assert_nothing_lost(&marked, &unrelated, &must, "control: an unrelated document");
    });
    let error = panicked.expect_err(
        "the control did not fail: assert_nothing_lost accepted an output containing none of \
         the objects it was told had to survive, so the test above proves nothing",
    );
    let message = error
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| error.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_default();
    assert!(
        message.contains("had to survive"),
        "the control panicked, but not because objects were lost: {message}"
    );
}

#[test]
fn every_page_s_content_stream_survives_the_re_encoding() {
    // THE ASSERTION THE RUNTIME CHECK CANNOT MAKE. Compression re-encodes how objects are
    // stored; it must not re-encode what they contain. An implementation that round-tripped
    // content through a filter would keep every object, keep every page, keep every `/Rotate`
    // -- and quietly resample somebody's scan.
    //
    // Compared decompressed, through the same `qpdf --qdf` expansion the harness uses, so this
    // is about the content rather than about which filter each side chose. That matters more
    // here than anywhere else: compress's whole job is to change the filter.
    let original = minimal_pdf::pdf_with_page_tree(6, RotationPlacement::OnTheRoot(90));
    let output = compress_marked(original.clone());

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
            "page {page}'s content stream did not survive the compression"
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

#[test]
fn an_inherited_rotation_is_still_inherited_or_pushed_down_but_never_lost() {
    // What each page DISPLAYS at is the promise; where the value lives is not. This asserts the
    // promise through the operation's own verification -- which would have refused the output
    // already -- and then independently, so a verification that stopped running would not take
    // this assertion with it.
    let original = minimal_pdf::pdf_with_page_tree(6, RotationPlacement::OnTheRoot(90));
    let output = compress_marked(original);
    let after = expanded(&output);

    let occurrences = count(&after, b"/Rotate 90");
    assert!(
        occurrences >= 1,
        "the 90-degree rotation is nowhere in the compressed output, so every page now \
         displays upright"
    );
}

#[test]
fn a_damaged_document_is_refused_rather_than_losing_a_page_silently() {
    // ISSUE #61, REACHING COMPRESS. The document opens as five pages and writes as four;
    // ADR 0022's verification reads the output back through a fresh engine, sees four where
    // five were promised, and refuses. That meets #61's own stated bar -- "a refusal would not
    // block; losing the page quietly does".
    //
    // THIS IS THE ONLY REAL-ENGINE TEST OF COMPRESS'S ADR 0022 REFUSAL. Everything else that
    // exercises it does so through a fake engine told to lie, which is necessary (a correct
    // engine never triggers it) and not sufficient: a fake that stopped lying, or a refactor
    // that changed how the fake is wired, would take the whole story with it. `reorder` has
    // exactly this test for exactly this reason; code review pointed out compress had no
    // counterpart, on a committed fixture that already fires it.
    //
    // It is not compress's defect. The same bytes lose the same page through a plain write, a
    // rotation by zero degrees, and a reorder by the identity -- which is why the fix is the
    // shared verification step and not a guard here.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/damaged/page-loss-on-write.pdf");
    let bytes = std::fs::read(&path).expect("the committed reproduction must be readable");

    let engine = Qpdf::new();
    let source = burrow_engines::DocumentCompressor::open(
        &engine,
        bytes.clone().into_boxed_slice(),
        &options(),
    )
    .expect("the document opens -- that is the whole point of this input class");
    let opened = burrow_engines::DocumentCompressor::pages(&engine, &source).expect("a page count");
    assert_eq!(
        opened, 5,
        "the document declares 6 pages in /Count and yields 5 that resolve"
    );

    let err = compress(&engine, bytes.into_boxed_slice(), &options())
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

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn count(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

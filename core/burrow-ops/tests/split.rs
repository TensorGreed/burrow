//! Property, golden and limit tests for `split`, against the real qpdf extractor.
//!
//! `core/burrow-ops/src/split/tests.rs` covers the partition arithmetic against a fake, so it
//! runs on every CI job. This file covers the half that needs an engine.
//!
//! # The invariant, from `docs/ROADMAP.md`
//!
//! > Splitting then merging round-trips to the original page sequence; every input page
//! > appears exactly once across outputs.
//!
//! Both halves are asserted, and the first is the one that needs an engine: a page count
//! cannot see a reordering, and a reordering is what somebody would notice first. Each page
//! is made **identifiable** by its `/MediaBox` width — the same device `merge.rs` uses, for
//! the same measured reason: qpdf flates content streams on write, so a marker in a content
//! stream is not in the output bytes at all, while page dictionaries are written plainly.
//!
//! **Every page in the fixtures has a DIFFERENT width.** A fixture whose pages are
//! interchangeable cannot fail an order test, which is the mistake `add-operation` §2c now
//! records twice over.

#![cfg(all(feature = "native-engines", target_os = "linux"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use burrow_engines::OpenOptions;
use burrow_engines::qpdf::Qpdf;
use burrow_ops::{Cuts, Input, merge, split};
use burrow_types::{Clock, Error, Limits, ManualClock};
use proptest::prelude::*;

/// The output reader, shared rather than written a third time.
///
/// `widths_in_order` used to live here, and a near-identical copy lived in `merge.rs`, and
/// `reorder` then wrote a third — which rediscovered, at the cost of six failing tests, the
/// two facts the comment above already records. The module carries the reasoning with the
/// code now.
#[path = "../../burrow-engines/testsupport/pdf_reading.rs"]
mod pdf_reading;

use pdf_reading::page_widths as widths_in_order;

fn options(limits: Limits) -> OpenOptions<'static> {
    OpenOptions::new(limits, Arc::new(ManualClock::new(0)) as Arc<dyn Clock>)
}

/// A document whose every page is a different width, so order and identity are observable.
///
/// Widths start at 101 so no page can be confused with a count, a generation number or the
/// `0 0` of a `/MediaBox` origin.
fn distinct_pages(pages: usize) -> Vec<u8> {
    let mut objects: Vec<Vec<u8>> = Vec::new();
    let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", 3 + i)).collect();

    objects.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    objects.push(
        format!(
            "<< /Type /Pages /Kids [{}] /Count {pages} >>",
            kids.join(" ")
        )
        .into_bytes(),
    );
    for i in 0..pages {
        objects.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} 200] /Resources << >> >>",
                101 + i
            )
            .into_bytes(),
        );
    }

    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (n, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", n + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for offset in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

fn split_at(doc: Vec<u8>, cuts: &[u64], limits: Limits) -> burrow_types::Result<Vec<Vec<u8>>> {
    split(
        &Qpdf::new(),
        doc.into_boxed_slice(),
        Cuts::after_pages(cuts),
        &options(limits),
    )
}

// ---------------------------------------------------------------- the invariant

#[test]
fn every_page_appears_exactly_once_across_the_outputs() {
    let source = distinct_pages(10);
    let expected = widths_in_order(&source);
    assert_eq!(expected.len(), 10, "the fixture itself is unreadable");

    let outputs = split_at(source, &[3, 7], Limits::default()).expect("split");

    let seen: Vec<u64> = outputs.iter().flat_map(|o| widths_in_order(o)).collect();
    assert_eq!(
        seen, expected,
        "the outputs are not the input's pages, once each, in order"
    );
}

#[test]
fn splitting_then_merging_round_trips() {
    // The ROADMAP invariant in full, and the one that needs both operations. It is worth more
    // than the two halves separately: a split that reordered pages and a merge that reordered
    // them back would pass each half's own test.
    let source = distinct_pages(9);
    let expected = widths_in_order(&source);
    // THE READER MUST HAVE READ SOMETHING. Without this the comparison below is `[] == []`
    // the moment `widths_in_order` stops parsing -- a qpdf bump writing a cross-reference
    // stream would do it -- and the test goes green forever. Code review found three tests
    // in this file with that hole; this is one of them.
    assert_eq!(expected.len(), 9, "the fixture itself is unreadable");

    let parts = split_at(source, &[2, 5], Limits::default()).expect("split");
    let rejoined = merge(
        &Qpdf::new(),
        parts
            .into_iter()
            .map(|p| Input::new(p.into_boxed_slice()))
            .collect(),
        &options(Limits::default()),
    )
    .expect("merge");

    assert_eq!(
        widths_in_order(&rejoined),
        expected,
        "the round trip lost order"
    );
}

#[test]
fn splitting_into_one_part_is_the_identity_in_page_terms() {
    let source = distinct_pages(4);
    let expected = widths_in_order(&source);
    assert_eq!(expected.len(), 4, "the fixture itself is unreadable");
    let outputs = split_at(source, &[], Limits::default()).expect("split");
    assert_eq!(outputs.len(), 1);
    assert_eq!(widths_in_order(&outputs[0]), expected);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]

    /// Whatever the document and wherever the cut, the pages come out once each in order.
    #[test]
    fn any_single_cut_partitions_the_document(pages in 2usize..7, cut in 1u64..6) {
        prop_assume!(cut < pages as u64);

        let source = distinct_pages(pages);
        let expected = widths_in_order(&source);
        prop_assert_eq!(expected.len(), pages, "the fixture itself is unreadable");
        let outputs = split_at(source, &[cut], Limits::default()).expect("split");

        prop_assert_eq!(outputs.len(), 2);
        let seen: Vec<u64> = outputs.iter().flat_map(|o| widths_in_order(o)).collect();
        prop_assert_eq!(seen, expected);
    }
}

// ---------------------------------------------------------------- golden

#[test]
fn the_output_is_deterministic() {
    // `qpdf_set_deterministic_ID` is set on every write, so the same split of the same bytes
    // must produce the same bytes. Without it a golden test is impossible and a content-hash
    // cache would be wrong.
    let first = split_at(distinct_pages(6), &[2], Limits::default()).expect("split");
    let second = split_at(distinct_pages(6), &[2], Limits::default()).expect("split");
    assert_eq!(
        first, second,
        "two identical splits produced different bytes"
    );
}

#[test]
fn an_output_is_a_document_the_engine_can_read_back() {
    let outputs = split_at(distinct_pages(5), &[2], Limits::default()).expect("split");
    for (index, output) in outputs.iter().enumerate() {
        assert_eq!(&output[..5], b"%PDF-", "output {index} is not a PDF");
        // Read it back through the same engine, which is the only check that the document is
        // whole rather than merely prefixed correctly.
        let reread = split_at(output.clone(), &[], Limits::default())
            .unwrap_or_else(|e| panic!("output {index} could not be reopened: {e:?}"));
        assert_eq!(reread.len(), 1);
    }
}

// ---------------------------------------------------------------- limits

#[test]
fn a_document_over_the_page_ceiling_is_refused() {
    let err = split_at(distinct_pages(6), &[3], Limits::with(|l| l.max_pages = 5))
        .expect_err("must refuse");
    match err {
        Error::LimitExceeded { limit, .. } => assert_eq!(limit, "max_pages"),
        other => panic!("expected LimitExceeded, got {other:?}"),
    }
}

#[test]
fn a_document_exactly_at_the_page_ceiling_is_allowed() {
    // The boundary, in the direction that matters: a tool that refused the largest document it
    // documents would be refusing what its own prose promises.
    split_at(distinct_pages(5), &[2], Limits::with(|l| l.max_pages = 5)).expect("at the ceiling");
}

#[test]
fn an_input_over_the_size_ceiling_is_refused() {
    let err = split_at(
        distinct_pages(4),
        &[2],
        Limits::with(|l| l.max_input_bytes = 10),
    )
    .expect_err("must refuse");
    match err {
        Error::LimitExceeded { limit, .. } => assert_eq!(limit, "max_input_bytes"),
        other => panic!("expected LimitExceeded, got {other:?}"),
    }
}

#[test]
fn an_unreadable_document_is_refused_rather_than_split() {
    let err =
        split_at(b"not a pdf at all".to_vec(), &[], Limits::default()).expect_err("must refuse");
    assert!(matches!(err, Error::Malformed(_)), "{err:?}");
}

// ------------------------------------------- a page count is verified against itself (#111)

#[test]
#[ignore = "#111: the design hole this pins is open; the assertion states the property, not today's behaviour"]
fn a_split_keeps_every_page_the_document_declares() {
    // THE PROPERTY, WRITTEN DOWN WHILE IT DOES NOT HOLD.
    //
    // `five-pages-or-six.pdf` declares `/Count 6` and carries six `/Kids`. A textual walk of the
    // page tree reads six; PDFium reads six; qpdf's `page_count` reads FIVE, dropping the page
    // whose `/Resources` points above INT_MAX. So this split emits 2 + 3 = five pages and
    // reports success, and ADR 0022's read-back verifies five against five -- because the
    // verification asks the same source the operation asked.
    //
    // One page is gone and nothing says so. That is not `split` misbehaving: it is what a
    // promise checked against its own input is worth, and every operation carrying a page count
    // inherits it. See #111 and ADR 0022's 2026-09-17 amendment.
    //
    // IGNORED RATHER THAN DELETED OR WEAKENED, the way #61's reproduction was: a test asserting
    // five would turn a known hole into recorded correct behaviour, which is the direction this
    // repository refuses to go.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/conformance/fixtures/five-pages-or-six.pdf");
    let source = std::fs::read(&path).expect("the divergence fixture must exist");

    let parts = split_at(source, &[2], Limits::default()).expect("the document splits today");
    let total: usize = parts
        .iter()
        .map(|part| pdf_reading::page_order(part).len())
        .sum();

    assert_eq!(
        total, 6,
        "the document declares six pages and the split handed back {total}; a page was lost and \
         the operation reported success"
    );
}

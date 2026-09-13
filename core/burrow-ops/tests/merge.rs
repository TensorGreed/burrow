//! Property and golden tests for `merge`, against the real qpdf assembler.
//!
//! `core/burrow-ops/src/merge/tests.rs` covers the operation's own logic against a fake, so
//! it runs on every CI job. This file covers the half that needs an engine: the ROADMAP's
//! invariants, the limits at their boundaries, and byte-level output.
//!
//! # The invariants, from `docs/ROADMAP.md`
//!
//! > Output page count equals the sum of inputs; page order is preserved; merging one
//! > document is the identity.
//!
//! Order is the one worth saying more about. A page count cannot see a reordering, and a
//! reordering is precisely what a merge tool's users would notice first. So each input's
//! pages are made **identifiable** and read back out of the merged document in sequence.
//!
//! The identifier is each page's `/MediaBox` width, and the first attempt used a marker in
//! the content stream instead. That failed, which is how this comment came to be accurate:
//! **qpdf flates the content streams on write** (`/Filter /FlateDecode`), so a literal
//! `(A1)` is not in the output bytes at all. The page tree and the page dictionaries are
//! written plainly, so the width survives and needs no decompressor.
//!
//! If qpdf ever starts writing page dictionaries into object streams, `page_widths_in_order`
//! finds nothing and these tests fail with an empty vector rather than passing vacuously.
//! That is the same way the content-stream version failed, and it is why the assertion
//! compares against a full expected sequence rather than merely checking for a subsequence.

// Gated on THIS crate's `native-engines`, which forwards to the engine crate's. The
// `burrow_native_engines` cfg is set by `burrow-engines`' build script and is not visible
// here, so the feature is the gate -- and `target_os` matters because only Linux natives
// are pinned (ADR 0004).
#![cfg(all(feature = "native-engines", target_os = "linux"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

/// The one generator, included by path rather than copied.
///
/// `core/burrow-engines/testsupport/minimal_pdf.rs` is already included this way from that
/// crate's own `src/` and `tests/`, for the reason `core/CLAUDE.md` gives: two generators
/// agree until the day they do not.
#[path = "../../burrow-engines/testsupport/minimal_pdf.rs"]
mod minimal_pdf;

use std::sync::Arc;

use burrow_engines::qpdf::Qpdf;
use burrow_engines::{OpenOptions, PageAssembler};
use burrow_ops::{Input, merge};
use burrow_types::{Clock, Error, Limits, ManualClock, Stage};
use proptest::prelude::*;

/// Options on a stopped clock: no test may fail because a machine was busy.
fn options(limits: Limits) -> OpenOptions<'static> {
    OpenOptions::new(limits, Arc::new(ManualClock::new(0)) as Arc<dyn Clock>)
}

fn merge_all(docs: Vec<Vec<u8>>, limits: Limits) -> burrow_types::Result<Vec<u8>> {
    let inputs = docs
        .into_iter()
        .map(|d| Input::new(d.into_boxed_slice()))
        .collect();
    merge(&Qpdf::new(), inputs, &options(limits))
}

/// Pages in a merged document, read back through the assembler that produced it.
fn pages_in(bytes: &[u8]) -> u64 {
    let engine = Qpdf::new();
    let assembly = engine
        .begin(bytes.to_vec().into_boxed_slice(), &options(Limits::DEFAULT))
        .expect("the merged document must be readable");
    engine.pages(&assembly).expect("page count")
}

#[test]
fn merging_one_document_is_the_identity_in_pages() {
    for n in [1_usize, 2, 10] {
        let out = merge_all(vec![minimal_pdf::pdf_with_pages(n)], Limits::default())
            .unwrap_or_else(|e| panic!("{n} pages: {e}"));
        assert_eq!(pages_in(&out), n as u64, "merging one document changed it");
    }
}

#[test]
fn the_output_page_count_is_the_sum_of_the_inputs() {
    let out = merge_all(
        vec![
            minimal_pdf::pdf_with_pages(1),
            minimal_pdf::pdf_with_pages(4),
            minimal_pdf::pdf_with_pages(7),
        ],
        Limits::default(),
    )
    .expect("merge");
    assert_eq!(pages_in(&out), 12);
}

#[test]
fn page_order_is_preserved_across_inputs() {
    // THE INVARIANT A PAGE COUNT CANNOT SEE. Each document's pages carry their own marker
    // in the content stream, so the merged document's byte sequence shows the order.
    let first = sized_pdf(200, 3);
    let second = sized_pdf(300, 2);
    let out = merge_all(vec![first, second], Limits::default()).expect("merge");

    assert_eq!(
        page_widths_in_order(&out),
        vec![200, 200, 200, 300, 300],
        "pages did not come out in the order they went in"
    );
}

#[test]
fn reversing_the_inputs_reverses_the_blocks() {
    // The control for the test above: if `markers_in_order` reported a fixed order, or the
    // assembler ignored input order, the assertion above would pass regardless.
    let out = merge_all(
        vec![sized_pdf(300, 2), sized_pdf(200, 3)],
        Limits::default(),
    )
    .expect("merge");
    assert_eq!(
        page_widths_in_order(&out),
        vec![300, 300, 200, 200, 200],
        "the assembler ignored the order the inputs were given in"
    );
}

#[test]
fn the_output_is_deterministic() {
    // `qpdf_set_deterministic_ID` is what makes a golden test possible: without it the
    // `/ID` is drawn from the clock and the random pool, and two merges of the same inputs
    // differ. This is the assertion that would fail if that call were removed or moved
    // before `qpdf_init_write_memory`.
    let docs = || {
        vec![
            minimal_pdf::pdf_with_pages(2),
            minimal_pdf::pdf_with_pages(3),
        ]
    };
    let a = merge_all(docs(), Limits::default()).expect("merge");
    let b = merge_all(docs(), Limits::default()).expect("merge");
    assert_eq!(
        a, b,
        "two merges of the same inputs produced different bytes"
    );
}

#[test]
fn the_merged_document_is_a_pdf_that_opens() {
    let out = merge_all(
        vec![
            minimal_pdf::pdf_with_pages(1),
            minimal_pdf::pdf_with_pages(1),
        ],
        Limits::default(),
    )
    .expect("merge");
    assert!(out.starts_with(b"%PDF-"), "output is not a PDF");
    assert_eq!(pages_in(&out), 2);
}

#[test]
fn an_encrypted_input_fails_with_its_index_and_reason() {
    let inputs = vec![
        minimal_pdf::pdf_with_pages(1),
        minimal_pdf::pdf_encrypted_with_unusable_credentials(),
    ];
    let err = merge_all(inputs, Limits::default()).expect_err("an unopenable input must fail");
    match err {
        Error::InputFailed { index, source } => {
            assert_eq!(index, 1);
            assert!(
                matches!(*source, Error::PasswordRequired | Error::Malformed(_)),
                "got {source:?}"
            );
        }
        other => panic!("expected InputFailed, got {other:?}"),
    }
}

#[test]
fn a_malformed_input_fails_with_its_index() {
    let inputs = vec![
        minimal_pdf::pdf_with_pages(1),
        minimal_pdf::not_a_pdf(),
        minimal_pdf::pdf_with_pages(1),
    ];
    let err = merge_all(inputs, Limits::default()).expect_err("a non-PDF must fail");
    assert!(
        matches!(err, Error::InputFailed { index: 1, .. }),
        "got {err:?}"
    );
}

#[test]
fn a_trailerless_input_fails_to_merge_even_though_qpdf_can_count_it() {
    // ADR 0017's correction to ADR 0013, pinned. qpdf reports three pages in this file and
    // then cannot extract the first, so "qpdf reads files PDFium refuses" does not carry
    // over to merging them. If a future qpdf makes this succeed, this test fails and the
    // ADR needs revisiting -- which is the point of asserting it rather than describing it.
    let err = merge_all(
        vec![
            minimal_pdf::trailer_removed(),
            minimal_pdf::pdf_with_pages(1),
        ],
        Limits::default(),
    )
    .expect_err("trailer-removed.pdf must not merge");
    assert!(
        matches!(err, Error::InputFailed { index: 0, .. }),
        "got {err:?}"
    );
}

#[test]
fn the_page_ceiling_is_on_the_output_not_on_each_input() {
    // Three inputs of four pages each is twelve, from a caller who allowed ten. Per-input
    // checking waves every one of them through.
    let limits = Limits::with(|l| l.max_pages = 10);
    let err = merge_all(
        vec![
            minimal_pdf::pdf_with_pages(4),
            minimal_pdf::pdf_with_pages(4),
            minimal_pdf::pdf_with_pages(4),
        ],
        limits,
    )
    .expect_err("12 pages against a 10-page ceiling must fail");

    let inner = match err {
        Error::InputFailed { source, .. } => *source,
        other => other,
    };
    match inner {
        Error::LimitExceeded {
            limit,
            stage,
            requested,
            allowed,
        } => {
            assert_eq!(limit, "max_pages");
            assert_eq!(stage, Stage::PageCount);
            assert_eq!(requested, 12, "the output total, not one input");
            assert_eq!(allowed, 10);
        }
        other => panic!("expected LimitExceeded, got {other:?}"),
    }
}

#[test]
fn the_page_ceiling_allows_exactly_its_boundary() {
    let limits = Limits::with(|l| l.max_pages = 8);
    merge_all(
        vec![
            minimal_pdf::pdf_with_pages(4),
            minimal_pdf::pdf_with_pages(4),
        ],
        limits,
    )
    .expect("exactly at the ceiling must be allowed");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// The sum invariant, over arbitrary shapes rather than three chosen ones.
    #[test]
    fn output_pages_equal_the_sum_of_input_pages(
        counts in prop::collection::vec(1_usize..6, 1..5)
    ) {
        let total: u64 = counts.iter().map(|n| *n as u64).sum();
        let docs = counts.iter().map(|n| minimal_pdf::pdf_with_pages(*n)).collect();
        let out = merge_all(docs, Limits::default()).expect("merge");
        prop_assert_eq!(pages_in(&out), total);
    }

    /// Any byte string is opened or typed-rejected — never a panic, never an abort.
    ///
    /// The same property `properties.rs` asserts for `open`, on the path that also writes.
    #[test]
    fn arbitrary_bytes_never_panic(junk in prop::collection::vec(any::<u8>(), 0..4096)) {
        let result = merge_all(
            vec![minimal_pdf::pdf_with_pages(1), junk],
            Limits::default(),
        );
        if let Err(error) = result {
            prop_assert!(
                !matches!(error, Error::Internal(_)),
                "arbitrary bytes produced an Internal error, which means a bug here rather \
                 than a bad file: {error:?}"
            );
        }
    }
}

// ---------------------------------------------------------------- sized fixtures

/// A PDF whose pages all have `width` as their `/MediaBox` width.
///
/// Width rather than a content-stream marker because qpdf flates content streams on write
/// and writes page dictionaries plainly — see this file's header. Built here rather than in
/// `minimal_pdf.rs` because only these tests need distinguishable pages; if a second caller
/// appears it should move.
fn sized_pdf(width: u32, pages: usize) -> Vec<u8> {
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
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} 200] /Contents {} 0 R \
                 /Resources << >> >>",
                3 + pages + i
            )
            .into_bytes(),
        );
    }
    for i in 0..pages {
        let content = format!("BT /F1 12 Tf 20 100 Td (page {}) Tj ET", i + 1);
        objects.push(
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            )
            .into_bytes(),
        );
    }

    let mut out = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = vec![0usize; objects.len() + 1];
    for (n, body) in objects.iter().enumerate() {
        offsets[n + 1] = out.len();
        out.extend_from_slice(format!("{} 0 obj\n", n + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets.iter().skip(1) {
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

/// Each page's `/MediaBox` width, in the order the page tree lists them.
///
/// Walks `/Kids` for the object numbers and then reads each page object's width, so it
/// reports the document's OWN order rather than the order objects happen to be written in.
/// Reading them in file order would pass a merge that wrote the pages out of sequence but
/// listed them correctly, and fail one that did the reverse — neither of which is the
/// question.
///
/// Returns an empty vector if the page tree is not written plainly, which fails the
/// assertions rather than satisfying them.
fn page_widths_in_order(bytes: &[u8]) -> Vec<u32> {
    let text = String::from_utf8_lossy(bytes);

    let Some(kids_at) = text.find("/Kids") else {
        return Vec::new();
    };
    let after = &text[kids_at..];
    let (Some(open), Some(close)) = (after.find('['), after.find(']')) else {
        return Vec::new();
    };
    let kids: Vec<u32> = after[open + 1..close]
        .split_whitespace()
        .filter_map(|t| t.parse::<u32>().ok())
        .collect();
    // `N 0 R` triples: keep the object numbers, drop the generations and the `R`s.
    let object_numbers: Vec<u32> = kids.chunks(2).filter_map(|c| c.first().copied()).collect();

    object_numbers
        .iter()
        .filter_map(|n| {
            let marker = format!("\n{n} 0 obj");
            let at = text.find(&marker)?;
            let body = &text[at..];
            let end = body.find("endobj")?;
            let box_at = body[..end].find("/MediaBox")?;
            let rest = &body[box_at..];
            let inner = rest.find('[').and_then(|o| {
                let tail = &rest[o + 1..];
                tail.find(']').map(|c| &tail[..c])
            })?;
            inner.split_whitespace().nth(2)?.parse::<u32>().ok()
        })
        .collect()
}

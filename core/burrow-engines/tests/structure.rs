//! M1 item 6: qpdf over a corpus of structurally damaged files.
//!
//! *"a corpus of structurally damaged files; each either succeeds or returns `Malformed`,
//! and no case aborts."*
//!
//! Plus two questions the roadmap attaches to this item and this file answers directly:
//!
//! - **Is "repair" a justification for carrying qpdf?** ADR 0004 originally said so, spike
//!   0001 contradicted it, and item 6 says the claim needs a corpus case PDFium actually
//!   fails. There are two: see [`qpdf_recovers_two_files_pdfium_refuses`].
//! - **Does the nesting limit work?** The other bomb shape: where the xref bomb attacks
//!   memory, a deeply nested object attacks the parser's stack, and a stack overflow is a
//!   signal no `catch_unwind` can see.

#![cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    // Test arithmetic on known-good numbers; the lint is for attacker-controlled sizes in
    // library code, where it stays denied.
    clippy::integer_division
)]

mod support;

use std::sync::Arc;

use burrow_engines::pdfium::Pdfium;
use burrow_engines::qpdf::Qpdf;
use burrow_engines::{CheckOptions, DocumentEngine, OpenOptions, StructureEngine};
use burrow_types::{Error, Limits, ManualClock};
use support::minimal_pdf;

fn check(bytes: Vec<u8>) -> burrow_types::Result<burrow_engines::StructureReport> {
    Qpdf::new().check(
        bytes.into_boxed_slice(),
        &CheckOptions::new(Limits::default(), stopped()),
    )
}

fn check_recovering(bytes: Vec<u8>) -> burrow_types::Result<burrow_engines::StructureReport> {
    let mut options = CheckOptions::new(Limits::default(), stopped());
    options.attempt_recovery = true;
    Qpdf::new().check(bytes.into_boxed_slice(), &options)
}

/// A stopped clock, so nothing here depends on how busy the machine is.
fn stopped() -> Arc<dyn burrow_types::Clock> {
    Arc::new(ManualClock::new(0))
}

fn open_pdfium(bytes: Vec<u8>) -> burrow_types::Result<u64> {
    let clock = stopped();
    Pdfium::new()
        .open(
            bytes.into_boxed_slice(),
            &OpenOptions::new(Limits::default(), clock),
        )
        .map(|doc| doc.pages_at_open())
}

/// The damaged corpus. Generated, so provenance is trivial and the damage is deliberate.
fn damaged_corpus() -> Vec<(&'static str, Vec<u8>)> {
    let valid = minimal_pdf::pdf_with_pages(3);

    let mut truncated_mid_object = valid.clone();
    truncated_mid_object.truncate(valid.len() / 2);

    let mut zeroed_xref = valid.clone();
    if let Some(at) = find(&zeroed_xref, b"xref") {
        for byte in zeroed_xref.iter_mut().skip(at).take(64) {
            *byte = b'0';
        }
    }

    let mut wrong_startxref = valid.clone();
    if let Some(at) = find(&wrong_startxref, b"startxref") {
        let tail = at + b"startxref\n".len();
        for (i, b) in b"999999".iter().enumerate() {
            if let Some(slot) = wrong_startxref.get_mut(tail + i) {
                *slot = *b;
            }
        }
    }

    let mut no_trailer = valid.clone();
    if let Some(at) = find(&no_trailer, b"trailer") {
        no_trailer.truncate(at);
    }

    let mut broken_root = valid.clone();
    if let Some(at) = find(&broken_root, b"/Root 1 0 R") {
        for (i, b) in b"/Root 9 9 R".iter().enumerate() {
            if let Some(slot) = broken_root.get_mut(at + i) {
                *slot = *b;
            }
        }
    }

    vec![
        ("truncated mid-object", truncated_mid_object),
        ("xref overwritten with zeros", zeroed_xref),
        ("startxref points past the end", wrong_startxref),
        ("trailer removed entirely", no_trailer),
        ("/Root points at a nonexistent object", broken_root),
        ("not a pdf at all", minimal_pdf::not_a_pdf()),
        ("header only", b"%PDF-1.7\n".to_vec()),
        ("empty", Vec::new()),
        ("nul bytes", vec![0u8; 4096]),
        ("page tree with no pages", minimal_pdf::pdf_with_no_pages()),
        // The abort guard. Harmless now, and aborts the suite if an untrapped qpdf call
        // is ever reintroduced -- see the generator's docs.
        (
            "object number above INT_MAX",
            minimal_pdf::pdf_with_object_number_above_int_max(),
        ),
    ]
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Item 6's stated test: every damaged file either succeeds or returns a typed error, and
/// **no case aborts**. Reaching the end of this test at all is most of the assertion.
#[test]
fn every_damaged_file_succeeds_or_returns_a_typed_error() {
    for (what, bytes) in damaged_corpus() {
        for recovering in [false, true] {
            let result = if recovering {
                check_recovering(bytes.clone())
            } else {
                check(bytes.clone())
            };
            match result {
                Ok(report) => {
                    // A success must still be internally coherent.
                    assert!(
                        report.pages <= Limits::DEFAULT.max_pages,
                        "{what}: reported {} pages, past the limit that was in force",
                        report.pages
                    );
                }
                Err(Error::Malformed(_) | Error::Unsupported(_) | Error::PasswordRequired) => {}
                Err(Error::LimitExceeded { .. }) => {}
                Err(other) => panic!(
                    "{what} (recovery={recovering}): expected success or a typed failure, \
                     got {other:?}"
                ),
            }
        }
    }
}

/// The two files that justify qpdf's repair claim, pinned as a regression.
///
/// [ADR 0004](../../../docs/adr/0004-native-engines.md) listed "repair" as one of qpdf's
/// jobs, [spike 0001](../../../docs/spikes/0001-wasm-engines.md) Finding 4 withdrew that
/// justification after PDFium reconstructed a fully corrupted xref by itself, and M1 item
/// 6 said the claim needed a corpus case PDFium actually fails.
///
/// **These are those cases**, measured on this build:
///
/// | file | PDFium | qpdf |
/// |---|---|---|
/// | a valid 3-page file truncated to 290 bytes | `Malformed` | **1 page** |
/// | the same file with its trailer removed | `Malformed` | **3 pages** |
///
/// Asserted rather than merely printed, because they are now the evidence behind a
/// documented decision. If PDFium improves and starts reading these, this test fails —
/// which is the correct outcome: the repair justification would then need re-examining
/// rather than quietly continuing to be cited.
#[test]
fn qpdf_recovers_two_files_pdfium_refuses() {
    let valid = minimal_pdf::pdf_with_pages(3);

    let mut truncated = valid.clone();
    truncated.truncate(valid.len() / 2);

    let mut no_trailer = valid.clone();
    let at = find(&no_trailer, b"trailer").expect("the generator writes a trailer");
    no_trailer.truncate(at);

    // The truncated case's recovered page count depends on exactly where the byte cut
    // lands, which is an accident of the generator's whitespace -- so it is asserted as a
    // range. The trailer-less case is not position-dependent and is pinned exactly.
    for (what, bytes, expected) in [
        ("truncated mid-object", truncated, 1..=3),
        ("trailer removed", no_trailer, 3..=3),
    ] {
        let by_pdfium = open_pdfium(bytes.clone());
        assert!(
            by_pdfium.is_err(),
            "{what}: PDFium now reads this file ({by_pdfium:?}). That is good news, but it \
             removes one of the two cases justifying ADR 0004's repair claim -- re-run the \
             comparison and update the ADR rather than deleting this assertion."
        );

        let by_qpdf = check_recovering(bytes)
            .unwrap_or_else(|e| panic!("{what}: qpdf should recover this file, got {e}"));
        assert!(
            expected.contains(&by_qpdf.pages),
            "{what}: qpdf recovered {} pages, outside {expected:?}",
            by_qpdf.pages
        );
    }
}

/// A file that once aborted the process, now handled as an ordinary document.
///
/// The regression test for M1 PR 3's critical review finding. If `qpdf_is_linearized` or
/// any other untrapped qpdf function is reintroduced, this does not fail — it **aborts**,
/// taking the test binary with it. That is the correct and most visible outcome for a
/// defect whose real-world form is a killed worker or a killed app.
#[test]
fn an_object_number_above_int_max_does_not_abort_the_process() {
    let bytes = minimal_pdf::pdf_with_object_number_above_int_max();

    // Reaching the end of this test is the assertion. Before the fix, the first of these
    // calls killed the binary with SIGABRT; whether the document opens is beside the
    // point. Both recovery dispositions, because they are different paths inside qpdf.
    let strict = check(bytes.clone());
    let recovering = check_recovering(bytes.clone());
    for result in [strict, recovering] {
        assert!(
            result.is_ok() || matches!(result, Err(Error::Malformed(_))),
            "expected a typed outcome, got {result:?}"
        );
    }

    // And through PDFium, which reads the same file by a different route.
    let _ = open_pdfium(bytes);
}

/// The other bomb shape: nesting deep enough to overflow a recursive parser's stack.
///
/// A stack overflow is a `SIGSEGV`, not a panic — no `catch_unwind` sees it and the
/// process dies. qpdf's `parser_max_nesting` global is what prevents it, and this is the
/// input that proves the setting took effect. Reaching the end of this test is the
/// assertion.
#[test]
fn a_deeply_nested_object_bomb_does_not_take_the_process_down() {
    for depth in [64usize, 1_000, 50_000, 500_000] {
        let bytes = minimal_pdf::pdf_with_nesting(depth);

        // The structural pre-scan sees this one as unremarkable -- it declares nothing
        // large, because the attack is on the parser's stack rather than on memory. So the
        // engines really do receive it, which is the point.
        let result = check_recovering(bytes.clone());
        assert!(
            !matches!(result, Err(Error::Internal(_))),
            "depth {depth}: qpdf returned Internal, which means something went wrong that \
             was not the file's fault: {result:?}"
        );

        // PDFium gets the same file. It has its own limits; what matters is that neither
        // engine takes the process with it.
        let _ = open_pdfium(bytes);
    }
}

/// qpdf runs on the caller's thread, so this is where that decision is tested.
///
/// ADR 0013: distinct `qpdf_data` objects may be used from distinct threads
/// (`qpdf-c.h:43-45`), and every one this crate creates lives inside a single function
/// call. What is process-global — the resource limits and the discarding logger — is set
/// once behind a `OnceLock`, and this is what would expose a race in that initialisation.
#[test]
fn many_threads_checking_at_once_all_get_their_own_correct_answers() {
    const THREADS: usize = 32;
    const ROUNDS: usize = 16;

    std::thread::scope(|scope| {
        for thread in 0..THREADS {
            scope.spawn(move || {
                for round in 0..ROUNDS {
                    // Vary the page count per thread and round, so two threads asking at
                    // the same moment expect different answers -- a mix-up is then a wrong
                    // number rather than a crash.
                    let pages = 1 + (thread + round) % 13;
                    let report = check(minimal_pdf::pdf_with_pages(pages))
                        .unwrap_or_else(|e| panic!("thread {thread} round {round}: {e}"));
                    assert_eq!(
                        report.pages,
                        u64::try_from(pages).unwrap(),
                        "thread {thread} round {round} got another document's page count"
                    );

                    // Interleave failures, which is where a shared-state bug in the error
                    // register would show up: qpdf's error slot is per-`qpdf_data`, and
                    // this asserts one thread's failure never becomes another's.
                    assert!(check(minimal_pdf::not_a_pdf()).is_err());
                }
            });
        }
    });
}

/// Both engines, from many threads, at the same time.
///
/// PDFium is serialised onto its own thread and qpdf is not, so this is the arrangement
/// where a mistake in either discipline would surface — and it is the shape real
/// operations will have, since an operation checks structure and then opens.
#[test]
fn qpdf_and_pdfium_interleaved_across_threads_stay_correct() {
    std::thread::scope(|scope| {
        for thread in 0..16 {
            scope.spawn(move || {
                for round in 0..8 {
                    let pages = 1 + (thread + round) % 7;
                    let bytes = minimal_pdf::pdf_with_pages(pages);
                    let expected = u64::try_from(pages).unwrap();

                    let report = check(bytes.clone()).expect("structure check");
                    assert_eq!(report.pages, expected, "qpdf disagreed");

                    let opened = open_pdfium(bytes).expect("pdfium open");
                    assert_eq!(opened, expected, "pdfium disagreed with qpdf");
                }
            });
        }
    });
}

/// An ordinary document reports an ordinary shape.
#[test]
fn an_ordinary_document_reports_an_ordinary_shape() {
    let report = check(minimal_pdf::pdf_with_pages(10)).expect("a plain document");
    assert_eq!(report.pages, 10);
}

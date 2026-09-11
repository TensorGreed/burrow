//! Proof that the serialisation holds.
//!
//! **PDFium is not thread-safe anywhere.** M1 PR 1 found the init half of that the hard
//! way: two tests calling `FPDF_InitLibrary` on separate harness threads tripped an
//! internal `CHECK` and the process died with `SIGTRAP`. A `Once` fixed that one call and
//! nothing else — every other `FPDF_*` call was still reachable concurrently, because
//! `cargo test` runs tests on parallel threads.
//!
//! These tests deliberately do what would have crashed: many threads, opening and reading
//! at once, plus a document opened on one thread and used from another. If the engine
//! thread in `src/pdfium/thread.rs` were removed, this file is what would fail — loudly,
//! and probably by taking the process down rather than by returning a wrong answer.
//!
//! A passing run is not a proof of the absence of a data race, and should not be read as
//! one. It is a regression test for a failure that has actually happened here.

#![cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

use burrow_types::Error;
use support::{minimal_pdf, open, page_count};

const THREADS: usize = 32;
const OPENS_PER_THREAD: usize = 32;

/// Many threads, opening a mix of valid and hostile inputs at once.
///
/// Each thread checks its own results, so a thread that got another thread's answer fails
/// rather than passing quietly — which is the failure mode that matters. A crash would
/// have been obvious; a document reporting the wrong page count would not.
#[test]
fn many_threads_opening_at_once_all_get_their_own_correct_answers() {
    let valid_pages = AtomicUsize::new(0);
    let rejected = AtomicUsize::new(0);

    std::thread::scope(|scope| {
        for thread in 0..THREADS {
            let valid_pages = &valid_pages;
            let rejected = &rejected;
            scope.spawn(move || {
                for round in 0..OPENS_PER_THREAD {
                    // Vary the page count per thread and round, so two threads asking at
                    // the same moment expect different answers. Identical answers would
                    // hide a handle mix-up completely.
                    let pages = 1 + (thread + round) % 17;

                    let doc = open(minimal_pdf::pdf_with_pages(pages))
                        .unwrap_or_else(|e| panic!("thread {thread} round {round}: {e}"));
                    assert_eq!(
                        doc.pages_at_open(),
                        u64::try_from(pages).unwrap(),
                        "thread {thread} round {round} got another document's page count"
                    );
                    assert_eq!(
                        page_count(&doc).unwrap(),
                        u64::try_from(pages).unwrap(),
                        "thread {thread} round {round}: page_count disagreed with open"
                    );
                    valid_pages.fetch_add(pages, Ordering::Relaxed);

                    // Interleave failures with successes: a failing open closes a document
                    // mid-flight, which is where an ordering bug would surface.
                    match open(minimal_pdf::truncated_pdf()) {
                        Err(Error::Malformed(_)) => {
                            rejected.fetch_add(1, Ordering::Relaxed);
                        }
                        other => panic!(
                            "thread {thread} round {round}: expected Malformed, got {other:?}"
                        ),
                    }
                    assert!(matches!(
                        open(minimal_pdf::not_a_pdf()),
                        Err(Error::Malformed(_))
                    ));
                }
            });
        }
    });

    let expected_pages: usize = (0..THREADS)
        .flat_map(|t| (0..OPENS_PER_THREAD).map(move |r| 1 + (t + r) % 17))
        .sum();
    assert_eq!(valid_pages.load(Ordering::Relaxed), expected_pages);
    assert_eq!(rejected.load(Ordering::Relaxed), THREADS * OPENS_PER_THREAD);
}

/// A document opened on one thread, read from another, and dropped on a third.
///
/// This is what `type Document: Send + Sync` on the trait is for. It compiles because
/// `PdfiumDocument` is an id and a deadline — no `unsafe impl Send` anywhere — and it
/// works because the raw handle never left the engine thread in the first place.
#[test]
fn a_document_opened_on_one_thread_is_usable_from_another() {
    let (tx, rx) = mpsc::channel();

    std::thread::spawn(move || {
        let doc = open(minimal_pdf::pdf_with_pages(7)).expect("the opener's document");
        tx.send(doc).expect("the receiver is waiting");
    })
    .join()
    .expect("the opening thread should not panic");

    let doc = rx.recv().expect("a document should have arrived");

    // Read it from several threads at once, sharing one handle. This needs `Sync`, not
    // just `Send`.
    let doc = Arc::new(doc);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let doc = Arc::clone(&doc);
            scope.spawn(move || {
                assert_eq!(page_count(&doc).unwrap(), 7);
            });
        }
    });

    // And drop it somewhere else again.
    let doc = Arc::try_unwrap(doc).expect("the only remaining reference");
    std::thread::spawn(move || drop(doc))
        .join()
        .expect("dropping a document on a third thread should not panic");
}

/// Opening many documents at once and letting them all drop together.
///
/// Drops submit a close to the engine thread without waiting, so this is the case where
/// the queue fills with closes while opens are still arriving.
#[test]
fn many_documents_open_and_close_concurrently_without_interfering() {
    std::thread::scope(|scope| {
        for thread in 0..THREADS {
            scope.spawn(move || {
                let docs: Vec<_> = (1..=8)
                    .map(|pages| {
                        let doc = open(minimal_pdf::pdf_with_pages(pages))
                            .unwrap_or_else(|e| panic!("thread {thread}: {e}"));
                        assert_eq!(doc.pages_at_open(), u64::try_from(pages).unwrap());
                        doc
                    })
                    .collect();

                // All eight are open at once on this thread; check each still answers for
                // itself before they all drop together at the end of the scope.
                for (index, doc) in docs.iter().enumerate() {
                    assert_eq!(page_count(doc).unwrap(), u64::try_from(index + 1).unwrap());
                }
            });
        }
    });
}

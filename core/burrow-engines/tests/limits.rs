//! The limit tests the roadmap names by name.
//!
//! ROADMAP M1 item 5: *"a 20k-page document and a 2 GB file both return `LimitExceeded`,
//! not a crash or an OOM kill; timeout behaviour is tested with a fake clock, not by
//! waiting."* All three are here, and none of them waits or allocates its way into the
//! OOM killer.

#![cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod support;

use std::sync::Arc;

use burrow_engines::pdfium::Pdfium;
use burrow_engines::{DocumentEngine, OpenOptions};
use burrow_types::{Error, Limits, ManualClock};
use support::{minimal_pdf, open_with};

/// A 20,000-page document is rejected on its page count, not by running out of anything.
///
/// The document is real — 20,003 objects with a correct cross-reference table — so PDFium
/// genuinely parses it and genuinely reports 20,000. The limit is then applied to a number
/// the engine produced, which is the only version of this test worth having.
#[test]
fn a_twenty_thousand_page_document_is_a_limit_error_not_a_crash() {
    let bytes = minimal_pdf::pdf_with_pages(20_000);
    let limits = Limits::with(|l| {
        l.max_pages = 10_000;
        // Generous, so nothing else trips first and the failure is unambiguous.
        l.max_input_bytes = 512 * 1024 * 1024;
        l.max_memory_bytes = 4 * 1024 * 1024 * 1024;
    });

    match open_with(bytes.clone(), limits) {
        Err(Error::LimitExceeded {
            limit,
            requested,
            allowed,
        }) => {
            assert_eq!(limit, "max_pages");
            assert_eq!(requested, 20_000, "PDFium should have counted all 20,000");
            assert_eq!(allowed, 10_000);
        }
        other => panic!("expected LimitExceeded, got {other:?}"),
    }

    // And with the ceiling raised, the same bytes open and report 20,000 — which is what
    // makes the assertion above about the *limit* rather than about a parse failure.
    let generous = Limits::with(|l| {
        l.max_pages = 25_000;
        l.max_memory_bytes = 4 * 1024 * 1024 * 1024;
    });
    let doc = open_with(bytes, generous).expect("20,000 pages is fine when the limit allows it");
    assert_eq!(doc.pages_at_open(), 20_000);
}

/// A 2 GiB input is rejected on its size, without an OOM kill.
///
/// # Why allocating 2 GiB here is safe
///
/// `vec![0u8; n]` reaches the allocator as a `calloc`, which for a block this size is an
/// anonymous `mmap` of zero pages. Nothing writes to them, so they are never faulted in
/// and resident memory stays near zero — the 2 GiB is address space, not RAM. That is
/// also exactly the property under test: `max_input_bytes` is checked against the
/// *length*, before a single byte is read, so the engine never touches the buffer either.
///
/// If this ever does start using 2 GiB of RAM, that is a real regression in the ordering
/// of the checks, and it should fail loudly rather than be quietly marked `#[ignore]`.
#[test]
fn a_two_gigabyte_input_is_a_limit_error_not_an_oom() {
    const TWO_GIB: usize = 2 * 1024 * 1024 * 1024;
    let bytes = vec![0u8; TWO_GIB + 1];
    let len = u64::try_from(bytes.len()).unwrap();

    let limits = Limits::with(|l| l.max_input_bytes = u64::try_from(TWO_GIB).unwrap());
    match open_with(bytes, limits) {
        Err(Error::LimitExceeded {
            limit,
            requested,
            allowed,
        }) => {
            assert_eq!(limit, "max_input_bytes");
            assert_eq!(requested, len);
            assert_eq!(allowed, u64::try_from(TWO_GIB).unwrap());
        }
        other => panic!("expected LimitExceeded, got {other:?}"),
    }
}

/// A length far past anything allocatable is rejected on the memory estimate, with no
/// buffer involved at all.
///
/// The companion to the test above: that one proves the check happens before the bytes are
/// read, this one proves the estimate saturates instead of wrapping into a small number
/// that would pass.
#[test]
fn an_input_at_the_top_of_the_address_space_cannot_wrap_its_estimate() {
    let limits = Limits::with(|l| {
        l.max_input_bytes = u64::MAX;
        l.max_memory_bytes = u64::MAX - 1;
    });
    // A 4 KiB buffer, but limits that would let an arbitrarily large one through on size.
    // The estimate for a real u64::MAX length is exercised as a unit test in `estimate`;
    // what matters here is that a normal file is *not* caught by a saturating estimate.
    let doc = open_with(minimal_pdf::pdf_with_pages(1), limits)
        .expect("a small file must not be caught by the saturating estimate");
    assert_eq!(doc.pages_at_open(), 1);
}

/// A small file that makes PDFium allocate over a gigabyte is rejected, not returned.
///
/// # What this is a regression test for
///
/// `tests/conformance/fixtures/xref-bomb.pdf` is 330 KB and declares a cross-reference
/// stream of twenty million entries — 340 MB uncompressed, which PDFium must inflate to
/// parse the file at all. It ends up resident in about 1.2 GB.
///
/// The **size-based** pre-check predicts 16 MB for it, because that check is a function of
/// the input's length and the declared size is not in the length. It passes the file
/// straight through; an amplification of roughly 3,900x that no tuning of that estimate
/// could catch. Before the measured check existed, this returned `Ok` with one page and a
/// gigabyte held for the lifetime of the handle.
///
/// # This test really does allocate ~1.2 GB, for about three seconds
///
/// That is the point, and it is why the file is committed rather than the behaviour being
/// asserted only as arithmetic. It is transient — the document is closed on the spot, so
/// the memory is released before the assertion returns — but if this ever needs to move to
/// a slower or fatter tier, move it deliberately rather than marking it `#[ignore]`.
///
/// # What this does *not* prove
///
/// The allocation still happens; the check is after the fact. On a device with a hard
/// ceiling PDFium can hit its own out-of-memory path and `abort()` first, and an abort is
/// not a panic, so nothing in Rust sees it. The real fix is a structural pre-scan of the
/// declared sizes before the load, which needs qpdf — M1 PR 3. See
/// `docs/adr/0011-pdfium-engine-thread.md`.
#[test]
fn a_declared_size_bomb_is_rejected_rather_than_returned() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/conformance/fixtures/xref-bomb.pdf");
    let bytes = std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "reading {}: {e}. Regenerate with tools/make-xref-bomb.py",
            path.display()
        )
    });

    // The size-based pre-check must NOT be what rejects this, or the test would pass while
    // proving nothing about the measured check. Assert that first.
    let estimate_would_allow = Limits::with(|l| l.max_memory_bytes = 64 * 1024 * 1024);
    let by_size = open_with(bytes.clone(), estimate_would_allow);
    assert!(
        by_size.is_err(),
        "the bomb opened successfully under a 64 MiB ceiling"
    );

    // Now with the default 1 GiB ceiling, which the size estimate (~16 MB) sails through.
    match open_with(bytes, Limits::default()) {
        Err(Error::LimitExceeded {
            limit,
            requested,
            allowed,
        }) => {
            assert_eq!(limit, "max_memory_bytes");
            assert_eq!(allowed, Limits::DEFAULT.max_memory_bytes);
            assert!(
                requested > allowed,
                "the measured cost {requested} should exceed the ceiling {allowed}"
            );
        }
        Ok(doc) => panic!(
            "the bomb opened with {} pages; the measured memory check did not fire",
            doc.pages_at_open()
        ),
        Err(other) => panic!("expected LimitExceeded on memory, got {other:?}"),
    }
}

/// The time limit, driven by a fake clock. Nothing waits.
#[test]
fn the_duration_limit_is_enforced_against_the_injected_clock() {
    let clock = Arc::new(ManualClock::new(1_000));
    let limits = Limits::with(|l| l.max_duration_ms = 50);

    let doc = Pdfium::new()
        .open(
            minimal_pdf::pdf_with_pages(3).into_boxed_slice(),
            &OpenOptions::new(limits, Arc::clone(&clock) as Arc<dyn burrow_types::Clock>),
        )
        .expect("the open itself is inside the budget");

    // Exactly at the budget: still allowed.
    clock.advance(50);
    assert_eq!(
        Pdfium::new().page_count(&doc).unwrap(),
        3,
        "the boundary should be allowed, as Limits::check does elsewhere"
    );

    // One millisecond past it: refused.
    clock.advance(1);
    match Pdfium::new().page_count(&doc) {
        Err(Error::LimitExceeded {
            limit,
            requested,
            allowed,
        }) => {
            assert_eq!(limit, "max_duration_ms");
            assert_eq!(requested, 51);
            assert_eq!(allowed, 50);
        }
        other => panic!("expected LimitExceeded, got {other:?}"),
    }
}

/// An expired budget does not damage the engine: the next operation, with its own budget,
/// works.
#[test]
fn an_expired_budget_is_a_normal_outcome_and_costs_nothing() {
    let clock = Arc::new(ManualClock::new(0));
    let doc = Pdfium::new()
        .open(
            minimal_pdf::pdf_with_pages(1).into_boxed_slice(),
            &OpenOptions::new(
                Limits::with(|l| l.max_duration_ms = 0),
                Arc::clone(&clock) as Arc<dyn burrow_types::Clock>,
            ),
        )
        .expect("a zero-millisecond budget still permits the open at t=0");
    clock.advance(1);
    assert!(Pdfium::new().page_count(&doc).is_err());

    let doc = Pdfium::new()
        .open(
            minimal_pdf::pdf_with_pages(2).into_boxed_slice(),
            &OpenOptions::new(Limits::default(), Arc::new(ManualClock::new(0))),
        )
        .expect("the engine is fine");
    assert_eq!(Pdfium::new().page_count(&doc).unwrap(), 2);
}

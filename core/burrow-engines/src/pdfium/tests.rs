//! Unit tests for the PDFium engine: the happy path, and one per error variant.
//!
//! The purely-arithmetic parts of the mapping live beside the code they test, in
//! `errors.rs` and `estimate.rs` — including the one that says a zero error code can
//! never read as success. What is here needs a real engine call.

#[path = "../../testsupport/minimal_pdf.rs"]
mod minimal_pdf;

use std::sync::Arc;

use burrow_types::{Clock, Error, Limits, ManualClock, Password, Result};

use super::{Pdfium, PdfiumDocument};
use crate::{DocumentEngine, OpenOptions};

/// Limits generous enough that nothing here trips one by accident.
fn generous() -> Limits {
    Limits::default()
}

/// A stopped clock, so nothing here depends on how busy the machine is.
fn stopped() -> Arc<dyn Clock> {
    Arc::new(ManualClock::new(0))
}

fn open(bytes: Vec<u8>) -> Result<PdfiumDocument> {
    Pdfium::new().open(
        bytes.into_boxed_slice(),
        &OpenOptions::new(generous(), stopped()),
    )
}

fn open_with_password(bytes: Vec<u8>, password: &Password) -> Result<PdfiumDocument> {
    let mut options = OpenOptions::new(generous(), stopped());
    options.password = Some(password);
    Pdfium::new().open(bytes.into_boxed_slice(), &options)
}

#[test]
fn the_engine_names_itself() {
    assert_eq!(Pdfium::new().name(), "pdfium");
}

#[test]
fn a_valid_document_opens_and_reports_its_pages() {
    let doc = open(minimal_pdf::pdf_with_pages(3)).expect("a generated 3-page pdf should open");
    assert_eq!(doc.pages_at_open(), 3);
    assert_eq!(
        Pdfium::new()
            .page_count(&doc)
            .expect("page_count should succeed"),
        3
    );
}

#[test]
fn a_document_handle_is_send_and_sync_without_any_unsafe_impl() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<PdfiumDocument>();
}

#[test]
fn bytes_that_are_not_a_pdf_are_malformed() {
    assert!(matches!(
        open(minimal_pdf::not_a_pdf()),
        Err(Error::Malformed(_))
    ));
}

#[test]
fn a_truncated_pdf_is_malformed() {
    assert!(matches!(
        open(minimal_pdf::truncated_pdf()),
        Err(Error::Malformed(_))
    ));
}

#[test]
fn empty_input_is_malformed_rather_than_a_zero_page_success() {
    match open(Vec::new()) {
        Err(Error::Malformed(_)) => {}
        other => panic!("expected Malformed, got {other:?}"),
    }
}

#[test]
fn a_pdf_with_an_empty_page_tree_is_malformed_not_an_empty_success() {
    // The case ADR 0006 requirement 6 is really about: "opened fine, zero pages" must not
    // be a thing burrow can return.
    match open(minimal_pdf::pdf_with_no_pages()) {
        Err(Error::Malformed(_)) => {}
        other => panic!("expected Malformed for a zero-page document, got {other:?}"),
    }
}

#[test]
fn an_encrypted_document_with_no_password_needs_one() {
    match open(minimal_pdf::pdf_encrypted_with_unusable_credentials()) {
        Err(Error::PasswordRequired) => {}
        other => panic!("expected PasswordRequired, got {other:?}"),
    }
}

#[test]
fn an_encrypted_document_with_the_wrong_password_still_needs_one() {
    // Exercises the path that builds and passes the NUL-terminated password buffer.
    let password = Password::new(b"not the password");
    match open_with_password(
        minimal_pdf::pdf_encrypted_with_unusable_credentials(),
        &password,
    ) {
        Err(Error::PasswordRequired) => {}
        other => panic!("expected PasswordRequired, got {other:?}"),
    }
}

#[test]
fn a_password_containing_a_nul_is_rejected_without_echoing_it() {
    let password = Password::new(b"before\0after");
    let error = open_with_password(minimal_pdf::pdf_with_pages(1), &password)
        .expect_err("a NUL in the password should be rejected");
    assert!(
        matches!(error, Error::InvalidArgument(_)),
        "expected InvalidArgument, got {error:?}"
    );
    let rendered = error.to_string();
    for leaked in ["before", "after"] {
        assert!(
            !rendered.contains(leaked),
            "{leaked:?} leaked into {rendered:?}"
        );
    }
}

#[test]
fn an_oversized_input_is_rejected_before_the_engine_sees_it() {
    let limits = Limits::with(|l| l.max_input_bytes = 16);
    let bytes = minimal_pdf::pdf_with_pages(1);
    let requested = u64::try_from(bytes.len()).unwrap();
    match Pdfium::new().open(
        bytes.into_boxed_slice(),
        &OpenOptions::new(limits, stopped()),
    ) {
        Err(Error::LimitExceeded {
            limit,
            requested: r,
            allowed,
        }) => {
            assert_eq!(limit, "max_input_bytes");
            assert_eq!(r, requested);
            assert_eq!(allowed, 16);
        }
        other => panic!("expected LimitExceeded, got {other:?}"),
    }
}

#[test]
fn too_many_pages_is_rejected_and_no_handle_is_returned() {
    let limits = Limits::with(|l| l.max_pages = 2);
    match Pdfium::new().open(
        minimal_pdf::pdf_with_pages(5).into_boxed_slice(),
        &OpenOptions::new(limits, stopped()),
    ) {
        Err(Error::LimitExceeded {
            limit,
            requested,
            allowed,
        }) => {
            assert_eq!(limit, "max_pages");
            assert_eq!(requested, 5);
            assert_eq!(allowed, 2);
        }
        other => panic!("expected LimitExceeded, got {other:?}"),
    }
}

#[test]
fn a_tight_memory_estimate_rejects_before_loading() {
    let limits = Limits::with(|l| l.max_memory_bytes = 1);
    match Pdfium::new().open(
        minimal_pdf::pdf_with_pages(1).into_boxed_slice(),
        &OpenOptions::new(limits, stopped()),
    ) {
        Err(Error::LimitExceeded { limit, .. }) => assert_eq!(limit, "max_memory_bytes"),
        other => panic!("expected LimitExceeded, got {other:?}"),
    }
}

#[test]
fn a_spent_budget_stops_the_next_call_without_waiting() {
    let clock = Arc::new(ManualClock::new(0));
    let limits = Limits::with(|l| l.max_duration_ms = 10);

    let doc = Pdfium::new()
        .open(
            minimal_pdf::pdf_with_pages(2).into_boxed_slice(),
            &OpenOptions::new(limits, Arc::clone(&clock) as Arc<dyn Clock>),
        )
        .expect("the document should open inside its budget");

    clock.advance(11);

    match Pdfium::new().page_count(&doc) {
        Err(Error::LimitExceeded { limit, .. }) => assert_eq!(limit, "max_duration_ms"),
        other => panic!("expected LimitExceeded, got {other:?}"),
    }
}

#[test]
fn the_engine_is_not_poisoned_by_any_of_the_errors_above() {
    // Every failure here is a normal outcome. If one of them had poisoned the engine, the
    // whole suite after it would fail with `Internal` -- so assert it directly rather
    // than discovering it as a cascade.
    let _ = open(minimal_pdf::not_a_pdf());
    let _ = open(Vec::new());
    let _ = open(minimal_pdf::pdf_encrypted_with_unusable_credentials());
    assert!(
        !super::thread::is_poisoned(),
        "a malformed or encrypted file poisoned the engine"
    );
    assert!(open(minimal_pdf::pdf_with_pages(1)).is_ok());
}

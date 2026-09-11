//! Unit tests for the qpdf structure engine.
//!
//! The purely-arithmetic parts live beside the code they test, in `errors.rs`, `ffi.rs`
//! and `limits.rs` — including the bitmask test and the zero-code test. What is here needs
//! a real qpdf call.
//!
//! # These tests are also the exception-safety evidence
//!
//! [`ffi`] declares qpdf's C API as plain `extern "C"` on the strength of upstream's claim
//! that every function catches C++ exceptions internally. That claim is load-bearing: if
//! it were false, an exception would unwind into Rust and the behaviour would be
//! undefined.
//!
//! So rather than trusting it, the tests below drive an error through **every entry point
//! this module calls** and assert a typed `Err` comes back. A process that survives
//! `qpdf_read_memory` on deliberate garbage, on a truncated file, and on an encrypted file
//! with the wrong password is a process where the guarantee held — and if it ever stops
//! holding, these abort rather than pass.

use crate::minimal_pdf;

use std::sync::Arc;

use burrow_types::{Clock, Error, Limits, ManualClock, Password, Result};

use super::Qpdf;
use crate::{CheckOptions, StructureEngine, StructureReport};

/// A stopped clock, so nothing here depends on how busy the machine is.
fn stopped() -> Arc<dyn Clock> {
    Arc::new(ManualClock::new(0))
}

fn check(bytes: Vec<u8>) -> Result<StructureReport> {
    Qpdf::new().check(
        bytes.into_boxed_slice(),
        &CheckOptions::new(Limits::default(), stopped()),
    )
}

fn check_with_password(bytes: Vec<u8>, password: &Password) -> Result<StructureReport> {
    let mut options = CheckOptions::new(Limits::default(), stopped());
    options.password = Some(password);
    Qpdf::new().check(bytes.into_boxed_slice(), &options)
}

#[test]
fn the_engine_names_itself() {
    assert_eq!(Qpdf::new().name(), "qpdf");
}

#[test]
fn a_valid_document_reports_its_structure() {
    let report = check(minimal_pdf::pdf_with_pages(3)).expect("a generated pdf should check out");
    assert_eq!(report.pages, 3);
}

#[test]
fn page_counts_agree_with_what_the_generator_wrote() {
    for pages in [1usize, 2, 10, 137] {
        let report = check(minimal_pdf::pdf_with_pages(pages)).expect("should check out");
        assert_eq!(report.pages, u64::try_from(pages).unwrap());
    }
}

/// A file that once aborted the process.
///
/// `qpdf_is_linearized` does not route through qpdf's `trap_errors`, and an object number
/// above `INT_MAX` makes it throw `std::range_error` straight through the FFI boundary.
/// This crate no longer calls it; if it ever does again, this aborts rather than fails.
#[test]
fn an_object_number_above_int_max_is_handled_not_fatal() {
    // The assertion is that this **returns at all**. Whether the document opens is beside
    // the point and depends on the generator's cross-reference table; what mattered before
    // the fix was that the process died here.
    let result = check(minimal_pdf::pdf_with_object_number_above_int_max());
    assert!(
        result.is_ok() || matches!(result, Err(Error::Malformed(_))),
        "expected a typed outcome, got {result:?}"
    );
}

/// The exception-safety evidence, entry point by entry point.
///
/// Each of these drives qpdf into a failure that its C++ core signals by throwing. A
/// typed `Err` here means the C API caught it; an abort or a crash would mean the
/// guarantee `ffi`'s docs rely on is false.
#[test]
fn every_failure_path_returns_a_typed_error_rather_than_unwinding() {
    let cases: [(&str, Vec<u8>); 4] = [
        ("not a pdf at all", minimal_pdf::not_a_pdf()),
        ("truncated mid-document", minimal_pdf::truncated_pdf()),
        ("a valid header and nothing else", b"%PDF-1.7\n".to_vec()),
        ("a single byte", vec![b'%']),
    ];
    for (what, bytes) in cases {
        let result = check(bytes);
        assert!(
            result.is_err(),
            "{what}: expected a typed error, got {result:?}"
        );
    }
}

#[test]
fn bytes_that_are_not_a_pdf_are_malformed() {
    assert!(matches!(
        check(minimal_pdf::not_a_pdf()),
        Err(Error::Malformed(_))
    ));
}

#[test]
fn empty_input_is_malformed() {
    assert!(matches!(check(Vec::new()), Err(Error::Malformed(_))));
}

/// The reason qpdf earns its place in this PR.
///
/// PDFium reports a password failure through a global error code that is only meaningful
/// immediately after the failing call. qpdf reports it as `qpdf_e_password`, a value with
/// one meaning, which is what makes this mapping safe to rely on.
#[test]
fn an_encrypted_document_without_a_password_is_password_required() {
    match check(minimal_pdf::pdf_encrypted_with_unusable_credentials()) {
        Err(Error::PasswordRequired) => {}
        other => panic!("expected PasswordRequired, got {other:?}"),
    }
}

#[test]
fn an_encrypted_document_with_the_wrong_password_is_password_required() {
    let password = Password::new(b"not the password");
    match check_with_password(
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
    let error = check_with_password(minimal_pdf::pdf_with_pages(1), &password)
        .expect_err("a NUL in the password should be rejected");
    assert!(matches!(error, Error::InvalidArgument(_)), "{error:?}");
    let rendered = error.to_string();
    for leaked in ["before", "after"] {
        assert!(
            !rendered.contains(leaked),
            "{leaked:?} leaked into {rendered:?}"
        );
    }
}

#[test]
fn an_oversized_input_is_rejected_before_qpdf_sees_it() {
    let limits = Limits::with(|l| l.max_input_bytes = 16);
    let bytes = minimal_pdf::pdf_with_pages(1);
    let requested = u64::try_from(bytes.len()).unwrap();
    match Qpdf::new().check(
        bytes.into_boxed_slice(),
        &CheckOptions::new(limits, stopped()),
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
fn too_many_pages_is_a_limit_error() {
    let limits = Limits::with(|l| l.max_pages = 2);
    match Qpdf::new().check(
        minimal_pdf::pdf_with_pages(5).into_boxed_slice(),
        &CheckOptions::new(limits, stopped()),
    ) {
        Err(Error::LimitExceeded {
            limit, requested, ..
        }) => {
            assert_eq!(limit, "max_pages");
            assert_eq!(requested, 5);
        }
        other => panic!("expected LimitExceeded, got {other:?}"),
    }
}

/// The pre-scan guards qpdf too, not just PDFium. qpdf is a C++ parser and this is
/// untrusted input; its own global limits are a layer under the pre-scan, not instead of
/// it.
#[test]
fn the_declared_size_bomb_is_refused_before_qpdf_parses_it() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/conformance/fixtures/xref-bomb.pdf");
    let Ok(bytes) = std::fs::read(&path) else {
        // The unit tests must not depend on a fixture the integration tests own; if it is
        // missing, `tests/limits.rs` is where that failure belongs.
        return;
    };
    match check(bytes) {
        Err(Error::LimitExceeded { limit, .. }) => assert_eq!(limit, "max_memory_bytes"),
        other => panic!("the pre-scan should have refused the bomb, got {other:?}"),
    }
}

#[test]
fn a_report_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<StructureReport>();
}

//! #285, fuzz builds only: qpdf's error text tells handle misuse apart from its own internal
//! errors, against the pinned qpdf -- the text read through the real error path, never a
//! constant compared with itself.
//!
//! The owner's conditions (2026-10-08): a genuine handle misuse against the pinned qpdf must
//! produce burrow's distinct message, so a qpdf release that rewords "attempted access to unknown
//! object handle" turns this red rather than quietly folding the class back into #285's entry; a
//! near-miss twin; and a mutation, asserted applied, that the test catches. The #285 text itself
//! needs an input that is held privately and cannot be committed; it is exercised end to end by
//! the nightly's own classifier on the fuzz target, against the private input, outside the repo.

use std::sync::Arc;

use burrow_types::{Clock, Error, Limits, ManualClock};

use super::{ffi, open_document};
use crate::OpenOptions;
use crate::codes::qpdf::{INTERNAL_UNKNOWN_HANDLE, internal_from_detail};

fn opened() -> super::Document {
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/conformance/fixtures/blank-1page.pdf"),
    )
    .expect("a committed fixture");
    let options = OpenOptions::new(
        Limits::default(),
        Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
    );
    let (document, _, _, _) =
        open_document(bytes.into_boxed_slice(), &options).expect("the fixture opens");
    document
}

#[test]
fn a_handle_qpdf_does_not_hold_is_named_as_a_burrow_defect() {
    let document = opened();
    // A HANDLE NOTHING ISSUED. qpdf's own lookup misses it and throws `qpdf_e_internal` with its
    // fixed text; the C API traps it. Real misuse, through the real error path.
    // SAFETY: `document.data` is live; qpdf validates the handle itself and reports a miss as an
    // error rather than dereferencing anything.
    let _ = unsafe { ffi::qpdf_oh_get_type_code(document.data, 0x7fff_fff0) };
    match document.take_error() {
        Some(Error::Internal(message)) => assert_eq!(
            message, INTERNAL_UNKNOWN_HANDLE,
            "qpdf's text for an unknown handle no longer matches; a rewording would fold \
             burrow's handle defects back into #285's ledger entry"
        ),
        other => panic!("expected burrow's unknown-handle message, got {other:?}"),
    }
}

#[test]
fn a_handle_qpdf_issued_is_not_an_error_at_all() {
    // THE TWIN: the same call on a handle qpdf issued raises nothing.
    let document = opened();
    // SAFETY: `document.data` is live; the trailer handle is issued by qpdf for this document.
    let trailer = unsafe { ffi::qpdf_get_trailer(document.data) };
    let _ = unsafe { ffi::qpdf_oh_get_type_code(document.data, trailer) };
    assert!(document.take_error().is_none());
}

#[test]
fn any_other_internal_text_keeps_the_generic_message() {
    // Not the text of either rule -- the caller falls back to `map_code`. No document needed:
    // this is the fallback, not the comparison the tests above hold against qpdf.
    assert!(internal_from_detail(b"some other logic error").is_none());
    assert!(internal_from_detail(b"attempted access to unknown object handle ").is_none());
}

//! Turning qpdf error codes into burrow's typed errors.
//!
//! The same three rules as [`crate::pdfium::errors`], for the same reasons:
//!
//! 1. **Failure is decided by the primary return value**, then classified by the code.
//! 2. **Classification is by code, never by prose.** qpdf makes this easy —
//!    `qpdf_e_password` is a machine-readable "encrypted, and the password did not work",
//!    so there is no temptation to match on `"invalid password"`.
//! 3. **Messages are fixed constants.**
//!
//! Rule 3 matters more here than it did for PDFium, and it is why `ffi.rs` does not even
//! declare `qpdf_get_error_full_text`, `qpdf_get_error_message_detail`,
//! `qpdf_get_error_filename` or `qpdf_get_error_file_position`. qpdf's error text quotes
//! the file:
//!
//! ```text
//! WARNING: spike-input (object 3 0, offset 9999999999): expected n n obj
//! ```
//!
//! Object numbers and byte offsets are file content by any reasonable reading
//! (spike 0001, Finding 6). A function that cannot be called cannot leak, so the safest
//! version of "do not forward qpdf's messages" is to have no way to obtain them.

use burrow_types::Error;
use core::ffi::c_int;

use super::ffi::code;

/// Map a qpdf error code to a typed error.
///
/// Call only when the call's own return value has already established failure.
pub(super) fn map_code(code: c_int) -> Error {
    match code {
        // The engine contradicted itself: the call failed, yet it reports no error. Same
        // reasoning as PDFium's zero-code case -- a state we do not understand is
        // `Internal`, which is fatal to a web worker under ADR 0009, and that is the
        // conservative response. What must never happen is it reading as success.
        code::SUCCESS => Error::Internal(
            "qpdf reported a failure with no error code; engine state is unknown".to_owned(),
        ),

        // The one that earns qpdf its place in this PR: a machine-readable distinction
        // between "the password was wrong" and "the file is broken", which PDFium does
        // not give us as reliably. Covers both "encrypted and none supplied" and
        // "encrypted and the wrong one supplied"; telling those apart is an oracle we
        // have no reason to provide.
        code::PASSWORD => Error::PasswordRequired,

        code::DAMAGED_PDF => Error::Malformed("qpdf: the document is damaged".to_owned()),
        code::PAGES => Error::Malformed("qpdf: the page structure is unusable".to_owned()),
        code::OBJECT => Error::Malformed("qpdf: an object is malformed".to_owned()),
        code::JSON => Error::Malformed("qpdf: malformed json".to_owned()),

        code::UNSUPPORTED => {
            Error::Unsupported("qpdf: the document uses an unsupported feature".to_owned())
        }

        // A linearization *warning* arriving as an error means the file's linearisation
        // data disagrees with its contents. The document is still readable, so this is
        // malformation rather than an unsupported feature.
        code::LINEARIZATION => {
            Error::Malformed("qpdf: the linearization data is inconsistent".to_owned())
        }

        // qpdf's own words for `qpdf_e_internal` are "logic/programming error --
        // indicates bug". Ours, not necessarily, but it is a state neither side
        // understands.
        code::INTERNAL => Error::Internal("qpdf reported an internal error".to_owned()),

        // `qpdf_e_system` is documented as "I/O error, memory error, etc." -- but this
        // crate only ever calls `qpdf_read_memory`, on a buffer it supplied itself. There
        // is no file, no descriptor and no disk, so there is no I/O to fail: in practice
        // this is qpdf running off the end of a truncated buffer. Measured: a nine-byte
        // input of `%PDF-1.7\n` produces exactly this code.
        //
        // So it maps to `Malformed`, which is what it means here. The residual risk is a
        // genuine allocation failure inside qpdf being reported as a damaged document, and
        // that is the better trade in both directions: a real out-of-memory inside C++
        // usually aborts rather than returning at all, whereas truncated input is
        // commonplace and telling that user their file is damaged is simply correct.
        code::SYSTEM => Error::Malformed("qpdf: the document ends unexpectedly".to_owned()),

        // A code from a qpdf newer than `ffi::code`. Same reasoning as the zero case.
        // The number is deliberately not included: no raw engine code may escape this
        // crate, and a fixed string cannot carry input-derived bytes.
        _ => Error::Internal("qpdf reported an unrecognised error code".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn variant_of(error: &Error) -> &'static str {
        match error {
            Error::Malformed(_) => "Malformed",
            Error::Unsupported(_) => "Unsupported",
            Error::PasswordRequired => "PasswordRequired",
            Error::LimitExceeded { .. } => "LimitExceeded",
            Error::InvalidArgument(_) => "InvalidArgument",
            Error::Io(_) => "Io",
            Error::Internal(_) => "Internal",
            _ => "unknown",
        }
    }

    #[test]
    fn a_zero_code_on_a_failed_call_is_never_success() {
        let err = map_code(code::SUCCESS);
        assert!(matches!(err, Error::Internal(_)), "{err:?}");
        let as_result: burrow_types::Result<()> = Err(err);
        assert!(as_result.is_err());
    }

    #[test]
    fn the_password_code_is_what_makes_encryption_detectable_without_string_matching() {
        assert!(matches!(map_code(code::PASSWORD), Error::PasswordRequired));
    }

    #[test]
    fn every_declared_code_maps_to_its_variant() {
        let cases = [
            (code::INTERNAL, "Internal"),
            (code::SYSTEM, "Malformed"),
            (code::UNSUPPORTED, "Unsupported"),
            (code::PASSWORD, "PasswordRequired"),
            (code::DAMAGED_PDF, "Malformed"),
            (code::PAGES, "Malformed"),
            (code::OBJECT, "Malformed"),
            (code::JSON, "Malformed"),
            (code::LINEARIZATION, "Malformed"),
        ];
        for (code, expected) in cases {
            let err = map_code(code);
            assert_eq!(variant_of(&err), expected, "code {code} mapped to {err:?}");
        }
    }

    #[test]
    fn an_unrecognised_code_is_internal_rather_than_silently_malformed() {
        assert!(matches!(map_code(9_999), Error::Internal(_)));
    }

    /// The messages are what a user and a log see, and qpdf's own text quotes object
    /// numbers and byte offsets. Ours must carry no number at all.
    #[test]
    fn no_mapped_message_contains_a_digit_so_nothing_from_a_file_can_have_leaked() {
        for code in -8..64 {
            let rendered = map_code(code).to_string();
            assert!(
                !rendered.chars().any(|c| c.is_ascii_digit()),
                "a digit reached the message for code {code}: {rendered:?}"
            );
        }
    }
}

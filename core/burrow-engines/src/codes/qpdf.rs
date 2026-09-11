//! Turning qpdf error codes into burrow's typed errors.
//!
//! The constants and the mapping both live here rather than in `qpdf::ffi`, so the web
//! implementation gets the *same* table without a second copy. See [`crate::codes`] for
//! why, and for the three rules this table follows.
//!
//! Rule 3 — fixed-constant messages — matters more here than it did for PDFium, and it is
//! why `qpdf::ffi` does not even declare `qpdf_get_error_full_text`,
//! `qpdf_get_error_message_detail`, `qpdf_get_error_filename` or
//! `qpdf_get_error_file_position`. qpdf's error text quotes the file:
//!
//! ```text
//! WARNING: spike-input (object 3 0, offset 9999999999): expected n n obj
//! ```
//!
//! Object numbers and byte offsets are file content by any reasonable reading
//! (spike 0001, Finding 6). A function that cannot be called cannot leak, so the safest
//! version of "do not forward qpdf's messages" is to have no way to obtain them — and on
//! the web that is enforced a second time, by the module's `EXPORTED_FUNCTIONS` allowlist.

use burrow_types::Error;
use core::ffi::c_int;

/// `typedef int QPDF_ERROR_CODE` — `qpdf-c.h:136`.
pub(crate) type QpdfErrorCode = c_int;

/// `#define QPDF_ERRORS 1 << 1` — `qpdf-c.h:139`. The **only** bit that means failure.
pub(crate) const QPDF_ERRORS: QpdfErrorCode = 1 << 1;

/// Whether a `QPDF_ERROR_CODE` reports an actual error.
///
/// `QPDF_ERROR_CODE` is a **bitmask, not an enum** (`qpdf-c.h:134-139`): `QPDF_SUCCESS` is
/// 0, but `QPDF_WARNINGS` and `QPDF_ERRORS` are separate bits that can both be set or
/// neither. So the obvious `result != QPDF_SUCCESS` is **wrong** — it reports a file that
/// parsed perfectly but emitted a warning as a failure. The only correct test is
/// `result & QPDF_ERRORS`.
///
/// This lives here, beside the code table, so the test below is the one place in the crate
/// the trap is pinned down — for the native path and the web path at once.
pub(crate) const fn has_errors(code: QpdfErrorCode) -> bool {
    code & QPDF_ERRORS != 0
}

/// `enum qpdf_error_code_e` — `Constants.h:85-96`. Upstream guarantees the numbering
/// across major releases, which is what makes mapping by code safe to rely on.
pub(crate) mod code {
    use core::ffi::c_int;

    /// `qpdf_e_success` — no error. `Constants.h:86`.
    pub(crate) const SUCCESS: c_int = 0;
    /// `qpdf_e_internal` — a logic error in qpdf; indicates a bug. `Constants.h:87`.
    pub(crate) const INTERNAL: c_int = 1;
    /// `qpdf_e_system` — I/O or memory error. `Constants.h:88`.
    pub(crate) const SYSTEM: c_int = 2;
    /// `qpdf_e_unsupported` — a PDF feature qpdf does not support. `Constants.h:89`.
    pub(crate) const UNSUPPORTED: c_int = 3;
    /// `qpdf_e_password` — **incorrect password for an encrypted file**. `Constants.h:90`.
    pub(crate) const PASSWORD: c_int = 4;
    /// `qpdf_e_damaged_pdf` — syntax errors or other damage. `Constants.h:91`.
    pub(crate) const DAMAGED_PDF: c_int = 5;
    /// `qpdf_e_pages` — erroneous or unsupported page structure. `Constants.h:92`.
    pub(crate) const PAGES: c_int = 6;
    /// `qpdf_e_object` — type or bounds error accessing an object. `Constants.h:93`.
    pub(crate) const OBJECT: c_int = 7;
    /// `qpdf_e_json` — error in qpdf JSON. `Constants.h:94`.
    pub(crate) const JSON: c_int = 8;
    /// `qpdf_e_linearization` — a linearization warning. `Constants.h:95`.
    pub(crate) const LINEARIZATION: c_int = 9;
}

/// Map a qpdf error code to a typed error.
///
/// Call only when the call's own return value has already established failure.
pub(crate) fn map_code(code: c_int) -> Error {
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

    /// The bitmask trap, pinned down. Same class as PDFium's `-0` sentinel, same treatment.
    #[test]
    fn warnings_alone_are_not_an_error() {
        const QPDF_SUCCESS: QpdfErrorCode = 0;
        const QPDF_WARNINGS: QpdfErrorCode = 1 << 0;

        assert!(!has_errors(QPDF_SUCCESS));
        assert!(
            !has_errors(QPDF_WARNINGS),
            "a warnings-only result must not read as an error"
        );
        assert!(has_errors(QPDF_ERRORS));
        assert!(
            has_errors(QPDF_ERRORS | QPDF_WARNINGS),
            "errors alongside warnings are still errors"
        );

        // And the naive test this helper exists to replace really is wrong.
        assert_ne!(QPDF_WARNINGS, QPDF_SUCCESS);
    }
}

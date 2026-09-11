//! Turning PDFium error codes into burrow's typed errors.
//!
//! Three rules, and each is here because breaking it has already caused a bug somewhere:
//!
//! 1. **Failure is decided by the primary return value, never by the error code.** A null
//!    handle or a negative count is what says "this failed"; the code only *classifies* a
//!    failure that is already established. Folding the two into one number is how
//!    `-FPDF_GetLastError()` came to yield `-0` — which `=== 0` in JS — so a failed load
//!    read as "opened fine, zero pages". See ADR 0006 requirement 6.
//! 2. **Classification is by code, never by prose.** Engine messages are not a stable API
//!    and are not ours; matching on them breaks silently on an engine bump.
//! 3. **Messages are fixed constants.** No error code, no engine text, and above all no
//!    input bytes. `core/CLAUDE.md` forbids a raw code escaping this crate, and an error
//!    string is one of the places file content leaks from.

use burrow_types::Error;
use core::ffi::c_ulong;

use super::ffi;

/// The message for a load failure PDFium could not explain beyond "something went wrong".
const COULD_NOT_OPEN: &str = "pdfium could not open the document";

/// A page count that could not be read.
///
/// Deliberately **not** classified by `FPDF_GetLastError()`. `fpdfview.h:625` says the
/// error global is only meaningful "in conjunction with APIs that mention
/// `FPDF_GetLastError()` in their documentation", and `FPDF_GetPageCount`'s block
/// (`fpdfview.h:697-703`) does not mention it — so after that call the global holds
/// whatever some earlier call left there. Reading it would attach another operation's
/// error to this one: a stale `FPDF_ERR_PASSWORD` would surface as
/// [`Error::PasswordRequired`] on a document that is already open, which is a confidently
/// wrong answer rather than a vague one.
///
/// Failure is still established by the primary return value — a negative count — which is
/// what rule 1 above actually requires. Only the classification is dropped.
pub(super) fn unreadable_page_tree() -> Error {
    Error::Malformed("pdfium could not read the page tree".to_owned())
}

/// Map a failed document load to a typed error.
///
/// Call this **only** when `FPDF_LoadMemDocument64`'s own return value has already
/// established failure — a null handle — and `code` must be the value
/// [`ffi::load_mem_document64`] returned alongside it, not a separately fetched one.
///
/// There is no equivalent for `FPDF_GetPageCount`: see [`unreadable_page_tree`], whose
/// whole point is that the error global is undefined after that call.
pub(super) fn map_failure(code: c_ulong) -> Error {
    match code {
        // The engine contradicted itself: the call failed, yet it reports no error. We do
        // not understand the state it is in, so this is `Internal` rather than a normal
        // outcome -- which on the web costs a worker (ADR 0009), and that is the
        // conservative response to an engine we can no longer reason about. Every failure
        // PDFium can explain sets a non-zero code, so this is not a path real files take.
        //
        // What must never happen is this reading as success. That is the whole of
        // ADR 0006 requirement 6, and it has its own test.
        ffi::FPDF_ERR_SUCCESS => Error::Internal(
            "pdfium reported a failure with no error code; engine state is unknown".to_owned(),
        ),

        ffi::FPDF_ERR_UNKNOWN | ffi::FPDF_ERR_FILE | ffi::FPDF_ERR_FORMAT => {
            Error::Malformed(COULD_NOT_OPEN.to_owned())
        }

        // Covers both "encrypted, no password given" and "the password given was wrong".
        // PDFium does not distinguish them, and neither do we: telling a caller which of
        // the two it was is an oracle we have no reason to provide.
        ffi::FPDF_ERR_PASSWORD => Error::PasswordRequired,

        ffi::FPDF_ERR_SECURITY => {
            Error::Unsupported("pdfium: unsupported document security scheme".to_owned())
        }

        ffi::FPDF_ERR_PAGE => {
            Error::Malformed("pdfium: page not found or content error".to_owned())
        }

        // XFA is a recognised construct we do not support, not a damaged file. Our build
        // is not expected to define `PDF_ENABLE_XFA`, but the code space belongs to the
        // engine: handling these keeps them out of the `Internal` arm below, where they
        // would cost a worker for a file that is merely unsupported.
        ffi::FPDF_ERR_XFALOAD | ffi::FPDF_ERR_XFALAYOUT => {
            Error::Unsupported("pdfium: XFA forms are not supported".to_owned())
        }

        // A code from a PDFium newer than the declarations in `ffi`. Same reasoning as
        // the zero case: an engine saying something we do not understand is a state we
        // must not paper over. The code itself is deliberately not included -- it must not
        // escape this crate, and a fixed string cannot carry input-derived bytes.
        _ => Error::Internal("pdfium reported an unrecognised error code".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ADR 0006 requirement 6, and ROADMAP M1 item 4's named test: a zero error code on a
    /// failed call must never be readable as success.
    #[test]
    fn a_zero_error_code_on_a_failed_call_is_never_success() {
        let err = map_failure(ffi::FPDF_ERR_SUCCESS);
        assert!(
            matches!(err, Error::Internal(_)),
            "FPDF_ERR_SUCCESS on a failure mapped to {err:?}, not Internal"
        );
        // The shape that matters: whatever it maps to, it is an error.
        let as_result: burrow_types::Result<()> = Err(err);
        assert!(as_result.is_err());
    }

    /// The variant of an error, as a name, so a table of expectations reads as a table.
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
    fn every_declared_code_maps_to_its_variant() {
        let cases = [
            (ffi::FPDF_ERR_UNKNOWN, "Malformed"),
            (ffi::FPDF_ERR_FILE, "Malformed"),
            (ffi::FPDF_ERR_FORMAT, "Malformed"),
            (ffi::FPDF_ERR_PASSWORD, "PasswordRequired"),
            (ffi::FPDF_ERR_SECURITY, "Unsupported"),
            (ffi::FPDF_ERR_PAGE, "Malformed"),
            (ffi::FPDF_ERR_XFALOAD, "Unsupported"),
            (ffi::FPDF_ERR_XFALAYOUT, "Unsupported"),
        ];
        for (code, expected) in cases {
            let err = map_failure(code);
            assert_eq!(variant_of(&err), expected, "code {code} mapped to {err:?}");
        }
    }

    #[test]
    fn an_unreadable_page_tree_is_malformed_and_carries_no_engine_state() {
        let err = unreadable_page_tree();
        assert!(matches!(err, Error::Malformed(_)));
        assert!(!err.to_string().chars().any(|c| c.is_ascii_digit()));
    }

    #[test]
    fn an_unrecognised_code_is_internal_rather_than_silently_malformed() {
        assert!(matches!(map_failure(9_999), Error::Internal(_)));
    }

    /// The messages are what a user and a log see. They must not carry the engine's code,
    /// its prose, or anything derived from the input.
    #[test]
    fn no_mapped_message_contains_a_digit_so_no_code_can_have_leaked() {
        for code in 0..64 {
            let rendered = map_failure(code).to_string();
            assert!(
                !rendered.chars().any(|c| c.is_ascii_digit()),
                "a digit reached the message for code {code}: {rendered:?}"
            );
        }
    }
}

//! Property tests for the PDFium document engine.
//!
//! The three invariants that matter for `open` and `page_count`:
//!
//! 1. A document with N pages reports N pages.
//! 2. **Any** byte string is either opened or typed-rejected — never a panic, never an
//!    abort, never a hang. This is the property the fuzz target explores; proptest covers
//!    the shrunk, reproducible half of it.
//! 3. The size and page limits trip exactly at their boundaries, not one either side.

#![cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod support;

use burrow_types::{Error, Limits};
use proptest::prelude::*;
use support::{minimal_pdf, open, open_with, page_count};

/// Every message `burrow-engines` can produce for an untrusted input.
///
/// Fixed strings, all of them. A message assembled from the input would not be in this
/// list, which is what makes the property below meaningful: it is the difference between
/// "we did not crash" and "nothing from the file reached the user".
const ALLOWED_MESSAGES: &[&str] = &[
    // errors.rs
    "pdfium could not open the document",
    "pdfium could not read the page tree",
    "pdfium: page not found or content error",
    "pdfium: unsupported document security scheme",
    "pdfium: XFA forms are not supported",
    "pdfium reported a failure with no error code; engine state is unknown",
    "pdfium reported an unrecognised error code",
    // mod.rs
    "document has no pages",
    "password contains a NUL byte, which pdfium's C API cannot carry",
    "pdfium reported a page count that is not a count",
    "input length does not fit in u64",
    // thread.rs
    "pdfium document ids exhausted",
    "pdfium document handle is not open",
    "the pdfium engine is no longer usable and was not reused",
];

/// Names a `LimitExceeded` may carry. Numbers in that variant are limits, not content.
const ALLOWED_LIMITS: &[&str] = &[
    "max_input_bytes",
    "max_memory_bytes",
    "max_duration_ms",
    "max_pages",
    "max_pixels",
];

/// The property: for **any** input, the outcome is a value or one of a fixed set of
/// errors, reached without a panic or an abort.
///
/// Returns a description of the failure rather than asserting, so `prop_assert!` can
/// shrink and report it.
fn check_outcome<T>(result: &Result<T, Error>) -> Result<(), String> {
    match result {
        Ok(_) => Ok(()),
        Err(Error::PasswordRequired) => Ok(()),
        Err(Error::LimitExceeded { limit, .. }) => {
            if ALLOWED_LIMITS.contains(limit) {
                Ok(())
            } else {
                Err(format!("unknown limit name {limit:?}"))
            }
        }
        Err(
            Error::Malformed(message)
            | Error::Unsupported(message)
            | Error::InvalidArgument(message)
            | Error::Io(message)
            | Error::Internal(message),
        ) => {
            if ALLOWED_MESSAGES.contains(&message.as_str()) {
                Ok(())
            } else {
                Err(format!(
                    "an error message that is not a fixed constant reached the caller, so it \
                     may carry input-derived bytes: {message:?}"
                ))
            }
        }
        // `Error` is `#[non_exhaustive]`. A variant nobody has taught this test about is a
        // gap in the review, not a pass.
        Err(other) => Err(format!("unreviewed error variant: {other:?}")),
    }
}

proptest! {
    // Every case here is a real PDFium parse. 128 keeps the whole file well under a second
    // on the dev machine while still exploring; the default 256 would also be fine, and the
    // number is a CI-headroom choice rather than a correctness one.
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// A generated N-page document reports N pages, through both paths that report it.
    #[test]
    fn a_generated_document_reports_the_pages_it_was_given(pages in 1usize..=64) {
        let doc = open(minimal_pdf::pdf_with_pages(pages))
            .expect("a generated document should open");
        let expected = u64::try_from(pages).unwrap();
        prop_assert_eq!(doc.pages_at_open(), expected);
        prop_assert_eq!(page_count(&doc).unwrap(), expected);
    }

    /// Arbitrary mutations of a valid PDF are opened or typed-rejected, never fatal.
    ///
    /// Mutating a *valid* file rather than generating noise is what gets past the header
    /// check and into the parser, which is where the interesting failures are.
    #[test]
    fn any_mutation_of_a_valid_pdf_returns_a_typed_result(
        seed in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 1..32),
    ) {
        let mut bytes = minimal_pdf::pdf_with_pages(4);
        for (index, value) in seed {
            let at = index.index(bytes.len());
            bytes[at] = value;
        }
        let result = open(bytes);
        if let Err(why) = check_outcome(&result) {
            prop_assert!(false, "{why}");
        }
        if let Ok(doc) = result
            && let Err(why) = check_outcome(&page_count(&doc))
        {
            prop_assert!(false, "{why}");
        }
    }

    /// Arbitrary bytes, with no valid document underneath at all.
    #[test]
    fn arbitrary_bytes_return_a_typed_result(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
        if let Err(why) = check_outcome(&open(bytes)) {
            prop_assert!(false, "{why}");
        }
    }

    /// `max_input_bytes` permits exactly its value and rejects one byte past it.
    #[test]
    fn the_input_size_limit_trips_exactly_at_its_boundary(pages in 1usize..=8) {
        let bytes = minimal_pdf::pdf_with_pages(pages);
        let len = u64::try_from(bytes.len()).unwrap();

        // Exactly at the limit: allowed, and the document really opens.
        let at = open_with(bytes.clone(), Limits::with(|l| l.max_input_bytes = len));
        prop_assert!(at.is_ok(), "a file of exactly max_input_bytes was rejected: {:?}", at.err());

        // One byte under: rejected, naming the limit and both numbers.
        match open_with(bytes, Limits::with(|l| l.max_input_bytes = len - 1)) {
            Err(Error::LimitExceeded { limit, requested, allowed }) => {
                prop_assert_eq!(limit, "max_input_bytes");
                prop_assert_eq!(requested, len);
                prop_assert_eq!(allowed, len - 1);
            }
            other => prop_assert!(false, "expected LimitExceeded, got {:?}", other.map(|_| ())),
        }
    }

    /// `max_pages` permits exactly its value and rejects one page past it.
    #[test]
    fn the_page_limit_trips_exactly_at_its_boundary(pages in 1usize..=32) {
        let n = u64::try_from(pages).unwrap();

        let at = open_with(
            minimal_pdf::pdf_with_pages(pages),
            Limits::with(|l| l.max_pages = n),
        );
        prop_assert!(at.is_ok(), "a document of exactly max_pages was rejected: {:?}", at.err());

        match open_with(
            minimal_pdf::pdf_with_pages(pages),
            Limits::with(|l| l.max_pages = n - 1),
        ) {
            Err(Error::LimitExceeded { limit, requested, allowed }) => {
                prop_assert_eq!(limit, "max_pages");
                prop_assert_eq!(requested, n);
                prop_assert_eq!(allowed, n - 1);
            }
            other => prop_assert!(false, "expected LimitExceeded, got {:?}", other.map(|_| ())),
        }
    }
}

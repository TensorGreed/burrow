//! Trait seams over the native engines burrow builds on.
//!
//! No engine is linked yet — that lands in M1, when the first operations need it. The
//! traits exist now so `burrow-ops` can be written against a boundary rather than
//! against PDFium's C API directly, which keeps engines swappable and keeps `unsafe`
//! confined to this crate.
//!
//! Engine selection and the prebuilt-versus-source tradeoff are recorded in
//! `docs/adr/0004-native-engines.md`.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

// M1 PR 1: proof that the vendored native engines link and run. Not a public API and
// not the real engine implementation -- that is PR 2. Only compiled when the engines are
// actually available, which `build.rs` sets after verifying their checksums.
#[cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
pub mod link_check;

use burrow_types::{Limits, Result};

/// A paged document engine: opens a document and reports its shape.
///
/// Implementors wrap an untrusted parser, so every method is fallible and every method
/// takes the [`Limits`] it must enforce. Implementors must not panic.
pub trait DocumentEngine {
    /// An opened document handle.
    type Document;

    /// Short identifier for the backing engine, e.g. `"pdfium"`. Used in diagnostics.
    fn name(&self) -> &'static str;

    /// Opens a document from an in-memory buffer.
    ///
    /// `bytes` is untrusted and possibly adversarial.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Malformed`](burrow_types::Error::Malformed) for unparseable
    /// input, [`Error::PasswordRequired`](burrow_types::Error::PasswordRequired) for
    /// encrypted input, or
    /// [`Error::LimitExceeded`](burrow_types::Error::LimitExceeded) when `limits` is hit.
    fn open(&self, bytes: &[u8], limits: &Limits) -> Result<Self::Document>;

    /// Number of pages in an opened document.
    ///
    /// # Errors
    ///
    /// Returns an error if the document's page tree cannot be read.
    fn page_count(&self, doc: &Self::Document) -> Result<u64>;
}

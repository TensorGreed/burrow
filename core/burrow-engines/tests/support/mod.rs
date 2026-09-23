//! Shared helpers for the integration tests.

#![allow(dead_code)]

#[path = "../../testsupport/minimal_pdf.rs"]
pub mod minimal_pdf;

pub mod pdf_builder;

// THE GEOMETRY ORACLE (#129). Behind the same gate as the engines themselves: it declares
// PDFium symbols directly, which only a target that links `libpdfium.so` may do.
#[cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
pub mod char_box_oracle;

use std::sync::Arc;

use burrow_engines::pdfium::{Pdfium, PdfiumDocument};
use burrow_engines::{DocumentEngine, OpenOptions};
use burrow_types::{Limits, ManualClock, Result};

/// Open `bytes` under `limits`, on a clock that does not move.
///
/// A stopped clock is the right default for everything except the deadline tests: it
/// means no test can fail because a machine was busy. The document keeps this clock, so
/// its budget can never be reset by a later call -- which is the bug this signature exists
/// to make unexpressible.
pub fn open_with(bytes: Vec<u8>, limits: Limits) -> Result<PdfiumDocument> {
    Pdfium::new().open(
        bytes.into_boxed_slice(),
        &OpenOptions::new(limits, Arc::new(ManualClock::new(0))),
    )
}

/// Open `bytes` under the default limits.
pub fn open(bytes: Vec<u8>) -> Result<PdfiumDocument> {
    open_with(bytes, Limits::default())
}

/// Read a document's page count, against the budget it was opened with.
pub fn page_count(doc: &PdfiumDocument) -> Result<u64> {
    Pdfium::new().page_count(doc)
}

/// Redact one page through the public operation, for the tests that were written against the
/// probe.
///
/// **The probe is gone** — `burrow_ops::redact::page` is the entry point now, and it verifies.
/// This is the two lines of setup those tests would otherwise each repeat: a real clock, the
/// default ceilings, and the page in its own redacted set.
///
/// # Errors
///
/// Whatever the operation refused.
#[cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
pub fn redact_page(
    bytes: &[u8],
    page: usize,
    redacted: std::collections::BTreeSet<usize>,
    region: burrow_engines::pdfsyntax::region::Region,
) -> Result<(Vec<u8>, burrow_engines::redact::Report)> {
    redact_page_with(bytes, page, redacted, region, Limits::default())
}

/// As [`redact_page`], with the ceilings stated.
///
/// # Errors
///
/// Whatever the operation refused.
#[cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
pub fn redact_page_with(
    bytes: &[u8],
    page: usize,
    redacted: std::collections::BTreeSet<usize>,
    region: burrow_engines::pdfsyntax::region::Region,
    limits: Limits,
) -> Result<(Vec<u8>, burrow_engines::redact::Report)> {
    use burrow_types::SystemClock;

    let options = OpenOptions::new(limits, Arc::new(SystemClock::new()));
    let done = burrow_ops::redact::page(
        &burrow_engines::qpdf::Qpdf,
        bytes,
        page,
        &redacted,
        region,
        &options,
    )?;
    Ok((done.document, done.report))
}

/// The open options the geometry helpers take, with a real clock.
///
/// A `ManualClock` here would make every deadline checkpoint inside the walk inert, which is
/// the defect these helpers were changed to stop having built into them.
#[cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
#[must_use]
pub fn walk_options() -> OpenOptions<'static> {
    OpenOptions::new(
        Limits::default(),
        Arc::new(burrow_types::SystemClock::new()),
    )
}

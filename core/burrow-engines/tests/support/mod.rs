//! Shared helpers for the integration tests.

#![allow(dead_code)]

#[path = "../../testsupport/minimal_pdf.rs"]
pub mod minimal_pdf;

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

//! PDFium's readings for ADR 0031 §5's page identity (native): its page count, every page's
//! size without loading it, and page *N*'s rotation.
//!
//! The comparison and its rules are `redact::flatten::identity`'s. This module is the reading.
//!
//! # The route, and its cost (ADR 0031 §5)
//!
//! - **Every page is read without `FPDF_LoadPage`**: `FPDF_GetPageSizeByIndexF`, through the
//!   existing no-load [`PageRenderer::page_size`]. A load builds the page's display list, one
//!   uninterruptible parse of a hostile page each, so the pass never loads a page.
//! - **Page *N*'s rotation is read on the page the render loads anyway** ([`rotation`], called by
//!   `flatten_render` straight after its `FPDF_LoadPage` and before its bitmap exists), so the
//!   identity check adds no load of its own.
//! - **The deadline is checked before every page**, by the comparison, and by
//!   [`PageRenderer::page_size`] around each engine call.
//! - **`max_memory_bytes` is detected, not bounded**: the resident set is read before the pass and
//!   checked after it. A no-load read allocates a page object without parsing its content, but
//!   nothing here bounds what it allocates.
//!
//! # Measured on the pinned build (2026-11-04, aarch64)
//!
//! The no-load size reflects `/Rotate`, an inherited one included, and PDFium counts the root's
//! `/Count`. Both are pinned by tests in `flatten_identity_tests.rs`, because the comparison
//! relies on the first and the count fixtures on the second.

#![allow(
    dead_code,
    reason = "ADR 0031 S9b: no caller outside tests until the integration slice wires the check"
)]

use core::ffi::c_int;

use burrow_types::{Deadline, Result};

use super::{Pdfium, PdfiumDocument, ffi, page_count_of};
use crate::redact::flatten::identity::{self, PdfiumPages, Survey};
use crate::redact::frames::PageFrames;
use crate::{OpenOptions, PageRenderer};

// THE ONE DECLARATION THIS SLICE ADDS, checked against the pinned
// `engines/vendor/native-*/include/pdfium/fpdf_edit.h` (the copy the build includes). Same
// precondition as every declaration in `ffi.rs`: engine thread only.
unsafe extern "C" {
    /// ```c
    /// FPDF_EXPORT int FPDF_CALLCONV FPDFPage_GetRotation(FPDF_PAGE page);
    /// ```
    /// `fpdf_edit.h:160-172`. "Returns one of the following indicating the page rotation: 0 - No
    /// rotation. 1 - Rotated 90 degrees clockwise. 2 - Rotated 180 degrees clockwise. 3 - Rotated
    /// 270 degrees clockwise. Or returns -1 on error."
    fn FPDFPage_GetRotation(page: ffi::FpdfPage) -> c_int;
}

/// A loaded page's rotation, in degrees clockwise: 0, 90, 180 or 270.
///
/// # Errors
///
/// [`burrow_types::Error::Internal`] for any value but the header's four.
///
/// # Safety
///
/// Must be called on the engine thread with a live `page` from a live document.
pub(super) unsafe fn rotation(page: ffi::FpdfPage) -> Result<u16> {
    // SAFETY: the caller guarantees the page is live and that this is the engine thread. The call
    // reads the page's `/Rotate` and writes nothing.
    let turns = unsafe { FPDFPage_GetRotation(page) };
    identity::quarter_turns_to_degrees(turns)
}

impl PdfiumPages for PdfiumDocument {
    /// The engine's count, read again rather than taken from the open, so that the comparison
    /// is of what PDFium says now.
    fn page_count(&self) -> Result<u64> {
        page_count_of(self)
    }

    /// `FPDF_GetPageSizeByIndexF`: no `FPDF_LoadPage`.
    fn page_size(&self, index: u64) -> Result<(f32, f32)> {
        PageRenderer::page_size(&Pdfium, self, index)
    }
}

/// [`identity::survey`] over `document`, with the measured memory check after the pass.
///
/// # Errors
///
/// What the survey returns, and [`burrow_types::Error::LimitExceeded`] for `max_memory_bytes`,
/// detected after the pass.
pub(crate) fn survey(
    frames: &PageFrames,
    document: &PdfiumDocument,
    options: &OpenOptions<'_>,
    deadline: &Deadline,
) -> Result<Survey> {
    let before = crate::rss::resident_bytes();
    let survey = identity::survey(frames, document, deadline, options.clock.as_ref())?;
    crate::estimate::check_measured_memory(before, crate::rss::resident_bytes(), &options.limits)?;
    Ok(survey)
}

/// [`identity::check_pages`] over `document`, with the measured memory check after the pass.
///
/// # Errors
///
/// The page-identity refusal on any disagreement, and what [`survey`] returns.
pub(crate) fn check_pages(
    frames: &PageFrames,
    document: &PdfiumDocument,
    options: &OpenOptions<'_>,
    deadline: &Deadline,
) -> Result<Survey> {
    let before = crate::rss::resident_bytes();
    let survey = identity::check_pages(frames, document, deadline, options.clock.as_ref())?;
    crate::estimate::check_measured_memory(before, crate::rss::resident_bytes(), &options.limits)?;
    Ok(survey)
}

#[cfg(test)]
#[path = "flatten_identity_tests.rs"]
mod tests;

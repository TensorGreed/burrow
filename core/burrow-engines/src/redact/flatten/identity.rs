//! ADR 0031 §5, PDFium's half: the page qpdf replaces is the page PDFium rendered.
//!
//! # What is compared
//!
//! If the two engines disagree about which page is page *N*, qpdf replaces one page with a picture
//! of another. The page that was meant to be redacted is then written out intact, elsewhere in the
//! document, and V1 to V7 would pass. So three readings are held to qpdf's:
//!
//! 1. **PDFium's page count** against qpdf's ([`survey`]).
//! 2. **Every page's display size, as PDFium reads it without loading the page**
//!    (`FPDF_GetPageSizeByIndexF`), against qpdf's display frame for the same index
//!    ([`survey`]). qpdf's frame is the display frame already: width and height are swapped for
//!    `/Rotate` 90 and 270 by [`PageFrame::displayed_size`]. PDFium reports 32-bit floats and qpdf
//!    64-bit, so the rule is **qpdf's value rounded to 32 bits equals PDFium's**, exactly.
//! 3. **Page *N*'s rotation**, PDFium's `FPDFPage_GetRotation` on the page the render loads anyway,
//!    against qpdf's ([`check_rotation`]). Read inside the render's own engine job, before its
//!    bitmap exists (`pdfium::flatten_render`).
//!
//! # Measured on the pinned PDFium before it is relied on (2026-11-04, aarch64)
//!
//! - **The no-load size reflects `/Rotate`**, an inherited one included: a 200 x 100 page at
//!   `/Rotate 90` or `270` reads (100, 200), and at `180` it reads (200, 100). So PDFium's side is
//!   compared as it comes, and only qpdf's is the display frame by construction. Pinned by
//!   `pdfium::flatten_identity`'s tests. If a PDFium update changed this, those tests would go red
//!   before this comparison went wrong.
//! - **PDFium counts the root's `/Count`**, so a root that overstates or understates its leaves
//!   is a real count disagreement with no repair on qpdf's side. An intermediate node's `/Count`
//!   lie is not trusted by this build. Both engines enumerate the same leaves there, and this
//!   module sees nothing: `[page-tree-ambiguous]` (ADR §5, qpdf's half) owns that shape.
//!
//! # What it does not detect
//!
//! - **Two pages of one size and one rotation exchanged.** On a document whose pages all share a
//!   size, comparing sizes adds nothing beyond the count, and identity rests on the structural
//!   refusals of qpdf's half. ADR §5 says so, and so does this.
//! - **A rotation disagreement on a page other than *N*** that the size cannot show: a square
//!   page, or 180 degrees. Only page *N* is loaded, so only page *N*'s rotation is read (ADR §5,
//!   the route and its cost).
//! - **A size equal in 32 bits and different in 64.** That is the rounding the rule states.
//!
//! # What it refuses that both engines read alike
//!
//! PDFium computes a size in 32 bits from 32-bit coordinates. qpdf computes it in 64 bits and is
//! rounded once. For some boxes with fractional coordinates the two land one ulp apart, although
//! both engines read the same box: `[0.1 0.2 612.123456789 792.987654321]` gives 612.0235 and
//! 612.02344 (measured). The exact rule refuses that page. It fails closed, and the flatten census
//! counts it.
//!
//! # The refusal
//!
//! [`Error::Unsupported`] with [`PAGE_IDENTITY_MISMATCH`] as its message. The bracketed id,
//! `[page-identity-mismatch]`, its `RULE_IDS` row and its web message are the integration
//! slice's (owner decision 90, item 3), so this slice raises no new bracketed id.
//!
//! # Not reachable yet
//!
//! Nothing outside this crate's tests calls this module until the integration slice.
//!
//! [`PageFrame::displayed_size`]: crate::pdfsyntax::region::PageFrame::displayed_size

#![allow(
    dead_code,
    reason = "ADR 0031 S9b: no caller outside tests until the integration slice wires the check"
)]

use burrow_types::{Clock, Deadline, Error, Result};

use crate::redact::frames::{PageFrameReading, PageFrames};

/// The page-identity refusal's message, until the integration slice brackets its id.
pub const PAGE_IDENTITY_MISMATCH: &str = "pdf flatten: the two PDF engines disagree about which \
     page is which, so the page replaced might not be the page drawn";

/// The page-identity refusal.
fn mismatch() -> Error {
    Error::Unsupported(PAGE_IDENTITY_MISMATCH.to_owned())
}

/// Whether `error` is the page-identity refusal.
#[must_use]
pub fn is_page_identity_mismatch(error: &Error) -> bool {
    matches!(error, Error::Unsupported(message) if message == PAGE_IDENTITY_MISMATCH)
}

/// PDFium's side of the comparison: what it says without loading a page.
pub trait PdfiumPages {
    /// PDFium's page count for the document.
    ///
    /// # Errors
    ///
    /// Whatever the engine refuses with, including the deadline.
    fn page_count(&self) -> Result<u64>;

    /// Page `index`'s size in points, as PDFium reports it **without loading the page**.
    ///
    /// # Errors
    ///
    /// Whatever the engine refuses with, including the deadline.
    fn page_size(&self, index: u64) -> Result<(f32, f32)>;
}

/// One thing the two engines disagree about.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum Disagreement {
    /// The page counts differ.
    Count {
        /// qpdf's.
        qpdf: u64,
        /// PDFium's.
        pdfium: u64,
    },
    /// Page `page`'s display size differs once qpdf's is rounded to 32 bits.
    Size {
        /// Zero-based.
        page: u64,
        /// qpdf's display frame, before rounding.
        qpdf: (f64, f64),
        /// PDFium's no-load size.
        pdfium: (f32, f32),
    },
    /// qpdf has no display frame for page `page` (its reading was a refusal), so nothing vouches
    /// for which page it is. Fails closed.
    Unframed {
        /// Zero-based.
        page: u64,
    },
}

/// What [`survey`] compared, and every disagreement it found.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Survey {
    /// The pages whose sizes were compared: the smaller of the two counts.
    pub examined: u64,
    /// Every disagreement, the count first, then pages in order.
    pub disagreements: Vec<Disagreement>,
}

/// qpdf's display size rounded to the 32 bits PDFium reports in.
#[allow(
    clippy::cast_possible_truncation,
    reason = "rounding to 32 bits is the rule (ADR 0031 §5)"
)]
fn rounded(width: f64, height: f64) -> (f32, f32) {
    (width as f32, height as f32)
}

/// Compare PDFium's page count and every page's no-load size with qpdf's `frames`, and report
/// every disagreement. **Refuses nothing itself**: [`check_pages`] refuses, and the census reads
/// this.
///
/// The sizes are compared for every index below the smaller count, so a count disagreement
/// still reports what the pages it can pair say. The deadline is checked before every page.
///
/// # Errors
///
/// [`Error::LimitExceeded`] for `max_duration_ms`; whatever `pdfium` refuses with; and
/// [`Error::Internal`] for a reading list shorter than the count qpdf reported, which
/// `page_frames` never builds.
pub fn survey(
    frames: &PageFrames,
    pdfium: &dyn PdfiumPages,
    deadline: &Deadline,
    clock: &dyn Clock,
) -> Result<Survey> {
    deadline.checkpoint(clock)?;
    let qpdf_pages = frames.pages;
    let pdfium_pages = pdfium.page_count()?;
    let mut disagreements = Vec::new();
    if qpdf_pages != pdfium_pages {
        disagreements.push(Disagreement::Count {
            qpdf: qpdf_pages,
            pdfium: pdfium_pages,
        });
    }
    let examined = qpdf_pages.min(pdfium_pages);
    for page in 0..examined {
        deadline.checkpoint(clock)?;
        let at = usize::try_from(page)
            .map_err(|_| Error::Internal("a page index that does not fit in usize".to_owned()))?;
        let reading = frames.readings.get(at).ok_or_else(|| {
            Error::Internal("fewer frame readings than the page count".to_owned())
        })?;
        let (width, height) = match reading {
            PageFrameReading::Frame { width, height } => (*width, *height),
            PageFrameReading::Refused { .. } => {
                disagreements.push(Disagreement::Unframed { page });
                continue;
            }
        };
        let theirs = pdfium.page_size(page)?;
        if rounded(width, height) != theirs {
            disagreements.push(Disagreement::Size {
                page,
                qpdf: (width, height),
                pdfium: theirs,
            });
        }
    }
    deadline.checkpoint(clock)?;
    Ok(Survey {
        examined,
        disagreements,
    })
}

/// [`survey`], refused unless the two engines agree on the count and on every page's size.
///
/// # Errors
///
/// The page-identity refusal ([`is_page_identity_mismatch`]) on any disagreement; otherwise what
/// [`survey`] returns.
pub fn check_pages(
    frames: &PageFrames,
    pdfium: &dyn PdfiumPages,
    deadline: &Deadline,
    clock: &dyn Clock,
) -> Result<Survey> {
    let survey = survey(frames, pdfium, deadline, clock)?;
    if survey.disagreements.is_empty() {
        Ok(survey)
    } else {
        Err(mismatch())
    }
}

/// PDFium's `FPDFPage_GetRotation` value as degrees: 0 to 3 quarter turns clockwise.
///
/// # Errors
///
/// [`Error::Internal`] for anything else: the header's -1 is "error", which a loaded page does
/// not produce.
pub fn quarter_turns_to_degrees(turns: i32) -> Result<u16> {
    match turns {
        0 => Ok(0),
        1 => Ok(90),
        2 => Ok(180),
        3 => Ok(270),
        _ => Err(Error::Internal(
            "pdfium could not read the page's rotation".to_owned(),
        )),
    }
}

/// Page *N*'s rotation, qpdf's (`PageFrame::rotate`, normalised) against PDFium's, exactly.
///
/// # Errors
///
/// The page-identity refusal when they differ.
pub fn check_rotation(qpdf: u16, pdfium: u16) -> Result<()> {
    if qpdf == pdfium {
        Ok(())
    } else {
        Err(mismatch())
    }
}

#[cfg(test)]
#[path = "identity_tests.rs"]
mod tests;

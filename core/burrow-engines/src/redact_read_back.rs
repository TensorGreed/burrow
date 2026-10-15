//! A second, independent reading of a redacted page, from the renderer (#206, #111, ADR 0029 §6).
//!
//! # Why
//!
//! [`crate::redact_verify`] re-derives every glyph's position **with the same walk the operation
//! used to decide what to remove**. A defect in placing a glyph is made twice and agreed with
//! twice: the operation removes the wrong glyphs and verification agrees they are gone. The
//! emitted-bytes reparse catches every defect in writing, splicing and re-encoding, and none in
//! *placing*. The ordinary ways placement goes wrong are `Tz` and `Ts`; `Tc` and `Tw`; the CTM
//! through `q`/`Q` and a form's `/Matrix`; and a Type 3 `/FontMatrix`.
//!
//! The same shape holds for the page count (#111): the promise is qpdf's reading of the input
//! and the check is qpdf's reading of the output, so a page qpdf loses on the way in is lost on
//! both sides of the comparison.
//!
//! So this asks a second engine -- PDFium, natively -- **where** the characters are, and how many
//! pages each document has, and refuses on a disagreement. It shares no code with the walk.
//!
//! # The rules, and each one only ADDS a refusal
//!
//! Run after [`crate::redact_verify::region_is_cleared_reporting`] has accepted, so every refusal
//! it makes still stands. An error, or a reading with no answer, is a refusal (ADR 0029 §6): an
//! unanswered second opinion is not a clean one.
//!
//! - **R1, `[second-reading-glyph-in-region]`.** No character PDFium reports on the redacted page,
//!   other than one it flags as generated, has its **origin** (`FPDFText_GetCharOrigin`) inside the
//!   region. The region is converted into content space with the **output's** frame as burrow
//!   reads it -- the same rectangle `redact_verify` tested burrow's own glyphs against -- and
//!   shrunk by [`TOLERANCE_PT`](crate::redact_read_back::TOLERANCE_PT) on every side, so an
//!   origin that merely sits on the edge is not a disagreement.
//!
//!   **Origins only, and that is a decision, not an omission** (owner, 2026-10-15, #206 option A).
//!   PDFium's ink box (`FPDFText_GetCharBox`) is not read. Calibrated against main, an ink-box rule
//!   refused 152 of 3,488 tiles that burrow redacts today, every one of them traced to burrow's
//!   conservative glyph box ignoring `/FontDescriptor`'s `/FontBBox` (#344) rather than to a glyph
//!   PDFium places in the region. The origin rule refused none. The ink half lands after #344.
//!
//!   A region no more than twice [`TOLERANCE_PT`](crate::redact_read_back::TOLERANCE_PT) wide or
//!   tall has nothing left to compare once the tolerance is taken off, and refuses as
//!   `[second-reading-region-too-small]`.
//! - **R2, `[second-reading-no-text]`.** burrow's walk places glyphs on the page and PDFium reports
//!   no character it did not generate. A reading that sees nothing cannot vouch for anything.
//! - **R3, `[second-reading-page-size]`.** PDFium's rotated size for the page equals the output
//!   frame's, within the size tolerance `redact::frame` uses (0.01 pt) -- so R1's region, converted
//!   with burrow's frame, is the region on the page PDFium shows.
//! - **R4, `[second-reading-page-count]`.** PDFium's page count of the output equals the count the
//!   operation promised, which is qpdf's count of the input. PDFium's count
//!   (`FPDF_GetPageCount`) is the page tree's DECLARED root `/Count`, not a walk of the tree: R4
//!   and R5 compare qpdf's reading against that declaration, so a tree whose declared `/Count`
//!   both engines accept, and whose kids say otherwise, is not caught here.
//! - **`[second-reading-nesting]`, before any page is loaded.** The output page's marked-content
//!   work is past [`MAX_MARKED_WORK`](crate::redact_read_back::MAX_MARKED_WORK): PDFium's page
//!   load grows with nesting, so it is capped before it rather than measured after.
//! - **R5, `[second-reading-input-page-count]`.** PDFium's page count of the input, its declared
//!   root `/Count` as above, equals qpdf's -- #111's half, here for redaction only.
//!
//! # Positions, not text
//!
//! No Unicode is read: no `FPDFText_GetUnicode`, no `FPDFText_GetText`, no `/ToUnicode`, no
//! `/ActualText`. What a character says is the document's own claim about itself (spike 0006,
//! channel 21), so it is not taken as a second opinion. Where a character is placed is read
//! instead -- but mapping is not wholly out of it: PDFium decides WHETHER to report a character
//! partly from its mapping, so a mapping can remove a character from this reading (see
//! "Characters PDFium does not report" below). The PDFium text functions this crate's library
//! source (`src/`) may name are exactly five -- the test-only oracle under `tests/support/`, which
//! never ships, is outside it -- and
//! `tests::the_pdfium_text_functions_named_are_the_five_permitted` refuses any other, with a probe
//! per rejected name.
//!
//! # What this leaves undetectable, stated because the rules could be read as covering it
//!
//! - **Ink that reaches the region from an origin outside it.** R1 reads where each character's
//!   pen starts, not how far its ink extends, so a glyph whose origin is outside the region and
//!   whose outline reaches into it is not a disagreement here. burrow's own check still tests its
//!   conservative box against the region, so such a glyph is held by burrow's reading alone --
//!   the same-source limit this module exists to narrow, narrowed for origins and not yet for ink.
//! - **A misplacement PDFium shares.** Two engines agreeing is not either of them being right. A
//!   term both get wrong the same way is invisible here, as it is to every viewer that reads it
//!   the same way.
//! - **Other viewers' readings.** PDFium is one renderer. A viewer that lays text out differently
//!   -- a substituted system font (DECISIONS rule 14), a different reading of a malformed operand
//!   -- may show a glyph in the region that neither burrow nor PDFium places there.
//! - **Mapping.** What a character *says* is not read, so nothing here speaks to `/ToUnicode`,
//!   `/Differences` or the embedded font's own `cmap`; that half is `redact_verify`'s check 2 and
//!   ADR 0029 §7's disclosure.
//! - **Generated characters, and retained `/ActualText`.** A character PDFium flags as generated
//!   is excluded, because PDFium invents a space in exactly the gap a redaction cuts (ADR 0029 §6);
//!   so a generated character inside the region is not a refusal. And inside a retained
//!   `/ActualText` span PDFium reports the span's replacement text, placed at the span's first
//!   text object with that object's box, and reports nothing for the span's later text objects
//!   -- so the placement of glyphs in those later objects is not read.
//! - **Characters PDFium does not report.** R1 can only refuse a character PDFium reports, and
//!   PDFium reports no character at all -- not one outside the region, none -- for three causes
//!   measured on #206's fixtures: a `/ToUnicode` that maps the code to a space or to nothing; a
//!   `/Differences` name the substituted font does not carry; and a Type 3 glyph whose procedure
//!   draws nothing. Such a glyph in the region is held by burrow's own check alone. R2 catches the
//!   case where every glyph on the page is one of these, and no other: a page with one reported
//!   character and many unreported ones passes it. A count-based R2 -- burrow's glyph count
//!   against PDFium's, page by page -- is a follow-up, to be built only once a census shows what
//!   it would cost.
//! - **Pages other than the redacted page.** Only the page that was redacted is read; text on
//!   every other page is held only by the page count and burrow's own checks.

use burrow_types::{Error, Result};

use crate::pdfsyntax::geometry::Rect;
use crate::pdfsyntax::region::Region;
use crate::redact_verify::{Cleared, ClearedWitness, region_is_cleared_reporting};

/// How far PDFium's edge may sit from burrow's and still count as agreement, in PDF points.
///
/// # Decided before the first measurement, and this is the record of that
///
/// Registered for the test-time oracle (`tests/support/char_box_oracle.rs`, #129) and moved here
/// unchanged when the oracle's reading became a runtime one (#206). A tolerance chosen after
/// seeing a result is fitted to the result: it will always pass, and it will pass a defect just
/// as readily. This number is derived from two bounds, neither of them a measurement of burrow's
/// output.
///
/// **From below — it must exceed float noise.** PDFium fills `FS_RECTF` with `float`. PDF's
/// maximum user-space coordinate is 14,400; at that magnitude an `f32` ulp is about 0.001 pt, and
/// a box composed through a CTM, a text matrix, a Form `/Matrix` and a `/FontMatrix` accumulates
/// a handful of those. The worst plausible disagreement from arithmetic alone is under 0.01 pt.
///
/// **From above — it must not be able to hide any term.** Every oracle fixture is built so that
/// the term it exercises displaces its glyph by **at least 1 pt**. This is a twentieth of that. A
/// term that is ignored entirely cannot land within the tolerance of one that is applied.
///
/// **This is not a knob.** Raising it to make a run pass is the one edit this constant exists to
/// forbid.
pub const TOLERANCE_PT: f64 = 0.05;

/// How many characters a reading may report on one page before it is refused.
///
/// The walk refuses a page with more than [`crate::pdfsyntax::geometry::MAX_GLYPHS`] glyphs, and
/// PDFium adds generated characters -- a space per gap, a line break per line -- to those it read.
/// Twice the walk's ceiling bounds what is held per page; a page past it has no answer here, which
/// is a refusal.
pub const MAX_CHARS: usize = 2 * crate::pdfsyntax::geometry::MAX_GLYPHS;

/// The most marked-content work a page the second reading loads may carry: the sum, over every
/// operation PDFium parses at the load, of the `BDC`/`BMC` depth in force when it is read
/// ([`crate::redact_verify::ClearedWitness::glyphs_and_marked_work_on`]).
///
/// PDFium's page load grows with marked-content nesting, and nothing interrupts it once it
/// starts, so the page is refused before the load rather than measured after it
/// (`[second-reading-nesting]`).
///
/// # A sound over-estimate, not a model
///
/// The count is an over-estimate of the marked-content work PDFium does at the load, built so that
/// it is never less; it is not a model of PDFium's parser, and does not try to be exact. **Any
/// undercount found later is a bug in this bound**, to be fixed here, not a property of the input.
///
/// The first version capped the deepest nesting, and review found three ways past it: a form's
/// unmatched `EMC`s lowered the page's count, a Type 3 glyph procedure was not counted at all,
/// and a cap on depth says nothing about a form drawn thousands of times, each draw under it. The
/// second review found a fourth: Type 3 procedures counted once per stream, where PDFium parses
/// one per font object and code. So it counts work, in three parts, each held by its own test:
///
/// - **a mark stack per stream**: a form starts at the depth open where it is drawn, its `EMC`s
///   cannot close the caller's marks, and the caller's depth is restored after it;
/// - **every form once per `Do`**, since PDFium parses a form again at each draw;
/// - **Type 3 procedures, per font object and per distinct code drawn in it**, each charged at the
///   font's heaviest procedure: PDFium parses a procedure for every font object and code that
///   draws it, and the walk does not enter one. A procedure no drawn code names is not charged,
///   because PDFium does not parse it at the load. That was measured, not assumed (2026-10-15,
///   release build, PDFium's share of the time alone): with one code drawn, a font of 1, 51 and
///   201 procedures nested 900 deep loaded in about 6, 10 and 8 ms; with every code drawn, the
///   same fonts loaded in about 7, 99 and 226 ms. `tests/pdfium_type_three_parse.rs` pins it, in
///   a binary of its own so its timing is PDFium's alone: a PDFium whose load does parse them
///   fails that test, and this charge has to change.
///
/// What it does not count: content PDFium does not parse at the load (annotation appearances,
/// pattern cells and soft-mask groups, which it parses when it renders). Render and previews
/// load pages too and are not capped by this.
///
/// # The number, and what it was chosen against
///
/// Measured before it was set (2026-10-15), as the witness counts it on every OUTPUT page that
/// reached this check: **at most 18,962** over the census (282 inputs at open and 4,824 tiles;
/// 3,655 readings), and **at most 32** over every committed fixture the workspace suite redacts
/// (1,191 readings, the tests built to cross this ceiling excluded). 1,000,000 is over 50 times
/// the heaviest real page, so the ceiling refuses no document burrow has seen; a page past it
/// refuses with its own rule, and the cost of a wrong guess is a refusal, never a leak.
pub const MAX_MARKED_WORK: u64 = 1_000_000;

/// One character as the second reading places it. Page space, as the content stream draws it.
///
/// No ink box: R1 reads origins only until #344 (see the module's R1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedChar {
    /// The pen position: `FPDFText_GetCharOrigin`.
    pub origin: (f64, f64),
    /// The reading invented the character rather than reading it from the file
    /// (`FPDFText_IsGenerated`).
    pub generated: bool,
}

/// What the second reading says about one page of a document.
#[derive(Debug, Clone, PartialEq)]
pub struct PageReading {
    /// How many pages the document has.
    pub pages: u64,
    /// Which page this is. Checked against the page asked for: a reading of another page is not
    /// a reading of this one.
    pub page: usize,
    /// The page's size as displayed -- its box, rotated -- in points.
    pub size: (f64, f64),
    /// Every character on the page, generated ones included.
    pub chars: Vec<PlacedChar>,
}

/// A second engine's reading of a document: its page count, and one page's characters.
///
/// `Ok(None)` is an engine with no answer, and is refused like an error.
pub trait SecondReading {
    /// How many pages `bytes` has.
    ///
    /// # Errors
    ///
    /// Whatever the engine reports; every one of them is a refusal.
    fn page_count(&self, bytes: &[u8]) -> Result<Option<u64>>;

    /// Page `page` of `bytes`, with the document's page count.
    ///
    /// # Errors
    ///
    /// Whatever the engine reports; every one of them is a refusal.
    fn read_page(&self, bytes: &[u8], page: usize) -> Result<Option<PageReading>>;
}

/// What the second reading must agree with: burrow's own reading of the output, and its promise.
#[derive(Debug, Clone, Copy)]
pub struct Agreement {
    /// The page that was redacted.
    pub page: usize,
    /// The region, as the caller gave it.
    pub region: Region,
    /// What `redact_verify` walked on the output page.
    pub walked: crate::redact_verify::Walked,
    /// The page count the operation promised: qpdf's count of the input, read at the open.
    pub promised_pages: u64,
}

/// Refuse the output unless the second reading agrees with burrow's (R1 to R5).
///
/// # Errors
///
/// [`Error::OutputRejected`] naming the rule, the page and counts -- never the file's content and
/// never a coordinate. A [`Error::LimitExceeded`] or [`Error::Internal`] from the reading passes
/// through as itself: running out of time is the operation's outcome, not a verdict on the page.
/// Any other error, and any `None`, is a rejection.
pub fn agrees<R: SecondReading>(
    reading: &R,
    input: &[u8],
    output: &[u8],
    expected: &Agreement,
) -> Result<()> {
    // R5: THE INPUT HAS THE PAGES QPDF SAYS IT HAS (#111). The promise R4 holds the output to is
    // qpdf's count, so it is checked first against a reading that is not qpdf's.
    let input_pages = reading
        .page_count(input)
        .map_err(|error| {
            wrapped(
                "[second-reading-unavailable] the input cannot be read",
                error,
            )
        })?
        .ok_or_else(|| unanswered("the input's page count"))?;
    if input_pages != expected.promised_pages {
        return Err(rejected(format!(
            "[second-reading-input-page-count] the input has {} page(s) to qpdf and \
             {input_pages} to the renderer",
            expected.promised_pages
        )));
    }

    // THE NESTING CEILING, BEFORE PDFIUM LOADS THE PAGE. PDFium's page load grows with
    // marked-content nesting, and nothing interrupts it once it starts; so a page whose work is
    // past [`MAX_MARKED_WORK`] is refused here, before `read_page` asks for the load.
    if expected.walked.marked_work > MAX_MARKED_WORK {
        return Err(rejected(format!(
            "[second-reading-nesting] page {} carries more marked-content nesting than the second \
             reading loads",
            expected.page
        )));
    }

    let page = reading
        .read_page(output, expected.page)
        .map_err(|error| {
            wrapped(
                "[second-reading-unavailable] the output cannot be read",
                error,
            )
        })?
        .ok_or_else(|| unanswered("the redacted page"))?;

    // R4: THE OUTPUT HAS THE PAGES THAT WERE PROMISED.
    if page.pages != expected.promised_pages {
        return Err(rejected(format!(
            "[second-reading-page-count] {} page(s) were promised and the renderer reads {} in \
             the output",
            expected.promised_pages, page.pages
        )));
    }
    // THE READING IS OF THE PAGE ASKED ABOUT. Every rule below is about one page.
    if page.page != expected.page {
        return Err(rejected(format!(
            "[second-reading-wrong-page] page {} was asked about and page {} was read",
            expected.page, page.page
        )));
    }

    // R3: THE PAGE PDFIUM SHOWS IS THE FRAME R1'S REGION IS CONVERTED WITH. Compared as
    // `redact::frame` compares a renderer's size: the display box rotated, in points, without
    // `/UserUnit`, which the size by index does not apply.
    let frame = &expected.walked.frame;
    let shown = &frame.display_box;
    let (ours_w, ours_h) = if frame.rotate == 90 || frame.rotate == 270 {
        (shown.top - shown.bottom, shown.right - shown.left)
    } else {
        (shown.right - shown.left, shown.top - shown.bottom)
    };
    let (width, height) = page.size;
    if !((ours_w - width).abs() < SIZE_TOLERANCE && (ours_h - height).abs() < SIZE_TOLERANCE) {
        return Err(rejected(format!(
            "[second-reading-page-size] page {} is shown at another size by the renderer than \
             burrow reads",
            expected.page
        )));
    }

    if page.chars.len() > MAX_CHARS {
        return Err(unanswered("a page with more characters than are read"));
    }
    let read: Vec<&PlacedChar> = page.chars.iter().filter(|c| !c.generated).collect();
    // A NUMBER THAT IS NOT ONE places a character nowhere, which is no answer, and every
    // comparison with a NaN is false -- so R1 would read it as outside the region.
    if read.iter().any(|c| !placed_finitely(c)) {
        return Err(unanswered(
            "a character placed at a coordinate that is not a number",
        ));
    }

    // R2: A READING THAT SEES NOTHING CANNOT VOUCH FOR ANYTHING.
    if expected.walked.glyphs > 0 && read.is_empty() {
        return Err(rejected(format!(
            "[second-reading-no-text] burrow places {} glyph(s) on page {} and the renderer \
             reads none",
            expected.walked.glyphs, expected.page
        )));
    }

    // R1: NOTHING PDFIUM PLACES IS INSIDE THE REGION.
    let region = expected.region.to_content_space(frame).map_err(|error| {
        rejected(format!(
            "[second-reading-unavailable] the region cannot be converted: {error}"
        ))
    })?;
    // A REGION NO WIDER OR TALLER THAN TWICE THE TOLERANCE has nothing left once the tolerance
    // is taken off each side, so R1 would test nothing and pass. Refused rather than read as
    // agreement: a check with nothing to compare is not a check.
    // Decided on the extent itself, with a relative margin, because `shrunk` adds and subtracts
    // and a region of exactly twice the tolerance can come out a float's width wide.
    let floor = 2.0 * TOLERANCE_PT * (1.0 + 1e-9);
    let too_small = region.right - region.left <= floor || region.top - region.bottom <= floor;
    let inner = shrunk(&region, TOLERANCE_PT).filter(|_| !too_small);
    let Some(inner) = inner else {
        return Err(rejected(format!(
            "[second-reading-region-too-small] the region on page {} is too small to compare \
             positions in",
            expected.page
        )));
    };
    let inside = read.iter().filter(|c| reaches(&inner, c)).count();
    if inside > 0 {
        return Err(rejected(format!(
            "[second-reading-glyph-in-region] the renderer places {inside} character(s) inside \
             the region on page {}",
            expected.page
        )));
    }
    Ok(())
}

/// [`crate::redact_verify::region_is_cleared`], then [`agrees`]: burrow's verdict first, so its
/// refusals stand whatever the second reading says, and the second reading's added after.
///
/// # Errors
///
/// The first refusal of either.
pub fn region_is_cleared_twice<W: ClearedWitness, R: SecondReading>(
    witness: &W,
    reading: &R,
    input: &[u8],
    output: &[u8],
    cleared: &Cleared,
    promised_pages: u64,
) -> Result<()> {
    let walked = region_is_cleared_reporting(witness, output, cleared)?;
    agrees(
        reading,
        input,
        output,
        &Agreement {
            page: cleared.page,
            region: cleared.region,
            walked,
            promised_pages,
        },
    )
}

/// The size tolerance `redact::frame` holds a renderer's page size to: it reports `f32`.
const SIZE_TOLERANCE: f64 = 0.01;

/// `rect` with `by` taken off every side, or `None` when nothing is left.
fn shrunk(rect: &Rect, by: f64) -> Option<Rect> {
    let inner = Rect {
        left: rect.left + by,
        bottom: rect.bottom + by,
        right: rect.right - by,
        top: rect.top - by,
    };
    (inner.left < inner.right && inner.bottom < inner.top).then_some(inner)
}

/// Whether a character's origin is inside `region`, closed on every edge so an origin exactly on
/// the shrunk edge counts.
fn reaches(region: &Rect, c: &PlacedChar) -> bool {
    let (x, y) = c.origin;
    x >= region.left && x <= region.right && y >= region.bottom && y <= region.top
}

/// Both coordinates of the character's origin are finite numbers.
fn placed_finitely(c: &PlacedChar) -> bool {
    c.origin.0.is_finite() && c.origin.1.is_finite()
}

/// The one shape of rejection this module produces.
fn rejected(what: String) -> Error {
    Error::OutputRejected(format!(
        "redact: the second reading refuses the output -- {what}"
    ))
}

/// A reading with no answer: refused, never read as "nothing there".
fn unanswered(what: &str) -> Error {
    rejected(format!(
        "[second-reading-unavailable] the renderer gave no answer for {what}"
    ))
}

/// Wrap an engine error as a rejection, unless it is a limit or an internal fault -- the guard
/// `redact_verify::wrapped` has, for the reason it gives.
fn wrapped(what: &str, error: Error) -> Error {
    if matches!(error, Error::LimitExceeded { .. } | Error::Internal(_)) {
        return error;
    }
    rejected(format!("{what}: {error}"))
}

#[cfg(test)]
mod tests;

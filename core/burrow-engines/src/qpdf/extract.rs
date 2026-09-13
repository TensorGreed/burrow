//! The qpdf implementation of [`PageExtractor`].
//!
//! # It builds an output; it does not carve one
//!
//! [ADR 0019](../../../../docs/adr/0019-how-split-builds-its-outputs.md) measured the
//! alternative. Reading the source and removing the unwanted pages keeps the source's catalog
//! whole, so a two-page output carried the outline titles of **all five** source pages — three
//! of them pointing at pages that are not in the document. Somebody extracting pages 2-3 to
//! send to another person would have been sending the titles of pages 4 and 5 with them.
//!
//! So the destination starts empty and pages are copied **in**. That is an allowlist by
//! construction: nothing is in the output unless this file put it there. Carving is a denylist
//! over a structure whose design permits keys nobody enumerated, and it closes only the leaks
//! somebody thought of.
//!
//! # The empty destination is our bytes, and that is not incidental
//!
//! qpdf offers `qpdf_empty_pdf` and **burrow may not call it**. It is not on
//! `engines/qpdf-trapped-functions.txt`, and it cannot earn an entry in
//! `engines/qpdf-untrapped-accepted.toml`, whose bar is "non-parsing … never touches the PDF's
//! bytes": its body is `QPDF::emptyPDF()`, which is `processMemoryFile("empty PDF", EMPTY_PDF,
//! …)` — the full parser, with no `trap_errors` wrapper on the C function. A `QPDFExc` out of
//! that crosses an `extern "C"` frame and aborts the process (ADR 0013 §1).
//!
//! [`BLANK_DOCUMENT`] goes in through the trapped `qpdf_read_memory` instead. It is a
//! compile-time constant in this crate, so it is not attacker-controlled and it is not
//! something an engine handed us.
//!
//! # One new FFI declaration, already trapped
//!
//! `open`, `get_page_n`, `add_page` and the write path were all declared for `merge`; this
//! route adds `qpdf_remove_page`, which is on `engines/qpdf-trapped-functions.txt` and so
//! needs no argued exception. The checklist's six-artifact table for a new engine capability
//! (`add-operation` §2a) therefore applies to one row rather than six: no qpdf rebuild, no
//! export-list change, no new content hashes, no size-budget re-measure. (An earlier version
//! of this comment said "no new FFI", which was false the moment the blank page needed
//! removing.)

use std::sync::Arc;

use burrow_types::{Deadline, Error, Limits, Result, Stage};

use super::{Document, Qpdf, ffi};
use crate::{OpenOptions, PageExtractor};

/// A valid PDF with a catalog and **one blank page**, used as the destination to build into.
///
/// # Why it is not empty, which is what it wants to be
///
/// **qpdf refuses to open a document with no pages.** The corpus already records this — the
/// conformance case `no-pages` expects `Malformed` — and the first version of this constant
/// was a zero-page document that `Document::open` rejected before any page could be copied
/// into it. So the destination starts with one blank page, and `extract` takes that page back
/// out once the wanted ones are in.
///
/// # Why it is not `qpdf_empty_pdf`
///
/// burrow may not call that: it is not on `engines/qpdf-trapped-functions.txt` and cannot earn
/// an entry in `engines/qpdf-untrapped-accepted.toml`, whose bar is "non-parsing … never
/// touches the PDF's bytes". Its body is `QPDF::emptyPDF()`, which is
/// `processMemoryFile("empty PDF", EMPTY_PDF, …)` — the full parser, with no `trap_errors`
/// wrapper. This constant goes in through the trapped `qpdf_read_memory` instead.
///
/// # What is in it
///
/// A catalog, a page tree, and a 1×1 page with no content stream and no resources. Nothing
/// that could reach an output: no metadata, no producer string, no dates. It is removed before
/// the write in any case, but "it gets removed" is a weaker guarantee than "there was nothing
/// in it", and ADR 0019 §2 is about what the emitted bytes contain.
///
/// The xref offsets are load-bearing and were **wrong when written by hand** — a unit test
/// computes them from the bytes rather than trusting the literal. The failure they prevent is
/// loud rather than silent: recovery is off, so a wrong offset makes `Document::open` return
/// `Malformed` and *every* extraction fail. (An earlier version of this comment claimed qpdf
/// would reconstruct the table and carry on. It does not, with recovery off, and the test is
/// worth having anyway — the constant is unreadable by eye.)
pub(super) const BLANK_DOCUMENT: &[u8] = b"%PDF-1.7\n\
1 0 obj\n\
<< /Type /Catalog /Pages 2 0 R >>\n\
endobj\n\
2 0 obj\n\
<< /Type /Pages /Kids [3 0 R] /Count 1 >>\n\
endobj\n\
3 0 obj\n\
<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] /Resources << >> >>\n\
endobj\n\
xref\n\
0 4\n\
0000000000 65535 f \n\
0000000009 00000 n \n\
0000000058 00000 n \n\
0000000115 00000 n \n\
trailer\n\
<< /Size 4 /Root 1 0 R >>\n\
startxref\n\
199\n\
%%EOF\n";

/// A source document held open for the life of a split.
pub struct QpdfSource {
    document: Document,
    pages: u64,
    /// The resident set before the source was opened.
    ///
    /// `max_memory_bytes` **detects rather than bounds** (ADR 0007), and detecting it needs a
    /// before. Split is the operation with the worst amplification for it: every output is
    /// held in full while the source stays in the engine heap, so a 500-page document cut 499
    /// ways holds 500 written documents at once.
    rss_before: Option<u64>,
    /// The ceilings the source was opened under, so `extract` can apply the measured check
    /// without the caller having to pass them twice.
    limits: Limits,
}

impl PageExtractor for Qpdf {
    type Source = QpdfSource;

    fn name(&self) -> &'static str {
        "qpdf"
    }

    fn open(&self, bytes: Box<[u8]>, options: &OpenOptions<'_>) -> Result<Self::Source> {
        let limits = options.limits;

        let input_len = u64::try_from(bytes.len())
            .map_err(|_| Error::Internal("input length does not fit in u64".to_owned()))?;
        Limits::check(
            Stage::InputSize,
            "max_input_bytes",
            input_len,
            limits.max_input_bytes,
        )?;

        // The structural pre-scan, before the engine sees the bytes. Pure Rust,
        // `forbid(unsafe_code)`, and the only pre-emptive defence there is (ADR 0013).
        crate::prescan::check(&bytes, &limits)?;

        let clock = Arc::clone(&options.clock);
        let deadline = Deadline::start(clock.as_ref(), &limits);
        deadline.checkpoint(clock.as_ref())?;

        // Recovery OFF, as `merge` has it and for the same reason: a split that silently
        // reconstructs a damaged input produces outputs whose relationship to what the person
        // handed us is unclear, and they will never know.
        let rss_before = crate::rss::resident_bytes();

        let document = Document::open(bytes, options.password, false)?;
        let pages = document.page_count()?;
        Limits::check(Stage::PageCount, "max_pages", pages, limits.max_pages)?;

        if let Some(error) = document.take_error() {
            return Err(error);
        }

        // CHECKED HERE TOO, not only in `extract`. `Qpdf::check` does this right after its
        // open and this did not, so a document that blew past the ceiling and was then given
        // an unusable cut list returned `InvalidArgument` from `runs_from` with no
        // `LimitExceeded` ever reported -- the memory was spent and no ceiling said so,
        // because the only check lived in a function that was never reached. Found by code
        // review. `core/CLAUDE.md`: every operation applies every ceiling in `Limits`.
        crate::estimate::check_measured_memory(rss_before, crate::rss::resident_bytes(), &limits)?;

        Ok(QpdfSource {
            document,
            pages,
            rss_before,
            limits,
        })
    }

    fn pages(&self, source: &Self::Source) -> Result<u64> {
        Ok(source.pages)
    }

    fn extract(
        &self,
        source: &Self::Source,
        first: u64,
        count: u64,
        options: &OpenOptions<'_>,
    ) -> Result<Vec<u8>> {
        // `options` carries the clock and the password; the CEILINGS come from `source.limits`
        // -- see the checks below for why. Binding `options.limits` here would put the laxer
        // set one line from the stricter one, which is how the two checks came to disagree.
        let _ = options;

        let end = first
            .checked_add(count)
            .ok_or_else(|| Error::InvalidArgument("page run overflows".to_owned()))?;
        if count == 0 || end > source.pages {
            return Err(Error::InvalidArgument(
                "page run is not inside the document".to_owned(),
            ));
        }
        // `source.limits`, for the reason the memory check below gives: the ceiling that
        // counts is the one the document was opened under, and a caller passing laxer options
        // to a later `extract` must not be able to raise it after the fact. `split` passes the
        // same options throughout, so this is unreachable from there -- but `PageExtractor` is
        // a public trait and the two checks disagreeing was an inconsistency, not a design.
        Limits::check(
            Stage::PageCount,
            "max_pages",
            count,
            source.limits.max_pages,
        )?;

        // The destination: our own blank document, through the trapped read.
        let dest = Document::open(BLANK_DOCUMENT.to_vec().into_boxed_slice(), None, false)?;
        if let Some(error) = dest.take_error() {
            return Err(error);
        }

        for n in first..end {
            let index = usize::try_from(n)
                .map_err(|_| Error::Internal("page index does not fit in usize".to_owned()))?;

            // SAFETY: `source.document.data` is a live handle whose document read
            // successfully, and `index` is below its page count, checked above. Routes
            // through `trap_errors`.
            let page = unsafe { ffi::qpdf_get_page_n(source.document.data, index) };
            if let Some(error) = source.document.take_error() {
                return Err(error);
            }

            // `QPDF_FALSE` is `first`: append rather than prepend, so pages arrive in source
            // order. The handle belongs to the source, which is why the source is passed
            // alongside it — handles are per-document.
            //
            // SAFETY: both handles are live, and `page` was just obtained from the source.
            // Routes through `trap_errors`.
            let added = unsafe {
                ffi::qpdf_add_page(dest.data, source.document.data, page, ffi::QPDF_FALSE)
            };
            // The ERROR bit, never `!= 0`: a warning here is an ordinary outcome.
            if ffi::has_errors(added) {
                return Err(dest
                    .take_error()
                    .or_else(|| source.document.take_error())
                    .unwrap_or_else(|| {
                        Error::Malformed("qpdf: a page could not be copied".to_owned())
                    }));
            }
            if let Some(error) = dest.take_error() {
                return Err(error);
            }
        }

        // THE BLANK PAGE COMES BACK OUT, now that the document has real pages in it. It has
        // to be removed last rather than first, because a document with no pages is not one
        // qpdf will hold — which is the same fact that made this constant one page instead of
        // none.
        //
        // SAFETY: `dest.data` is a live handle, and index 0 is in range: the blank page is
        // still there and at least one more page was just added. Routes through `trap_errors`.
        let blank = unsafe { ffi::qpdf_get_page_n(dest.data, 0) };
        if let Some(error) = dest.take_error() {
            return Err(error);
        }
        // SAFETY: `blank` was just obtained from `dest` and handles are per-document, so it
        // belongs to the document it is being removed from. Routes through `trap_errors`.
        let removed = unsafe { ffi::qpdf_remove_page(dest.data, blank) };
        if ffi::has_errors(removed) {
            return Err(dest.take_error().unwrap_or_else(|| {
                Error::Internal("qpdf: the blank destination page could not be removed".to_owned())
            }));
        }
        if let Some(error) = dest.take_error() {
            return Err(error);
        }

        let output = write_out(&dest, &source.document)?;

        // WHAT THE SPLIT HAS COST SO FAR. `max_memory_bytes` DETECTS rather than bounds
        // (ADR 0007's 2026-09-12 amendment), and this is the only place on the split path
        // that can detect anything: the caller accumulates every output, the source stays
        // open in the engine heap throughout, and neither is visible from inside a single
        // extraction. Checked per output rather than once at the end, because "once at the
        // end" is after the memory has already been spent.
        //
        // `source.limits` rather than `options.limits`: the ceiling that matters is the one
        // the document was opened under, and a caller passing a laxer one to a later
        // `extract` must not be able to raise it after the fact.
        crate::estimate::check_measured_memory(
            source.rss_before,
            crate::rss::resident_bytes(),
            &source.limits,
        )?;

        Ok(output)
    }
}

/// Serialise the destination, with the source still alive.
///
/// **The source must outlive this call.** qpdf resolves a foreign page's indirect objects
/// lazily, when the destination is written, so releasing it first yields a *truncated document
/// rather than an error* — the same rule `assemble.rs` records, arriving from the other
/// direction. It is a parameter here so a caller cannot forget.
fn write_out(dest: &Document, _source_must_outlive_this: &Document) -> Result<Vec<u8>> {
    // SAFETY: `dest.data` is a live handle. Routes through `trap_errors`.
    let init = unsafe { ffi::qpdf_init_write_memory(dest.data) };
    if ffi::has_errors(init) {
        return Err(dest
            .take_error()
            .unwrap_or_else(|| Error::Io("qpdf could not prepare an in-memory write".to_owned())));
    }

    // AFTER `qpdf_init_write_memory`, never before: the writer does not exist until that call
    // succeeds, and this dereferences it. Found by core dump during ADR 0017's comparison.
    //
    // SAFETY: the writer was just created successfully. The call assigns a bool and parses
    // nothing; argued in `engines/qpdf-untrapped-accepted.toml`.
    unsafe { ffi::qpdf_set_deterministic_ID(dest.data, ffi::QPDF_TRUE) };

    // SAFETY: `dest.data` is live with a prepared writer, and the source is alive for the
    // whole of this call because it is borrowed by it. Routes through `trap_errors`.
    let wrote = unsafe { ffi::qpdf_write(dest.data) };
    if ffi::has_errors(wrote) {
        return Err(dest.take_error().unwrap_or_else(|| {
            Error::Malformed("qpdf: the output could not be written".to_owned())
        }));
    }
    if let Some(error) = dest.take_error() {
        return Err(error);
    }

    // SAFETY: the write succeeded, so the output buffer exists. Both accessors are untrapped
    // and argued in `engines/qpdf-untrapped-accepted.toml` under exactly this precondition.
    let len = unsafe { ffi::qpdf_get_buffer_length(dest.data) };
    // SAFETY: as above. The pointer is owned by `dest.data` and dies on the next
    // `qpdf_init_write*` or `qpdf_cleanup`, so it is copied out immediately and never stored.
    let ptr = unsafe { ffi::qpdf_get_buffer(dest.data) };

    // BOTH are checked, not one. qpdf returns a null buffer when the writer has none and
    // reports the length from a separate accessor — so a caller trusting only the length
    // would read from null, and one trusting only the pointer would copy zero bytes and call
    // it a document.
    if ptr.is_null() || len == 0 {
        return Err(Error::Io("qpdf produced no output".to_owned()));
    }

    // SAFETY: `ptr` is non-null and qpdf reports `len` readable bytes at it, valid until the
    // next write call or cleanup on this handle. Copied out immediately.
    Ok(unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec())
}

#[cfg(test)]
mod tests {
    use super::BLANK_DOCUMENT;

    /// The constant's xref offsets are right.
    ///
    /// **Written by hand and wrong the first time** -- object 2 was recorded at 60 when it is
    /// at 58, and `startxref` at 117 when it is at 110. qpdf would have reconstructed the
    /// table and carried on, so every split output would silently have come from the repair
    /// path and nothing would have said so. That is why this is a test and not a comment
    /// saying the offsets are load-bearing.
    #[test]
    fn the_empty_documents_xref_points_where_it_says() {
        let doc = BLANK_DOCUMENT;
        let find = |needle: &[u8]| {
            doc.windows(needle.len())
                .position(|w| w == needle)
                .unwrap_or_else(|| panic!("{} not found", String::from_utf8_lossy(needle)))
        };

        let recorded = |line: usize| -> usize {
            // The xref entries are fixed-width: 10 digits, a space, 5 digits, a space, a
            // type letter, a space. Line 0 is the free head, so object N is line N.
            let xref = find(b"xref\n0 4\n") + b"xref\n0 4\n".len();
            let at = xref + line * 20;
            std::str::from_utf8(&doc[at..at + 10])
                .expect("ascii")
                .parse()
                .expect("a number")
        };

        assert_eq!(recorded(1), find(b"1 0 obj"), "object 1's offset");
        assert_eq!(recorded(2), find(b"2 0 obj"), "object 2's offset");
        assert_eq!(recorded(3), find(b"3 0 obj"), "object 3's offset");

        let startxref = find(b"startxref\n") + b"startxref\n".len();
        let stated: usize = std::str::from_utf8(&doc[startxref..startxref + 3])
            .expect("ascii")
            .parse()
            .expect("a number");
        assert_eq!(stated, find(b"xref\n"), "startxref");
    }

    /// It has exactly one page, and nothing else that could reach an output.
    #[test]
    fn the_blank_document_is_one_empty_page_and_nothing_more() {
        let text = String::from_utf8_lossy(BLANK_DOCUMENT);
        assert!(text.contains("/Count 1"), "{text}");
        // No content stream, no resources, and nothing that carries a string into an output.
        for forbidden in ["/Contents", "/Producer", "/Creator", "/Title", "/Metadata"] {
            assert!(
                !text.contains(forbidden),
                "the blank destination carries {forbidden}, which would reach every split output"
            );
        }
    }
}

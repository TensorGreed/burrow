//! The PDFium implementation of [`DocumentEngine`].
//!
//! Compiled only when `build.rs` has confirmed the vendored engines are present and
//! checksum-verified. See `docs/adr/0004-native-engines.md` for acquisition,
//! `docs/adr/0011-pdfium-engine-thread.md` for why every call here runs on one thread,
//! and `docs/adr/0007-limit-enforcement-per-platform.md` for what the limits actually
//! promise.

// Private to `pdfium`, deliberately. Every function in it carries the unlisted
// precondition "call me only from the engine thread", and keeping the module private is
// what makes "no `FPDF_*` call exists outside this directory" checkable with `rg` rather
// than by reading `unsafe` blocks. Only the link check needs anything from it, and it gets
// exactly one item, below.
mod ffi;

mod errors;
mod estimate;
mod rss;
pub(crate) mod thread;

use std::sync::Arc;

use core::ffi::{c_char, c_int, c_void};

use burrow_types::{Clock, Deadline, Error, Limits, Result};
use zeroize::Zeroizing;

use crate::{DocumentEngine, OpenOptions};

/// Re-exported for the link check, which proves `FPDF_DestroyLibrary` binds without ever
/// calling it. The only part of [`ffi`] visible outside this module.
#[cfg(test)]
pub(crate) use self::ffi::destroy_library_symbol;

/// The PDFium-backed document engine.
///
/// Stateless: the engine itself is a process-wide singleton started on first use, so
/// constructing this is free and constructing several is the same as constructing one.
#[derive(Debug, Clone, Copy, Default)]
pub struct Pdfium;

impl Pdfium {
    /// A handle to the PDFium engine.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

/// An open PDFium document.
///
/// An identifier, a time budget, and the clock that budget is measured against. The raw
/// `FPDF_DOCUMENT` and the buffer it reads from stay inside the engine thread, which is
/// why this is `Send + Sync` without any `unsafe impl`.
///
/// **The clock is captured here at open, not passed per call.** An earlier design took a
/// `&dyn Clock` on every method, and that silently disabled `max_duration_ms`: a
/// [`Deadline`] records `start_ms` from one clock's *unspecified* epoch, so measuring it
/// against a second clock produced a saturated zero and the limit could never fire again —
/// no error, no warning. Capturing the clock removes the parameter that made that
/// expressible.
///
/// Dropping this closes the document. The close is submitted to the engine thread and not
/// waited for, so a drop never blocks.
pub struct PdfiumDocument {
    id: u64,
    deadline: Deadline,
    clock: Arc<dyn Clock>,
    pages_at_open: u64,
}

impl PdfiumDocument {
    /// The page count read at open, before any page was touched.
    ///
    /// This is the number `max_pages` was checked against, which is why it is kept rather
    /// than re-read: it is the value the limit decision was actually made on.
    #[must_use]
    pub fn pages_at_open(&self) -> u64 {
        self.pages_at_open
    }
}

impl core::fmt::Debug for PdfiumDocument {
    /// Hand-written because `Arc<dyn Clock>` has no `Debug`.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PdfiumDocument")
            .field("id", &self.id)
            .field("deadline", &self.deadline)
            .field("pages_at_open", &self.pages_at_open)
            .finish_non_exhaustive()
    }
}

impl Drop for PdfiumDocument {
    fn drop(&mut self) {
        thread::close(self.id);
    }
}

/// The password argument PDFium's C API takes: NUL-terminated, or null for none.
///
/// Held in a [`Zeroizing`] so the copy we made is wiped when the load returns, not left
/// in freed memory.
type PasswordArg = Option<Zeroizing<Vec<u8>>>;

/// Build the NUL-terminated password buffer PDFium wants.
///
/// # Errors
///
/// [`Error::InvalidArgument`](burrow_types::Error::InvalidArgument) if the password
/// contains a NUL byte. PDFium takes an `FPDF_BYTESTRING`, so an interior NUL would
/// silently truncate the password and the document would fail to open for a reason the
/// user could not possibly guess. Failing loudly is better — and the message says only
/// that, never the password.
fn password_arg(options: &OpenOptions<'_>) -> Result<PasswordArg> {
    let Some(password) = options.password else {
        return Ok(None);
    };
    if password.as_bytes().contains(&0) {
        return Err(Error::InvalidArgument(
            "password contains a NUL byte, which pdfium's C API cannot carry".to_owned(),
        ));
    }
    let mut buf = Vec::with_capacity(password.len().saturating_add(1));
    buf.extend_from_slice(password.as_bytes());
    buf.push(0);
    Ok(Some(Zeroizing::new(buf)))
}

/// Widen a page count PDFium reported, having already established it is not negative.
fn page_count_to_u64(count: c_int) -> Result<u64> {
    u64::try_from(count)
        .map_err(|_| Error::Internal("pdfium reported a page count that is not a count".to_owned()))
}

impl DocumentEngine for Pdfium {
    type Document = PdfiumDocument;

    fn name(&self) -> &'static str {
        "pdfium"
    }

    fn open(&self, bytes: Box<[u8]>, options: &OpenOptions<'_>) -> Result<Self::Document> {
        let limits = options.limits;

        // Empty input can never be a document, and refusing it here also avoids handing
        // PDFium the dangling (if correctly aligned) pointer an empty `Box<[u8]>` carries.
        if bytes.is_empty() {
            return Err(Error::Malformed("input is empty".to_owned()));
        }

        // 1. Input size, before the buffer goes anywhere near the engine.
        let input_len = u64::try_from(bytes.len())
            .map_err(|_| Error::Internal("input length does not fit in u64".to_owned()))?;
        Limits::check("max_input_bytes", input_len, limits.max_input_bytes)?;

        // 2. The size-based memory pre-check. A floor, not a ceiling -- see `estimate`'s
        //    docs, and step (e) below for the half that catches what a byte count cannot.
        estimate::check_open_memory(input_len, &limits)?;

        // 3. The structural pre-scan: what the file *declares*, checked before anything
        //    parses it. This is what step 2 cannot see and step (e) can only see after the
        //    fact -- a 330 KB file declaring twenty million cross-reference entries is
        //    refused here, having cost nothing, rather than after PDFium has allocated
        //    1.2 GB for it. Pure Rust, bounded by construction; see `crate::prescan`.
        crate::prescan::check(&bytes, &limits)?;

        // 4. The password copy, before the deadline starts: it is our work, not the
        //    engine's, and a rejected password should not consume the caller's budget.
        let password = password_arg(options)?;

        // 5. The budget for everything that follows, including later calls on the handle.
        let clock = Arc::clone(&options.clock);
        let deadline = Deadline::start(clock.as_ref(), &limits);
        deadline.checkpoint(clock.as_ref())?;

        let opened = thread::submit(move |registry| {
            // a. Claim the id, then park the buffer. Both happen before the load, so the
            //    only fallible step comes while there is still nothing to clean up, and
            //    the pointer handed to PDFium is read from the buffer's final home.
            let id = registry.reserve_id()?;
            let (data, size) = registry.park_buffer(id, bytes);
            let password_ptr = password
                .as_ref()
                .map_or(core::ptr::null(), |p| p.as_ptr().cast::<c_char>());

            // From here on, `registry.remove(id)` is the single cleanup for every failure:
            // it frees the buffer, and closes the document too once one is attached.
            let before = rss::resident_bytes();

            // SAFETY: `data` is valid for `size` bytes -- it points into the buffer the
            // registry now owns, which is not moved again until `remove`, and PDFium reads
            // from it for as long as the document is open (`fpdfview.h:451`). `size` is
            // non-zero, so the pointer is a real allocation. `password_ptr` is null, or
            // points at a NUL-terminated buffer alive for the whole call. This runs inside
            // a job, so it is on the engine thread and after init.
            let (doc, code) =
                unsafe { ffi::load_mem_document64(data.cast::<c_void>(), size, password_ptr) };

            // The copy of the password has done its job; wipe it now rather than at the
            // end of the closure.
            drop(password);

            if doc.is_null() {
                // Failure is established by the null handle. The code only classifies it,
                // and it was read in the same call, so it is this load's code.
                registry.remove(id);
                return Err(errors::map_failure(code));
            }

            // b. Hand the document to the registry immediately, so from here every exit --
            //    including an unwind -- closes it exactly once.
            //
            // SAFETY: `doc` is non-null (checked above), came from the load on the line
            // before, was loaded from the buffer parked under `id`, and is not yet closed.
            unsafe { registry.attach(id, doc) };

            let checked = (|| -> Result<u64> {
                // c. The page count, immediately, before any page is touched.
                //
                // SAFETY: `doc` is non-null and open -- the registry holds it and nothing
                // has closed it. Still inside the job, so still on the engine thread.
                let (count, _code) = unsafe { ffi::get_page_count(doc) };
                if count < 0 {
                    // `_code` is deliberately unused. `fpdfview.h:625` says the error is
                    // only meaningful for APIs whose own documentation mentions
                    // `FPDF_GetLastError`, and `FPDF_GetPageCount`'s block does not. The
                    // global would hold whatever some earlier call left there, so
                    // classifying by it would pin another operation's error to this one --
                    // a stale `FPDF_ERR_PASSWORD` would surface as `PasswordRequired` on a
                    // document that is already open.
                    return Err(errors::unreadable_page_tree());
                }
                let pages = page_count_to_u64(count)?;
                if pages == 0 {
                    return Err(Error::Malformed("document has no pages".to_owned()));
                }

                // d. The page limit, on a number the engine produced.
                Limits::check("max_pages", pages, limits.max_pages)?;

                // e. What the open actually cost. Step 2's estimate is blind to anything
                //    the file *declares*, and a small file declaring an enormous structure
                //    is exactly the case it misses.
                estimate::check_measured_memory(before, rss::resident_bytes(), &limits)?;

                Ok(pages)
            })();

            match checked {
                Ok(pages) => Ok((id, pages)),
                Err(error) => {
                    // A document that has already broken a limit, or whose page tree
                    // cannot be read, is never handed back.
                    registry.remove(id);
                    Err(error)
                }
            }
        })?;

        deadline.checkpoint(clock.as_ref())?;

        let (id, pages) = opened;
        Ok(PdfiumDocument {
            id,
            deadline,
            clock,
            pages_at_open: pages,
        })
    }

    fn page_count(&self, doc: &Self::Document) -> Result<u64> {
        // Against the clock captured at open, so the budget covers the whole operation and
        // cannot be reset by handing in a different one.
        doc.deadline.checkpoint(doc.clock.as_ref())?;

        let id = doc.id;
        let pages = thread::submit(move |registry| {
            let handle = registry.handle(id)?;
            // SAFETY: `handle` came from the registry, so the document is open and its
            // buffer is alive. Inside a job, so on the engine thread.
            let (count, _code) = unsafe { ffi::get_page_count(handle) };
            if count < 0 {
                // See the note in `open`: the error global is undefined after this call.
                return Err(errors::unreadable_page_tree());
            }
            page_count_to_u64(count)
        })?;

        doc.deadline.checkpoint(doc.clock.as_ref())?;
        Ok(pages)
    }
}

#[cfg(test)]
mod tests;

//! PDFium as redaction's second reading (#206): where characters are, and how many pages there are.
//!
//! The rules are [`crate::redact_read_back`]'s; this is the reading. Geometry only -- see
//! `ffi.rs`'s `fpdf_text.h` block for why no function returning text is declared.
//!
//! # Limits, because `FPDFText_LoadPage` cannot be interrupted
//!
//! - **The operation's deadline, not a fresh one.** Both opens spend it (`open_spending`), and it
//!   is checkpointed before and after every engine call, and every 4,096 characters while their
//!   origins are read.
//! - **`estimate::before_page_load` before the page is loaded.** `FPDF_LoadPage` builds the
//!   display list and `FPDFText_LoadPage` the text layer, in two calls nothing can checkpoint;
//!   the only lever on them is refusing to begin, which is what render does too.
//! - **The measured memory check after them**, which detects rather than bounds (ADR 0007).
//! - **[`crate::redact_read_back::MAX_MARKED_WORK`], before any of it.** PDFium's page load
//!   grows with marked-content nesting; capped before it, by the rules, which refuse before
//!   `read_page` is asked.
//! - **[`crate::redact_read_back::MAX_CHARS`]**: a page reporting more characters is refused
//!   before any is read.
//! - **PDFium's own one-thread engine**, which is process-wide and initialised once (ADR 0011).

use std::sync::Arc;

use burrow_types::{Clock, Deadline, Error, Limits, Result};

use super::{PdfiumDocument, ffi, open_spending, page_count_of, page_index, thread};
use crate::OpenOptions;
use crate::redact_read_back::{MAX_CHARS, PageReading, PlacedChar, SecondReading};

/// How many characters are read between deadline checkpoints.
const CHARS_PER_CHECKPOINT: usize = 4096;

/// PDFium, reading what a redaction took in and what it emitted.
pub(crate) struct PdfiumReading<'a> {
    limits: Limits,
    clock: Arc<dyn Clock>,
    password: Option<&'a burrow_types::Password>,
    deadline: Deadline,
}

impl<'a> PdfiumReading<'a> {
    /// A reading spending the operation's ceilings, password and remaining time.
    pub(crate) fn over(options: &OpenOptions<'a>, deadline: Deadline) -> Self {
        Self {
            limits: options.limits,
            clock: Arc::clone(&options.clock),
            password: options.password,
            deadline,
        }
    }

    /// Open `bytes` under the operation's deadline, with the input ceilings raised to fit them.
    ///
    /// Raised as `burrow_ops::verify::read_back_options` raises them, and for its reason: the
    /// output is burrow's, and a ceiling on what a caller may hand in would otherwise refuse an
    /// output a few hundred bytes larger than an input that passed it. `max_pages`,
    /// `max_duration_ms` and the deadline are untouched.
    fn open(&self, bytes: &[u8]) -> Result<PdfiumDocument> {
        let size = u64::try_from(bytes.len())
            .map_err(|_| Error::Internal("a document length does not fit in u64".to_owned()))?;
        let mut limits = self.limits;
        limits.max_input_bytes = limits.max_input_bytes.max(size);
        limits.max_memory_bytes = limits
            .max_memory_bytes
            .max(crate::estimate::estimated_open_bytes(size));
        let mut options = OpenOptions::new(limits, Arc::clone(&self.clock));
        options.password = self.password;
        let bytes = bytes.to_vec().into_boxed_slice();
        // R10: THE COPY PDFIUM IS HANDED, under the name its caller gave it.
        #[cfg(test)]
        crate::redact::hooks::record_open(&bytes);
        open_spending(bytes, &options, Some(self.deadline))
    }
}

impl SecondReading for PdfiumReading<'_> {
    fn page_count(&self, bytes: &[u8]) -> Result<Option<u64>> {
        #[cfg(test)]
        if let Some(error) = crate::redact::hooks::forced_second_reading_failure() {
            return Err(error);
        }
        #[cfg(test)]
        crate::redact::hooks::second_reading_begins();
        // R10: WHAT EACH READING OPENS, by method, recorded in `open`. See `hooks::record_open`.
        #[cfg(test)]
        crate::redact::hooks::opening(crate::redact::hooks::Opened::PageCount);
        let document = self.open(bytes)?;
        page_count_of(&document).map(Some)
    }

    fn read_page(&self, bytes: &[u8], page: usize) -> Result<Option<PageReading>> {
        #[cfg(test)]
        if let Some(error) = crate::redact::hooks::forced_second_reading_failure() {
            return Err(error);
        }
        #[cfg(test)]
        crate::redact::hooks::opening(crate::redact::hooks::Opened::ReadPage);
        let document = self.open(bytes)?;
        let pages = page_count_of(&document)?;
        // A HOOK THAT READS ANOTHER PAGE, for the one question no document asks: is a reading of
        // a page with nothing on it -- the wrong page -- refused?
        #[cfg(test)]
        let page = crate::redact::hooks::second_reading_page(page);
        let index = u64::try_from(page)
            .map_err(|_| Error::Internal("a page index that does not fit in u64".to_owned()))?;
        let (width, height) = crate::PageRenderer::page_size(&super::Pdfium, &document, index)?;
        let chars = char_placements(&document, index, &self.limits, self.deadline)?;
        Ok(Some(PageReading {
            pages,
            page,
            size: (f64::from(width), f64::from(height)),
            chars,
        }))
    }
}

/// Every character PDFium places on page `index`, generated ones included.
fn char_placements(
    document: &PdfiumDocument,
    index: u64,
    limits: &Limits,
    deadline: Deadline,
) -> Result<Vec<PlacedChar>> {
    let clock = Arc::clone(&document.clock);
    deadline.checkpoint(clock.as_ref())?;

    // THE UNINTERRUPTIBLE LOADS ARE REFUSED BEFORE THEY BEGIN when this document has already
    // spent the budget, as `render` refuses them.
    crate::estimate::before_page_load(
        document.memory_at_open,
        crate::rss::resident_bytes(),
        limits,
    )?;

    let page_index = page_index(index, document.pages_at_open)?;
    let id = document.id;
    let limits = *limits;
    let clock_in = Arc::clone(&clock);
    let placed = thread::submit(move |registry| {
        let handle = registry.handle(id)?;
        let before = crate::rss::resident_bytes();

        // SAFETY: `handle` came from the registry, so the document is open. Engine thread.
        let page = unsafe { ffi::load_page(handle, page_index) };
        if page.is_null() {
            return Err(Error::Malformed("a page could not be loaded".to_owned()));
        }

        // Everything fallible is inside this closure so the page is closed on every exit by the
        // single call after it.
        let read = (|| -> Result<Vec<PlacedChar>> {
            // SAFETY: `page` is live and from this document. Engine thread.
            let text = unsafe { ffi::text_load_page(page) };
            if text.is_null() {
                return Err(Error::Malformed(
                    "a page's text could not be read".to_owned(),
                ));
            }
            let chars = (|| -> Result<Vec<PlacedChar>> {
                // SAFETY: `text` is live. Engine thread.
                let count = unsafe { ffi::text_count_chars(text) };
                let count = usize::try_from(count).map_err(|_| {
                    Error::Malformed("a page's characters could not be counted".to_owned())
                })?;
                if count > MAX_CHARS {
                    return Err(Error::Unsupported(
                        "pdf redaction: more characters on the page than the second reading \
                         reads"
                            .to_owned(),
                    ));
                }
                crate::estimate::check_measured_memory(
                    before,
                    crate::rss::resident_bytes(),
                    &limits,
                )?;
                let mut out = Vec::with_capacity(count);
                for at in 0..count {
                    if at % CHARS_PER_CHECKPOINT == 0 {
                        deadline.checkpoint(clock_in.as_ref())?;
                    }
                    let at_c = core::ffi::c_int::try_from(at).map_err(|_| {
                        Error::Internal("a character index that does not fit in an int".to_owned())
                    })?;
                    // SAFETY: `text` is live and `at_c` is below the count it reported. Engine
                    // thread.
                    let placement =
                        unsafe { ffi::text_char_placement(text, at_c) }.ok_or_else(|| {
                            Error::Malformed("a character's place could not be read".to_owned())
                        })?;
                    out.push(PlacedChar {
                        origin: placement.origin,
                        generated: placement.generated,
                    });
                }
                Ok(out)
            })();
            // SAFETY: `text` is live, this is its only close, and its page is still open.
            unsafe { ffi::text_close_page(text) };
            chars
        })();

        // SAFETY: `page` is live, this is its only close, and the document is still open.
        unsafe { ffi::close_page(page) };
        read
    })?;

    deadline.checkpoint(clock.as_ref())?;
    Ok(placed)
}

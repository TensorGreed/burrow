//! PDFium in the render worker as redaction's second reading (#206's web half): where characters
//! are on a page, and how many pages there are.
//!
//! The web counterpart of `pdfium::read_back`, and deliberately the same algorithm over
//! [`PdfiumBridge`]: the same five `fpdf_text.h` functions, the same order (the page, its text
//! layer, the count, each origin and whether it was generated), the same closes on every exit,
//! and the same ceilings. The rules are [`crate::redact_read_back`]'s, and they do not run here:
//! this module reads, and the redaction module decides, over a recording of what was read --
//! see [`crate::redact_read_back::recorded`].
//!
//! # Limits, because `FPDFText_LoadPage` cannot be interrupted
//!
//! - **One deadline for the whole reading**, started once when the request arrived and spent by
//!   both opens (`WebPdfium::open_spending`), never a deadline per open: the input's page count
//!   and the output's page read share it, as native's two opens share the operation's. Checkpointed
//!   before and after every engine call and every 4,096 characters.
//! - **`estimate::before_page_load` before the page is loaded**, as `render` refuses a load on a
//!   heap that has already grown past the ceiling.
//! - **The measured heap check after the loads**, which detects rather than bounds (ADR 0007).
//! - **The marked-content ceiling is NOT here.** It needs burrow's walk of the output, which only
//!   the redaction module has, so the redaction module refuses a page past
//!   [`crate::redact_read_back::MAX_MARKED_WORK`] before it asks for this reading at all.
//! - **[`MAX_CHARS`]**: a page reporting more characters is refused before any is read.

use std::sync::Arc;

use burrow_types::{Deadline, Error, Result};

use super::{WebDocument, WebPdfium, page_index};
use crate::redact_read_back::{MAX_CHARS, PageReading, PlacedChar, SecondReading};
use crate::{OpenOptions, PageRenderer};

/// How many characters are read between deadline checkpoints, as on native.
const CHARS_PER_CHECKPOINT: usize = 4096;

/// The render worker's PDFium, reading what a redaction took in and what it emitted.
pub struct WebPdfiumReading<'a> {
    engine: &'a WebPdfium,
    options: &'a OpenOptions<'a>,
    deadline: Deadline,
}

impl<'a> WebPdfiumReading<'a> {
    /// A reading through `engine`, under `options`' ceilings and password, spending `deadline`.
    ///
    /// `deadline` is the request's: started once, when the request arrived, and spent by every
    /// open the reading makes. The redaction worker keeps its own deadline and does not read this
    /// one back; this one only bounds how long the render worker spends.
    #[must_use]
    pub fn over(engine: &'a WebPdfium, options: &'a OpenOptions<'a>, deadline: Deadline) -> Self {
        Self {
            engine,
            options,
            deadline,
        }
    }

    /// Open `bytes`, with the input ceilings raised to fit them, as `burrow_ops::verify` raises
    /// them for a read-back: the output is burrow's own, a few bytes larger than an input that
    /// passed. `max_pages` and the duration are untouched.
    fn open(&self, bytes: &[u8]) -> Result<WebDocument> {
        let size = u64::try_from(bytes.len())
            .map_err(|_| Error::Internal("a document length does not fit in u64".to_owned()))?;
        let mut limits = self.options.limits;
        limits.max_input_bytes = limits.max_input_bytes.max(size);
        limits.max_memory_bytes = limits
            .max_memory_bytes
            .max(crate::estimate::estimated_open_bytes(size));
        let mut options = OpenOptions::new(limits, Arc::clone(&self.options.clock));
        options.password = self.options.password;
        self.engine.open_spending(
            bytes.to_vec().into_boxed_slice(),
            &options,
            Some(self.deadline),
        )
    }
}

impl SecondReading for WebPdfiumReading<'_> {
    fn page_count(&self, bytes: &[u8]) -> Result<Option<u64>> {
        let document = self.open(bytes)?;
        Ok(Some(document.pages_at_open))
    }

    fn read_page(&self, bytes: &[u8], page: usize) -> Result<Option<PageReading>> {
        let document = self.open(bytes)?;
        let index = u64::try_from(page)
            .map_err(|_| Error::Internal("a page index that does not fit in u64".to_owned()))?;
        let (width, height) = self.engine.page_size(&document, index)?;
        let chars = char_placements(self.engine, &document, index, self.options)?;
        Ok(Some(PageReading {
            pages: document.pages_at_open,
            page,
            size: (f64::from(width), f64::from(height)),
            chars,
        }))
    }
}

/// Every character PDFium places on page `index`, generated ones included.
fn char_placements(
    engine: &WebPdfium,
    document: &WebDocument,
    index: u64,
    options: &OpenOptions<'_>,
) -> Result<Vec<PlacedChar>> {
    let deadline: &Deadline = &document.deadline;
    let clock = Arc::clone(&document.clock);
    deadline.checkpoint(clock.as_ref())?;
    let bridge = &engine.bridge;

    // THE UNINTERRUPTIBLE LOADS ARE REFUSED BEFORE THEY BEGIN when the heap has already spent the
    // budget, as `render` refuses them.
    crate::estimate::before_page_load(
        document.memory_at_open,
        Some(bridge.heap_bytes()),
        &options.limits,
    )?;
    let at = page_index(index, document.pages_at_open)?;
    let before = bridge.heap_bytes();

    let page = bridge.load_page(document.doc, at);
    if page.is_null() {
        return Err(Error::Malformed("a page could not be loaded".to_owned()));
    }
    // Everything fallible is inside this closure, so the page is closed on every exit by the one
    // call after it. There is no `Drop` across the bridge to fall back on.
    let read = (|| -> Result<Vec<PlacedChar>> {
        let text = bridge.text_load_page(page);
        if text.is_null() {
            return Err(Error::Malformed(
                "a page's text could not be read".to_owned(),
            ));
        }
        let chars = (|| -> Result<Vec<PlacedChar>> {
            let count = usize::try_from(bridge.text_count_chars(text)).map_err(|_| {
                Error::Malformed("a page's characters could not be counted".to_owned())
            })?;
            if count > MAX_CHARS {
                return Err(Error::Unsupported(
                    "pdf redaction: more characters on the page than the second reading reads"
                        .to_owned(),
                ));
            }
            crate::estimate::check_measured_memory(
                Some(before),
                Some(bridge.heap_bytes()),
                &options.limits,
            )?;
            let mut out = Vec::with_capacity(count);
            for at in 0..count {
                if at % CHARS_PER_CHECKPOINT == 0 {
                    deadline.checkpoint(clock.as_ref())?;
                }
                let at = i32::try_from(at).map_err(|_| {
                    Error::Internal("a character index that does not fit in an int".to_owned())
                })?;
                // NATIVE'S ORDER AND NATIVE'S READING (`pdfium::ffi::text_char_placement`), so the two
                // platforms cannot disagree about one character: `IsGenerated` then the origin,
                // negative or a failed origin is no answer, and only 1 is "generated".
                let generated = bridge.text_is_generated(text, at);
                let origin = bridge.text_char_origin(text, at);
                let (Some(origin), false) = (origin, generated < 0) else {
                    return Err(Error::Malformed(
                        "a character's place could not be read".to_owned(),
                    ));
                };
                out.push(PlacedChar {
                    origin,
                    generated: generated == 1,
                });
            }
            Ok(out)
        })();
        // Its only close, while its page is still open.
        bridge.text_close_page(text);
        chars
    })();
    // Its only close, while the document is still open.
    bridge.close_page(page);
    let placed = read?;
    deadline.checkpoint(clock.as_ref())?;
    Ok(placed)
}

#[cfg(test)]
mod tests;

//! Redaction, in its own artifact: `burrow_wasm_redact_bg.wasm`, and nothing else is in it.
//!
//! # Why a third artifact
//!
//! ADR 0029's 2026-09-21 amendment, and #137 carries the measurement. Redaction's Rust — the
//! content-stream rewriter, the geometry walk, the font surgery — cost nothing in the base
//! module only while nothing called it: LTO strips unreachable code whole. The moment an entry
//! point reaches it, it lands on whatever module that entry point is compiled into. In the base
//! module, that would mean everyone who only ever rotates a PDF downloads the redaction engine,
//! against a first-load margin already under the 10% `apps/web/size-budget.json` records as
//! policy. So this crate gains a third mutually exclusive feature, `redact`, and this is the
//! only module it compiles.
//!
//! It links **qpdf** through the same bridge the base module uses (`crate::qpdf_engine`), and
//! no PDFium: redaction's verification reads back through qpdf and burrow's own pdfsyntax,
//! never through a renderer. What holds that is structural, not a scan: `lib.rs`'s
//! `compile_error!` refuses a `wasm32` build with `redact` and `render` together, so this module
//! cannot compile the PDFium bridge; and the bundle's own source list in
//! `tools/stage-web-engines.mjs` concatenates no PDFium glue. **No scan examines this module for
//! PDFium**: `tools/check-pdfium-is-render-only.sh` reads production builds, and this bundle is
//! staged into harness builds only (security and code review of #137).
//!
//! # What this does not do
//!
//! It does not put redaction on the site. The bundle this module ships in is staged into
//! harness builds only, and `/redact-pdf` is held under #125, #180–#182 and #227–#229
//! (`src/production-build.test.ts` in `apps/web`). #136 is the page.

use std::collections::BTreeSet;
use std::sync::Arc;

use burrow_core::engines::OpenOptions;
use burrow_core::engines::pdfsyntax::region::Region;
use burrow_core::{Clock, Error};
use wasm_bindgen::prelude::wasm_bindgen;
use zeroize::Zeroizing;

use crate::qpdf_engine::qpdf;
use crate::{Reply, WebClock, WebLimits};

/// Clear a region on one page, and return the document only if it reads back as promised.
///
/// # Arguments, and why each is shaped as it is
///
/// - `page` and `covered` are **1-based page numbers**, like every other operation's. Zero is
///   refused rather than read as "the first page": a caller that is off by one should hear
///   about it, which is the rule `rotate` states and this keeps.
/// - `covered` is every page the redaction covers; `page` must be one of them. It decides which
///   fonts may be cut — a font is cut only when every page using it is covered — so it is the
///   caller's statement of intent, not a detail. See `burrow_ops::redact::page`.
/// - `region` is a [`WebRegion`]: four numbers in **display units**, from the top-left of the
///   displayed page. A non-finite or empty region is the core's to refuse, not this boundary's.
///
/// # The reply
///
/// On success, [`Reply::take_output`] is the verified document, and the report rides beside it
/// ([`Reply::report`], [`Reply::retained_fonts`], [`Reply::dropped_carried_text`]). **No page
/// count**: a redaction cannot change it, the verification already required it unchanged, and
/// reading it back here would copy the redacted document once more into a heap that ADR 0029
/// and #199 want holding as few copies as possible.
///
/// # Errors
///
/// Never panics. Every refusal arrives typed in the reply: a geometry rule the document breaks,
/// a ceiling reached, `OutputRejected` if the output does not read back as promised, or
/// `InvalidArgument` for a page outside the document or outside `covered`.
#[wasm_bindgen]
#[must_use]
pub fn redact(
    bytes: Box<[u8]>,
    page: u32,
    covered: &[u32],
    region: WebRegion,
    password: Option<Box<[u8]>>,
    limits: WebLimits,
) -> Reply {
    // WIPED WHEN DROPPED (#199): the document being redacted is the secret.
    let bytes = Zeroizing::new(bytes);
    let limits = limits.to_core();
    let clock: Arc<dyn Clock> = Arc::new(WebClock);
    let password = crate::password_from(password);

    let mut options = OpenOptions::new(limits, clock);
    options.password = password.as_ref();

    let Some(index) = page_index(page) else {
        return Reply::failure(&zero_page()).with_lifecycle(&limits);
    };
    let mut indexes = BTreeSet::new();
    for number in covered {
        let Some(covered_index) = page_index(*number) else {
            return Reply::failure(&zero_page()).with_lifecycle(&limits);
        };
        indexes.insert(covered_index);
    }

    // RECYCLED AFTER EVERY REDACTION THAT READ THE DOCUMENT, success or refusal alike (#199):
    // qpdf's object cache holds the decoded page content -- the text being removed -- and is
    // freed unwiped. A refusal has read it too. The page-number refusals above opened nothing.
    match burrow_core::ops::redact::page(&qpdf(), &bytes, index, &indexes, region.0, &options) {
        Ok(done) => Reply::redacted(done.document, &done.report),
        Err(error) => Reply::failure(&error),
    }
    .with_lifecycle(&limits)
    .recycled()
}

/// Every page's display frame, or the page-level rule that refuses every region on it (#136).
///
/// What the redaction page asks before a person draws: the frame a region on each page is measured
/// against -- width and height in points, after `/Rotate` and `/UserUnit`, `/CropBox` falling back
/// to `/MediaBox` -- or the rule that would refuse every region on that page. Read-only: nothing is
/// written, and no page's content is decoded. See `burrow_ops::redact::page_frames`.
///
/// # The reply
///
/// On success, [`Reply::frames`] carries `[width, height]` per page and [`Reply::frame_refusals`] a
/// rule name per page (`""` for a page with a frame); `pages` is the page count. No output. A
/// document the redaction refuses at its open -- a ceiling, a password, a repaired input -- is a
/// failure reply, with the same error the redaction gives. Here, unlike natively, a page whose
/// `/MediaBox` comes from the page tree is `media-box-unverified`: the redaction worker has no
/// renderer to vouch for it (#206).
///
/// # Errors
///
/// Never panics. Every refusal arrives typed in the reply.
#[wasm_bindgen]
#[must_use]
pub fn page_frames(bytes: Box<[u8]>, password: Option<Box<[u8]>>, limits: WebLimits) -> Reply {
    // WIPED WHEN DROPPED (#199): the document is the person's, whatever is read from it.
    let bytes = Zeroizing::new(bytes);
    let limits = limits.to_core();
    let clock: Arc<dyn Clock> = Arc::new(WebClock);
    let password = crate::password_from(password);

    let mut options = OpenOptions::new(limits, clock);
    options.password = password.as_ref();

    // RECYCLED AFTER EVERY READING, success or refusal, as a redaction is (#199): it decodes no
    // page content, but qpdf's heap held the input, and the rule this bundle keeps is that a
    // worker which held a person's document is not handed the next one.
    match burrow_core::ops::redact::page_frames(&qpdf(), &bytes, &options) {
        Ok(readings) => Reply::framed(&readings),
        Err(error) => Reply::failure(&error),
    }
    .with_lifecycle(&limits)
    .recycled()
}

/// A region on the displayed page, as JavaScript hands it over.
///
/// A struct rather than four more arguments on [`redact`], for the reason [`WebLimits`] is one:
/// constructed in the worker, consumed by value, and the numbers cannot be passed in the wrong
/// order at the call site because each has a name there.
///
/// **Consumed by the call.** wasm-bindgen moves a struct argument into Rust, so the worker must
/// not `free()` it afterwards — `WebLimits`' own comment in `main.js` records what that does.
#[wasm_bindgen]
pub struct WebRegion(Region);

#[wasm_bindgen]
impl WebRegion {
    /// Distances from the top-left of the displayed page, in display units. Downwards is
    /// positive, as in `burrow_ops`' `Region`.
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new(left: f64, top: f64, width: f64, height: f64) -> Self {
        Self(Region {
            left,
            top,
            width,
            height,
        })
    }
}

/// A 1-based page number as the core's 0-based index, or `None` for zero.
fn page_index(number: u32) -> Option<usize> {
    let index = number.checked_sub(1)?;
    usize::try_from(index).ok()
}

fn zero_page() -> Error {
    Error::InvalidArgument("redact: pages are numbered from 1, and 0 names no page".to_owned())
}

#[cfg(test)]
mod tests {
    use super::page_index;

    #[test]
    fn page_numbers_are_one_based_and_zero_is_refused() {
        assert_eq!(page_index(0), None);
        assert_eq!(page_index(1), Some(0));
        assert_eq!(page_index(u32::MAX), usize::try_from(u32::MAX - 1).ok());
    }
}

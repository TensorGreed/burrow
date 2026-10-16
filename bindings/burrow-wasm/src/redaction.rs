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
        // A SECOND READING STILL DUE IS NOT A RESULT (#206's web half): this one-call entry has no
        // renderer to take it, so it refuses rather than hand out output nobody has read twice.
        // `redact_begin` and `redact_confirm` are the path that releases a redaction.
        Ok(done) if done.report.second_reading.is_some() => Reply::failure(&Error::OutputRejected(
            "redact: the second reading refuses the output -- [second-reading-unavailable] this \
             entry takes no second reading; use the two-phase path"
                .to_owned(),
        )),
        Ok(done) => Reply::redacted(done.document, &done.report),
        Err(error) => Reply::failure(&error),
    }
    .with_lifecycle(&limits)
    .recycled()
}

// ---- the second reading, in two phases (#206's web half) ------------------------------------
//
// The redaction worker holds no PDFium, so the second reading is taken in a fresh render worker
// and judged HERE, against burrow's own walk of the output, over the render worker's recording.
// The design (security-reviewed, owner option A, 2026-10-15):
//
// - `redact_begin` redacts and runs burrow's check; the walk's agreement, the input, the output and
//   this worker's OWN deadline go into one pending slot under a monotonic nonce. The output does
//   NOT leave: the reply carries copies of the two documents for the render worker, never the
//   result. This worker is not recycled between the phases.
// - `redact_confirm` takes the render worker's answer for that nonce, once. The verdict is made
//   here, over this worker's own digests of its own bytes; nothing the page or the render worker
//   sends can turn a disagreement into Ok, and a time reported back is never read.
// - `redact_abort` is the only thing the page may do to a pending reading: end it. It cannot
//   confirm one.
// - Either way the slot is wiped and the reply recycles the worker (#199).

/// The reading still to be judged, held in this worker between the two phases.
struct Pending {
    nonce: u64,
    input: Zeroizing<Vec<u8>>,
    output: Zeroizing<Vec<u8>>,
    report: burrow_core::engines::redact::Report,
    due: burrow_core::engines::redact::SecondReadingDue,
    deadline: burrow_core::Deadline,
    limits: burrow_core::Limits,
}

thread_local! {
    /// ONE pending reading per worker, and a worker redacts one document. A second `begin` while
    /// one is pending replaces nothing: it is refused.
    static PENDING: core::cell::RefCell<Option<Pending>> = const { core::cell::RefCell::new(None) };
    /// Monotonic, so an answer for an earlier reading can never be mistaken for this one.
    static NEXT_NONCE: core::cell::Cell<u64> = const { core::cell::Cell::new(1) };
}

/// Phase 1's reply: an outcome, and on success the copies the render worker reads.
#[wasm_bindgen]
pub struct Begun {
    reply: Reply,
    nonce: u64,
    input: Vec<u8>,
    output: Vec<u8>,
    remaining_ms: u64,
}

#[wasm_bindgen]
impl Begun {
    /// The outcome. A success here is **not a result**: it means a second reading is due.
    #[must_use]
    pub fn reply(&self) -> Reply {
        self.reply.clone()
    }

    /// The nonce `redact_confirm` and `redact_abort` must name. Zero on a failure.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn nonce(&self) -> u64 {
        self.nonce
    }

    /// The operation's budget left, for the render worker's limits. Its own deadline is kept here.
    #[wasm_bindgen(getter, js_name = remainingMs)]
    #[must_use]
    pub fn remaining_ms(&self) -> u64 {
        self.remaining_ms
    }

    /// A copy of the input for the render worker, moved out once.
    #[wasm_bindgen(js_name = takeInput)]
    pub fn take_input(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.input)
    }

    /// A copy of the output for the render worker, moved out once. Not the result.
    #[wasm_bindgen(js_name = takeOutput)]
    pub fn take_output(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.output)
    }
}

impl Begun {
    fn refused(reply: Reply) -> Self {
        Self {
            reply,
            nonce: 0,
            input: Vec::new(),
            output: Vec::new(),
            remaining_ms: 0,
        }
    }
}

/// Phase 1: [`redact`]'s work, with the second reading still due. See the block above.
#[wasm_bindgen(js_name = redactBegin)]
#[must_use]
pub fn redact_begin(
    bytes: Box<[u8]>,
    page: u32,
    covered: &[u32],
    region: WebRegion,
    password: Option<Box<[u8]>>,
    limits: WebLimits,
) -> Begun {
    let bytes = Zeroizing::new(bytes);
    let limits = limits.to_core();
    let clock: Arc<dyn Clock> = Arc::new(WebClock);
    // THIS WORKER'S OWN DEADLINE FOR THE WHOLE OPERATION, both phases. Never read back from
    // anywhere else.
    let deadline = burrow_core::Deadline::start(clock.as_ref(), &limits);
    let password = crate::password_from(password);
    let mut options = OpenOptions::new(limits, Arc::clone(&clock));
    options.password = password.as_ref();

    let refused =
        |error: &Error| Begun::refused(Reply::failure(error).with_lifecycle(&limits).recycled());
    if PENDING.with(|slot| slot.borrow().is_some()) {
        return refused(&Error::Internal(
            "redact: a second reading is already pending in this worker".to_owned(),
        ));
    }
    let Some(index) = page_index(page) else {
        return refused(&zero_page());
    };
    let mut indexes = BTreeSet::new();
    for number in covered {
        let Some(covered_index) = page_index(*number) else {
            return refused(&zero_page());
        };
        indexes.insert(covered_index);
    }
    let done = match burrow_core::ops::redact::page(
        &qpdf(),
        &bytes,
        index,
        &indexes,
        region.0,
        &options,
    ) {
        Ok(done) => done,
        Err(error) => return refused(&error),
    };
    // TAKEN OUT OF THE REPORT, not copied: the report reaches the page in the reply, and what a
    // reading is due over is the redaction's own geometry, which is not the page's to see. Its
    // `Debug` is then the one R8's report rule holds.
    let mut report = done.report;
    let Some(due) = report.second_reading.take() else {
        return refused(&Error::Internal(
            "redact: the web's redaction returned with no second reading due".to_owned(),
        ));
    };
    let nonce = NEXT_NONCE.with(|next| {
        let nonce = next.get();
        next.set(nonce.saturating_add(1));
        nonce
    });
    let input = bytes.to_vec();
    let output = done.document.clone();
    let remaining_ms = limits
        .max_duration_ms
        .saturating_sub(deadline.elapsed_ms(clock.as_ref()));
    PENDING.with(|slot| {
        *slot.borrow_mut() = Some(Pending {
            nonce,
            input: Zeroizing::new(bytes.to_vec()),
            output: Zeroizing::new(done.document),
            report,
            due,
            deadline,
            limits,
        });
    });
    Begun {
        // NOT RECYCLED: the worker must hold the pending reading until it is judged.
        reply: Reply::success(0),
        nonce,
        input,
        output,
        remaining_ms,
    }
}

/// The longest answer [`redact_confirm`] accepts, for the worker to check before it copies one in.
#[wasm_bindgen(js_name = maxSecondReadingAnswer)]
#[must_use]
pub fn max_second_reading_answer() -> u32 {
    u32::try_from(burrow_core::engines::redact_read_back::recorded::MAX_ANSWER_LEN)
        .unwrap_or(u32::MAX)
}

/// Phase 2: judge the render worker's `answer` for `nonce`, and only then release the output.
///
/// An answer for a nonce that is not the pending one is **dropped**: the pending reading is left
/// as it was, and the reply is a refusal the worker discards. The pending reading is judged once.
#[wasm_bindgen(js_name = redactConfirm)]
#[must_use]
pub fn redact_confirm(nonce: u64, answer: &[u8]) -> Reply {
    use burrow_core::engines::redact_read_back::recorded::{
        Answer, agrees_recorded, decode_answer,
    };

    let pending = PENDING.with(|slot| {
        let mut slot = slot.borrow_mut();
        match slot.as_ref() {
            Some(held) if held.nonce == nonce => slot.take(),
            _ => None,
        }
    });
    let Some(pending) = pending else {
        return Reply::failure(&Error::InvalidArgument(
            "redact: an answer for no pending second reading".to_owned(),
        ));
    };
    let clock: Arc<dyn Clock> = Arc::new(WebClock);
    let limits = pending.limits;
    let verdict = (|| {
        // THE OPERATION'S DEADLINE ON ARRIVAL, then again before the rules (design review L2).
        pending.deadline.checkpoint(clock.as_ref())?;
        let recorded = match decode_answer(answer) {
            Ok(Answer::Read(recorded)) => recorded,
            Ok(Answer::Failed(failure)) => return Err(failure.into_error()),
            // A REPLY THAT DOES NOT DECODE IS NO ANSWER, refused by name.
            Err(_) => {
                return Err(
                    burrow_core::engines::redact_read_back::recorded::Failure::NoAnswer
                        .into_error(),
                );
            }
        };
        pending.deadline.checkpoint(clock.as_ref())?;
        agrees_recorded(recorded, &pending.input, &pending.output, &pending.due.0)
    })();
    match verdict {
        Ok(()) => Reply::redacted(pending.output.to_vec(), &pending.report),
        Err(error) => Reply::failure(&error),
    }
    .with_lifecycle(&limits)
    .recycled()
}

/// End the pending reading for `nonce` without a verdict: the output is wiped, never released.
///
/// The one thing the page may do to a pending reading (owner option A): end it, on a timeout or a
/// person stopping. It cannot confirm one.
#[wasm_bindgen(js_name = redactAbort)]
#[must_use]
pub fn redact_abort(nonce: u64) -> Reply {
    let ended = PENDING.with(|slot| {
        let mut slot = slot.borrow_mut();
        matches!(slot.as_ref(), Some(held) if held.nonce == nonce) && slot.take().is_some()
    });
    let error = if ended {
        Error::OutputRejected(
            "redact: the second reading refuses the output -- [second-reading-unavailable] the \
             renderer's reading did not arrive"
                .to_owned(),
        )
    } else {
        Error::InvalidArgument("redact: an abort for no pending second reading".to_owned())
    };
    Reply::failure(&error).recycled()
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

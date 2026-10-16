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
    // A SECOND READING STILL DUE IS NOT A RESULT (#206's web half), and `ops::redact::page` itself
    // refuses it by name: this module holds no renderer to take one. `redact_begin` and
    // `redact_confirm` are the path that releases a redaction.
    match burrow_core::ops::redact::page(&qpdf(), &bytes, index, &indexes, region.0, &options) {
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
///
/// The input and the output are MOVED in, not copied: they are the only copies this module keeps.
/// The render worker's copies are made from them one at a time, when it asks (`Begun::take_input`).
struct Pending {
    nonce: u64,
    input: Zeroizing<Box<[u8]>>,
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

// ---- the slot, as a state machine with no JavaScript in it, so it is tested natively ---------

/// Whether a reading is pending.
fn pending() -> bool {
    PENDING.with(|slot| slot.borrow().is_some())
}

/// Hold `pending` under the next nonce, or refuse because a reading is already pending.
fn hold(make: impl FnOnce(u64) -> Pending) -> Result<u64, Error> {
    if pending() {
        return Err(Error::Internal(
            "redact: a second reading is already pending in this worker".to_owned(),
        ));
    }
    let nonce = NEXT_NONCE.with(|next| {
        let nonce = next.get();
        next.set(nonce.saturating_add(1));
        nonce
    });
    PENDING.with(|slot| *slot.borrow_mut() = Some(make(nonce)));
    Ok(nonce)
}

/// Take the pending reading for `nonce`, ONCE. Any other nonce leaves the slot as it was.
fn take_for(nonce: u64) -> Option<Pending> {
    PENDING.with(|slot| {
        let mut slot = slot.borrow_mut();
        match slot.as_ref() {
            Some(held) if held.nonce == nonce => slot.take(),
            _ => None,
        }
    })
}

/// Drop whatever is pending, under any nonce, so its input and output are wiped NOW -- by
/// `Zeroizing` and the wipe-on-free allocator -- rather than left in a heap the recycle only
/// abandons. Every reply that recycles the worker without judging the slot calls this first.
fn discard() {
    drop(PENDING.with(|slot| slot.borrow_mut().take()));
}

/// A copy of the pending input or output for `nonce`, for the render worker.
fn copy_of(nonce: u64, output: bool) -> Vec<u8> {
    PENDING.with(|slot| match slot.borrow().as_ref() {
        Some(held) if held.nonce == nonce && output => held.output.to_vec(),
        Some(held) if held.nonce == nonce => held.input.to_vec(),
        _ => Vec::new(),
    })
}

/// The verdict on `answer` for `pending`, in Rust and over this worker's own bytes.
///
/// THE OPERATION'S DEADLINE ON ARRIVAL, and again after the answer decodes -- which is before the
/// digests `agrees_recorded` takes first and the rules after them (design review L2). The digests
/// themselves are not interrupted: two SHA-256 passes over documents `max_input_bytes` bounds,
/// cooperative like every other step.
fn verdict(pending: &Pending, answer: &[u8], clock: &dyn Clock) -> Result<(), Error> {
    use burrow_core::engines::redact_read_back::recorded::{
        Answer, Failure, agrees_recorded, decode_answer,
    };

    pending.deadline.checkpoint(clock)?;
    let recorded = match decode_answer(answer) {
        Ok(Answer::Read(recorded)) => recorded,
        Ok(Answer::Failed(failure)) => return Err(failure.into_error()),
        // A REPLY THAT DOES NOT DECODE IS NO ANSWER, refused by name.
        Err(_) => return Err(Failure::NoAnswer.into_error()),
    };
    pending.deadline.checkpoint(clock)?;
    agrees_recorded(recorded, &pending.input, &pending.output, &pending.due.0)
}

/// Phase 1's reply: an outcome, and on success the nonce the copies are asked for under.
#[wasm_bindgen]
pub struct Begun {
    reply: Reply,
    nonce: u64,
    remaining_ms: u64,
    input_taken: bool,
    output_taken: bool,
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

    /// A copy of the input for the render worker, made now and handed out once.
    #[wasm_bindgen(js_name = takeInput)]
    pub fn take_input(&mut self) -> Vec<u8> {
        if core::mem::replace(&mut self.input_taken, true) {
            return Vec::new();
        }
        copy_of(self.nonce, false)
    }

    /// A copy of the output for the render worker, made now and handed out once. Not the result.
    #[wasm_bindgen(js_name = takeOutput)]
    pub fn take_output(&mut self) -> Vec<u8> {
        if core::mem::replace(&mut self.output_taken, true) {
            return Vec::new();
        }
        copy_of(self.nonce, true)
    }
}

impl Begun {
    fn refused(reply: Reply) -> Self {
        Self {
            reply,
            nonce: 0,
            remaining_ms: 0,
            input_taken: true,
            output_taken: true,
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
    if pending() {
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
    let done = match burrow_core::ops::redact::page_awaiting_second_reading(
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
    let remaining_ms = limits
        .max_duration_ms
        .saturating_sub(deadline.elapsed_ms(clock.as_ref()));
    let document = done.document;
    match hold(|nonce| Pending {
        nonce,
        input: bytes,
        output: Zeroizing::new(document),
        report,
        due,
        deadline,
        limits,
    }) {
        Ok(nonce) => Begun {
            // NOT RECYCLED: the worker must hold the pending reading until it is judged.
            reply: Reply::success(0),
            nonce,
            remaining_ms,
            input_taken: false,
            output_taken: false,
        },
        Err(error) => refused(&error),
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
/// An answer for a nonce that is not the pending one is refused and **recycles the worker**, as
/// [`redact_abort`] does: the slot is not judged, and the worker holding it is ended. The pending
/// reading is judged once.
#[wasm_bindgen(js_name = redactConfirm)]
#[must_use]
pub fn redact_confirm(nonce: u64, answer: &[u8]) -> Reply {
    let Some(pending) = take_for(nonce) else {
        // RECYCLED, AND EMPTIED FIRST (round 2): the slot is not judged, and what it holds is
        // wiped here rather than when the terminated worker's heap is thrown away unwiped.
        discard();
        return Reply::failure(&Error::InvalidArgument(
            "redact: an answer for no pending second reading".to_owned(),
        ))
        .recycled();
    };
    let limits = pending.limits;
    match verdict(&pending, answer, &WebClock) {
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
    let error = if take_for(nonce).is_some() {
        Error::OutputRejected(
            "redact: the second reading refuses the output -- [second-reading-unavailable] the \
             renderer's reading did not arrive"
                .to_owned(),
        )
    } else {
        // AN ABORT FOR ANOTHER NONCE still ends this worker, so what it holds is wiped first.
        discard();
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

    /// The two-phase slot's state machine, natively: everything but the JavaScript (#206).
    mod slot {
        use std::sync::atomic::{AtomicU64, Ordering};

        use burrow_core::engines::pdfsyntax::geometry::Rect;
        use burrow_core::engines::pdfsyntax::region::{PageFrame, Region};
        use burrow_core::engines::redact::{Report, SecondReadingDue};
        use burrow_core::engines::redact_read_back::Agreement;
        use burrow_core::engines::redact_verify::Walked;
        use burrow_core::{Clock, Deadline, Error, Limits};
        use zeroize::Zeroizing;

        use super::super::{
            Pending, hold, pending, redact_abort, redact_confirm, take_for, verdict,
        };

        /// A clock a test moves by hand.
        struct Hand(AtomicU64);
        impl Clock for Hand {
            fn now_ms(&self) -> u64 {
                self.0.load(Ordering::SeqCst)
            }
        }

        fn limits() -> Limits {
            let mut limits = Limits::default();
            limits.max_duration_ms = 1_000;
            limits
        }

        fn make(clock: &Hand) -> impl FnOnce(u64) -> Pending + '_ {
            move |nonce| Pending {
                nonce,
                input: Zeroizing::new(b"input".to_vec().into_boxed_slice()),
                output: Zeroizing::new(b"output".to_vec()),
                report: Report {
                    fonts: Vec::new(),
                    dropped_carried_text: 0,
                    second_reading: None,
                },
                due: SecondReadingDue(Agreement {
                    page: 0,
                    region: Region {
                        left: 0.0,
                        top: 0.0,
                        width: 10.0,
                        height: 10.0,
                    },
                    walked: Walked {
                        frame: PageFrame {
                            display_box: Rect {
                                left: 0.0,
                                bottom: 0.0,
                                right: 612.0,
                                top: 792.0,
                            },
                            rotate: 0,
                            user_unit: 1.0,
                        },
                        glyphs: 0,
                        marked_work: 0,
                    },
                    promised_pages: 1,
                }),
                deadline: Deadline::start(clock, &limits()),
                limits: limits(),
            }
        }

        /// Empty the slot, so each case starts from a worker with nothing pending.
        fn clear() {
            super::super::PENDING.with(|slot| *slot.borrow_mut() = None);
        }

        #[test]
        fn begin_is_refused_while_a_reading_is_pending() {
            clear();
            let clock = Hand(AtomicU64::new(0));
            let first = hold(make(&clock)).expect("an empty slot holds");
            assert!(matches!(hold(make(&clock)), Err(Error::Internal(_))));
            // AND THE FIRST IS STILL THERE, under its own nonce: the refusal replaced nothing.
            assert!(take_for(first).is_some());
            clear();
        }

        #[test]
        fn an_unknown_nonce_leaves_the_slot_as_it_was() {
            clear();
            let clock = Hand(AtomicU64::new(0));
            let nonce = hold(make(&clock)).expect("holds");
            assert!(take_for(nonce + 1).is_none());
            assert!(take_for(nonce.wrapping_sub(1)).is_none());
            assert!(pending(), "a foreign nonce emptied the slot");
            assert!(take_for(nonce).is_some());
            clear();
        }

        #[test]
        fn a_confirm_or_abort_for_another_nonce_recycles_and_empties_the_slot() {
            // THE STATE MACHINE keeps the slot on a foreign nonce (above); the EXPORTS end the
            // worker, so they empty it first and its bytes are wiped, not abandoned (round 2).
            for refuse in [
                (|n: u64| redact_confirm(n + 1, b"")) as fn(u64) -> crate::Reply,
                |n: u64| redact_abort(n + 1),
            ] {
                clear();
                let clock = Hand(AtomicU64::new(0));
                let nonce = hold(make(&clock)).expect("holds");
                let reply = refuse(nonce);
                assert!(!reply.ok());
                assert!(
                    reply.recycle(),
                    "a refusal for another nonce did not recycle"
                );
                assert!(
                    !pending(),
                    "the slot was left full in a worker being recycled"
                );
            }
        }

        #[test]
        fn a_reading_is_judged_once() {
            clear();
            let clock = Hand(AtomicU64::new(0));
            let nonce = hold(make(&clock)).expect("holds");
            let held = take_for(nonce).expect("taken once");
            assert!(!pending(), "taking the reading left it pending");
            assert!(
                take_for(nonce).is_none(),
                "the same reading was taken twice"
            );
            // A REPLY THAT IS NO ANSWER is refused by name, not passed.
            match verdict(&held, b"not an answer", &clock) {
                Err(Error::OutputRejected(message)) => assert!(message.contains('['), "{message}"),
                other => panic!("expected a named refusal, got {other:?}"),
            }
            clear();
        }

        #[test]
        fn an_abort_empties_the_slot_and_a_second_finds_nothing() {
            clear();
            let clock = Hand(AtomicU64::new(0));
            let nonce = hold(make(&clock)).expect("holds");
            // `redact_abort` is `take_for` and a fixed reply; the reply needs the JS heap, the
            // state change does not.
            assert!(take_for(nonce).is_some());
            assert!(!pending());
            assert!(take_for(nonce).is_none());
            // AND THE NEXT BEGIN IS ADMITTED, under a NEW nonce.
            let next = hold(make(&clock)).expect("an emptied slot holds again");
            assert!(
                next > nonce,
                "the nonce is not monotonic: {next} after {nonce}"
            );
            clear();
        }

        #[test]
        fn a_reading_past_the_deadline_is_refused_on_arrival() {
            clear();
            let clock = Hand(AtomicU64::new(0));
            let nonce = hold(make(&clock)).expect("holds");
            let held = take_for(nonce).expect("taken");
            clock.0.store(1_001, Ordering::SeqCst);
            assert!(matches!(
                verdict(&held, b"", &clock),
                Err(Error::LimitExceeded { .. })
            ));
        }
    }

    #[test]
    fn page_numbers_are_one_based_and_zero_is_refused() {
        assert_eq!(page_index(0), None);
        assert_eq!(page_index(1), Some(0));
        assert_eq!(page_index(u32::MAX), usize::try_from(u32::MAX - 1).ok());
    }
}

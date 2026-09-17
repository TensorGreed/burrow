//! The document operations: everything burrow does with a PDF that produces another PDF,
//! plus the two reads that answer a question about one.
//!
//! # Why this is a module rather than the whole crate
//!
//! This crate is built **twice**, into two wasm artifacts that never meet:
//!
//! | feature | artifact | engine | worker |
//! |---|---|---|---|
//! | `documents` | `burrow_wasm_bg.wasm` | qpdf | `burrow-worker.js` |
//! | `render` | `burrow_wasm_render_bg.wasm` | PDFium | `burrow-render-worker.js` |
//!
//! The two are **mutually exclusive** (`lib.rs` refuses a build that asks for both or
//! neither), and that exclusivity is the whole mechanism behind the base payload's claim
//! that a person who merges two files downloads no PDFium at all. It is not a discipline
//! and it is not a check that has to keep passing: with `render` off, the
//! `__burrow_pdfium_*` imports do not exist in the artifact, so there is nothing to leave
//! behind. See [ADR 0026](../../../docs/adr/0026-how-rendering-loads-without-returning-to-the-old-payload.md).
//!
//! Spike 0004's note in `bridge.rs` is why that matters more than it looks: a `no-modules`
//! wasm-bindgen import resolves from the worker's **global scope**, so an import declared
//! against a module the worker does not load fails at *instantiation* — in the browser and
//! nowhere else. An unused import here is not dead weight, it is a broken build that
//! `cargo build` cannot see.

use std::sync::Arc;

use burrow_core::engines::web::WebQpdf;
use burrow_core::engines::{CheckOptions, OpenOptions, StructureEngine};
use burrow_core::{Clock, Deadline, Error, Limits, Password};
use wasm_bindgen::prelude::wasm_bindgen;

use crate::{Reply, WebClock, WebLimits, bridge_qpdf};

thread_local! {
    /// One engine pair per worker, built on first use.
    ///
    /// **Not constructed per operation.** `WebQpdf` holds the process-global setup — qpdf's
    /// resource limits and the shared discarding logger — behind a `OnceLock`, so a fresh
    /// engine per call would apply the limits repeatedly and, worse, create a logger per
    /// call into a heap that never shrinks.
    ///
    /// `thread_local` rather than a `static`: a wasm worker is one thread, so this is one
    /// instance per worker, which is exactly the lifetime ADR 0006 requirement 1 describes.
    static QPDF: WebQpdf = WebQpdf::new(Arc::new(bridge_qpdf::JsQpdf));
}

pub(crate) fn qpdf() -> WebQpdf {
    QPDF.with(Clone::clone)
}

/// Open a document with **qpdf** and report its page count.
///
/// **The document is opened WITHOUT recovery**, which is part of the contract and not an
/// implementation detail: a damaged file is refused here rather than read optimistically, and
/// there is no parameter through which a caller could ask for anything else. That posture is
/// why this can be qpdf at all — see the comment in the body, and
/// `core/burrow-ops/tests/optimistic_counts.rs` for what pins it.
///
/// It said "with PDFium" until spike 0004 took PDFium out of the web payload, which is the
/// change this function IS. A stale summary line on the one item a change repoints is the
/// doc-comment-as-a-bug case the root `CLAUDE.md` names, and it reached a commit here.
///
/// `bytes` is taken by value: it arrives as a transferred `ArrayBuffer`, so there is no
/// copy from the page, and ownership passing to Rust is what the trait requires anyway.
///
/// `password` is **bytes, not a string**. A PDF password is a byte sequence and need not be
/// valid UTF-8; taking a `String` would corrupt some passwords and make others unusable.
#[wasm_bindgen]
#[must_use]
pub fn page_count(bytes: Box<[u8]>, password: Option<Box<[u8]>>, limits: WebLimits) -> Reply {
    let limits = limits.to_core();
    let clock: Arc<dyn Clock> = Arc::new(WebClock);
    let password = password.map(|p| Password::new(&p));

    // QPDF, NOT PDFIUM, and this is the call that let PDFium leave the web payload at all.
    //
    // It was `pdfium().open(bytes, …).pages_at_open()`, and it was the ONLY place the web
    // build called into PDFium --- 79.7% of the first-load payload reachable from one
    // function (spike 0004). Every other web operation is qpdf, and `WebQpdf` already answers
    // this question: `StructureEngine::check` returns a `StructureReport` carrying `pages`,
    // which is the same call `structure_check` makes.
    //
    // `attempt_recovery` IS FALSE, which is the posture every write path opens under and the
    // one thing that makes this substitution safe rather than merely smaller. Recovery is
    // what produces an optimistic count on a damaged document --- qpdf reads
    // `truncated-mid-object.pdf` as one page with it on and refuses the file with it off ---
    // and `core/burrow-ops/tests/optimistic_counts.rs` pins all of it: the posture is what
    // produces the difference, an optimistic count never reaches a write, and at equal
    // posture qpdf is the STRICTER engine. Stricter is the safe direction: a refusal cannot
    // hand somebody a document quietly short of a page.
    let mut options = CheckOptions::new(limits, clock);
    options.password = password.as_ref();
    options.attempt_recovery = false;

    match qpdf().check(bytes, &options) {
        Ok(report) => Reply::success(report.pages),
        Err(error) => Reply::failure(&error),
    }
    .with_lifecycle(&limits)
}

/// Merge several documents into one, in the order given.
///
/// # The inputs arrive as one flat buffer, not an array of arrays
///
/// `inputs` is every document's bytes end to end, and `lengths` says how long each one is.
/// wasm-bindgen can marshal a `Vec<Vec<u8>>`, and doing so copies each document twice --
/// once into a JS array of `Uint8Array`s and once back out. A merge is the largest thing
/// this boundary carries, and the whole point of the `Blob` discipline on the page
/// (ADR 0015 §4) is that the bytes exist in as few places as possible. One buffer plus a
/// length table is the shape that keeps that true.
///
/// The lengths are validated against the buffer here rather than trusted: they come from
/// JavaScript, and a table that overran would be a read past the end of the input.
///
/// # Passwords are deliberately absent
///
/// The core takes one per input and is tested with them; this entry point does not, because
/// the page has no password UI yet and an API that accepted them would suggest it did.
/// Adding them later is additive.
///
/// # Errors
///
/// Never panics. Everything arrives as a [`Reply`], including a `lengths` table that does
/// not describe `inputs`, which is [`Error::InvalidArgument`] -- a bug in the caller rather
/// than in any document.
#[wasm_bindgen]
#[must_use]
pub fn merge(inputs: Box<[u8]>, lengths: Box<[u32]>, limits: WebLimits) -> Reply {
    let limits = limits.to_core();
    let clock: Arc<dyn Clock> = Arc::new(WebClock);
    let options = OpenOptions::new(limits, clock);

    let mut documents: Vec<burrow_core::ops::Input<'static>> = Vec::with_capacity(lengths.len());
    let mut at: usize = 0;
    for len in &lengths {
        let len = *len as usize;
        // Checked, not trusted. `inputs` and `lengths` cross the boundary separately, so
        // nothing but this stops a table that claims more than the buffer holds.
        let Some(end) = at.checked_add(len) else {
            return Reply::failure(&Error::InvalidArgument(
                "the input lengths overflow".to_owned(),
            ))
            .with_lifecycle(&limits);
        };
        let Some(slice) = inputs.get(at..end) else {
            return Reply::failure(&Error::InvalidArgument(
                "the input lengths do not describe the input buffer".to_owned(),
            ))
            .with_lifecycle(&limits);
        };
        documents.push(burrow_core::ops::Input::new(
            slice.to_vec().into_boxed_slice(),
        ));
        at = end;
    }
    if at != inputs.len() {
        // A table that describes LESS than the buffer is as wrong as one that describes
        // more: it means a document was silently dropped before the operation began, which
        // is the failure ADR 0017 §2 refuses, arriving one layer earlier than it expects.
        return Reply::failure(&Error::InvalidArgument(
            "the input lengths do not account for the whole input buffer".to_owned(),
        ))
        .with_lifecycle(&limits);
    }
    drop(inputs);

    match burrow_core::ops::merge(&qpdf(), documents, &options) {
        Ok(output) => {
            // The page count of what was produced, so a UI does not have to re-open the
            // document to show it. Counted from the lengths table's sum? No -- from the
            // assembler, which is the only thing that knows what actually went in.
            let pages = output_page_count(&output);
            Reply::produced(pages, output)
        }
        Err(error) => Reply::failure(&error),
    }
    .with_lifecycle(&limits)
}

/// Put a document's pages in a different order and return the result.
///
/// `order` is **one-based** and names every page exactly once, because that is what a
/// permutation is and this boundary is where a person's request arrives.
/// [`burrow_core::Permutation`] refuses anything else — the wrong length, a page number of
/// zero, one past the end, or one named twice — **before the document is opened**, so a bad
/// argument costs no parse of an untrusted file.
///
/// # This changes no shape either
///
/// `merge` made [`Reply`] carry bytes and `rotate` needed nothing more; neither does this.
/// One document in, one document out, and the page list crosses as a `Uint32Array` exactly as
/// rotate's does — bounded by `max_pages`, which is 10,000 by default and could not plausibly
/// be raised past `u32`.
///
/// # The output's page count is a cross-check, not a readout
///
/// A permutation cannot change how many pages there are; that is the operation's invariant.
/// Reporting the count of what was actually produced means the page shows a number it
/// measured rather than one it assumed, and a reorder that lost a page shows it.
///
/// # Errors
///
/// Never panics. Everything arrives as a [`Reply`]: `InvalidArgument` for an order that is not
/// a permutation of the document's pages, and the ordinary document errors for an input that
/// cannot be read.
#[wasm_bindgen]
#[must_use]
pub fn reorder(
    bytes: Box<[u8]>,
    order: &[u32],
    password: Option<Box<[u8]>>,
    limits: WebLimits,
) -> Reply {
    let limits = limits.to_core();
    let clock: Arc<dyn Clock> = Arc::new(WebClock);
    let password = password.map(|p| Password::new(&p));

    let mut options = OpenOptions::new(limits, clock);
    options.password = password.as_ref();

    let numbers: Vec<u64> = order.iter().map(|n| u64::from(*n)).collect();

    match burrow_core::ops::reorder(&qpdf(), bytes, &numbers, &options) {
        Ok(output) => {
            let pages = output_page_count(&output);
            Reply::produced(pages, output)
        }
        Err(error) => Reply::failure(&error),
    }
    .with_lifecycle(&limits)
}

/// A split in progress: the source held open, parts pulled one at a time.
///
/// **ADR 0023.** `split` is the first operation whose output is more than one document, and
/// returning them in one `Reply` would hold the whole output in the engine heap and then copy it
/// all again on the way out — resident twice, for a fifty-way split of a large file. The worker
/// pulls one part, posts it, and drops it.
///
/// # Pull, not push
///
/// There is no callback into JS. A callback would be Rust invoking a JS function mid-operation: a
/// new place for an unwind to cross the binding boundary, and a branch on engine state living in
/// JS, which ADR 0009 §2 forbids. The worker drives the loop instead, which is also how
/// `PageAssembler` is shaped and for the same reason.
///
/// # The session owns its password
///
/// `OpenOptions` borrows one, and every call needs the same options — so a session that borrowed
/// them would be self-referential. It keeps the password and builds the options per call, which is
/// what the engine traits do anyway: `extract`, `rotations` and `verify` each take `options`.
#[wasm_bindgen]
pub struct SplitSession {
    /// `None` when `begin` failed, or once every part has been produced.
    inner: Option<burrow_core::ops::Split<WebQpdf>>,
    /// Kept alive for the life of the session; `OpenOptions` borrows it per call.
    password: Option<Password>,
    limits: Limits,
    /// How many parts this split produces. Known before the first one is.
    parts: u32,
    /// The failure that ended this split, if one did.
    ///
    /// **Held so that a call after a failure is that failure again**, rather than the empty
    /// success that means "exhausted". `inner` is cleared on both, so on its own it could not
    /// tell the two apart -- and a caller that asked once more after a failure would have read
    /// the split as having finished normally, which is the one thing ADR 0023 §3 requires it
    /// not to conclude. Found by code review, which also found the counter that used to sit
    /// here: `delivered` was written on every part and read by nothing.
    failed: Option<Reply>,
    /// The failure `begin` reported, if it failed. The session IS the reply for that phase.
    began: Reply,
}

#[wasm_bindgen]
impl SplitSession {
    /// Whether the split could be started at all.
    ///
    /// When this is false the session yields no parts and [`SplitSession::begin_reply`] carries
    /// why — the typed kind, the message, and whether the instance is poisoned.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn ok(&self) -> bool {
        self.began.ok
    }

    /// How many parts this split will produce.
    ///
    /// **Known before the first part is**, which is what lets the worker report "part 1 of 10"
    /// rather than counting as they arrive. Zero when `begin` failed.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn parts(&self) -> u32 {
        self.parts
    }

    /// The outcome of starting the split: the failure, or an empty success.
    ///
    /// A copy, because `wasm_bindgen` moves what it returns into JS and the session outlives
    /// the call: the worker reads this to decide whether to pull any parts at all, and a
    /// `begin_reply` that emptied the session would make asking twice a different answer.
    /// It carries no output, so the copy is three small fields.
    #[must_use]
    pub fn begin_reply(&self) -> Reply {
        self.began.clone()
    }

    /// Produce, verify and return the next part.
    ///
    /// **Each part is verified before it is returned** (ADR 0023 §4), so nothing unverified leaves
    /// the engine heap. `Reply::pages` is that part's page count and `Reply::output` its bytes.
    ///
    /// A reply with `ok == false` means the **whole split has failed**: the caller must discard any
    /// part it already holds and deliver nothing (ADR 0023 §3). `split` is defined as a partition,
    /// and a subset of the parts is not a partition of anything.
    ///
    /// Returns a reply with no output and `pages == 0` once every part has been produced; the
    /// worker stops on the count rather than on that, which is why `parts` is known up front.
    #[must_use]
    pub fn next_part(&mut self) -> Reply {
        let clock: Arc<dyn Clock> = Arc::new(WebClock);
        let mut options = OpenOptions::new(self.limits, clock);
        options.password = self.password.as_ref();

        // A FAILED SPLIT STAYS FAILED. Answering the empty success here would say "exhausted",
        // and the caller's rule for the two is opposite: one delivers the parts it holds, the
        // other discards them.
        if let Some(failed) = &self.failed {
            return failed.clone();
        }

        let Some(session) = self.inner.as_mut() else {
            return Reply::success(0).with_lifecycle(&self.limits);
        };

        match session.next_part(&qpdf(), &options) {
            Ok(Some(output)) => {
                let pages = output_page_count(&output);
                Reply::produced(pages, output).with_lifecycle(&self.limits)
            }
            Ok(None) => {
                // EXHAUSTED. The source is dropped here rather than held until the session is,
                // because the worker posts the last part and then does other things -- and the
                // source is a whole parsed document sitting in a heap with a fixed ceiling.
                self.inner = None;
                Reply::success(0).with_lifecycle(&self.limits)
            }
            Err(error) => {
                // THE SOURCE GOES ON FAILURE TOO, and the session yields nothing further: the
                // split has failed, and holding a parsed document open for a caller that must
                // discard everything anyway is the worst of both.
                self.inner = None;
                let reply = Reply::failure(&error).with_lifecycle(&self.limits);
                self.failed = Some(reply.clone());
                reply
            }
        }
    }
}

/// Begin a split, returning a session the caller pulls parts from.
///
/// Everything that can fail for a reason the caller could have avoided happens here: the cuts are
/// validated against the real page count, the ceilings are applied, and the promise each part will
/// be verified against is computed from the source. See [`SplitSession`].
#[wasm_bindgen]
#[must_use]
pub fn split_begin(
    bytes: Box<[u8]>,
    cuts: &[u32],
    password: Option<Box<[u8]>>,
    limits: WebLimits,
) -> SplitSession {
    let limits = limits.to_core();
    let clock: Arc<dyn Clock> = Arc::new(WebClock);
    let password = password.map(|p| Password::new(&p));

    let mut options = OpenOptions::new(limits, Arc::clone(&clock));
    options.password = password.as_ref();

    let after: Vec<u64> = cuts.iter().map(|n| u64::from(*n)).collect();

    match burrow_core::ops::split_begin(
        &qpdf(),
        bytes,
        burrow_core::ops::Cuts::after_pages(&after),
        &options,
    ) {
        Ok(session) => {
            let parts = u32::try_from(session.parts()).unwrap_or(u32::MAX);
            SplitSession {
                inner: Some(session),
                password,
                limits,
                parts,
                failed: None,
                began: Reply::success(0).with_lifecycle(&limits),
            }
        }
        Err(error) => SplitSession {
            inner: None,
            password,
            limits,
            parts: 0,
            failed: None,
            began: Reply::failure(&error).with_lifecycle(&limits),
        },
    }
}

/// Turn chosen pages of a document and return the result.
///
/// `pages` is **one-based**, because that is how a person names a page and this boundary is
/// where a person's request arrives. `degrees` is any multiple of 90, negative or over 360;
/// [`burrow_core::Rotation`] reduces it and refuses anything else **before the document is
/// opened**, so a bad argument costs no parse of an untrusted file.
///
/// # This changes no shape
///
/// `merge` made [`Reply`] carry bytes, and rotate needs nothing more: one document in, one
/// document out. That is the whole saving of doing merge's bridge work first — the protocol
/// and the reply were the expensive parts and they are already paid for.
///
/// # Why the page list crosses as `u32`
///
/// A page number is bounded by `max_pages`, which is 10,000 by default and could not
/// plausibly be raised past `u32`. The array arrives as a `Uint32Array` from the worker,
/// which is what a JS caller naturally has, and widening here would imply a range the
/// operation cannot accept.
///
/// # Errors
///
/// Never panics. Everything arrives as a [`Reply`]: `InvalidArgument` for a rotation that is
/// not a quarter turn, an empty page list, a page number of zero or one past the end, or a
/// page named twice; and the ordinary document errors for an input that cannot be read.
#[wasm_bindgen]
#[must_use]
pub fn rotate(
    bytes: Box<[u8]>,
    // `&[u32]` rather than `Box<[u32]>`: the list is read and never owned, and wasm-bindgen
    // marshals a `Uint32Array` into a borrowed slice without the extra allocation. `merge`
    // takes its length table by value because it consumes the buffer alongside it.
    pages: &[u32],
    degrees: i32,
    password: Option<Box<[u8]>>,
    limits: WebLimits,
) -> Reply {
    let limits = limits.to_core();
    let clock: Arc<dyn Clock> = Arc::new(WebClock);
    let password = password.map(|p| Password::new(&p));

    let mut options = OpenOptions::new(limits, clock);
    options.password = password.as_ref();

    let numbers: Vec<u64> = pages.iter().map(|n| u64::from(*n)).collect();

    match burrow_core::ops::rotate(
        &qpdf(),
        bytes,
        burrow_core::ops::Pages::numbered(&numbers),
        i64::from(degrees),
        &options,
    ) {
        Ok(output) => {
            // The page count of what was produced. A rotation cannot change it -- that is one
            // of the operation's invariants -- so this is a readout the page can show without
            // re-opening the document, and a cheap cross-check that the invariant held.
            let pages = output_page_count(&output);
            Reply::produced(pages, output)
        }
        Err(error) => Reply::failure(&error),
    }
    .with_lifecycle(&limits)
}

/// Re-encode a document smaller, and say so when it could not be.
///
/// # The reply carries both sizes, and that is the whole shape of this entry point
///
/// `compress` is the only operation whose successful answer can be *"nothing, and here is
/// why"*. Measured: roughly **8.7%** of qpdf's own corpus does not get smaller — 3.4% grows
/// strictly and 5.3% comes out byte-identical (spike 0005) — so this is a common answer rather
/// than an edge case, and it is a **success**, not a failure. Nothing went wrong; the file was
/// already efficiently stored, which is a fact about the file.
///
/// So the reply carries [`Reply::original_bytes`] and [`Reply::produced_bytes`] whichever way
/// it went, and [`Reply::take_output`] is empty when the re-encoding was not kept. A page can
/// report the real result from one message, with no second round trip and no measuring of its
/// own — which matters because the number that decided it is the one burrow measured, not the
/// one the page could measure for itself.
///
/// **No copy of the input crosses this boundary in either direction.** The core returns two
/// counts rather than the caller's bytes ([ADR 0025](../../../docs/adr/0025-what-compress-does-and-what-it-refuses-to-do.md) §3),
/// and the page still holds the `File` it sent (ADR 0015), so a page offering the original
/// needs nothing from here.
///
/// # What it does, and what it will not do
///
/// Lossless, by construction: one storage lever, no image re-encoded, no font subsetted, no
/// content stream's meaning changed. What it is worth varies by two orders of magnitude —
/// 85.6% on a form of many small objects, 0.15% on a scan — and a page quoting a single figure
/// would be averaging across incomparable documents.
///
/// # Errors
///
/// Never panics. Every failure arrives as the typed result in the [`Reply`]: a document that
/// cannot be read, a ceiling reached, or `OutputRejected` if what burrow produced is not what
/// it promised (ADR 0022).
#[wasm_bindgen]
#[must_use]
pub fn compress(bytes: Box<[u8]>, password: Option<Box<[u8]>>, limits: WebLimits) -> Reply {
    let limits = limits.to_core();
    let clock: Arc<dyn Clock> = Arc::new(WebClock);
    let password = password.map(|p| Password::new(&p));

    let mut options = OpenOptions::new(limits, clock);
    options.password = password.as_ref();

    match burrow_core::ops::compress(&qpdf(), bytes, &options) {
        Ok(outcome) => {
            let original = outcome.original_bytes();
            let produced = outcome.produced_bytes();
            match outcome {
                burrow_core::ops::Outcome::Smaller { document, .. } => {
                    // The page count of what was produced. Compression cannot change it --
                    // that is one of the operation's invariants and ADR 0022 already refused
                    // an output where it did -- so this is a readout the page can show without
                    // re-opening the document.
                    let pages = output_page_count(&document);
                    Reply::compared(pages, original, produced, document)
                }
                // NOT A FAILURE, and the reply says so: `ok` is true, there are simply no
                // bytes. A page that treated an empty output as an error would tell somebody
                // their file was broken when it was merely already efficient.
                //
                // The page count is 0 rather than the input's: nothing was produced to count,
                // and reading it back off a document burrow did not return would be reporting
                // a number about bytes the caller never receives from here.
                burrow_core::ops::Outcome::NotSmaller { .. } => {
                    Reply::compared(0, original, produced, Vec::new())
                }
            }
        }
        Err(error) => Reply::failure(&error),
    }
    .with_lifecycle(&limits)
}

/// Every page's effective rotation, in page order.
///
/// # Why this exists
///
/// **For the differential conformance harness**, and it is honest surface rather than a test
/// hook: it opens a document and reports an attribute of it, exactly as [`page_count`] does.
/// Nothing about it is test-only, and a tool page that wanted to show a page's current
/// rotation would use this.
///
/// It exists because a rotate conformance case comparing only a page count would be vacuous
/// -- a rotation cannot change the page count, so an implementation that did nothing would
/// pass. The effective rotation is the nearest `/Rotate` up the page tree, which the native
/// and web paths walk separately on purpose; this is what lets the corpus catch them
/// disagreeing.
///
/// # Errors
///
/// Never panics. A document that cannot be read, or whose `/Rotate` is not an integer
/// multiple of 90, arrives as the typed failure in the [`Reply`].
#[wasm_bindgen]
#[must_use]
pub fn page_rotations(bytes: Box<[u8]>, password: Option<Box<[u8]>>, limits: WebLimits) -> Reply {
    let limits = limits.to_core();
    let clock: Arc<dyn Clock> = Arc::new(WebClock);
    let password = password.map(|p| Password::new(&p));

    let mut options = OpenOptions::new(limits, clock);
    options.password = password.as_ref();

    let engine = qpdf();
    let read =
        burrow_core::engines::PageRotator::open(&engine, bytes, &options).and_then(|source| {
            let pages = burrow_core::engines::PageRotator::pages(&engine, &source)?;

            // THE DEADLINE, CHECKED PER PAGE. Without it this loop is `max_pages` (10,000 by
            // default) inheritance walks of up to 64 ancestors each -- roughly 3.2 M engine
            // calls between the entry point and its return, with `max_duration_ms` never
            // consulted, while `Limits`' own rustdoc promises a check at page boundaries. The
            // same defect the native `rotate` had and fixed, reintroduced at a new entry
            // point; both reviewers found it here independently.
            //
            // This is also the only entry point that drives an engine trait directly rather
            // than through `burrow-ops`, so it does not inherit the checkpoints
            // `burrow_ops::rotate` puts around its engine call.
            let clock: Arc<dyn Clock> = Arc::new(WebClock);
            let deadline = Deadline::start(clock.as_ref(), &limits);

            let mut rotations = Vec::with_capacity(usize::try_from(pages).unwrap_or(0));
            for page in 0..pages {
                deadline.checkpoint(clock.as_ref())?;
                rotations.push(
                    burrow_core::engines::PageRotator::effective_rotation(&engine, &source, page)?
                        .degrees(),
                );
            }
            Ok((pages, rotations))
        });

    match read {
        Ok((pages, rotations)) => Reply::with_rotations(pages, rotations),
        Err(error) => Reply::failure(&error),
    }
    .with_lifecycle(&limits)
}

/// The page count of a document burrow just produced.
///
/// Re-opens the output through the assembler. That is a second parse of bytes we wrote
/// ourselves, which is worth it: the alternative is summing the inputs' page counts, and a
/// sum is a claim about what the engine did rather than a reading of what it produced. If
/// they ever disagree, the reading is the true one.
///
/// A failure here is reported as zero rather than as an error: the merge succeeded, and a
/// page count nobody could read is a display problem, not a reason to throw away a document
/// the person asked for.
fn output_page_count(output: &[u8]) -> u64 {
    let clock: Arc<dyn Clock> = Arc::new(WebClock);
    let options = OpenOptions::new(Limits::DEFAULT, clock);
    let engine = qpdf();
    match burrow_core::engines::PageAssembler::begin(
        &engine,
        output.to_vec().into_boxed_slice(),
        &options,
    ) {
        Ok(assembly) => burrow_core::engines::PageAssembler::pages(&engine, &assembly).unwrap_or(0),
        Err(_) => 0,
    }
}

/// Check a document's structure with qpdf.
#[wasm_bindgen]
#[must_use]
pub fn structure_check(
    bytes: Box<[u8]>,
    password: Option<Box<[u8]>>,
    attempt_recovery: bool,
    limits: WebLimits,
) -> Reply {
    let limits = limits.to_core();
    let clock: Arc<dyn Clock> = Arc::new(WebClock);
    let password = password.map(|p| Password::new(&p));

    let mut options = CheckOptions::new(limits, clock);
    options.password = password.as_ref();
    options.attempt_recovery = attempt_recovery;

    match qpdf().check(bytes, &options) {
        Ok(report) => Reply::success(report.pages),
        Err(error) => Reply::failure(&error),
    }
    .with_lifecycle(&limits)
}

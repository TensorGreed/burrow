//! wasm-bindgen bindings for burrow, for the web app.
//!
//! Everything here runs in the browser tab and touches user file content, so nothing in
//! this crate or below it may make a network call. The one network operation on the web —
//! fetching the three `.wasm` modules and the worker bundle's own source — happens in the
//! worker's JavaScript, before any file exists, under a CSP whose `connect-src` names
//! exactly those four URLs. No Rust in this crate can reach the network: its only
//! dependencies are `burrow-core` and `wasm-bindgen`, and neither `js-sys` nor `web-sys` is
//! in the graph, so there is no binding to `fetch` here to call.
//!
//! # What crosses this boundary
//!
//! Bytes in, and a [`Reply`] out. The reply carries a **verdict**, not a raw error: the
//! classification is done in Rust by [`burrow_core`]'s error taxonomy, and the worker reads
//! a boolean.
//!
//! That matters most for [`Reply::fatal`]. ADR 0009's web contract is that an
//! `Error::Internal` — or any exception out of the module — is fatal to the instance and
//! costs a worker, while `Malformed`, `LimitExceeded` and `PasswordRequired` are normal
//! outcomes that must **not**. Deciding which is which in JavaScript would be error
//! classification in a binding, which ADR 0009 forbids, and it would also mean iOS,
//! Android and the web could drift apart. So `fatal` is computed here, by one Rust
//! function, and the worker does `if (reply.fatal) { terminate(); }`.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

mod bridge;

use std::sync::Arc;

use burrow_core::engines::web::{WebPdfium, WebQpdf};
use burrow_core::engines::{CheckOptions, DocumentEngine, OpenOptions, StructureEngine};
use burrow_core::{Clock, Deadline, Error, Limits, Password};
use wasm_bindgen::prelude::wasm_bindgen;

#[wasm_bindgen]
extern "C" {
    /// `performance.now()` truncated to whole milliseconds.
    ///
    /// The truncation happens in JavaScript so no float crosses into Rust: converting one
    /// needs the casts this workspace denies, and `Clock` wants whole milliseconds anyway.
    /// It reaches Rust as a `u64` (a JS `BigInt`), which is exact.
    #[wasm_bindgen(js_name = __burrow_now_ms)]
    fn now_ms() -> u64;
}

/// The web clock.
///
/// ADR 0007: on `wasm32-unknown-unknown` there is no clock — `Instant::now()` **panics**,
/// and on this target a panic is an uncatchable trap that poisons the instance. So
/// `SystemClock` is `cfg`'d out of existence here and this is the only implementation
/// available, which is the point: there is nothing to reach for by accident.
struct WebClock;

impl Clock for WebClock {
    fn now_ms(&self) -> u64 {
        // Straight through. `performance.now()` is monotonic within a browsing context and
        // measured from its own time origin, which is exactly the "unspecified epoch" the
        // `Clock` contract describes -- and why a `Deadline` must only ever be compared
        // against the clock it was started with.
        now_ms()
    }
}

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
    static PDFIUM: WebPdfium = WebPdfium::new(Arc::new(bridge::JsPdfium));
    static QPDF: WebQpdf = WebQpdf::new(Arc::new(bridge::JsQpdf));
}

fn pdfium() -> WebPdfium {
    PDFIUM.with(Clone::clone)
}

fn qpdf() -> WebQpdf {
    QPDF.with(Clone::clone)
}

/// The outcome of one operation, as the worker sees it.
///
/// Every field is a value Rust decided. The worker forwards them; it does not interpret
/// them.
#[wasm_bindgen]
pub struct Reply {
    ok: bool,
    kind: String,
    fatal: bool,
    message: String,
    pages: u64,
    limit: String,
    stage: String,
    requested: u64,
    allowed: u64,
    recycle: bool,
    pdfium_heap_bytes: u64,
    qpdf_heap_bytes: u64,
    /// What was wrong with the failing input, or empty.
    ///
    /// The `kind` of the wrapped error, so a page can say "that one needs a password"
    /// without unwrapping anything itself -- and without `kind` having to mean two
    /// different things depending on the operation.
    inner_kind: String,
    /// Which input failed, or `-1`.
    ///
    /// So a page can mark the offending file without parsing the message. A UI that had to
    /// read "input 2: ..." out of prose would be parsing an error string, which is the one
    /// thing this boundary is careful never to make anyone do.
    failed_input: i32,
    /// Every page's effective rotation, in page order, or empty.
    ///
    /// **This is a reply shape change, and it was not in the plan.** Rotate itself needed
    /// none -- one document in, one out, and `merge` had already made `Reply` carry bytes.
    /// The differential conformance harness is what needed it: a rotate case that compared
    /// only a page count would pass against an implementation that did nothing, because a
    /// rotation cannot change the page count. The rotations are the readout that tells a real
    /// rotation from a no-op, and comparing them across the two implementations is the one
    /// thing the corpus is for -- the inheritance walk is where native and web would most
    /// plausibly diverge.
    rotations: Vec<i64>,
    /// The document an operation produced, or empty for one that produces none.
    ///
    /// **The first thing a `Reply` carries that is not a scalar.** Held as `Vec<u8>` and
    /// handed over by the getter below, which MOVES it: see `take_output`.
    output: Vec<u8>,
}

#[wasm_bindgen]
impl Reply {
    /// Whether the operation succeeded.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn ok(&self) -> bool {
        self.ok
    }

    /// The error variant's name, or an empty string on success.
    ///
    /// One of `Malformed`, `Unsupported`, `PasswordRequired`, `LimitExceeded`,
    /// `InvalidArgument`, `Io`, `Internal`, or `Unknown` — the last because
    /// [`burrow_core::Error`] is `#[non_exhaustive]` and a future variant must arrive as
    /// something the page can recognise rather than as a panic.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn kind(&self) -> String {
        self.kind.clone()
    }

    /// **Whether this result poisons the engine instance.**
    ///
    /// Computed in Rust, not by the worker comparing `kind`. See the module docs: this is
    /// ADR 0009's web contract, and it has to have one implementation.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn fatal(&self) -> bool {
        self.fatal
    }

    /// The error's `Display` text.
    ///
    /// A fixed constant for every variant **except `LimitExceeded`**, whose `Display`
    /// renders `requested` — and on the pre-scan path that number is computed from the
    /// file's declared cross-reference size, so it is input-derived. It is not secret (it
    /// is a declared size, and the page already holds the file), and the caller needs it to
    /// know which ceiling they hit. Naming the exception here rather than claiming
    /// otherwise.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn message(&self) -> String {
        self.message.clone()
    }

    /// The page count, on success.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn pages(&self) -> u64 {
        self.pages
    }

    /// Which limit was exceeded, for a `LimitExceeded` result.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn limit(&self) -> String {
        self.limit.clone()
    }

    /// **Which check rejected the operation**, for a `LimitExceeded` result.
    ///
    /// Empty for every other outcome. `limit` names the field the caller set; this names the
    /// mechanism that fired, and for `max_memory_bytes` those are three different mechanisms
    /// — the length-based estimate, the structural pre-scan, and the measured post-open check.
    ///
    /// ROADMAP item 12's differential harness compares this between the native and web
    /// implementations, because the same error kind reached by a different route is a
    /// divergence. See [`burrow_core::Stage`].
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn stage(&self) -> String {
        self.stage.clone()
    }

    /// What was requested, for a `LimitExceeded` result.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn requested(&self) -> u64 {
        self.requested
    }

    /// What was allowed, for a `LimitExceeded` result.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn allowed(&self) -> u64 {
        self.allowed
    }

    /// **Whether the page should recycle this worker after delivering the result.**
    ///
    /// Not a failure, and the caller must never see it as one: the reply is delivered, and
    /// only then is the worker terminated and a fresh one spawned.
    ///
    /// Computed in Rust for the same reason [`Reply::fatal`] is. The threshold is derived
    /// from [`burrow_core::Limits::max_memory_bytes`], and ADR 0009 forbids a binding
    /// enforcing any part of `Limits`. See [`burrow_core::engines::web::recycle`].
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn recycle(&self) -> bool {
        self.recycle
    }

    /// The PDFium module's heap size after this operation.
    ///
    /// Reported so the measurement harness can record it and so a page can show it while
    /// debugging. **The decision is [`Reply::recycle`]** — a page comparing this against a
    /// number of its own would be the thing this design exists to prevent.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn pdfium_heap_bytes(&self) -> u64 {
        self.pdfium_heap_bytes
    }

    /// The qpdf module's heap size after this operation. See [`Reply::pdfium_heap_bytes`].
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn qpdf_heap_bytes(&self) -> u64 {
        self.qpdf_heap_bytes
    }
}

/// Whether an error is fatal to the engine instance.
///
/// **The single definition of ADR 0009's web contract.** If a future variant becomes
/// fatal, this one line changes and the page, iOS and Android all follow — which is the
/// whole reason it is not `kind === "Internal"` in JavaScript.
fn is_fatal(error: &Error) -> bool {
    // The NON-fatal variants are listed, and everything else is fatal. That is the right way
    // round for an `#[non_exhaustive]` enum: a variant added upstream defaults to costing a
    // worker, which is the conservative answer for a state we cannot reason about.
    //
    // `Error::Io` IS fatal here, which differs from what the variant means on native. On the
    // web path it is produced in exactly one situation -- an engine `_malloc` returned 0 --
    // and wasm linear memory never shrinks, so that module has reached its `MAXIMUM_MEMORY`
    // ceiling for the rest of the worker's life and every later operation fails identically.
    // Reporting it as recoverable would have the page keep a permanently broken worker
    // rather than respawn into a fresh one, which is precisely the outcome ADR 0009's
    // "poisons the instance" rule exists to avoid.
    //
    // It was `matches!(error, Error::Internal(_))` with the unknown case bolted on at the
    // call site as `|| kind_of(error) == "Unknown"` — so the contract this function claims to
    // be the single definition of was actually decided in two places, one of them by
    // comparing strings.
    // `InputFailed` IS ITS SOURCE. A merge that refused because one document is malformed is
    // as ordinary an outcome as opening that document alone would have been -- and treating
    // it as fatal cost a worker per bad file, so three files a person picked by mistake
    // would latch the circuit breaker and take the page offline (ADR 0015 §3).
    //
    // Found by the conformance harness, not by review: `merge-refuses-when-any-input-is-
    // malformed` failed the "no corpus file may cost a worker" assertion in all three
    // browsers. The wrapper had inherited the `#[non_exhaustive]` default below, which is
    // the right default and the wrong answer here.
    if let Error::InputFailed { .. } = error {
        return is_fatal(innermost(error));
    }

    !matches!(
        error,
        Error::Malformed(_)
            | Error::Unsupported(_)
            | Error::PasswordRequired
            | Error::LimitExceeded { .. }
            | Error::InvalidArgument(_)
    )
}

/// What an [`Error::InputFailed`] is *about*.
///
/// One definition, used by everything that looks through the wrapper. `is_fatal` recursed,
/// `failure` unwrapped one level and `inner_kind` read the source directly — three sites that
/// agreed only because [`burrow_ops::merge`]'s `at()` never re-wraps, and that agreed for
/// three different reasons. That is a drift surface with nothing holding it shut.
fn innermost(error: &Error) -> &Error {
    match error {
        Error::InputFailed { source, .. } => innermost(source),
        other => other,
    }
}

/// The variant's name, as a stable string for the page.
fn kind_of(error: &Error) -> &'static str {
    match error {
        Error::Malformed(_) => "Malformed",
        Error::Unsupported(_) => "Unsupported",
        Error::PasswordRequired => "PasswordRequired",
        Error::LimitExceeded { .. } => "LimitExceeded",
        Error::InvalidArgument(_) => "InvalidArgument",
        // `InputFailed`, NOT the inner kind, and that was the second answer.
        //
        // Reporting the inner kind first looked kinder to a UI -- "needs a password" is what
        // a person has to be told -- and it made the web and native paths disagree about the
        // typed outcome of the same file. The conformance harness caught it in all three
        // browsers: native recorded `InputFailed` and the web recorded `Malformed`. The two
        // implementations must produce the SAME typed outcome; that is the property ADR 0016
        // exists to hold, and at M2 a divergence like this would be a redaction bug.
        //
        // So the wrapper is reported, and what a UI needs comes from `failedInput` and
        // `innerKind` alongside -- structured fields rather than a kind that means two
        // things depending on the operation.
        Error::InputFailed { .. } => "InputFailed",
        Error::Io(_) => "Io",
        Error::Internal(_) => "Internal",
        // `Error` is `#[non_exhaustive]`. A variant added later must arrive as something
        // the page can act on, and the conservative action is the safe one.
        _ => "Unknown",
    }
}

impl Reply {
    fn success(pages: u64) -> Self {
        Self {
            ok: true,
            kind: String::new(),
            fatal: false,
            message: String::new(),
            pages,
            limit: String::new(),
            stage: String::new(),
            requested: 0,
            allowed: 0,
            recycle: false,
            pdfium_heap_bytes: 0,
            qpdf_heap_bytes: 0,
            inner_kind: String::new(),
            failed_input: -1,
            output: Vec::new(),
            rotations: Vec::new(),
        }
    }

    /// A success that carries a document.
    ///
    /// `pages` is still reported, because the page count of a merged document is the one
    /// number a UI wants without re-opening it -- and re-opening it to find out would mean
    /// a second parse of bytes we just wrote.
    fn produced(pages: u64, output: Vec<u8>) -> Self {
        let mut reply = Self::success(pages);
        reply.output = output;
        reply
    }

    /// A success carrying every page's effective rotation.
    fn with_rotations(pages: u64, rotations: Vec<i64>) -> Self {
        let mut reply = Self::success(pages);
        reply.rotations = rotations;
        reply
    }

    /// Attach the worker-lifecycle verdict: both engine heap sizes, and whether the page
    /// should recycle this worker after delivering the result.
    ///
    /// **Both engines are read whichever operation ran.** A worker holds one of each and is
    /// only as healthy as its worse half, so reading only the engine that did the work would
    /// let a qpdf-heavy session grow unbounded behind a run of PDFium operations.
    ///
    /// Applied on the success and the failure path alike. A file that fails is exactly the
    /// kind that grows a heap — `xref-bomb.pdf` returns `LimitExceeded` *after* PDFium has
    /// allocated — so skipping this on failure would miss the case it exists for.
    fn with_lifecycle(mut self, limits: &Limits) -> Self {
        self.pdfium_heap_bytes = pdfium().heap_bytes();
        self.qpdf_heap_bytes = qpdf().heap_bytes();
        self.recycle = burrow_core::engines::web::should_recycle(
            self.pdfium_heap_bytes,
            self.qpdf_heap_bytes,
            limits,
        );
        self
    }

    fn failure(error: &Error) -> Self {
        // THROUGH THE WRAPPER, for the same reason `is_fatal` and `inner_kind` look through
        // it: `InputFailed` says WHICH input, never WHAT. A per-input ceiling arrives here
        // wrapped, so reading only the outer variant dropped the limit name and both numbers
        // -- and the page then rendered its "more than burrow will take on" fallback for a
        // failure it could have quoted exactly. Measured on `/merge-pdf` with 74 files: the
        // core said `max_pages`, 10138 of 10000, and the page said none of it.
        let (limit, stage, requested, allowed) = match innermost(error) {
            Error::LimitExceeded {
                limit,
                stage,
                requested,
                allowed,
            } => (
                (*limit).to_owned(),
                stage.as_str().to_owned(),
                *requested,
                *allowed,
            ),
            _ => (String::new(), String::new(), 0, 0),
        };
        Self {
            ok: false,
            fatal: is_fatal(error),
            kind: kind_of(error).to_owned(),
            message: error.to_string(),
            pages: 0,
            limit,
            stage,
            requested,
            allowed,
            recycle: false,
            pdfium_heap_bytes: 0,
            qpdf_heap_bytes: 0,
            inner_kind: match error {
                Error::InputFailed { .. } => kind_of(innermost(error)).to_owned(),
                _ => String::new(),
            },
            failed_input: match error {
                Error::InputFailed { index, .. } => i32::try_from(*index).unwrap_or(-1),
                _ => -1,
            },
            // A FAILURE CARRIES NO BYTES, ever. ADR 0017 §2 refuses partial success, so
            // there is nothing a failed merge could honestly put here -- and a half-written
            // document reaching the page is exactly the silent data loss that decision
            // exists to prevent.
            output: Vec::new(),
            rotations: Vec::new(),
        }
    }
}

#[wasm_bindgen]
impl Reply {
    /// Take the produced document, leaving the reply empty.
    ///
    /// **It moves rather than copies, and the name says so.** A `&[u8]` getter would make
    /// wasm-bindgen copy the whole document into a fresh `Uint8Array` on every access, and
    /// a merged PDF is the largest thing this boundary ever carries -- reading it twice
    /// would double the peak for no reason. Taking it also means the bytes stop existing on
    /// the Rust side at the moment the worker has them, which is the shorter lifetime and
    /// the right one for file content.
    ///
    /// Returns an empty array for an operation that produces no document, and for a second
    /// call. The worker calls it once, inside `drainReply`.
    #[wasm_bindgen(js_name = takeOutput)]
    #[must_use]
    pub fn take_output(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.output)
    }

    /// Every page's effective rotation, in page order.
    ///
    /// Empty for every operation but [`page_rotations`]. Crosses as a `BigInt64Array`: a
    /// rotation is one of four small numbers, but `/Rotate` is an integer in the file and the
    /// type that reads it is `i64`, so narrowing here would be this boundary inventing a
    /// range the engine does not have.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn rotations(&self) -> Vec<i64> {
        self.rotations.clone()
    }

    /// What was wrong with the failing input, or an empty string.
    #[wasm_bindgen(getter, js_name = innerKind)]
    #[must_use]
    pub fn inner_kind(&self) -> String {
        self.inner_kind.clone()
    }

    /// Which input failed, or `-1` for a failure that is not about a particular input.
    #[wasm_bindgen(getter, js_name = failedInput)]
    #[must_use]
    pub fn failed_input(&self) -> i32 {
        self.failed_input
    }

    /// How many bytes the produced document has, without taking it.
    ///
    /// So a caller can tell "no document" from "a document I have already taken" -- which
    /// `take_output` alone cannot, since both come back empty.
    #[wasm_bindgen(getter, js_name = outputLength)]
    #[must_use]
    pub fn output_length(&self) -> usize {
        self.output.len()
    }
}

/// The caller's limits, as one object the page builds and reuses.
///
/// A struct rather than five loose parameters on every entry point: the operations
/// otherwise took eight arguments each, and a positional list of five same-typed integers
/// is a transposition bug waiting to happen — swapping `max_pages` and `max_pixels` would
/// type-check and silently change what is enforced.
#[wasm_bindgen]
#[derive(Clone, Copy)]
pub struct WebLimits {
    max_input_bytes: u64,
    max_memory_bytes: u64,
    max_duration_ms: u64,
    max_pages: u64,
    max_pixels: u64,
}

#[wasm_bindgen]
impl WebLimits {
    /// Limits with every ceiling given explicitly.
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new(
        max_input_bytes: u64,
        max_memory_bytes: u64,
        max_duration_ms: u64,
        max_pages: u64,
        max_pixels: u64,
    ) -> Self {
        Self {
            max_input_bytes,
            max_memory_bytes,
            max_duration_ms,
            max_pages,
            max_pixels,
        }
    }

    /// The largest accepted input, in bytes.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn max_input_bytes(&self) -> u64 {
        self.max_input_bytes
    }

    /// The peak working memory an operation may use, in bytes.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn max_memory_bytes(&self) -> u64 {
        self.max_memory_bytes
    }

    /// The wall-clock ceiling for one operation, in milliseconds.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn max_duration_ms(&self) -> u64 {
        self.max_duration_ms
    }

    /// The largest accepted page count.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn max_pages(&self) -> u64 {
        self.max_pages
    }

    /// The largest accepted decoded raster, in pixels.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn max_pixels(&self) -> u64 {
        self.max_pixels
    }

    /// The core's defaults, so a page that has no opinion still gets every limit enforced.
    ///
    /// A page that omitted them would otherwise be tempted to pass zeros, which would mean
    /// "nothing is allowed" rather than "no opinion".
    #[wasm_bindgen(js_name = defaults)]
    #[must_use]
    pub fn defaults() -> Self {
        Self::from_core(&Limits::default())
    }

    fn from_core(limits: &Limits) -> Self {
        Self {
            max_input_bytes: limits.max_input_bytes,
            max_memory_bytes: limits.max_memory_bytes,
            max_duration_ms: limits.max_duration_ms,
            max_pages: limits.max_pages,
            max_pixels: limits.max_pixels,
        }
    }
}

impl WebLimits {
    /// The core `Limits` this describes.
    ///
    /// Built through `Limits::with` because `Limits` is `#[non_exhaustive]` — which also
    /// means a field added upstream keeps its default here rather than silently becoming
    /// zero, i.e. a limit of "nothing is allowed".
    fn to_core(self) -> Limits {
        Limits::with(|l| {
            l.max_input_bytes = self.max_input_bytes;
            l.max_memory_bytes = self.max_memory_bytes;
            l.max_duration_ms = self.max_duration_ms;
            l.max_pages = self.max_pages;
            l.max_pixels = self.max_pixels;
        })
    }
}

/// Open a document with PDFium and report its page count.
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

    let mut options = OpenOptions::new(limits, clock);
    options.password = password.as_ref();

    match pdfium().open(bytes, &options) {
        Ok(document) => Reply::success(document.pages_at_open()),
        Err(error) => Reply::failure(&error),
    }
    .with_lifecycle(&limits)
}

/// Whether a set of inputs is small enough in total, **before any of them is read**.
///
/// # Why this exists
///
/// The aggregate `max_input_bytes` check in [`merge`] happens before a single document is
/// opened, which is the right place in the core and is too late on the web. The transport
/// gets there first: the worker reads every `Blob`, copies them into one flat buffer, and
/// wasm-bindgen copies that into linear memory -- roughly four times the payload before Rust
/// is consulted at all. A selection several times over the ceiling can exhaust the tab on the
/// way in, and what the person sees is "something inside burrow failed" rather than the
/// refusal the ceiling exists to give them. Measured and recorded as issue #51.
///
/// So the worker calls this with each `Blob`'s `size` -- a number it already has, without
/// reading anything -- and refuses there if this refuses.
///
/// # This does not move the decision into JavaScript
///
/// [ADR 0009](../../../docs/adr/0009-web-panic-contract-and-binding-boundary.md) §2 forbids a
/// binding enforcing any part of `Limits`, and comparing sizes in the worker would be exactly
/// that. The comparison is [`burrow_core::ops::check_total_input_bytes`], and it is **the same
/// function [`merge`] calls** -- not a mirror of it. The two cannot disagree about the
/// ceiling, because there is only one of them.
///
/// # Sizes cross as `f64`
///
/// A `Blob`'s `size` is a JavaScript number. `f64` is what that is, and converting in Rust
/// means the rejection of a negative, fractional or non-finite one is a typed error rather
/// than a silent `as` cast -- the truncation class `core/CLAUDE.md` denies casts for, arriving
/// from the one direction where the value is entirely caller-controlled.
///
/// # Errors
///
/// Never panics. `LimitExceeded` when the total is over the ceiling, carrying the same limit
/// name, stage and numbers [`merge`] would have produced; `InvalidArgument` for a size that is
/// not a whole non-negative number.
#[wasm_bindgen]
#[must_use]
pub fn check_input_budget(sizes: Box<[f64]>, limits: WebLimits) -> Reply {
    let limits = limits.to_core();
    input_budget_reply(&sizes, &limits).with_lifecycle(&limits)
}

/// The verdict itself, without the lifecycle fields.
///
/// Split out **so it can be tested at all**: `with_lifecycle` reads the engine heaps through
/// wasm-bindgen imports, which panic on a native target, so every native test of the entry
/// point above died at `bridge.rs` before reaching an assertion. Code review found the whole
/// function untested — the web test stubs it out and the conformance case exercises `merge` —
/// and this is the seam that makes the branches reachable from `cargo test`.
fn input_budget_reply(sizes: &[f64], limits: &Limits) -> Reply {
    let mut exact: Vec<u64> = Vec::with_capacity(sizes.len());
    for size in sizes {
        // Rejected rather than rounded. A size that is not a whole non-negative number did
        // not come from a `Blob`, and guessing what was meant is how a ceiling gets skipped.
        let Some(whole) = whole_bytes(*size) else {
            return Reply::failure(&Error::InvalidArgument(
                "an input size is not a whole number of bytes".to_owned(),
            ));
        };
        exact.push(whole);
    }

    match burrow_core::ops::check_total_input_bytes(exact, limits) {
        // Zero pages: nothing was opened, and claiming a page count from a size check would
        // be inventing a number. The caller wants the verdict, not a measurement.
        Ok(_) => Reply::success(0),
        Err(error) => Reply::failure(&error),
    }
}

/// A `f64` as a byte count, or `None` if it is not one.
///
/// **Every condition the `as` below relies on is checked HERE**, rather than by the caller.
/// Security review found the first version stating this contract and not holding it: the
/// guards lived at the call site, so `whole_bytes(f64::NAN)` and `whole_bytes(-5.0)` each
/// returned `Some(0)` -- and zero is the one value that makes a ceiling pass. The comment at
/// the call site even claimed the fallible shape existed "so a future edit to the guard cannot
/// silently truncate", which was exactly backwards: deleting the guard turned a hostile size
/// into a passing one. A function whose invariant depends on being called correctly is not an
/// invariant.
fn whole_bytes(size: f64) -> Option<u64> {
    // NaN and both infinities.
    if !size.is_finite() {
        return None;
    }
    // Negative, or a fraction. `-0.0` passes both and converts to 0, which is the right
    // answer for it rather than a hole: an empty `Blob` is empty.
    if size < 0.0 || size.fract() != 0.0 {
        return None;
    }
    // 2^53 is where an f64 stops representing consecutive integers, so anything above it is
    // not a byte count anyone can rely on -- and it is nine petabytes, far past any ceiling.
    if size > 9_007_199_254_740_992.0 {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "guarded above: finite, non-negative, integral, and below 2^53"
    )]
    Some(size as u64)
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

/// `Limits::DEFAULT`, as the page sees it.
///
/// The conformance harness needs this: `expectations.json` says an omitted `limits` block means
/// `Limits::DEFAULT`, the native side honours that literally, and the web side would otherwise
/// merge the case's overrides onto whatever the *page* happened to default to. A differential
/// harness whose two sides run under different ceilings is not comparing what it says it is.
///
/// Carried from Rust rather than restated in JavaScript, for the same reason
/// [`min_converging_memory_bytes`] is.
#[wasm_bindgen]
#[must_use]
pub fn default_limits() -> WebLimits {
    WebLimits::defaults()
}

/// The smallest `max_memory_bytes` at which worker recycling converges.
///
/// Below this a worker is recycled after **every** operation: the recycling threshold falls
/// under the heap a working engine already occupies, so the first operation on a fresh worker
/// exceeds it and the heap never gets back under the line. Nothing clamps it — silently
/// ignoring a ceiling the caller set would be worse — so a page choosing limits for a
/// constrained device needs the number.
///
/// Exposed rather than copied into the page. It is derived from what the Emscripten modules
/// declare as their initial memory, and a JavaScript literal of the same value would be a
/// second definition free to drift from the one that actually decides.
/// `apps/web/e2e/measure.spec.ts` asserts the measured baselines against it.
#[wasm_bindgen]
#[must_use]
pub fn min_converging_memory_bytes() -> u64 {
    burrow_core::engines::web::MIN_CONVERGING_MEMORY_BYTES
}

/// The version of this build of burrow, for the web app's footer.
#[wasm_bindgen]
#[must_use]
pub fn version() -> String {
    burrow_core::version().to_owned()
}

/// Names of the operations compiled into this wasm module, in stable order.
#[must_use]
pub fn available_operations() -> &'static [&'static str] {
    burrow_core::available_operations()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Only the tests name a `Stage` directly: `Reply::failure` reads one off an error rather
    // than choosing one, which is the whole point -- the binding classifies nothing.
    use burrow_core::Stage;

    #[test]
    fn version_is_reported_to_the_web_app() {
        assert!(!version().is_empty());
    }

    #[test]
    fn operation_list_matches_the_core_build() {
        assert_eq!(available_operations(), burrow_core::available_operations());
    }

    /// ADR 0009's contract, as a table. `Internal` costs a worker; the ordinary outcomes
    /// must not, or one malformed PDF would tear down the engine.
    #[test]
    fn only_internal_errors_are_fatal_to_the_instance() {
        let cases: [(Error, bool); 6] = [
            (Error::Malformed("x".to_owned()), false),
            (Error::Unsupported("x".to_owned()), false),
            (Error::PasswordRequired, false),
            (
                Error::LimitExceeded {
                    limit: "max_pages",
                    stage: Stage::PageCount,
                    requested: 2,
                    allowed: 1,
                },
                false,
            ),
            (Error::InvalidArgument("x".to_owned()), false),
            (Error::Internal("x".to_owned()), true),
        ];
        for (error, expected) in cases {
            assert_eq!(
                Reply::failure(&error).fatal(),
                expected,
                "{} was classified wrongly",
                kind_of(&error)
            );
        }
    }

    /// An error variant this build does not know must default to fatal.
    ///
    /// `Error` is `#[non_exhaustive]`, so this cannot be written with a literal variant —
    /// but the property is checkable through the list `is_fatal` inverts: every variant it
    /// names is non-fatal, and nothing else is. If a future variant is added and someone
    /// adds it to that list without thinking, this test does not help; if they leave it
    /// alone, the default is the safe one.
    #[test]
    fn the_non_fatal_list_is_exactly_the_ordinary_outcomes() {
        // Every variant that must NOT cost a worker. A malformed PDF tearing down the
        // engine would make one bad file poison a session.
        for error in [
            Error::Malformed(String::new()),
            Error::Unsupported(String::new()),
            Error::PasswordRequired,
            Error::LimitExceeded {
                limit: "max_pages",
                stage: Stage::PageCount,
                requested: 1,
                allowed: 0,
            },
            Error::InvalidArgument(String::new()),
        ] {
            assert!(
                !is_fatal(&error),
                "{} must not cost a worker",
                kind_of(&error)
            );
        }

        assert!(is_fatal(&Error::Internal(String::new())));
        // `Io` is fatal on the web specifically, which differs from the variant's meaning on
        // native. Here it is produced in exactly one situation -- an engine `_malloc`
        // returned 0 -- and wasm memory never shrinks, so the module is at its ceiling for
        // the rest of the worker's life and every later operation fails identically. Keeping
        // such a worker is worse than respawning into a fresh one.
        assert!(
            is_fatal(&Error::Io(String::new())),
            "an exhausted engine heap must cost the worker"
        );
    }

    /// A `LimitExceeded` must carry all three numbers, or the page cannot tell the user
    /// what to change.
    #[test]
    fn a_limit_failure_carries_the_limit_and_both_numbers() {
        let reply = Reply::failure(&Error::LimitExceeded {
            limit: "max_input_bytes",
            stage: Stage::InputSize,
            requested: 900,
            allowed: 100,
        });
        assert_eq!(reply.kind(), "LimitExceeded");
        assert_eq!(reply.limit(), "max_input_bytes");
        // The route, not just the ceiling. The differential harness compares it.
        assert_eq!(reply.stage(), "input_size");
        assert_eq!(reply.requested(), 900);
        assert_eq!(reply.allowed(), 100);
        assert!(!reply.fatal());
    }

    /// `Limits` is `#[non_exhaustive]`; a field added upstream must keep its default here
    /// rather than silently becoming zero, which would be a limit of "nothing is allowed".
    #[test]
    fn limits_round_trip_through_the_web_type() {
        let limits = WebLimits::new(1, 2, 3, 4, 5).to_core();
        assert_eq!(limits.max_input_bytes, 1);
        assert_eq!(limits.max_memory_bytes, 2);
        assert_eq!(limits.max_duration_ms, 3);
        assert_eq!(limits.max_pages, 4);
        assert_eq!(limits.max_pixels, 5);
    }

    /// A page with no opinion must get the core's ceilings, not zeros. Zero would mean
    /// "nothing is allowed", so this is the difference between a working page and one that
    /// rejects every file.
    #[test]
    fn the_defaults_are_the_cores_defaults_not_zeros() {
        let defaults = WebLimits::defaults().to_core();
        let core = Limits::default();
        assert_eq!(defaults.max_input_bytes, core.max_input_bytes);
        assert_eq!(defaults.max_memory_bytes, core.max_memory_bytes);
        assert_eq!(defaults.max_duration_ms, core.max_duration_ms);
        assert_eq!(defaults.max_pages, core.max_pages);
        assert_eq!(defaults.max_pixels, core.max_pixels);
        assert!(
            core.max_pages > 0,
            "a zero default would disable the page limit"
        );
    }

    #[test]
    fn a_successful_reply_carries_no_error_state() {
        let reply = Reply::success(42);
        assert!(reply.ok());
        assert!(!reply.fatal());
        assert_eq!(reply.pages(), 42);
        assert!(reply.kind().is_empty());
        assert!(reply.message().is_empty());
    }

    // ---------------------------------------------------------------- check_input_budget

    // Code review: the pre-flight had no Rust test at all. The web test stubs this function
    // out, and the conformance case exercises `merge`, so nothing in CI executed it.

    fn limits(max_input_bytes: u64) -> Limits {
        Limits::with(|l| l.max_input_bytes = max_input_bytes)
    }

    fn budget(sizes: &[f64], max_input_bytes: u64) -> Reply {
        input_budget_reply(sizes, &limits(max_input_bytes))
    }

    #[test]
    fn a_set_within_the_ceiling_is_allowed() {
        let reply = budget(&[1_000.0, 1_000.0], 2_000);
        assert!(reply.ok(), "{}", reply.kind());
        // Nothing was opened, so there is no page count to report and none is invented.
        assert_eq!(reply.pages(), 0);
    }

    #[test]
    fn a_set_over_the_ceiling_is_refused_with_both_numbers() {
        let reply = budget(&[1_000.0, 1_001.0], 2_000);
        assert!(!reply.ok());
        assert_eq!(reply.kind(), "LimitExceeded");
        assert_eq!(reply.limit(), "max_input_bytes");
        assert_eq!(reply.stage(), "input_size");
        assert_eq!(reply.requested(), 2001);
        assert_eq!(reply.allowed(), 2000);
        // A ceiling is an ordinary outcome. Reporting it as fatal would cost a worker, and
        // three would latch the circuit breaker (ADR 0015 §3).
        assert!(
            !reply.fatal(),
            "a ceiling must not poison the engine instance"
        );
    }

    #[test]
    fn exactly_the_ceiling_is_allowed() {
        // The boundary, in the direction that matters: a tool that refused the largest set it
        // documents would be refusing what its own prose promises.
        assert!(budget(&[2_000.0], 2_000).ok());
    }

    #[test]
    fn a_size_that_is_not_a_byte_count_refuses_the_whole_set() {
        for bad in [-1.0, 0.5, f64::NAN, f64::INFINITY, 9_007_199_254_740_994.0] {
            // Paired with a legitimate size, so the case is "one bad entry among good ones"
            // rather than "a single strange argument".
            let reply = budget(&[10.0, bad], u64::MAX);
            assert!(!reply.ok(), "{bad} was accepted");
            assert_eq!(reply.kind(), "InvalidArgument", "{bad}");
        }
    }

    #[test]
    fn an_empty_set_is_allowed_rather_than_refused() {
        // `merge` refuses an empty list separately, with `InvalidArgument`. The pre-flight
        // must not answer a different question: nothing is not too big.
        assert!(budget(&[], 0).ok());
    }

    // ---------------------------------------------------------------- whole_bytes

    // Security review found the first version of this function stating a contract it did not
    // hold: the guards were at the call site, so called directly it turned NaN and -5.0 into
    // `Some(0)`. Zero is the one value that makes a size ceiling pass, so these are the cases
    // that matter, and they are asserted against the function rather than against the caller.

    #[test]
    fn a_size_that_is_not_a_byte_count_is_refused() {
        for size in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            -1.0,
            -5.0,
            1.5,
            f64::MIN_POSITIVE,
            // Exactly representable, and nine petabytes past any ceiling.
            9_007_199_254_740_994.0,
            f64::MAX,
        ] {
            assert_eq!(
                whole_bytes(size),
                None,
                "{size} was accepted as a byte count"
            );
        }
    }

    #[test]
    fn an_ordinary_size_survives_exactly() {
        for size in [0.0, 1.0, 1_388.0, 536_870_912.0, 9_007_199_254_740_992.0] {
            let whole = whole_bytes(size).expect("a whole non-negative size");
            #[expect(
                clippy::cast_precision_loss,
                reason = "the values above are all exactly representable; this compares back"
            )]
            let round_tripped = whole as f64;
            assert!(
                (round_tripped - size).abs() < f64::EPSILON,
                "{size} did not survive the conversion"
            );
        }
    }

    #[test]
    fn negative_zero_is_an_empty_file_rather_than_a_hole() {
        // `-0.0 < 0.0` is false and its fractional part is `-0.0`, so it reaches the
        // conversion. That is correct -- an empty Blob is empty -- and it is asserted rather
        // than left as something a reader has to work out.
        assert_eq!(whole_bytes(-0.0), Some(0));
    }
}

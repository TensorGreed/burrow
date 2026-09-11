//! wasm-bindgen bindings for burrow, for the web app.
//!
//! Everything here runs in the browser tab and touches user file content, so nothing in
//! this crate or below it may make a network call. The one network operation on the web —
//! fetching the two engine `.wasm` files — happens in the worker's JavaScript, before any
//! file exists, under a CSP whose `connect-src` names exactly those two URLs. No Rust in
//! this crate can reach the network at all: it depends on wasm-bindgen and nothing else.
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
use burrow_core::{Clock, Error, Limits, Password};
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
    requested: u64,
    allowed: u64,
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
    // It was `matches!(error, Error::Internal(_))` with the unknown case bolted on at the
    // call site as `|| kind_of(error) == "Unknown"` — so the contract this function claims to
    // be the single definition of was actually decided in two places, one of them by
    // comparing strings.
    !matches!(
        error,
        Error::Malformed(_)
            | Error::Unsupported(_)
            | Error::PasswordRequired
            | Error::LimitExceeded { .. }
            | Error::InvalidArgument(_)
            | Error::Io(_)
    )
}

/// The variant's name, as a stable string for the page.
fn kind_of(error: &Error) -> &'static str {
    match error {
        Error::Malformed(_) => "Malformed",
        Error::Unsupported(_) => "Unsupported",
        Error::PasswordRequired => "PasswordRequired",
        Error::LimitExceeded { .. } => "LimitExceeded",
        Error::InvalidArgument(_) => "InvalidArgument",
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
            requested: 0,
            allowed: 0,
        }
    }

    fn failure(error: &Error) -> Self {
        let (limit, requested, allowed) = match error {
            Error::LimitExceeded {
                limit,
                requested,
                allowed,
            } => ((*limit).to_owned(), *requested, *allowed),
            _ => (String::new(), 0, 0),
        };
        Self {
            ok: false,
            fatal: is_fatal(error),
            kind: kind_of(error).to_owned(),
            message: error.to_string(),
            pages: 0,
            limit,
            requested,
            allowed,
        }
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
                requested: 1,
                allowed: 0,
            },
            Error::InvalidArgument(String::new()),
            Error::Io(String::new()),
        ] {
            assert!(
                !is_fatal(&error),
                "{} must not cost a worker",
                kind_of(&error)
            );
        }
        assert!(is_fatal(&Error::Internal(String::new())));
    }

    /// A `LimitExceeded` must carry all three numbers, or the page cannot tell the user
    /// what to change.
    #[test]
    fn a_limit_failure_carries_the_limit_and_both_numbers() {
        let reply = Reply::failure(&Error::LimitExceeded {
            limit: "max_input_bytes",
            requested: 900,
            allowed: 100,
        });
        assert_eq!(reply.kind(), "LimitExceeded");
        assert_eq!(reply.limit(), "max_input_bytes");
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
}

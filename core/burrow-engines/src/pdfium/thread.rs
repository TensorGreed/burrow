//! The one thread every PDFium call runs on.
//!
//! # Why a thread, and not a lock
//!
//! **PDFium is not thread-safe anywhere, not just at init.** M1 PR 1 discovered the init
//! half the hard way: two tests calling `FPDF_InitLibrary` on separate harness threads
//! tripped an internal `CHECK` and the process died with `SIGTRAP`. A `Once` fixes that
//! one call and nothing else — `cargo test` runs tests on parallel threads, so concurrent
//! `FPDF_*` calls are still undefined behaviour.
//!
//! A process-wide mutex would serialise them. A dedicated thread is used instead, for
//! reasons a mutex does not give (recorded in full in
//! `docs/adr/0011-pdfium-engine-thread.md`):
//!
//! - A mutex provides mutual exclusion; PDFium also keeps **thread-local** state. One
//!   thread provides both properties. The difference is undefined behaviour that no test
//!   would reliably show.
//! - **No `unsafe impl Send` anywhere.** `FpdfDocument` is a raw pointer and is not
//!   `Send`; under a mutex, handing a document between threads needs an `unsafe impl` a
//!   reviewer has to take on trust. Here the pointer never leaves [`Registry`], the
//!   public handle is a `u64`, and `Send + Sync` is *derived*. The compiler enforces the
//!   discipline instead of a comment.
//! - `FPDF_InitLibrary` gets exactly one caller on exactly one thread, structurally.
//! - It is the same shape as the web implementation M1 PR 4 will write over a JS bridge —
//!   submit a request, await a reply, hold an opaque handle — so the two implementations
//!   of one trait read alike, which is what the differential conformance harness depends
//!   on.
//!
//! # What one thread costs, beyond throughput
//!
//! It is an **availability coupling between unrelated documents**, which is easy to miss
//! if the single thread is read as only a throughput ceiling. [`submit`] blocks on a
//! reply, so a hostile file that makes one engine call run for seconds delays every other
//! caller for that whole time — including callers whose own `max_duration_ms` is far
//! smaller. Their deadline then fires on time consumed by somebody else's file, which is a
//! wrong answer and not merely a slow one. ADR 0011 records this; the fix — a bounded wait
//! that distinguishes "the queue was busy" from "your own work was slow" — is not in
//! M1 PR 2.
//!
//! # Poisoning
//!
//! A panic caught inside a job means the engine's invariants may be broken. The engine is
//! then **permanently** poisoned: the loop stops, every queued and future call fails with
//! [`Error::Internal`](burrow_types::Error::Internal), and it is never used again. That is
//! the same rule ADR 0009 states for the web, applied here — an instance that trapped is
//! not reused.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{OnceLock, mpsc};

use burrow_types::{Error, Result};

use super::ffi::{self, FpdfDocument};

/// Set once anything makes the engine untrustworthy. Never cleared.
static POISONED: AtomicBool = AtomicBool::new(false);

/// `FPDF_GetLastError()` sampled immediately after `FPDF_InitLibrary`.
///
/// `u64::MAX` until the engine thread has run its init, which is not a valid PDFium code.
static INIT_ERROR: AtomicU64 = AtomicU64::new(u64::MAX);

/// The engine, started on first use and never stopped.
static ENGINE: OnceLock<Engine> = OnceLock::new();

/// A unit of work for the engine thread.
///
/// A boxed closure rather than a command enum: the protocol would otherwise grow a
/// variant per engine call, and every operation in M1 adds calls.
type Job = Box<dyn FnOnce(&mut Registry) + Send + 'static>;

struct Engine {
    tx: mpsc::Sender<Job>,
}

/// One entry in the registry: the input buffer, and the document reading from it.
///
/// Holds a raw pointer, so it is `!Send`: the compiler guarantees it can only ever exist
/// inside the engine thread's [`Registry`]. That is the whole safety argument for the
/// handle discipline, and it is checked rather than asserted.
///
/// The buffer is parked here **before** the document is loaded, and `doc` is filled in
/// afterwards. That ordering is deliberate: the pointer handed to PDFium is read from the
/// buffer's final home, so nothing moves the `Box` — not even by value into another
/// function — while PDFium holds a pointer into it.
struct Loaded {
    /// `None` between parking the buffer and a successful load.
    doc: Option<FpdfDocument>,
    /// PDFium reads from this buffer for as long as the document is open
    /// (`fpdfview.h:451`). Never read from Rust after the load — its job is to exist.
    bytes: Box<[u8]>,
}

impl Drop for Loaded {
    fn drop(&mut self) {
        // `Drop::drop` runs before the struct's fields are dropped, so the document is
        // always closed before the buffer it reads from is freed. Declaration order does
        // not have to be trusted, and neither does any call site. An entry that was parked
        // but never loaded has no document and simply frees its buffer.
        if let Some(doc) = self.doc.take() {
            // SAFETY: `doc` was returned non-null by `ffi::load_mem_document64` and stored
            // by `Registry::attach`; `take` makes this the only close. `Loaded` is `!Send`
            // and lives only in the engine thread's registry, so this runs on the engine
            // thread. The buffer is still alive: it is a field of `self`.
            unsafe { ffi::close_document(doc) }
        }
    }
}

/// Every document the engine currently has open, keyed by the id callers hold.
pub(super) struct Registry {
    next_id: u64,
    docs: HashMap<u64, Loaded>,
}

impl Registry {
    fn new() -> Self {
        Self {
            next_id: 1,
            docs: HashMap::new(),
        }
    }

    /// Claim the next document id.
    ///
    /// Called before anything is loaded, so the only fallible step happens while there is
    /// still nothing to clean up.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`](burrow_types::Error::Internal) if the id space is exhausted.
    pub(super) fn reserve_id(&mut self) -> Result<u64> {
        let id = self.next_id;
        // `checked_add` rather than a wrap: a reused id would alias two documents. At one
        // open per nanosecond this still takes 584 years to reach.
        self.next_id = id
            .checked_add(1)
            .ok_or_else(|| Error::Internal("pdfium document ids exhausted".to_owned()))?;
        Ok(id)
    }

    /// Take ownership of the input buffer and return a pointer to it **in its final home**.
    ///
    /// The returned pointer stays valid until the entry is removed: the `Box` is never
    /// moved again, and growing the `HashMap` relocates the `Loaded` struct — the fat
    /// pointer — not the heap block it addresses.
    ///
    /// Returning the pointer from here, rather than taking it from a local and moving the
    /// `Box` in afterwards, is the point. A `Box` passed **by value** into a function is
    /// `noalias` to the compiler, which asserts nothing else touches that allocation for
    /// the duration of the call — and PDFium would be holding a pointer into it. Parking
    /// first means Rust never hands that ownership around again.
    pub(super) fn park_buffer(&mut self, id: u64, bytes: Box<[u8]>) -> (*const u8, usize) {
        let entry = self.docs.entry(id).or_insert(Loaded { doc: None, bytes });
        (entry.bytes.as_ptr(), entry.bytes.len())
    }

    /// Record the document loaded from `id`'s parked buffer.
    ///
    /// # Safety
    ///
    /// `doc` must be a non-null handle from [`ffi::load_mem_document64`], loaded from the
    /// buffer parked under `id`, and not yet closed.
    pub(super) unsafe fn attach(&mut self, id: u64, doc: FpdfDocument) {
        if let Some(entry) = self.docs.get_mut(&id) {
            entry.doc = Some(doc);
        }
    }

    /// The raw handle for `id`.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`](burrow_types::Error::Internal) if the id is not registered or
    /// carries no document, which can only mean a use-after-close inside this crate.
    pub(super) fn handle(&self, id: u64) -> Result<FpdfDocument> {
        self.docs
            .get(&id)
            .and_then(|loaded| loaded.doc)
            .ok_or_else(|| Error::Internal("pdfium document handle is not open".to_owned()))
    }

    /// Close and forget `id`, freeing its buffer. Unknown ids are ignored.
    pub(super) fn remove(&mut self, id: u64) {
        drop(self.docs.remove(&id));
    }
}

/// Start the engine thread if it is not already running.
fn engine() -> &'static Engine {
    ENGINE.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();

        let spawned = std::thread::Builder::new()
            .name("burrow-pdfium".to_owned())
            .spawn(move || {
                // SAFETY: this is the only call to `ffi::init_library` in the process, it
                // runs before any other PDFium call because every other call arrives as a
                // job on the queue below, and `OnceLock::get_or_init` runs this spawn at
                // most once. That discharges `init_library`'s "exactly once, on the engine
                // thread, first" precondition.
                let init_code = unsafe { ffi::init_library() };
                // `c_ulong` is already `u64` on the 64-bit Linux targets this module is
                // gated to, which is why clippy calls the conversion useless -- but it is
                // `u32` elsewhere, and a cast would be the silent-truncation class the
                // workspace denies as a whole.
                #[allow(
                    clippy::useless_conversion,
                    reason = "c_ulong is u32 on some targets; a cast here would truncate"
                )]
                INIT_ERROR.store(u64::from(init_code), Ordering::SeqCst);
                if init_code != ffi::FPDF_ERR_SUCCESS {
                    // The library loaded but says its own init failed. Nothing it reports
                    // afterwards can be trusted, so refuse work rather than serve it.
                    POISONED.store(true, Ordering::SeqCst);
                    return;
                }

                let mut registry = Registry::new();
                for job in rx {
                    // A panic here would otherwise unwind out of the thread, drop the
                    // receiver, and strand every future caller on a send error with no
                    // explanation. Catching it lets the caller get a typed `Internal`.
                    //
                    // The payload is deliberately dropped without being formatted: panic
                    // messages can carry input-derived bytes, which is the same reason
                    // `burrow-ffi::guard` discards it.
                    if catch_unwind(AssertUnwindSafe(|| job(&mut registry))).is_err() {
                        POISONED.store(true, Ordering::SeqCst);
                        // STOP, rather than continue to the next job. A job already on the
                        // queue would otherwise run `FPDF_*` calls against an engine whose
                        // invariants we have just declared broken, and `submit`'s flag
                        // check cannot catch a job enqueued before the flag was set.
                        // Breaking drops `rx`, so every stranded caller gets the receive
                        // error that maps to `poisoned_error()`. This is what makes ADR
                        // 0011's "never used again" structural rather than racy.
                        break;
                    }
                }
                // Dropping `registry` closes every document still open, on this thread --
                // the only thread allowed to close them.
            });

        if spawned.is_err() {
            // `rx` was moved into the closure, which was returned in the error and
            // dropped, so every `send` below will fail. Flagging it here just makes the
            // first caller's error immediate and accurate rather than incidental.
            POISONED.store(true, Ordering::SeqCst);
        }

        Engine { tx }
    })
}

/// The error returned for every call once the engine is poisoned.
///
/// Content-free by construction: it describes our state, never the input that reached it.
fn poisoned_error() -> Error {
    Error::Internal("the pdfium engine is no longer usable and was not reused".to_owned())
}

/// Run `f` on the engine thread and wait for its result.
///
/// # Errors
///
/// Whatever `f` returns, or [`Error::Internal`](burrow_types::Error::Internal) if the
/// engine is poisoned, cannot be reached, or panicked while running `f`.
pub(super) fn submit<T, F>(f: F) -> Result<T>
where
    F: FnOnce(&mut Registry) -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    if POISONED.load(Ordering::SeqCst) {
        return Err(poisoned_error());
    }

    let (reply_tx, reply_rx) = mpsc::channel::<Result<T>>();
    let job: Job = Box::new(move |registry| {
        // If the receiver has gone (the caller was cancelled, or unwound), the send fails
        // and the result is dropped. That is fine; nothing is waiting for it.
        let _ = reply_tx.send(f(registry));
    });

    if engine().tx.send(job).is_err() {
        POISONED.store(true, Ordering::SeqCst);
        return Err(poisoned_error());
    }

    match reply_rx.recv() {
        Ok(result) => result,
        // The job's `reply_tx` was dropped without sending: the job panicked and the
        // engine thread's `catch_unwind` swallowed it, or the thread is gone. Either way
        // the engine is not trustworthy.
        Err(_) => {
            POISONED.store(true, Ordering::SeqCst);
            Err(poisoned_error())
        }
    }
}

/// Run `f` on the engine thread without waiting for it.
///
/// For work whose result nobody can act on — closing a document as its handle is dropped.
/// Not waiting also means a `Drop` can never block, and can never deadlock by waiting on
/// the thread it is already running on.
fn submit_detached<F>(f: F)
where
    F: FnOnce(&mut Registry) + Send + 'static,
{
    if POISONED.load(Ordering::SeqCst) {
        return;
    }
    let _ = engine().tx.send(Box::new(f));
}

/// Close the document `id` refers to, if it is still open.
pub(super) fn close(id: u64) {
    submit_detached(move |registry| registry.remove(id));
}

/// Start the engine and return `FPDF_GetLastError()` as sampled right after init.
///
/// Waits for the engine thread to have finished initialising, by round-tripping a job
/// through it — jobs are only served after init.
///
/// Only the link check asks this; ordinary callers start the engine implicitly by using
/// it, and have no business seeing a raw PDFium code.
///
/// # Errors
///
/// [`Error::Internal`](burrow_types::Error::Internal) if the engine could not be started
/// **and** never got as far as reporting an init code.
#[cfg(test)]
pub(crate) fn ensure_started() -> Result<u64> {
    match submit(|_| Ok(())) {
        Ok(()) => Ok(INIT_ERROR.load(Ordering::SeqCst)),
        Err(e) => {
            // A failed init poisons the engine, so `submit` refuses before it can report
            // anything -- and the init code is the whole point of this function. Prefer it
            // over the generic error whenever init actually ran, so a caller is told *how*
            // PDFium failed rather than only that it did.
            let code = INIT_ERROR.load(Ordering::SeqCst);
            if code == u64::MAX { Err(e) } else { Ok(code) }
        }
    }
}

/// Whether the engine has been permanently poisoned.
#[cfg(test)]
pub(super) fn is_poisoned() -> bool {
    POISONED.load(Ordering::SeqCst)
}

//! A fake pair of engine modules, for testing the web orchestration without a browser.
//!
//! This is the payoff for [`super`] being compiled on every target. It models the two
//! things about a real Emscripten module that the orchestration has to get right —
//! **a heap that must be allocated into and freed from**, and **engine calls that can fail
//! in engine-specific ways** — and records every call, so a test can assert not just the
//! outcome but the *order of operations* that produced it.
//!
//! What it deliberately does **not** model is PDF parsing. The scripted outcomes are the
//! point: a fake that opened real documents would only tell us PDFium works, which the
//! native tests already cover. What is untested without this is the orchestration — that
//! `max_pages` is checked after the load and before the document escapes, that a failed
//! page count still frees the buffer, that a password is wiped from the engine heap.
//!
//! # The leak detector
//!
//! [`FakeHeap`] tracks live allocations. A test that finishes with any allocation
//! outstanding has found a real bug: on the web there is no `Drop` on the other side of the
//! bridge and no allocator to notice, so a leaked pointer is simply a worker that grows
//! until it is recycled. [`FakeHeap::assert_empty`] is what turns that into a failure.

use std::sync::{Arc, Mutex};

use super::bridge::{LoadOutcome, PdfiumBridge, PdfiumPtr, QpdfBridge, QpdfPtr};

/// One recorded bridge call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Call {
    CopyIn(usize),
    WipeAndFree(u32, u32),
    AbandonInput(u32),
    Load,
    PageCount,
    CloseDocument(u32, u32),
    Init,
    Cleanup,
    SilenceErrors,
    SuppressWarnings(bool),
    SetLogger,
    AttemptRecovery(bool),
    ReadMemory,
    NumPages,
    Free(u32),
}

/// A stand-in for an Emscripten module's linear memory.
///
/// Addresses start above zero so that null is distinguishable, and are never reused, so a
/// use-after-free shows up as an unknown address rather than as a silent success.
#[derive(Debug, Default)]
struct Heap {
    next: u32,
    live: Vec<(u32, Vec<u8>)>,
    bytes_grown: u64,
    /// When set, the next `copy_in` returns null — an allocation failure.
    fail_next_alloc: bool,
}

impl Heap {
    fn alloc(&mut self, bytes: &[u8]) -> u32 {
        if self.fail_next_alloc {
            self.fail_next_alloc = false;
            return 0;
        }
        // Start at a non-zero, non-trivial address: a test that accidentally treats an
        // offset as a pointer should not land on something plausible.
        self.next = if self.next == 0 {
            0x1_0000
        } else {
            self.next + 0x1_0000
        };
        let at = self.next;
        self.live.push((at, bytes.to_vec()));
        // A real Emscripten heap grows in pages and never shrinks. Model the growth, since
        // the measured-memory check reads exactly this.
        self.bytes_grown = self
            .bytes_grown
            .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        at
    }

    fn free(&mut self, ptr: u32) {
        let before = self.live.len();
        self.live.retain(|(at, _)| *at != ptr);
        assert!(
            ptr == 0 || before != self.live.len(),
            "freed {ptr:#x}, which was not a live allocation -- a double free or a stray pointer"
        );
    }
}

/// Shared state behind both fake bridges.
#[derive(Debug, Default)]
pub(super) struct FakeHeap {
    heap: Mutex<Heap>,
    calls: Mutex<Vec<Call>>,
    wiped: Mutex<Option<Vec<u8>>>,
}

impl FakeHeap {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Every call made so far, in order.
    pub(super) fn calls(&self) -> Vec<Call> {
        self.calls.lock().expect("not poisoned").clone()
    }

    fn record(&self, call: Call) {
        self.calls.lock().expect("not poisoned").push(call);
    }

    /// Assert nothing is still allocated in the engine heap.
    ///
    /// The whole reason the fake tracks allocations. A leak here is a leak in the browser,
    /// where nothing would report it.
    pub(super) fn assert_empty(&self) {
        let heap = self.heap.lock().expect("not poisoned");
        assert!(
            heap.live.is_empty(),
            "{} allocation(s) still live in the engine heap: {:?}",
            heap.live.len(),
            heap.live
                .iter()
                .map(|(at, b)| (*at, b.len()))
                .collect::<Vec<_>>()
        );
    }

    /// What the last wiped buffer contained at the moment it was freed.
    ///
    /// Recorded because `wipe_and_free` frees as well as wipes, so there is nothing left to
    /// inspect afterwards — and "we called the wipe" is a weaker claim than "the bytes were
    /// zero when it was released".
    pub(super) fn last_wiped_contents(&self) -> Option<Vec<u8>> {
        self.wiped.lock().expect("not poisoned").clone()
    }

    /// Make the next allocation fail.
    pub(super) fn fail_next_alloc(&self) {
        self.heap.lock().expect("not poisoned").fail_next_alloc = true;
    }
}

/// What the fake PDFium should do when asked.
#[derive(Debug, Clone, Copy)]
pub(super) struct PdfiumScript {
    /// `FPDF_GetLastError` value to report alongside a failed load.
    pub(super) load_error_code: core::ffi::c_ulong,
    /// Whether the load succeeds.
    pub(super) load_succeeds: bool,
    /// What `FPDF_GetPageCount` returns. Negative means the page tree is unreadable.
    pub(super) page_count: i32,
    /// Extra heap growth attributed to the load, in bytes — a declared-size bomb's cost.
    pub(super) load_grows_heap_by: u64,
}

impl Default for PdfiumScript {
    fn default() -> Self {
        Self {
            load_error_code: 0,
            load_succeeds: true,
            page_count: 1,
            load_grows_heap_by: 0,
        }
    }
}

/// A fake PDFium Emscripten module.
#[derive(Debug)]
pub(super) struct FakePdfium {
    pub(super) state: Arc<FakeHeap>,
    script: PdfiumScript,
    /// Handed out as the document handle. Not an allocation: PDFium's handle is opaque and
    /// is released by `FPDF_CloseDocument`, not by `free`.
    doc_handle: u32,
}

impl FakePdfium {
    pub(super) fn new(state: Arc<FakeHeap>, script: PdfiumScript) -> Self {
        Self {
            state,
            script,
            doc_handle: 0xD0C0_0001,
        }
    }
}

impl PdfiumBridge for FakePdfium {
    fn copy_in(&self, bytes: &[u8]) -> PdfiumPtr {
        self.state.record(Call::CopyIn(bytes.len()));
        PdfiumPtr(self.state.heap.lock().expect("not poisoned").alloc(bytes))
    }

    fn wipe_and_free(&self, ptr: PdfiumPtr, len: u32) {
        self.state.record(Call::WipeAndFree(ptr.0, len));
        let mut heap = self.state.heap.lock().expect("not poisoned");
        // Model the wipe as a real one, so a test can observe that it happened.
        if let Some((_, bytes)) = heap.live.iter_mut().find(|(at, _)| *at == ptr.0) {
            bytes.fill(0);
            *self.state.wiped.lock().expect("not poisoned") = Some(bytes.clone());
        }
        heap.free(ptr.0);
    }

    fn abandon_input(&self, ptr: PdfiumPtr) {
        self.state.record(Call::AbandonInput(ptr.0));
        self.state.heap.lock().expect("not poisoned").free(ptr.0);
    }

    fn load_mem_document64(
        &self,
        _data: PdfiumPtr,
        _len: u32,
        _password: PdfiumPtr,
    ) -> LoadOutcome {
        self.state.record(Call::Load);
        {
            let mut heap = self.state.heap.lock().expect("not poisoned");
            heap.bytes_grown = heap
                .bytes_grown
                .saturating_add(self.script.load_grows_heap_by);
        }
        if self.script.load_succeeds {
            LoadOutcome {
                handle: PdfiumPtr(self.doc_handle),
                code: 0,
            }
        } else {
            LoadOutcome {
                handle: PdfiumPtr::NULL,
                code: self.script.load_error_code,
            }
        }
    }

    fn get_page_count(&self, _doc: PdfiumPtr) -> i32 {
        self.state.record(Call::PageCount);
        self.script.page_count
    }

    fn close_document(&self, doc: PdfiumPtr, data: PdfiumPtr) {
        // Recorded as one call, which is the point: the ordering is not expressible at a
        // call site, so a test asserting "close before free" is asserting about this.
        self.state.record(Call::CloseDocument(doc.0, data.0));
        self.state.heap.lock().expect("not poisoned").free(data.0);
    }

    fn heap_bytes(&self) -> u64 {
        self.state.heap.lock().expect("not poisoned").bytes_grown
    }
}

/// What the fake qpdf should do when asked.
#[derive(Debug, Clone, Copy)]
pub(super) struct QpdfScript {
    /// The raw `QPDF_ERROR_CODE` bitmask `qpdf_read_memory` returns.
    pub(super) read_status: i32,
    /// A pending error code, or `None` for "no error in the slot".
    pub(super) pending_error: Option<i32>,
    /// What `qpdf_get_num_pages` returns.
    pub(super) page_count: i32,
    /// Whether `qpdf_init` succeeds.
    pub(super) init_succeeds: bool,
    /// An error that appears in the slot *again*, once, after the first drain.
    ///
    /// Models qpdf recording a further problem after the orchestration's last
    /// `take_error` — which is the only way the slot can still be occupied when `Drop`
    /// runs, and therefore the only way to exercise the drain loop there. Re-arming once
    /// rather than forever also pins that the loop terminates.
    pub(super) error_reappears_once: Option<i32>,
}

impl Default for QpdfScript {
    fn default() -> Self {
        Self {
            read_status: 0,
            pending_error: None,
            page_count: 1,
            init_succeeds: true,
            error_reappears_once: None,
        }
    }
}

/// A fake qpdf Emscripten module.
#[derive(Debug)]
pub(super) struct FakeQpdf {
    pub(super) state: Arc<FakeHeap>,
    script: QpdfScript,
    /// Drained by `get_error`, exactly as qpdf's slot is.
    pending: Mutex<Option<i32>>,
    /// Whether the one-shot re-arm above is still available.
    rearm: Mutex<Option<i32>>,
}

impl FakeQpdf {
    pub(super) fn new(state: Arc<FakeHeap>, script: QpdfScript) -> Self {
        Self {
            state,
            script,
            pending: Mutex::new(script.pending_error),
            rearm: Mutex::new(script.error_reappears_once),
        }
    }
}

impl QpdfBridge for FakeQpdf {
    fn copy_in(&self, bytes: &[u8]) -> QpdfPtr {
        self.state.record(Call::CopyIn(bytes.len()));
        QpdfPtr(self.state.heap.lock().expect("not poisoned").alloc(bytes))
    }

    fn free(&self, ptr: QpdfPtr) {
        self.state.record(Call::Free(ptr.0));
        self.state.heap.lock().expect("not poisoned").free(ptr.0);
    }

    fn wipe_and_free(&self, ptr: QpdfPtr, len: u32) {
        self.state.record(Call::WipeAndFree(ptr.0, len));
        let mut heap = self.state.heap.lock().expect("not poisoned");
        if let Some((_, bytes)) = heap.live.iter_mut().find(|(at, _)| *at == ptr.0) {
            bytes.fill(0);
            *self.state.wiped.lock().expect("not poisoned") = Some(bytes.clone());
        }
        heap.free(ptr.0);
    }

    fn init(&self) -> QpdfPtr {
        self.state.record(Call::Init);
        if self.script.init_succeeds {
            QpdfPtr(0x0DF0_0001)
        } else {
            QpdfPtr::NULL
        }
    }

    fn cleanup(&self, _data: QpdfPtr) {
        // Real qpdf does not fail here -- it writes
        //
        //   WARNING: application did not handle error: <text>
        //
        // to `QPDFLogger::defaultLogger()` (`qpdf-c.cc:109-120`), bypassing both the
        // document's discarding logger and `qpdf_silence_errors`, carrying byte offsets
        // from the user's file. In a browser that reaches the devtools console.
        //
        // Silently emitting it here would make the fake model the bug rather than catch
        // it, so it panics instead: the invariant is then enforced by every qpdf test that
        // exists, not asserted by one that happens to remember.
        assert!(
            self.pending.lock().expect("not poisoned").is_none(),
            "qpdf_cleanup with an error still in the slot: qpdf would write \
             'application did not handle error: <text>' -- carrying file offsets -- to the \
             default logger, which the per-document logger does not cover"
        );
        self.state.record(Call::Cleanup);
    }

    fn silence_errors(&self, _data: QpdfPtr) {
        self.state.record(Call::SilenceErrors);
    }

    fn set_suppress_warnings(&self, _data: QpdfPtr, value: bool) {
        self.state.record(Call::SuppressWarnings(value));
    }

    fn set_logger(&self, _data: QpdfPtr, _logger: QpdfPtr) {
        self.state.record(Call::SetLogger);
    }

    fn set_attempt_recovery(&self, _data: QpdfPtr, value: bool) {
        self.state.record(Call::AttemptRecovery(value));
    }

    fn read_memory(
        &self,
        _data: QpdfPtr,
        _description: QpdfPtr,
        _buffer: QpdfPtr,
        _size: u64,
        _password: QpdfPtr,
    ) -> i32 {
        self.state.record(Call::ReadMemory);
        self.script.read_status
    }

    fn has_error(&self, _data: QpdfPtr) -> bool {
        self.pending.lock().expect("not poisoned").is_some()
    }

    fn get_error(&self, _data: QpdfPtr) -> QpdfPtr {
        // Consumes the slot, exactly as qpdf's does. If it did not, the drain loop in
        // `Session::drop` would spin forever -- which is a property worth having the fake
        // model faithfully.
        let mut pending = self.pending.lock().expect("not poisoned");
        *pending = None;
        // One-shot re-arm, if the script asked for one. Once only: a slot that refilled
        // forever would hang the drain loop, and the loop terminating is part of what is
        // being tested.
        if let Some(code) = self.rearm.lock().expect("not poisoned").take() {
            *pending = Some(code);
        }
        QpdfPtr(0x0E44_0001)
    }

    fn get_error_code(&self, _data: QpdfPtr, _error: QpdfPtr) -> i32 {
        self.script.pending_error.unwrap_or(0)
    }

    fn get_num_pages(&self, _data: QpdfPtr) -> i32 {
        self.state.record(Call::NumPages);
        self.script.page_count
    }

    fn logger_create(&self) -> QpdfPtr {
        QpdfPtr(0x109_0001)
    }

    fn logger_discard_all(&self, _logger: QpdfPtr) {}

    fn heap_bytes(&self) -> u64 {
        self.state.heap.lock().expect("not poisoned").bytes_grown
    }
}

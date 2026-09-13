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
    GlobalSet(i32, u32),
    LoggerCreate,
    LoggerDiscardAll(i32),
    GetPageN(u32),
    AddPage { page: u32, first: bool },
    InitWriteMemory,
    SetDeterministicId(bool),
    Write,
    GetBufferLength,
    GetBuffer,
    CopyOut(u32),
}

/// A stand-in for an Emscripten module's linear memory.
///
/// Addresses start above zero so that null is distinguishable, and are never reused, so a
/// use-after-free shows up as an unknown address rather than as a silent success.
#[derive(Debug, Default)]
struct Heap {
    next: u32,
    live: Vec<(u32, Vec<u8>)>,
    /// Allocations that are *meant* to outlive an operation — currently only qpdf's shared
    /// discarding logger, which has no `qpdflogger_cleanup` on either path and lives as
    /// long as the worker.
    ///
    /// Held apart from `live` so `assert_empty` can stay strict about everything else. The
    /// interesting property is not "is it freed" but **"is there exactly one"**: one is the
    /// design, one per operation is the unbounded leak this split exists to catch.
    retained: Vec<(u32, Vec<u8>)>,
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

    /// Allocate something that deliberately outlives the operation. See `retained`.
    fn alloc_retained(&mut self, bytes: &[u8]) -> u32 {
        let at = self.alloc(bytes);
        if at != 0
            && let Some(pos) = self.live.iter().position(|(a, _)| *a == at)
        {
            let entry = self.live.remove(pos);
            self.retained.push(entry);
        }
        at
    }

    /// The bytes at `ptr`, from either list.
    ///
    /// **Panics on an address it does not know**, which is the point: addresses are never
    /// reused, so reading a freed or invented pointer is a use-after-free rather than a
    /// plausible-looking success. A fake that returned zeroes here would let exactly the
    /// bug it exists to catch pass as an empty document.
    fn read(&self, ptr: u32, len: usize) -> Vec<u8> {
        let found = self
            .live
            .iter()
            .chain(self.retained.iter())
            .find(|(at, _)| *at == ptr);
        let (_, bytes) = found.unwrap_or_else(|| {
            panic!("read {ptr:#x}, which is not a live or retained allocation -- a stray or freed pointer")
        });
        assert!(
            len <= bytes.len(),
            "read {len} bytes from a {}-byte allocation at {ptr:#x} -- an out-of-bounds read",
            bytes.len()
        );
        bytes[..len].to_vec()
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

    /// How many deliberately long-lived allocations exist. See `Heap::retained`.
    pub(super) fn retained(&self) -> usize {
        self.heap.lock().expect("not poisoned").retained.len()
    }

    /// Assert nothing is still allocated in the engine heap.
    ///
    /// The whole reason the fake tracks allocations. A leak here is a leak in the browser,
    /// where nothing would report it. Deliberately long-lived allocations are excluded —
    /// [`retained`](FakeHeap::retained) is how those are checked instead.
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
///
/// `Clone` but not `Copy` since M1 PR B2: `output` is a real payload rather than a length,
/// so a test can assert the bytes that reach Rust are the bytes the engine produced. A
/// `copy_out` that returned the right LENGTH of the wrong memory satisfies a size
/// assertion, which is why the fake carries content at all.
#[derive(Debug, Clone)]
pub(super) struct QpdfScript {
    /// The raw `QPDF_ERROR_CODE` bitmask `qpdf_read_memory` returns.
    pub(super) read_status: i32,
    /// A pending error code, or `None` for "no error in the slot".
    pub(super) pending_error: Option<i32>,
    /// What `qpdf_get_num_pages` returns.
    pub(super) page_count: i32,
    /// Whether `qpdf_init` succeeds.
    pub(super) init_succeeds: bool,
    /// Extra heap growth attributed to the read, in bytes — what a decompression bomb costs.
    pub(super) read_grows_heap_by: u64,
    /// An error that appears in the slot *again*, once, after the first drain.
    ///
    /// Models qpdf recording a further problem after the orchestration's last
    /// `take_error` — which is the only way the slot can still be occupied when `Drop`
    /// runs, and therefore the only way to exercise the drain loop there. Re-arming once
    /// rather than forever also pins that the loop terminates.
    pub(super) error_reappears_once: Option<i32>,
    /// The bitmask `qpdf_add_page` returns.
    pub(super) add_page_status: i32,
    /// The bitmask `qpdf_init_write_memory` returns.
    pub(super) init_write_status: i32,
    /// The bitmask `qpdf_write` returns.
    pub(super) write_status: i32,
    /// The document `qpdf_get_buffer` hands back.
    ///
    /// A real payload rather than a length, so a test can assert the bytes that reach Rust
    /// are the bytes the engine produced -- a `copy_out` that returned the right LENGTH of
    /// the wrong memory would satisfy a size assertion.
    pub(super) output: Vec<u8>,
    /// Make `qpdf_get_buffer` return null while `get_buffer_length` still reports a length.
    ///
    /// The shape upstream can actually produce: `qpdf_get_buffer` returns null when the
    /// writer has no buffer, and the length accessor independently returns 0 -- but a
    /// caller that checked only one of them would read from a null pointer. Scripting them
    /// apart is how that gets tested.
    pub(super) buffer_is_null: bool,
}

impl Default for QpdfScript {
    fn default() -> Self {
        Self {
            read_status: 0,
            pending_error: None,
            page_count: 1,
            add_page_status: 0,
            init_write_status: 0,
            write_status: 0,
            output: b"%PDF-1.7\nmerged\n".to_vec(),
            buffer_is_null: false,
            init_succeeds: true,
            read_grows_heap_by: 0,
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
    /// Output buffers handed out by `get_buffer`, keyed by the document that owns them.
    ///
    /// Freed by `cleanup`, because that is when qpdf frees them.
    buffers: Mutex<Vec<(u32, u32)>>,
    /// Whether the one-shot re-arm above is still available.
    rearm: Mutex<Option<i32>>,
}

impl FakeQpdf {
    pub(super) fn new(state: Arc<FakeHeap>, script: QpdfScript) -> Self {
        let pending = Mutex::new(script.pending_error);
        let rearm = Mutex::new(script.error_reappears_once);
        Self {
            state,
            script,
            pending,
            rearm,
            buffers: Mutex::new(Vec::new()),
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

    fn cleanup(&self, data: QpdfPtr) {
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

        // The output buffer dies with the document, as it does in qpdf -- `qpdf-c.h` says
        // the pointer is owned by the `qpdf_data`, so the orchestration never frees it and
        // must not. Modelled here rather than exempted from the leak detector: an exemption
        // is a place a genuine per-operation leak could hide.
        let mut buffers = self.buffers.lock().expect("not poisoned");
        let mine: Vec<u32> = buffers
            .iter()
            .filter(|(d, _)| *d == data.0)
            .map(|(_, at)| *at)
            .collect();
        buffers.retain(|(d, _)| *d != data.0);
        drop(buffers);
        for at in mine {
            self.state.heap.lock().expect("not poisoned").free(at);
        }
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
        {
            let mut heap = self.state.heap.lock().expect("not poisoned");
            heap.bytes_grown = heap
                .bytes_grown
                .saturating_add(self.script.read_grows_heap_by);
        }
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

    fn global_set_uint32(&self, param: i32, value: u32) -> i32 {
        self.state.record(Call::GlobalSet(param, value));
        0
    }

    fn logger_create(&self) -> QpdfPtr {
        self.state.record(Call::LoggerCreate);
        // Allocated through the tracked heap, NOT returned as a fixed handle.
        //
        // It was a fixed handle, and that made the leak detector structurally blind to the
        // one allocation the web path actually leaked: a logger created per operation and
        // never released. A detector has to be able to see the thing it exists to detect.
        //
        // Retained rather than live: this one is *supposed* to outlive the operation, so the
        // property worth asserting is that there is exactly one of them however many
        // operations run.
        QpdfPtr(
            self.state
                .heap
                .lock()
                .expect("not poisoned")
                .alloc_retained(b"logger"),
        )
    }

    fn logger_discard_all(&self, _logger: QpdfPtr, destination: i32) {
        self.state.record(Call::LoggerDiscardAll(destination));
    }

    fn get_page_n(&self, _data: QpdfPtr, n: u32) -> u32 {
        self.state.record(Call::GetPageN(n));
        // Handles are 1-based in qpdf and never zero for a valid page, so returning `n + 1`
        // keeps a real handle distinguishable from a default-initialised one.
        n + 1
    }

    fn add_page(&self, _data: QpdfPtr, _source: QpdfPtr, page: u32, first: bool) -> i32 {
        self.state.record(Call::AddPage { page, first });
        self.script.add_page_status
    }

    fn init_write_memory(&self, _data: QpdfPtr) -> i32 {
        self.state.record(Call::InitWriteMemory);
        self.script.init_write_status
    }

    fn set_deterministic_id(&self, _data: QpdfPtr, value: bool) {
        self.state.record(Call::SetDeterministicId(value));
    }

    fn write(&self, _data: QpdfPtr) -> i32 {
        self.state.record(Call::Write);
        self.script.write_status
    }

    fn get_buffer_length(&self, _data: QpdfPtr) -> u32 {
        self.state.record(Call::GetBufferLength);
        u32::try_from(self.script.output.len()).unwrap_or(u32::MAX)
    }

    fn get_buffer(&self, data: QpdfPtr) -> QpdfPtr {
        self.state.record(Call::GetBuffer);
        if self.script.buffer_is_null {
            return QpdfPtr::NULL;
        }
        // A REAL ALLOCATION IN THE TRACKED HEAP, not a made-up address.
        //
        // LIVE, not retained, and that was got wrong first. qpdf frees this buffer with
        // `qpdf_cleanup`; it does not outlive the document. Modelling it as retained made
        // `retained()` report two where the design says one, and would have left a genuine
        // per-operation leak hiding behind the logger's exemption.
        let at = self
            .state
            .heap
            .lock()
            .expect("not poisoned")
            .alloc(&self.script.output);
        self.buffers
            .lock()
            .expect("not poisoned")
            .push((data.0, at));
        QpdfPtr(at)
    }

    fn copy_out(&self, ptr: QpdfPtr, len: u32) -> Vec<u8> {
        self.state.record(Call::CopyOut(len));
        // READ THROUGH THE HEAP, never straight from the script. A fake that returned
        // `script.output` regardless of the pointer would pass a caller that copied from
        // the wrong address, or from a buffer it had already freed -- which is the class of
        // bug this whole fake exists to make visible.
        self.state
            .heap
            .lock()
            .expect("not poisoned")
            .read(ptr.0, len as usize)
    }

    fn heap_bytes(&self) -> u64 {
        self.state.heap.lock().expect("not poisoned").bytes_grown
    }
}

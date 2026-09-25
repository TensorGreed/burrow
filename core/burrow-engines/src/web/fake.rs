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
    AbandonInput(u32, u32),
    RemovePage(u32),
    AddPageAt {
        page: u32,
        before: bool,
    },
    OhObject(u32),
    OhUnparse(u32),
    OhGetName(u32),
    OhRemoveKey {
        oh: u32,
        key: String,
    },
    OhArrayLen(u32),
    OhArrayItem {
        oh: u32,
        at: i32,
    },
    OhSetArrayItem {
        oh: u32,
        at: i32,
        item: u32,
    },
    OhEraseItem {
        oh: u32,
        at: i32,
    },
    OhGetDict(u32),
    OhPageContent(u32),
    OhStreamData(u32),
    Load,
    PageCount,
    CloseDocument(u32, u32, u32),
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
    AddPage {
        page: u32,
        first: bool,
    },
    InitWriteMemory,
    SetDeterministicId(bool),
    /// `set_object_stream_mode`, with the mode Rust chose. 1 is preserve, 2 is generate.
    ///
    /// Recorded rather than ignored so a test can assert WHICH mode an operation asked for --
    /// the difference between `compress` and every other operation is this one number, and a
    /// fake that swallowed it could not tell them apart.
    SetObjectStreamMode(u32),
    Write,
    GetBufferLength,
    GetBuffer,
    CopyOut(u32),
    OhGetKey {
        oh: u32,
        key: String,
    },
    OhGetTypeCode(u32),
    OhGetIntValue(u32),
    OhNewInteger(i64),
    OhNewNull,
    OhReplaceKey {
        oh: u32,
        key: String,
        item: u32,
    },
    /// The bytes are recorded IN FULL, not as a length.
    ///
    /// This is redaction's write, and the thing a test needs to assert about it is *what was
    /// written* -- that the stream a redaction emitted no longer holds the run it removed. A
    /// length would let a fake pass for an operation that wrote the right number of wrong
    /// bytes, which is the failure the whole milestone is about (ADR 0029 §8).
    OhReplaceStreamData {
        stream: u32,
        bytes: Vec<u8>,
        filter: u32,
        decode_parms: u32,
    },
    OhRelease(u32),

    // ---- the PDFium render path (#57) ----------------------------------------------
    //
    // Distinct variants rather than reuse of `CopyOut` above, which is qpdf's: one `Call`
    // enum serves both fakes, and a shared variant would make a test asserting "the render
    // copied out" indistinguishable from one asserting a document was written.
    LoadPage(i32),
    ClosePage(u32),
    PageSizeByIndex(i32),
    BitmapCreate {
        width: i32,
        height: i32,
        alpha: i32,
    },
    BitmapFillRect {
        bitmap: u32,
        width: i32,
        height: i32,
        color: u32,
    },
    BitmapBuffer(u32),
    BitmapStride(u32),
    BitmapDestroy(u32),
    PdfiumCopyOut(u32, u32),
    PauseCreate,
    PauseDestroy(u32),
    RenderPageStart {
        bitmap: u32,
        page: u32,
        rotate: i32,
        flags: i32,
    },
    RenderPageContinue(u32),
    RenderPageClose(u32),
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
    /// The whole allocation at `ptr`.
    ///
    /// For a NUL-terminated string, where the caller does not know the length in advance.
    /// Same base-address rule as [`read`](Self::read): an address this heap did not hand out
    /// is a stray pointer and panics.
    fn read_all(&self, ptr: u32) -> Vec<u8> {
        let found = self
            .live
            .iter()
            .chain(self.retained.iter())
            .find(|(at, _)| *at == ptr);
        let (_, bytes) = found.unwrap_or_else(|| {
            panic!("read {ptr:#x}, which is not a live or retained allocation -- a stray or freed pointer")
        });
        bytes.clone()
    }

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

    // ---- the render path (#57) -------------------------------------------------------
    /// Whether `FPDF_LoadPage` succeeds. `false` returns null, as a damaged page does.
    pub(super) page_load_succeeds: bool,
    /// Whether `FPDFBitmap_Create` succeeds.
    ///
    /// **An allocation failure, never a ceiling.** `max_pixels` is checked in Rust before the
    /// bitmap is created, so a test that scripts this false is driving the out-of-memory path
    /// and not the limit path — and the two must reach different typed errors.
    pub(super) bitmap_alloc_succeeds: bool,
    /// Extra bytes per row, on top of `width * 4`.
    ///
    /// **The padding PDFium is entitled to add**, and the reason `FPDFBitmap_GetStride`
    /// exists. Scripted rather than always zero because a reader that assumed `width * 4` is
    /// correct on every unpadded bitmap, which is most of them.
    pub(super) stride_padding: u32,
    /// Report this stride instead of the real one, for the engine-contradicts-itself cases.
    pub(super) stride_override: Option<i32>,
    /// Whether `FPDFBitmap_GetBuffer` returns a pointer at all.
    pub(super) bitmap_buffer_is_null: bool,
    /// What `FPDF_GetPageSizeByIndexF` reports. `None` is the engine refusing.
    pub(super) page_size: Option<(f32, f32)>,
    /// How many slices a progressive render takes before it reports `DONE`.
    ///
    /// **More than one by default**, because one slice is the shape that would make every
    /// checkpoint-between-slices test vacuous: a render that never yields has nothing to
    /// checkpoint between, and a fake that never yielded would agree with a loop that had no
    /// checkpoint at all. The real engine yields 30,002 times on a 3 M-path page.
    pub(super) render_slices: u32,
    /// Whether the pause interface can be allocated.
    pub(super) pause_alloc_succeeds: bool,
    /// Report this terminal state instead of `DONE`, for the engine-gave-up path.
    pub(super) render_final_state: i32,
    /// Heap growth attributed to each slice, in bytes.
    pub(super) slice_grows_heap_by: u64,
}

impl Default for PdfiumScript {
    fn default() -> Self {
        Self {
            load_error_code: 0,
            load_succeeds: true,
            page_count: 1,
            load_grows_heap_by: 0,
            page_load_succeeds: true,
            bitmap_alloc_succeeds: true,
            stride_padding: 0,
            stride_override: None,
            bitmap_buffer_is_null: false,
            page_size: Some((200.0, 400.0)),
            render_slices: 4,
            pause_alloc_succeeds: true,
            render_final_state: 2,
            slice_grows_heap_by: 0,
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
    /// A clock this fake advances as it renders, and by how much per slice.
    ///
    /// **The only way to drive a deadline that comes due INSIDE a render.** A stopped clock
    /// cannot, and advancing it before the call makes the refusal arrive at the checkpoint
    /// before the loop -- which is a test of the wrong checkpoint, and is how the first version
    /// of `a_deadline_that_comes_due_between_slices` failed. `FakeRotator` carries the same
    /// pair for the same reason.
    slice_clock: Option<(Arc<burrow_types::ManualClock>, u64)>,
    /// Slices left in the progressive render in flight. `None` when none is running.
    ///
    /// A cell rather than a field on the script, because it is *state* the fake mutates as the
    /// caller drives the loop -- the script says how many slices a render takes, this says how
    /// many are left.
    render_remaining: Mutex<Option<u32>>,
    /// Geometry for every bitmap handed out, by handle: `(width, height, stride)`.
    ///
    /// The fake has to remember this because `FPDFBitmap_GetStride` and
    /// `FPDFBitmap_GetBuffer` take only a handle — the caller no longer has the dimensions
    /// at those call sites, which is exactly why the stride has to be asked for rather than
    /// computed.
    bitmaps: Mutex<Vec<Bitmap>>,
}

/// One bitmap the fake has handed out.
///
/// **`id`, NOT `handle`, and the name is load-bearing.** `tools/check-handle-identity.py`
/// refuses a raw `handle` compared for equality anywhere in a file that deals in qpdf object
/// handles -- and this file does, because `FakeQpdf` lives in it. That rule is right for
/// `qpdf_oh`, which qpdf reissues on every call so that two handles to one object never
/// compare equal. A PDFium `FPDF_BITMAP` is the opposite: a stable pointer for the life of the
/// bitmap, and comparing it is how this table finds anything. Renamed rather than exempted,
/// so the rule keeps its meaning and nobody has to read an argument to see why this is fine.
#[derive(Debug, Clone, Copy)]
struct Bitmap {
    id: u32,
    width: i32,
    height: i32,
    stride: usize,
}

impl FakePdfium {
    pub(super) fn new(state: Arc<FakeHeap>, script: PdfiumScript) -> Self {
        Self {
            state,
            script,
            doc_handle: 0xD0C0_0001,
            slice_clock: None,
            render_remaining: Mutex::new(None),
            bitmaps: Mutex::new(Vec::new()),
        }
    }

    /// The same fake, advancing `clock` by `ms` on every progressive slice.
    pub(super) fn advancing(
        state: Arc<FakeHeap>,
        script: PdfiumScript,
        clock: Arc<burrow_types::ManualClock>,
        ms: u64,
    ) -> Self {
        let mut fake = Self::new(state, script);
        fake.slice_clock = Some((clock, ms));
        fake
    }

    /// Ink the top-left quadrant of `bitmap`, as both render entry points do.
    fn paint(&self, bitmap: PdfiumPtr, _size_x: i32, _size_y: i32) {
        let geometry = self.bitmap(bitmap.0);
        // Ink in the top-left quadrant and nowhere else -- the same shape as the native
        // fixture `minimal_pdf::pdf_with_ink`, so a test can tell a correct render from a
        // flip, a mirror, or a quarter turn. Not a model of PDF content; a model of "the
        // engine writes into the bitmap it was given, in row-major order, top row first".
        let mut heap = self.state.heap.lock().expect("not poisoned");
        if let Some((_, bytes)) = heap.live.iter_mut().find(|(at, _)| *at == bitmap.0) {
            let height = usize::try_from(geometry.height).expect("a non-negative height");
            let width = usize::try_from(geometry.width).expect("a non-negative width");
            for row in 0..height / 2 {
                for column in 0..width / 2 {
                    let at = row * geometry.stride + column * 4;
                    bytes[at..at + 4].copy_from_slice(&[0, 0, 0, 0xFF]);
                }
            }
        }
    }

    /// The geometry of a bitmap this fake handed out.
    ///
    /// **Panics on an id it does not know**, for the reason `Heap::read` panics on a stray
    /// pointer: a fake that invented an answer for a destroyed bitmap would let a
    /// use-after-free read as an ordinary render.
    fn bitmap(&self, id: u32) -> Bitmap {
        let bitmaps = self.bitmaps.lock().expect("not poisoned");
        *bitmaps
            .iter()
            .find(|b| b.id == id)
            .unwrap_or_else(|| panic!("bitmap {id:#x} was never created, or was destroyed"))
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

    fn abandon_input(&self, ptr: PdfiumPtr, len: u32) {
        self.state.record(Call::AbandonInput(ptr.0, len));
        // MODELS THE WIPE, like `wipe_and_free` above. A fake that only freed would let a
        // test assert "the call happened" and nothing about the bytes, which is the weaker
        // claim -- and this buffer holds the user's document.
        let mut heap = self.state.heap.lock().expect("not poisoned");
        if let Some((_, bytes)) = heap.live.iter_mut().find(|(at, _)| *at == ptr.0) {
            bytes.fill(0);
            *self.state.wiped.lock().expect("not poisoned") = Some(bytes.clone());
        }
        heap.free(ptr.0);
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

    fn close_document(&self, doc: PdfiumPtr, data: PdfiumPtr, len: u32) {
        // Recorded as one call, which is the point: the ordering is not expressible at a
        // call site, so a test asserting "close before free" is asserting about this.
        self.state.record(Call::CloseDocument(doc.0, data.0, len));
        let mut heap = self.state.heap.lock().expect("not poisoned");
        // The wipe, modelled -- see `abandon_input`.
        if let Some((_, bytes)) = heap.live.iter_mut().find(|(at, _)| *at == data.0) {
            bytes.fill(0);
            *self.state.wiped.lock().expect("not poisoned") = Some(bytes.clone());
        }
        heap.free(data.0);
    }

    fn heap_bytes(&self) -> u64 {
        self.state.heap.lock().expect("not poisoned").bytes_grown
    }

    // ---- the render path (#57) -----------------------------------------------------------
    //
    // What is modelled here is what the orchestration has to get right and a browser will not
    // reproduce on demand: that a page handle is closed on EVERY exit, that a bitmap is
    // destroyed on every exit, that the stride is asked for rather than assumed, and that the
    // fill happens before the render. PDF parsing is still not modelled -- `render_page_bitmap`
    // paints a fixed quadrant, which is a model of the BITMAP contract rather than of PDFium.

    fn load_page(&self, _doc: PdfiumPtr, index: i32) -> PdfiumPtr {
        self.state.record(Call::LoadPage(index));
        if !self.script.page_load_succeeds {
            return PdfiumPtr::NULL;
        }
        // A real allocation, so a page left open by an early return is a live entry
        // `assert_empty` fails on. There is no `Drop` across a real bridge either.
        PdfiumPtr(self.state.heap.lock().expect("not poisoned").alloc(b"page"))
    }

    fn close_page(&self, page: PdfiumPtr) {
        self.state.record(Call::ClosePage(page.0));
        self.state.heap.lock().expect("not poisoned").free(page.0);
    }

    fn page_size_by_index(&self, _doc: PdfiumPtr, index: i32) -> Option<(f32, f32)> {
        self.state.record(Call::PageSizeByIndex(index));
        // NO ALLOCATION AND NO PAGE HANDLE, which is the property under test: a strip must be
        // able to ask for a page's size without paying for a page load.
        self.script.page_size
    }

    fn bitmap_create(&self, width: i32, height: i32, alpha: i32) -> PdfiumPtr {
        self.state.record(Call::BitmapCreate {
            width,
            height,
            alpha,
        });
        if !self.script.bitmap_alloc_succeeds {
            return PdfiumPtr::NULL;
        }
        let w = usize::try_from(width).expect("a non-negative width");
        let h = usize::try_from(height).expect("a non-negative height");
        let stride = w * 4 + usize::try_from(self.script.stride_padding).expect("padding fits");

        // 0xCD everywhere, NOT zero. A fresh bitmap's contents are undefined, and a fake that
        // handed back zeroes would make a missing `FPDFBitmap_FillRect` look like a black
        // page -- plausible enough to pass a test that only checked the length. 0xCD is not a
        // colour anything here produces, so it shows up.
        let id = self
            .state
            .heap
            .lock()
            .expect("not poisoned")
            .alloc(&vec![0xCD_u8; stride * h]);
        self.bitmaps.lock().expect("not poisoned").push(Bitmap {
            id,
            width,
            height,
            stride,
        });
        PdfiumPtr(id)
    }

    fn bitmap_fill_rect(
        &self,
        bitmap: PdfiumPtr,
        _left: i32,
        _top: i32,
        width: i32,
        height: i32,
        color: u32,
    ) {
        self.state.record(Call::BitmapFillRect {
            bitmap: bitmap.0,
            width,
            height,
            color,
        });
        let geometry = self.bitmap(bitmap.0);
        // `color` is 0xAARRGGBB; the bytes in memory are B, G, R, A.
        let pixel = [
            u8::try_from(color & 0xFF).expect("a byte"),
            u8::try_from((color >> 8) & 0xFF).expect("a byte"),
            u8::try_from((color >> 16) & 0xFF).expect("a byte"),
            u8::try_from((color >> 24) & 0xFF).expect("a byte"),
        ];
        let mut heap = self.state.heap.lock().expect("not poisoned");
        if let Some((_, bytes)) = heap.live.iter_mut().find(|(at, _)| *at == bitmap.0) {
            for row in 0..usize::try_from(geometry.height).expect("a non-negative height") {
                for column in 0..usize::try_from(geometry.width).expect("a non-negative width") {
                    let at = row * geometry.stride + column * 4;
                    bytes[at..at + 4].copy_from_slice(&pixel);
                }
            }
        }
    }

    fn bitmap_buffer(&self, bitmap: PdfiumPtr) -> PdfiumPtr {
        self.state.record(Call::BitmapBuffer(bitmap.0));
        if self.script.bitmap_buffer_is_null {
            return PdfiumPtr::NULL;
        }
        // The bitmap's own allocation. A real `FPDFBitmap_GetBuffer` returns a pointer INTO
        // the bitmap, which is why nothing frees this separately: `bitmap_destroy` does.
        bitmap
    }

    fn bitmap_stride(&self, bitmap: PdfiumPtr) -> i32 {
        self.state.record(Call::BitmapStride(bitmap.0));
        if let Some(stride) = self.script.stride_override {
            return stride;
        }
        i32::try_from(self.bitmap(bitmap.0).stride).expect("a stride that fits")
    }

    fn bitmap_destroy(&self, bitmap: PdfiumPtr) {
        self.state.record(Call::BitmapDestroy(bitmap.0));
        self.bitmaps
            .lock()
            .expect("not poisoned")
            .retain(|b| b.id != bitmap.0);
        self.state.heap.lock().expect("not poisoned").free(bitmap.0);
    }

    fn copy_out(&self, ptr: PdfiumPtr, len: u32) -> Vec<u8> {
        self.state.record(Call::PdfiumCopyOut(ptr.0, len));
        self.state
            .heap
            .lock()
            .expect("not poisoned")
            .read(ptr.0, usize::try_from(len).expect("a length that fits"))
    }

    // ---- progressive rendering -----------------------------------------------------------

    fn pause_create(&self) -> PdfiumPtr {
        self.state.record(Call::PauseCreate);
        if !self.script.pause_alloc_succeeds {
            return PdfiumPtr::NULL;
        }
        // A REAL ALLOCATION, so a pause interface that is never destroyed is a live entry
        // `assert_empty` fails on. On the real bridge it is a `_malloc` plus an `addFunction`
        // table entry, and the table entry is the half that leaks quietly.
        PdfiumPtr(
            self.state
                .heap
                .lock()
                .expect("not poisoned")
                .alloc(b"pause"),
        )
    }

    fn pause_destroy(&self, pause: PdfiumPtr) {
        self.state.record(Call::PauseDestroy(pause.0));
        self.state.heap.lock().expect("not poisoned").free(pause.0);
    }

    fn render_page_start(
        &self,
        bitmap: PdfiumPtr,
        page: PdfiumPtr,
        size_x: i32,
        size_y: i32,
        rotate: i32,
        flags: i32,
        _pause: PdfiumPtr,
    ) -> i32 {
        self.state.record(Call::RenderPageStart {
            bitmap: bitmap.0,
            page: page.0,
            rotate,
            flags,
        });
        // The ink goes down on the FIRST slice, as PDFium's does. A caller that abandoned the
        // render part way must not come back with a blank bitmap that looks finished.
        self.paint(bitmap, size_x, size_y);
        let remaining = self.script.render_slices.saturating_sub(1);
        *self.render_remaining.lock().expect("not poisoned") = Some(remaining);
        if remaining == 0 {
            self.script.render_final_state
        } else {
            1
        }
    }

    fn render_page_continue(&self, page: PdfiumPtr, _pause: PdfiumPtr) -> i32 {
        self.state.record(Call::RenderPageContinue(page.0));
        {
            let mut heap = self.state.heap.lock().expect("not poisoned");
            heap.bytes_grown = heap
                .bytes_grown
                .saturating_add(self.script.slice_grows_heap_by);
        }
        if let Some((clock, ms)) = &self.slice_clock {
            clock.advance(*ms);
        }
        let mut remaining = self.render_remaining.lock().expect("not poisoned");
        let left = remaining.unwrap_or(0).saturating_sub(1);
        *remaining = Some(left);
        if left == 0 {
            self.script.render_final_state
        } else {
            1
        }
    }

    fn render_page_close(&self, page: PdfiumPtr) {
        self.state.record(Call::RenderPageClose(page.0));
        *self.render_remaining.lock().expect("not poisoned") = None;
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
    /// Whether `qpdf_oh_replace_stream_data` gets its bytes into the engine.
    ///
    /// Defaults to `true`. On the web the false case is an allocation failure in the engine
    /// heap, which is a refusal rather than a panic -- and a redaction that reports success on
    /// a write the engine never took is the failure ADR 0029 §6 exists to make impossible, so
    /// it needs a way to be provoked.
    pub(super) replace_stream_data_succeeds: bool,
    /// An error code `qpdf_oh_replace_stream_data` LATCHES, as the real one does.
    ///
    /// Distinct from `replace_stream_data_succeeds`, and the distinction is the whole hazard:
    /// the C function returns `void`, so a qpdf that refuses the write says nothing at the call
    /// site and leaves a code on the `qpdf_data`. A caller that reads only the bridge's `true`
    /// sees a successful write. `pending_error` cannot model this -- `Session::open` drains it
    /// before any handle exists -- so the error has to be armed at the call itself.
    pub(super) replace_stream_data_latches: Option<i32>,
    /// Extra heap growth attributed to the read, in bytes — what a decompression bomb costs.
    pub(super) read_grows_heap_by: u64,
    /// The bitmask `qpdf_remove_page` returns.
    pub(super) remove_page_status: i32,
    /// What a handle's `unparse` returns, by the key that produced the handle.
    ///
    /// Absent means `<< >>` — an empty dictionary, which is what qpdf unparses for one and
    /// what the policy must survive. A fake whose default were richer than the engine's would
    /// teach the policy habits the engine will not honour.
    pub(super) oh_unparsed: std::collections::BTreeMap<String, String>,
    /// How many items an array has, by the key that produced it. Absent means zero.
    pub(super) oh_array_len: std::collections::BTreeMap<String, i32>,
    /// What `qpdf_oh_get_page_content_data` returns for any page.
    pub(super) page_content: Vec<u8>,
    /// What `qpdf_oh_get_stream_data` returns.
    ///
    /// `None` is "could not decode", which the policy turns into a refusal — a case worth
    /// driving, and the reason this is scripted rather than defaulted.
    pub(super) stream_data: Option<Vec<u8>>,
    /// Fail `get_page_n` from this call onwards, by latching an error.
    ///
    /// What makes an error DURING the permutation reachable. Without it the only failure a
    /// reorder test could script was one `Session::open` drains first -- so the test that
    /// claimed to cover the error path released zero handles because zero had been taken.
    /// Found by code review.
    pub(super) get_page_n_fails_after: Option<usize>,
    /// Latch an error from `oh_object` only from this call onwards.
    ///
    /// Needed to put the failure on the LAST comparison. With every call failing, the latched
    /// error is drained by the next iteration's `page_handle` and the operation refuses
    /// anyway -- so a test could not tell the drain from its absence. On the last iteration
    /// there is no next `page_handle`, and the drain is the only thing standing between a
    /// failed identity read and a document written as though nothing needed moving.
    pub(super) oh_object_fails_after: Option<usize>,
    /// Latch an error from `oh_object`, and return 0 from it.
    ///
    /// The exact hazard the module header names: both halves of an object's identity return 0
    /// on failure, so an undrained failure compares EQUAL and the page is silently not moved.
    /// Deleting the drain left every test green until this knob existed.
    pub(super) oh_object_fails: bool,
    /// The bitmask `qpdf_add_page_at` returns.
    pub(super) add_page_at_status: i32,
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
    /// The type code `qpdf_oh_get_type_code` reports, by key.
    ///
    /// Keyed rather than a single value, because rotate's walk asks about `/Rotate` and
    /// `/Parent` in turn and a fake that answered the same for both could not model a page
    /// whose rotation is absent but whose parent is a dictionary -- which is every page in
    /// a document that inherits.
    pub(super) oh_type_codes: std::collections::BTreeMap<String, i32>,
    /// What a NAME object unparses to, by the key that produced the handle.
    ///
    /// Absent means "the key itself", which is the right answer for the common case
    /// (`/Resources /Font /F1` is a handle whose name is `/F1`) and the wrong one for the
    /// only place the policy reads a name it did not ask for by that name: `/Type`. An
    /// optional-content group is a dictionary whose `/Type` is `/OCG`, so without this the
    /// fake answers `/Type` and the refusal is unreachable from here.
    pub(super) oh_names: std::collections::BTreeMap<String, String>,
    /// The integer `qpdf_oh_get_int_value` reports.
    pub(super) oh_int_value: i64,
    /// How many handles the fake has issued and not seen released.
    ///
    /// The web path has no `ObjectHandle` to make release automatic, so it is discipline
    /// rather than a type -- and discipline needs a measurement. `web/tests.rs` asserts this
    /// is zero after a rotation, after a refusal, and after the depth-exhausted walk.
    ///
    /// It said the same thing while nothing read it -- three writes, no readers -- which is a
    /// counter that reports "no leak" for every input. Both reviewers found it.
    pub(super) live_handles: Arc<Mutex<i64>>,
}

impl Default for QpdfScript {
    fn default() -> Self {
        Self {
            read_status: 0,
            pending_error: None,
            page_count: 1,
            add_page_status: 0,
            remove_page_status: 0,
            get_page_n_fails_after: None,
            oh_object_fails: false,
            oh_object_fails_after: None,
            add_page_at_status: 0,
            init_write_status: 0,
            write_status: 0,
            output: b"%PDF-1.7\nmerged\n".to_vec(),
            buffer_is_null: false,
            init_succeeds: true,
            replace_stream_data_succeeds: true,
            replace_stream_data_latches: None,
            read_grows_heap_by: 0,
            error_reappears_once: None,
            // A page tree that INHERITS: no `/Rotate` on the page (null), and a `/Parent`
            // that is a dictionary. The default models the case a naive implementation gets
            // wrong, rather than the case it gets right by accident.
            oh_type_codes: [("/Rotate".to_owned(), 2), ("/Parent".to_owned(), 9)]
                .into_iter()
                .collect(),
            oh_names: std::collections::BTreeMap::new(),
            oh_int_value: 0,
            // EMPTY, so a page with nothing scripted prunes to nothing rather than to
            // something. `<< >>` is what qpdf unparses for an empty dictionary, and a default
            // richer than the engine's would teach the policy habits the engine will not
            // honour.
            oh_unparsed: std::collections::BTreeMap::new(),
            oh_array_len: std::collections::BTreeMap::new(),
            page_content: Vec::new(),
            // SOME, not None. `None` is "qpdf could not decode this", which the policy turns
            // into a refusal -- so a default of `None` would make every fake document refuse
            // and every test that did not think about it pass for the wrong reason.
            stream_data: Some(Vec::new()),
            live_handles: Arc::new(Mutex::new(0)),
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
    /// Which key produced each handle, so `oh_get_type_code` can answer per key.
    ///
    /// A real qpdf handle is an index into a cache of objects; the fake needs only enough of
    /// that to tell `/Rotate` from `/Parent`, because rotate's walk asks about both and a
    /// fake that answered the same for each could not model a page that inherits.
    handle_keys: Mutex<std::collections::BTreeMap<u32, String>>,
    /// The document's pages, as OBJECT NUMBERS, in their current order.
    ///
    /// # Why the fake models this at all
    ///
    /// A fake whose `oh_object` returned the handle would be useless for `reorder` in the one
    /// way that matters: the defect the whole design guards against is treating a handle as an
    /// identity, and such a fake agrees with the defect. So identity is modelled separately
    /// from handles here -- `get_page_n` issues a fresh handle every call, as qpdf does, and
    /// maps it to a stable object number.
    ///
    /// Modelling the order as well means a test can assert the permutation the engine
    /// performed, not merely that it made the calls. `remove_page` and `add_page_at` move
    /// entries in this vector exactly as qpdf moves them in `/Kids`.
    page_objects: Mutex<Vec<u32>>,
    /// Which object each issued handle refers to.
    handle_objects: Mutex<std::collections::BTreeMap<u32, u32>>,
    /// The next handle id. Monotonic and never reused, exactly as qpdf's `next_oh` is.
    next_handle: Mutex<u32>,
}

impl FakeQpdf {
    /// A NUL-terminated string read back out of the fake heap.
    ///
    /// Read rather than assumed: a caller that copied in the wrong bytes, or a pointer to
    /// memory it had already freed, must be distinguishable from a correct one.
    fn read_c_string(&self, ptr: QpdfPtr) -> String {
        // THE WHOLE ALLOCATION, then up to the NUL. Reading byte by byte was wrong and was
        // never exercised: `Heap::read` looks an allocation up by its EXACT base address on
        // purpose, so `read(ptr + 1, 1)` panics with "not a live or retained allocation" --
        // which meant `oh_get_key` aborted the moment anything called it. Nothing did, which
        // is how it survived. Found by code review, which wrote the missing test and hit it.
        let bytes = self
            .state
            .heap
            .lock()
            .expect("not poisoned")
            .read_all(ptr.0);
        let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
        String::from_utf8_lossy(&bytes[..end]).into_owned()
    }

    /// The key that produced `handle`, which is how this fake tells one object from another.
    fn handle_name(&self, handle: u32) -> String {
        self.handle_keys
            .lock()
            .expect("not poisoned")
            .get(&handle)
            .cloned()
            .unwrap_or_default()
    }

    /// Put a NUL-terminated string in the fake heap and return a pointer to it.
    ///
    /// The real bridge hands back a pointer into qpdf's own storage that dies on the next call
    /// returning one. This does not model that lifetime: a fake that recycled the buffer would
    /// be testing the caller's copy-immediately discipline, which `copy_c_string` on both sides
    /// already enforces by construction, and the cost would be a fake heap that grows per call.
    fn stash_c_string(&self, text: &str) -> QpdfPtr {
        let mut bytes = text.as_bytes().to_vec();
        bytes.push(0);
        self.copy_in(&bytes)
    }

    /// Issue a handle, remembering which key produced it.
    ///
    /// Monotonic and never reused, as qpdf's `next_oh` is -- so a test that released a
    /// handle and saw the id come back would be seeing a fake that models the cache wrongly.
    fn issue_handle(&self, key: &str) -> u32 {
        let mut next = self.next_handle.lock().expect("not poisoned");
        *next += 1;
        let handle = *next;
        self.handle_keys
            .lock()
            .expect("not poisoned")
            .insert(handle, key.to_owned());
        *self.script.live_handles.lock().expect("not poisoned") += 1;
        handle
    }

    /// Which object a handle refers to, or `None` if it was never a page handle.
    fn object_of(&self, handle: u32) -> Option<u32> {
        self.handle_objects
            .lock()
            .expect("not poisoned")
            .get(&handle)
            .copied()
    }

    /// The document's pages, in their current order, as object numbers.
    ///
    /// What a reorder test asserts on. Without it a test could only check that the engine made
    /// the right CALLS, which is a weaker claim than that the document ended up in the right
    /// order -- and the difference is where an off-by-one lives.
    pub(super) fn page_order(&self) -> Vec<u32> {
        self.page_objects.lock().expect("not poisoned").clone()
    }

    pub(super) fn new(state: Arc<FakeHeap>, script: QpdfScript) -> Self {
        let pending = Mutex::new(script.pending_error);
        let rearm = Mutex::new(script.error_reappears_once);
        let pages = (1..=u32::try_from(script.page_count.max(0)).unwrap_or(0)).collect();
        Self {
            state,
            script,
            handle_keys: Mutex::new(std::collections::BTreeMap::new()),
            // Object numbers start at 1 and are distinct, so an order read back out of the
            // fake is unambiguous. Length follows the scripted page count.
            page_objects: Mutex::new(pages),
            handle_objects: Mutex::new(std::collections::BTreeMap::new()),
            next_handle: Mutex::new(0),
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
        // THROUGH `issue_handle`, like every other handle the fake hands out. Returning `n + 1`
        // directly meant a page handle was never counted as live and never registered against a
        // key -- so a caller that released it drove `live_handles` NEGATIVE, cancelling out a
        // genuine leak, and `n + 1` collided with the first issued id so `oh_get_type_code`
        // answered for the wrong object. Found by code review.
        if let Some(after) = self.script.get_page_n_fails_after {
            let taken = self
                .state
                .calls()
                .iter()
                .filter(|c| matches!(c, Call::GetPageN(_)))
                .count();
            if taken > after {
                *self.pending.lock().expect("not poisoned") = Some(2);
                return 0;
            }
        }
        let handle = self.issue_handle(&format!("<page {n}>"));
        // THE HANDLE IS FRESH; THE OBJECT IS NOT. Asking twice for the same page gives two
        // handles and one object number, which is exactly qpdf's behaviour and exactly what a
        // caller comparing handles gets wrong.
        let object = self
            .page_objects
            .lock()
            .expect("not poisoned")
            .get(usize::try_from(n).unwrap_or(usize::MAX))
            .copied();
        if let Some(object) = object {
            self.handle_objects
                .lock()
                .expect("not poisoned")
                .insert(handle, object);
        }
        handle
    }

    fn remove_page(&self, _data: QpdfPtr, page: u32) -> i32 {
        self.state.record(Call::RemovePage(page));
        if let Some(object) = self.object_of(page) {
            self.page_objects
                .lock()
                .expect("not poisoned")
                .retain(|o| *o != object);
        }
        self.script.remove_page_status
    }

    fn add_page_at(
        &self,
        _data: QpdfPtr,
        _source: QpdfPtr,
        page: u32,
        before: bool,
        refpage: u32,
    ) -> i32 {
        self.state.record(Call::AddPageAt { page, before });
        let (Some(object), Some(reference)) = (self.object_of(page), self.object_of(refpage))
        else {
            return self.script.add_page_at_status;
        };
        let mut order = self.page_objects.lock().expect("not poisoned");
        // The reference page's CURRENT position, looked up rather than remembered -- it moves
        // as earlier pages are reordered, which is the whole reason the engine re-asks qpdf
        // for it on every iteration instead of tracking it.
        let at = order.iter().position(|o| *o == reference);
        let Some(index) = at else {
            // A REFERENCE PAGE THAT IS NOT IN THE TREE IS AN ERROR, not an append. The first
            // version pushed the page onto the end, so a bug that lost the reference page
            // produced a plausible-looking order here instead of a refusal -- a fake that is
            // more forgiving than the engine it stands in for. Real qpdf raises. Found by
            // code review.
            *self.pending.lock().expect("not poisoned") = Some(2);
            return 2;
        };
        if before {
            order.insert(index, object);
        } else {
            order.insert(index + 1, object);
        }
        self.script.add_page_at_status
    }

    fn oh_object(&self, _data: QpdfPtr, oh: u32) -> u64 {
        self.state.record(Call::OhObject(oh));
        let asked = self
            .state
            .calls()
            .iter()
            .filter(|c| matches!(c, Call::OhObject(_)))
            .count();
        let late = self.script.oh_object_fails_after.is_some_and(|n| asked > n);
        if self.script.oh_object_fails || late {
            // EXACTLY WHAT QPDF DOES ON FAILURE: `do_with_oh<int>(.., return_T<int>(0), ..)`
            // returns the fallback AND latches the error. Returning 0 without latching would
            // model the harmless half and leave the dangerous half untestable -- which is
            // what the fake did until code review measured that deleting the drain changed
            // nothing.
            *self.pending.lock().expect("not poisoned") = Some(2);
            return 0;
        }
        // Generation 0 for every object, as a freshly written document has. The packing is
        // the bridge's contract: `(object_number << 32) | generation`.
        u64::from(self.object_of(oh).unwrap_or(0)) << 32
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

    fn set_object_stream_mode(&self, _data: QpdfPtr, mode: u32) {
        self.state.record(Call::SetObjectStreamMode(mode));
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

    fn oh_get_key(&self, _data: QpdfPtr, oh: u32, key: QpdfPtr) -> u32 {
        // THE KEY IS READ BACK OUT OF THE HEAP, not taken from the script. A caller that
        // copied in the wrong string, or a pointer to memory it had already freed, would
        // otherwise be indistinguishable from a correct one -- the same reason `copy_out`
        // reads through the heap rather than returning `script.output` directly.
        let key = self.read_c_string(key);
        self.state.record(Call::OhGetKey {
            oh,
            key: key.clone(),
        });
        self.issue_handle(&key)
    }

    fn oh_get_type_code(&self, _data: QpdfPtr, oh: u32) -> i32 {
        self.state.record(Call::OhGetTypeCode(oh));
        // The handle remembers which key produced it, so the fake can answer differently for
        // `/Rotate` and `/Parent` -- see `oh_type_codes`.
        let name = self
            .handle_keys
            .lock()
            .expect("not poisoned")
            .get(&oh)
            .cloned()
            .unwrap_or_default();
        // `ot_null` (2) by default: an unknown key is an absent key, which is what qpdf
        // reports for one.
        self.script.oh_type_codes.get(&name).copied().unwrap_or(2)
    }

    // ---- split's pruning ------------------------------------------------------------
    //
    // Scripted like the rest of this fake: each records the call so a test can assert the
    // SHAPE of the walk, and answers from `script` so a test can drive a particular document
    // without a real qpdf. The defaults are the ones qpdf gives for an absent or wrong-typed
    // object -- an empty dictionary unparses as `<< >>`, an array has no items -- because a
    // fake whose defaults are richer than the engine's teaches the policy habits the engine
    // will not honour.

    fn oh_unparse_resolved(&self, _data: QpdfPtr, oh: u32) -> QpdfPtr {
        self.state.record(Call::OhUnparse(oh));
        let name = self.handle_name(oh);
        let text = self
            .script
            .oh_unparsed
            .get(&name)
            .cloned()
            .unwrap_or_else(|| "<< >>".to_owned());
        self.stash_c_string(&text)
    }

    fn oh_get_name(&self, _data: QpdfPtr, oh: u32) -> QpdfPtr {
        self.state.record(Call::OhGetName(oh));
        let key = self.handle_name(oh);
        let name = self.script.oh_names.get(&key).cloned().unwrap_or(key);
        self.stash_c_string(&name)
    }

    fn oh_remove_key(&self, _data: QpdfPtr, oh: u32, key: QpdfPtr) {
        let key = self.read_c_string(key);
        self.state.record(Call::OhRemoveKey { oh, key });
    }

    fn oh_get_array_n_items(&self, _data: QpdfPtr, oh: u32) -> i32 {
        self.state.record(Call::OhArrayLen(oh));
        let name = self.handle_name(oh);
        self.script.oh_array_len.get(&name).copied().unwrap_or(0)
    }

    fn oh_get_array_item(&self, _data: QpdfPtr, oh: u32, at: i32) -> u32 {
        self.state.record(Call::OhArrayItem { oh, at });
        self.issue_handle(&format!("{}[{at}]", self.handle_name(oh)))
    }

    fn oh_erase_item(&self, _data: QpdfPtr, oh: u32, at: i32) {
        self.state.record(Call::OhEraseItem { oh, at });
    }

    fn oh_set_array_item(&self, _data: QpdfPtr, oh: u32, at: i32, item: u32) {
        self.state.record(Call::OhSetArrayItem { oh, at, item });
    }

    fn oh_get_dict(&self, _data: QpdfPtr, oh: u32) -> u32 {
        self.state.record(Call::OhGetDict(oh));
        self.issue_handle(&format!("{}.dict", self.handle_name(oh)))
    }

    fn oh_page_content(&self, _data: QpdfPtr, page: u32) -> Option<Vec<u8>> {
        self.state.record(Call::OhPageContent(page));
        Some(self.script.page_content.clone())
    }

    fn oh_stream_data(&self, _data: QpdfPtr, oh: u32) -> Option<Vec<u8>> {
        self.state.record(Call::OhStreamData(oh));
        // `None` means "could not decode", which the policy turns into a refusal. The script
        // says so explicitly rather than the fake guessing: an undecodable stream is a case
        // worth driving, and a default of `None` would make every fake document refuse.
        self.script.stream_data.clone()
    }

    fn copy_c_string(&self, ptr: QpdfPtr) -> Vec<u8> {
        self.read_c_string(ptr).into_bytes()
    }

    fn oh_get_int_value(&self, _data: QpdfPtr, oh: u32) -> i64 {
        self.state.record(Call::OhGetIntValue(oh));
        self.script.oh_int_value
    }

    fn oh_new_integer(&self, _data: QpdfPtr, value: i64) -> u32 {
        self.state.record(Call::OhNewInteger(value));
        self.issue_handle("<integer>")
    }

    fn oh_new_null(&self, _data: QpdfPtr) -> u32 {
        self.state.record(Call::OhNewNull);
        self.issue_handle("<null>")
    }

    fn oh_replace_stream_data(
        &self,
        _data: QpdfPtr,
        stream: u32,
        bytes: &[u8],
        filter: u32,
        decode_parms: u32,
    ) -> bool {
        self.state.record(Call::OhReplaceStreamData {
            stream,
            bytes: bytes.to_vec(),
            filter,
            decode_parms,
        });
        // LATCHED, NOT RETURNED -- exactly as the C function does. `qpdf_oh_replace_stream_data`
        // is `void`, so a refusal is only ever visible in the error slot.
        if let Some(code) = self.script.replace_stream_data_latches {
            *self.pending.lock().expect("not poisoned") = Some(code);
        }
        self.script.replace_stream_data_succeeds
    }

    fn oh_replace_key(&self, _data: QpdfPtr, oh: u32, key: QpdfPtr, item: u32) {
        let key = self.read_c_string(key);
        self.state.record(Call::OhReplaceKey { oh, key, item });
    }

    fn oh_release(&self, _data: QpdfPtr, oh: u32) {
        self.state.record(Call::OhRelease(oh));
        // ASSERTED, NOT ASSUMED. Decrementing blindly let a double release drive the count
        // negative and cancel out a real leak, which would make the `== 0` assertion in the
        // tests pass for a document that leaked one handle and released another twice. qpdf
        // itself tolerates a stray release -- `oh_cache.erase` on a missing key is a no-op --
        // so this is stricter than the engine on purpose: the fake is where a caller's
        // bookkeeping is meant to be caught.
        let known = self
            .handle_keys
            .lock()
            .expect("not poisoned")
            .remove(&oh)
            .is_some();
        assert!(
            known,
            "released handle {oh}, which this document never issued (or already released)"
        );
        *self.script.live_handles.lock().expect("not poisoned") -= 1;
    }

    fn heap_bytes(&self) -> u64 {
        self.state.heap.lock().expect("not poisoned").bytes_grown
    }
}

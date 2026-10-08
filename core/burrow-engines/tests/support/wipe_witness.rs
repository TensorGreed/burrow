//! A witness under the global allocator: on every free, does the block still hold the canary?
//!
//! Shared by two test binaries, because a binary has one global allocator:
//!
//! - `wipe_on_free_control.rs` puts the witness directly under the program, with nothing wiping.
//!   It must **see** the canary freed. That is what makes the other binary's zero a measurement:
//!   a witness that never saw the canary would report zero over any allocator.
//! - `wipe_on_free.rs` puts it under `burrow_engines::wipe::WipeOnFree`, the type the bindings
//!   declare. It must see the same frees and **no** canary in any of them.
//!
//! Only frees inside the redaction call are examined (`ARMED`). The test's own copy of the input
//! is built before and dropped after, so it cannot count either way.

use std::alloc::{GlobalAlloc, Layout};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// The text drawn in the region. Distinctive enough that nothing else in a redaction carries it,
/// and within the test font's `/FirstChar 32 /LastChar 94`.
pub const CANARY: &[u8] = b"BURROWWIPECANARY7319";

static ARMED: AtomicBool = AtomicBool::new(false);
static FREED: AtomicUsize = AtomicUsize::new(0);
static FREED_BYTES: AtomicUsize = AtomicUsize::new(0);
static HELD_CANARY: AtomicUsize = AtomicUsize::new(0);
static DIRTY: AtomicUsize = AtomicUsize::new(0);
static LARGEST_CANARY_BLOCK: AtomicUsize = AtomicUsize::new(0);

/// What the witness saw while armed.
#[derive(Debug, Clone, Copy)]
pub struct Seen {
    /// Blocks freed.
    pub freed: usize,
    /// Their total size.
    pub freed_bytes: usize,
    /// How many of them still held [`CANARY`] when they were freed.
    pub held_canary: usize,
    /// How many held any non-zero byte at all. Under `WipeOnFree` this must be zero whatever the
    /// canary's encoding and whatever the block's size: the canary count alone let a wipe that
    /// skipped blocks over 4 KiB pass (#199's review), because every canary block here was small
    /// until the document was padded.
    pub dirty: usize,
    /// The size of the largest freed block that held the canary. The control requires it past
    /// 64 KiB, so a size-dependent wipe is examined at a size it would skip.
    pub largest_canary_block: usize,
}

/// Run `f` with the witness armed, and report what it saw.
pub fn watch<T>(f: impl FnOnce() -> T) -> (T, Seen) {
    FREED.store(0, Ordering::SeqCst);
    FREED_BYTES.store(0, Ordering::SeqCst);
    HELD_CANARY.store(0, Ordering::SeqCst);
    DIRTY.store(0, Ordering::SeqCst);
    LARGEST_CANARY_BLOCK.store(0, Ordering::SeqCst);
    ARMED.store(true, Ordering::SeqCst);
    let out = f();
    ARMED.store(false, Ordering::SeqCst);
    let seen = Seen {
        freed: FREED.load(Ordering::SeqCst),
        freed_bytes: FREED_BYTES.load(Ordering::SeqCst),
        held_canary: HELD_CANARY.load(Ordering::SeqCst),
        dirty: DIRTY.load(Ordering::SeqCst),
        largest_canary_block: LARGEST_CANARY_BLOCK.load(Ordering::SeqCst),
    };
    (out, seen)
}

/// Scans each block it frees, and then frees it through `A`.
pub struct Witness<A>(pub A);

impl<A> Witness<A> {
    fn examine(ptr: *mut u8, len: usize) {
        if !ARMED.load(Ordering::Relaxed) {
            return;
        }
        FREED.fetch_add(1, Ordering::Relaxed);
        FREED_BYTES.fetch_add(len, Ordering::Relaxed);
        // SAFETY: the allocator's caller still owns `len` bytes at `ptr`; nothing here allocates.
        let block = unsafe { std::slice::from_raw_parts(ptr, len) };
        if block.windows(CANARY.len()).any(|window| window == CANARY) {
            HELD_CANARY.fetch_add(1, Ordering::Relaxed);
            LARGEST_CANARY_BLOCK.fetch_max(len, Ordering::Relaxed);
        }
        if block.iter().any(|&byte| byte != 0) {
            DIRTY.fetch_add(1, Ordering::Relaxed);
        }
    }
}

// SAFETY: forwards to `A`; `realloc` is built from `A`'s `alloc` and `dealloc` so that the block
// it moves away from is examined like any other free.
unsafe impl<A: GlobalAlloc> GlobalAlloc for Witness<A> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { self.0.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        Self::examine(ptr, layout.size());
        unsafe { self.0.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let Ok(new_layout) = Layout::from_size_align(new_size, layout.align()) else {
            return std::ptr::null_mut();
        };
        let moved = unsafe { self.0.alloc(new_layout) };
        if !moved.is_null() {
            unsafe {
                std::ptr::copy_nonoverlapping(ptr, moved, layout.size().min(new_size));
                self.dealloc(ptr, layout);
            }
        }
        moved
    }
}

/// How far the content stream is padded: past 64 KiB, so the decoded content -- and every copy of
/// it that holds the canary -- is a large block. A wipe that skipped large blocks would otherwise
/// pass, since an unpadded page's copies are all under 1 KiB.
const PADDING: usize = 80 * 1024;

/// One page: the canary inside the region, and a kept word outside it.
pub fn canary_document() -> Vec<u8> {
    use crate::support::pdf_builder::Builder;

    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let page = pdf.reserve();
    let font = pdf.add(&format!(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 32 /LastChar 94 \
         /Widths {} >>",
        crate::support::pdf_builder::HELVETICA_WIDTHS
    ));
    let canary = std::str::from_utf8(CANARY).unwrap();
    let content = pdf.stream(
        "",
        &format!(
            "BT /F1 24 Tf 72 700 Td ({canary}) Tj ET\nBT /F1 24 Tf 72 300 Td (KEPT) Tj ET\n{}\n",
            "q Q ".repeat(PADDING.div_euclid(4))
        ),
    );
    pdf.put(
        page,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 {font} 0 R >> >> /Contents {content} 0 R >>"
        ),
    );
    pdf.put(
        pages,
        &format!("<< /Type /Pages /Count 1 /Kids [{page} 0 R] >>"),
    );
    pdf.put(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    pdf.build(catalog)
}

/// The region over the canary: the page's upper band, in display coordinates.
pub fn region() -> burrow_engines::pdfsyntax::region::Region {
    burrow_engines::pdfsyntax::region::Region {
        left: 40.0,
        top: 40.0,
        width: 500.0,
        height: 120.0,
    }
}

/// Redact the canary document's only page, with the witness armed for exactly that call.
pub fn redact_watched() -> (Vec<u8>, Seen) {
    let input = canary_document();
    assert!(
        input.windows(CANARY.len()).any(|w| w == CANARY),
        "the fixture must carry the canary uncompressed, or nothing below is measured"
    );
    let redacted: std::collections::BTreeSet<usize> = [0].into_iter().collect();
    let (result, seen) = watch(|| crate::support::redact_page(&input, 0, redacted, region()));
    let (output, _report) = match result {
        Ok(done) => done,
        Err(error) => panic!("the canary document must redact, and it refused: {error:?}"),
    };
    println!(
        "witness: {} blocks freed ({} bytes) during the redaction; {} still holding the canary \
         (largest {} bytes), {} holding any non-zero byte",
        seen.freed, seen.freed_bytes, seen.held_canary, seen.largest_canary_block, seen.dirty
    );
    (output, seen)
}

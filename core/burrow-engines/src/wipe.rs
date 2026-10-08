//! A global allocator that zeroes every block before it returns it (#199).
//!
//! # Why the whole heap, and not the buffers somebody listed
//!
//! Redaction reads a page's content **decoded**, and that is the text being removed. On the web a
//! worker serves many documents in one session, so a buffer freed with its bytes intact sits in
//! the allocator's free list, readable by whatever allocates next: the secret persisting in the
//! worker after the operation that removed it from the document.
//!
//! Wrapping the buffers the engine hands back in `Zeroizing` would wipe those buffers and nothing
//! derived from them: the lexer's tokens, a string operand copied out of a `TJ` array, the edited
//! stream, a rewritten `/ToUnicode` program, every `Vec` that grew and left its old block behind.
//! Each is a copy of the content, and a list of them is exactly the kind of list that is complete
//! on the day it is written. Wiping at the allocator covers every one of them, including the ones
//! nobody has thought of, because every Rust heap block passes through here on its way out.
//!
//! # What it does not reach
//!
//! - **Memory that is never freed** while the worker lives. A live buffer is not wiped, because
//!   it is still in use; that is a leak to fix where it is, and the wasm-heap canary is what
//!   would see it.
//! - **The engines' own heaps.** qpdf and PDFium allocate with their own `malloc`, in a separate
//!   Emscripten module on the web and in C++ natively. This allocator sees none of it.
//!   `bridge-qpdf.js` zeroes the buffers qpdf hands across; qpdf's internal object cache is
//!   out of the C API's reach, which is why the worker is recycled.
//! - **The stack and registers.** Not heap, and not addressed here.
//!
//! # Cost
//!
//! One pass of writes over every block freed, and `realloc` loses the in-place growth path:
//! it always allocates, copies, wipes and frees, because an in-place `realloc` that moves the
//! block frees the old one inside the inner allocator, where nothing can wipe it.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{Ordering, compiler_fence};

/// A [`GlobalAlloc`] that zeroes every block before handing it back to `A`.
///
/// Declare it once, in a binding:
///
/// ```ignore
/// #[global_allocator]
/// static ALLOCATOR: WipeOnFree = WipeOnFree::system();
/// ```
///
/// The zeroing uses volatile writes followed by a compiler fence. An ordinary `memset` before a
/// `free` is a dead store an optimiser is entitled to remove, which would leave this allocator
/// wiping nothing in a release build while every debug-build test passed.
#[derive(Debug, Default)]
pub struct WipeOnFree<A = System> {
    inner: A,
}

impl WipeOnFree<System> {
    /// Wipe-on-free over the platform's allocator: `malloc` natively, `dlmalloc` on
    /// `wasm32-unknown-unknown`.
    #[must_use]
    pub const fn system() -> Self {
        Self { inner: System }
    }
}

impl<A> WipeOnFree<A> {
    /// Wipe-on-free over `inner`. The tests use this to put a witness underneath.
    #[must_use]
    pub const fn new(inner: A) -> Self {
        Self { inner }
    }
}

/// Zero `len` bytes at `ptr` with writes the optimiser may not drop.
///
/// # Safety
///
/// `ptr` must be valid for writes of `len` bytes.
unsafe fn wipe(ptr: *mut u8, len: usize) {
    const WORD: usize = core::mem::size_of::<u64>();
    // Bytes up to the first word boundary, then words, then the tail. Word writes because a
    // byte-at-a-time volatile loop is the slowest correct way to do this, and it runs on
    // every free.
    let head = ptr.align_offset(WORD).min(len);
    for offset in 0..head {
        // SAFETY: `offset < head <= len`, inside the block the caller vouched for.
        unsafe { ptr.add(offset).write_volatile(0) };
    }
    let words = (len - head).div_euclid(WORD);
    // SAFETY: `head <= len`, so this stays inside the block; it is the first word boundary,
    // so `cast` to `u64` yields an aligned pointer.
    #[allow(clippy::cast_ptr_alignment)]
    let aligned = unsafe { ptr.add(head) }.cast::<u64>();
    for word in 0..words {
        // SAFETY: `head + (word + 1) * WORD <= len`, and `aligned` is 8-byte aligned.
        unsafe { aligned.add(word).write_volatile(0) };
    }
    for offset in head + words * WORD..len {
        // SAFETY: `offset < len`.
        unsafe { ptr.add(offset).write_volatile(0) };
    }
    compiler_fence(Ordering::SeqCst);
}

// SAFETY: every method forwards to `inner`, which upholds `GlobalAlloc`'s contract, and adds
// only writes inside a block the caller still owns. `realloc` is built from `inner`'s `alloc` and
// `dealloc` with the same layouts the contract would have passed to `inner.realloc`.
unsafe impl<A: GlobalAlloc> GlobalAlloc for WipeOnFree<A> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded unchanged; the caller upholds `alloc`'s contract.
        unsafe { self.inner.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded unchanged.
        unsafe { self.inner.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller owns `ptr`, allocated with `layout`, so `layout.size()` bytes are
        // writable until it is handed back on the next line.
        unsafe {
            wipe(ptr, layout.size());
            self.inner.dealloc(ptr, layout);
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // `GlobalAlloc::realloc`'s contract guarantees this layout is valid, so `Err` is
        // unreachable; returning null is the contract's own way to say "could not".
        let Ok(new_layout) = Layout::from_size_align(new_size, layout.align()) else {
            return core::ptr::null_mut();
        };
        // SAFETY: `new_layout` has a non-zero size, as `realloc`'s contract requires of
        // `new_size`.
        let moved = unsafe { self.inner.alloc(new_layout) };
        if !moved.is_null() {
            // SAFETY: both blocks are live and distinct; the copy is the smaller of their sizes.
            // The old block is then wiped and freed through `dealloc`, with its own layout.
            unsafe {
                core::ptr::copy_nonoverlapping(ptr, moved, layout.size().min(new_size));
                self.dealloc(ptr, layout);
            }
        }
        // On failure the old block is untouched and still the caller's, as the contract says.
        moved
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every length and alignment offset the three loops split on, wiped exactly: nothing left,
    /// and nothing written past the end.
    #[test]
    fn wipe_zeroes_exactly_the_block() {
        for start in 0..16 {
            for len in 0..40 {
                let mut buffer = [0xAAu8; 64];
                let base = buffer.as_mut_ptr();
                // SAFETY: `start + len <= 56 < 64`.
                unsafe { wipe(base.add(start), len) };
                for (i, byte) in buffer.iter().enumerate() {
                    let inside = (start..start + len).contains(&i);
                    assert_eq!(
                        *byte,
                        if inside { 0 } else { 0xAA },
                        "start {start} len {len} at {i}"
                    );
                }
            }
        }
    }

    /// An inner allocator that checks each block is zero when it is handed back, and refuses to
    /// `realloc` at all: an inner `realloc` that moved the block would free the old one where
    /// nothing wipes it.
    struct MustArriveZeroed {
        freed: std::sync::atomic::AtomicUsize,
        freed_dirty: std::sync::atomic::AtomicUsize,
    }

    // SAFETY: forwards to `System`.
    unsafe impl GlobalAlloc for MustArriveZeroed {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            unsafe { System.alloc(layout) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            let block = unsafe { std::slice::from_raw_parts(ptr, layout.size()) };
            self.freed.fetch_add(1, Ordering::SeqCst);
            if block.iter().any(|&b| b != 0) {
                self.freed_dirty.fetch_add(1, Ordering::SeqCst);
            }
            unsafe { System.dealloc(ptr, layout) }
        }
        unsafe fn realloc(&self, _: *mut u8, _: Layout, _: usize) -> *mut u8 {
            panic!(
                "WipeOnFree must not delegate realloc: the inner one frees the old block unwiped"
            )
        }
    }

    /// The block a `realloc` moves away from is wiped before the inner allocator gets it back,
    /// growing and shrinking. The witness test's redaction never grows a block that holds its
    /// canary, so a `realloc` that skipped the wipe survived it; this is the test for that path.
    #[test]
    fn realloc_wipes_the_block_it_leaves() {
        let allocator = WipeOnFree::new(MustArriveZeroed {
            freed: std::sync::atomic::AtomicUsize::new(0),
            freed_dirty: std::sync::atomic::AtomicUsize::new(0),
        });
        let layout = Layout::from_size_align(40, 8).unwrap();
        // SAFETY: a non-zero layout; every pointer is checked and freed with its own layout.
        unsafe {
            let ptr = allocator.alloc(layout);
            assert!(!ptr.is_null());
            ptr.write_bytes(0xAA, 40);
            let grown = allocator.realloc(ptr, layout, 300);
            assert!(!grown.is_null());
            grown.write_bytes(0xBB, 300);
            let shrunk = allocator.realloc(grown, Layout::from_size_align(300, 8).unwrap(), 3);
            assert!(!shrunk.is_null());
            shrunk.write_bytes(0xCC, 3);
            allocator.dealloc(shrunk, Layout::from_size_align(3, 8).unwrap());
        }
        assert_eq!(
            allocator.inner.freed.load(Ordering::SeqCst),
            3,
            "three blocks handed back"
        );
        assert_eq!(
            allocator.inner.freed_dirty.load(Ordering::SeqCst),
            0,
            "each one zeroed"
        );
    }

    /// `realloc` keeps the bytes it moved, in both directions.
    #[test]
    fn realloc_keeps_the_prefix() {
        let allocator = WipeOnFree::system();
        let layout = Layout::from_size_align(24, 8).unwrap();
        // SAFETY: a non-zero layout; every pointer is checked and freed with its own layout.
        unsafe {
            let ptr = allocator.alloc(layout);
            assert!(!ptr.is_null());
            for i in 0..24 {
                ptr.add(i).write(u8::try_from(i).unwrap());
            }
            let grown = allocator.realloc(ptr, layout, 100);
            assert!(!grown.is_null());
            for i in 0..24 {
                assert_eq!(grown.add(i).read(), u8::try_from(i).unwrap());
            }
            let grown_layout = Layout::from_size_align(100, 8).unwrap();
            let shrunk = allocator.realloc(grown, grown_layout, 5);
            assert!(!shrunk.is_null());
            for i in 0..5 {
                assert_eq!(shrunk.add(i).read(), u8::try_from(i).unwrap());
            }
            allocator.dealloc(shrunk, Layout::from_size_align(5, 8).unwrap());
        }
    }
}

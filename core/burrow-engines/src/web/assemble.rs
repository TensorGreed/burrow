//! The web implementation of [`PageAssembler`], over the JS bridge.
//!
//! **The same Rust as the native path, against a different seam.** `qpdf/assemble.rs` calls
//! `ffi::qpdf_add_page`; this calls `bridge.add_page`. Everything between — the limit
//! ordering, the aggregate page ceiling, which errors are which, when the sources are
//! released — is duplicated deliberately rather than shared, for the reason
//! `web/mod.rs` gives: the two are separate implementations of one trait and the
//! differential harness is what holds them together. Code shared between them could not
//! diverge; code that *cannot* diverge cannot be caught diverging either.
//!
//! Where they genuinely differ, it is noted at the point of difference rather than smoothed
//! over.
//!
//! # The loop crosses the boundary once per page, and that is the design
//!
//! [ADR 0009](../../../../docs/adr/0009-web-panic-contract-and-binding-boundary.md) §2 is
//! explicit: a binding may marshal data and hold handles, but "no branch on engine state
//! may live in JS". So the per-page `get_page_n` / `add_page` pair is driven from here, one
//! round trip each, rather than handed to the bridge as "append this document". That costs
//! call overhead on a path that is already the slow one, and the alternative is a decision
//! in the one place `cargo test` cannot reach and iOS cannot reuse.

use std::sync::Arc;

use burrow_types::{Deadline, Error, Limits, Result, Stage};

use super::qpdf::{Session, WebQpdf};
use crate::{OpenOptions, PageAssembler};

/// A merge in progress, on the web.
///
/// **Every source stays open until [`finish`](PageAssembler::finish).** qpdf resolves a
/// foreign page's indirect objects lazily, during the write, so releasing one early yields
/// a truncated document rather than an error — the silent data loss
/// [ADR 0017](../../../../docs/adr/0017-merge-engine-and-failure-semantics.md) §2 refuses.
///
/// Here that also means the engine-heap buffers stay allocated: each open document owns its
/// input buffer inside the module's heap and frees it on drop. So the peak heap for a merge
/// is the sum of every input, and the recycling threshold in [`super::recycle`] is what
/// notices.
///
/// (The type holding each one is private to this module's sibling, so it is described rather
/// than linked -- a public item cannot link to it.)
pub struct WebAssembly {
    dest: Session,
    /// Held, not used. Dropping one before `finish` truncates the output.
    sources: Vec<Session>,
    pages: u64,
    heap_before: u64,
    limits: Limits,
}

impl PageAssembler for WebQpdf {
    type Assembly = WebAssembly;

    fn name(&self) -> &'static str {
        "qpdf-wasm"
    }

    fn begin(&self, first: Box<[u8]>, options: &OpenOptions<'_>) -> Result<Self::Assembly> {
        let limits = options.limits;

        let input_len = u64::try_from(first.len())
            .map_err(|_| Error::Internal("input length does not fit in u64".to_owned()))?;
        Limits::check(
            Stage::InputSize,
            "max_input_bytes",
            input_len,
            limits.max_input_bytes,
        )?;

        // The structural pre-scan, per input, before the engine sees the bytes. Pure Rust,
        // `forbid(unsafe_code)`, and the only pre-emptive defence there is (ADR 0013).
        crate::prescan::check(&first, &limits)?;

        let clock = Arc::clone(&options.clock);
        let deadline = Deadline::start(clock.as_ref(), &limits);
        deadline.checkpoint(clock.as_ref())?;

        let heap_before = self.bridge().heap_bytes();

        // Recovery OFF, as on native and for the stronger reason merge gives it: a merge
        // that silently reconstructs a damaged input produces an output whose relationship
        // to what the person handed us is unclear, and they will never know.
        let dest = Session::open(self, &first, options.password, false)?;
        drop(first);

        let pages = dest.page_count()?;
        Limits::check(Stage::PageCount, "max_pages", pages, limits.max_pages)?;

        if let Some(error) = dest.take_error() {
            return Err(error);
        }

        Ok(WebAssembly {
            dest,
            sources: Vec::new(),
            pages,
            heap_before,
            limits,
        })
    }

    fn pages(&self, assembly: &Self::Assembly) -> Result<u64> {
        Ok(assembly.pages)
    }

    fn append(
        &self,
        assembly: &mut Self::Assembly,
        next: Box<[u8]>,
        options: &OpenOptions<'_>,
    ) -> Result<u64> {
        let limits = options.limits;

        let input_len = u64::try_from(next.len())
            .map_err(|_| Error::Internal("input length does not fit in u64".to_owned()))?;
        Limits::check(
            Stage::InputSize,
            "max_input_bytes",
            input_len,
            limits.max_input_bytes,
        )?;
        crate::prescan::check(&next, &limits)?;

        let source = Session::open(self, &next, options.password, false)?;
        drop(next);
        let count = source.page_count()?;

        // The ceiling is on the OUTPUT, checked before a single page is copied. ADR 0017 §3.
        let total = assembly
            .pages
            .checked_add(count)
            .ok_or_else(|| Error::Internal("merged page count does not fit in u64".to_owned()))?;
        Limits::check(Stage::PageCount, "max_pages", total, limits.max_pages)?;

        for index in 0..count {
            let n = u32::try_from(index)
                .map_err(|_| Error::Internal("page index does not fit in u32".to_owned()))?;

            let page = self.bridge().get_page_n(source.data(), n);
            if let Some(error) = source.take_error() {
                return Err(error);
            }

            // `false` is `first`: append rather than prepend. The handle belongs to
            // `source`, which is why `source` is passed alongside it -- handles are
            // per-document and qpdf resolves this one against the document it is given.
            let added = self
                .bridge()
                .add_page(assembly.dest.data(), source.data(), page, false);

            // The ERROR bit, never `!= 0`: a warning here is an ordinary outcome.
            if crate::codes::qpdf::has_errors(added) {
                return Err(assembly
                    .dest
                    .take_error()
                    .or_else(|| source.take_error())
                    .unwrap_or_else(|| {
                        Error::Malformed("qpdf: a page could not be appended".to_owned())
                    }));
            }
            if let Some(error) = assembly.dest.take_error() {
                return Err(error);
            }
        }

        assembly.sources.push(source);
        assembly.pages = total;
        Ok(count)
    }

    fn finish(&self, assembly: Self::Assembly) -> Result<Vec<u8>> {
        let WebAssembly {
            dest,
            sources,
            pages: _,
            heap_before,
            limits,
        } = assembly;

        let init = self.bridge().init_write_memory(dest.data());
        if crate::codes::qpdf::has_errors(init) {
            return Err(dest.take_error().unwrap_or_else(|| {
                Error::Io("qpdf could not prepare an in-memory write".to_owned())
            }));
        }

        // AFTER `init_write_memory`, never before: the writer does not exist until the call
        // above succeeds, and this dereferences it. Calling it first takes the process down
        // -- measured on the native path, by core dump, during ADR 0017's comparison.
        self.bridge().set_deterministic_id(dest.data(), true);

        let wrote = self.bridge().write(dest.data());
        if crate::codes::qpdf::has_errors(wrote) {
            return Err(dest.take_error().unwrap_or_else(|| {
                Error::Malformed("qpdf: the merge could not be written".to_owned())
            }));
        }
        if let Some(error) = dest.take_error() {
            return Err(error);
        }

        let len = self.bridge().get_buffer_length(dest.data());
        let ptr = self.bridge().get_buffer(dest.data());

        // BOTH are checked, not one. qpdf returns a null buffer when the writer has none,
        // and reports the length from a separate accessor -- so a caller that trusted only
        // the length would read from null, and one that trusted only the pointer would copy
        // zero bytes and call it a document.
        if ptr.is_null() || len == 0 {
            return Err(Error::Io("qpdf produced no output".to_owned()));
        }

        let out = self.bridge().copy_out(ptr, len);

        // Dropped here, after the write and after the copy out of the engine heap, and in
        // this order so the reason stays visible: the sources were needed until `write`
        // returned, and the buffer `ptr` points into belongs to `dest`.
        drop(sources);
        drop(dest);

        // What the whole assembly cost. `max_memory_bytes` DETECTS rather than bounds --
        // ADR 0007's 2026-09-12 amendment -- and a merge is the clearest case for saying
        // so: every input has already been copied into the engine heap by the time this
        // runs, and the heap never shrinks.
        crate::estimate::check_measured_memory(
            Some(heap_before),
            Some(self.bridge().heap_bytes()),
            &limits,
        )?;

        Ok(out)
    }
}

/// Sanity: the assembler and the structure check agree about which engine they are.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_assembler_names_the_same_engine_as_the_structure_check() {
        // Two traits, one struct, and the differential harness keys outcomes by engine
        // name. A mismatch would make the web assembler's results look like another
        // engine's.
        let engine = WebQpdf::new(Arc::new(super::super::fake::FakeQpdf::new(
            super::super::fake::FakeHeap::new(),
            super::super::fake::QpdfScript::default(),
        )));
        assert_eq!(
            PageAssembler::name(&engine),
            <WebQpdf as crate::StructureEngine>::name(&engine)
        );
    }
}

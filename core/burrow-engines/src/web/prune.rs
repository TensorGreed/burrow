//! The web half of the pruning seam: the JS bridge, and nothing else.
//!
//! **The policy lives in [`crate::prune`] and is written once.** This file is the marshalling —
//! a key is copied into the engine heap and freed, a handle is a bare `u32` that has to be
//! released, and the session's latched error is drained after every call so the policy never has
//! to.
//!
//! Its native twin is `crate::qpdf::prune`, and the two are *expected* to be near-identical in
//! shape: that is what makes them auditable against each other. What must never be duplicated is
//! the decision about what to remove. See `crate::prune`'s header for why this one capability is
//! shared where `rotate`, `reorder` and `merge` deliberately are not.
//!
//! # Releasing handles was this file's whole burden, and is now [`super::handle`]'s
//!
//! The native seam hands back an `ObjectHandle` that releases on drop, so "remember to release"
//! is not something a reader has to check. Across the bridge a handle is a `u32` with no `Drop`,
//! and this file grew its own [`WebHandle`] to close that — while
//! `reorder.rs` grew a different guard and `rotate.rs`, `extract.rs` and `compress.rs` released
//! by hand at every exit. Three answers to one question.
//!
//! That type is shared now, and it carries a second property this file never needed and the
//! others did: a handle and the `qpdf_data` it belongs to are **one value**, so no call can pair
//! a handle from one document with another's. Pruning takes far more handles than the rest — a
//! key lookup, an array item, a stream dictionary, each per page and several per resource — so
//! this is still where the cost of getting it wrong is largest. `qpdf_oh_release` is a map erase;
//! not releasing is one cache entry per node for the life of the document, invisible to
//! `max_memory_bytes` because it is steady growth well under a ceiling.

use core::cell::RefCell;
use std::collections::BTreeMap;

use burrow_types::{Error, Result};

use super::WebQpdf;
use super::bridge::QpdfPtr;
use super::handle::WebHandle;
use super::qpdf::Session;
use crate::prune::graph::ObjectGraph;

/// A document reached across the bridge, seen as an object graph.
pub(super) struct WebGraph<'a> {
    engine: &'a WebQpdf,
    session: &'a Session,
    /// Keys copied into the engine heap, freed when the graph is dropped.
    ///
    /// **One allocation per DISTINCT key**, held until the graph drops. The policy asks for the
    /// same dozen keys over and over — `/Resources`, `/Annots`, `/Type` — once per page, per
    /// resource, per nested stream, so allocating per lookup would be a large number of small
    /// engine-heap allocations all retained for the whole part.
    keys: RefCell<BTreeMap<Vec<u8>, QpdfPtr>>,
}

impl<'a> WebGraph<'a> {
    /// See `session` as a graph.
    pub(super) fn over(engine: &'a WebQpdf, session: &'a Session) -> Self {
        Self {
            engine,
            session,
            keys: RefCell::new(BTreeMap::new()),
        }
    }

    /// Drain whatever the session latched, as an error.
    ///
    /// **After every call, without exception** — see the native seam for the same rule and the
    /// same reason: qpdf reports failure by latching, so a missed drain surfaces later against
    /// something unrelated.
    fn drained(&self) -> Result<()> {
        self.session.take_error().map_or(Ok(()), Err)
    }

    /// A key in the engine heap, NUL-terminated, kept alive for the life of the graph.
    fn key_ptr(&self, key: &[u8]) -> Result<QpdfPtr> {
        // MEMOISED, and it was not until code review measured the difference between this and
        // its own comment. The comment said "one allocation per distinct key rather than per
        // use"; the code did an unconditional `copy_in` on every lookup and kept the pointer
        // until the graph dropped. The policy asks for the same dozen keys -- `/Resources`,
        // `/Annots`, `/Type` -- once per page, per resource, per nested stream, so that was a
        // large number of small engine-heap allocations all retained for the whole part, on the
        // platform with the fixed ceiling. The native twin allocates a Rust `Vec` and drops it
        // immediately, so the web path was strictly worse AND documented itself as better.
        if let Some(ptr) = self.keys.borrow().get(key) {
            return Ok(*ptr);
        }
        let mut owned = Vec::with_capacity(key.len() + 1);
        owned.extend_from_slice(key);
        owned.push(0);
        let ptr = self.engine.bridge().copy_in(&owned);
        if ptr == QpdfPtr::NULL {
            return Err(Error::Internal(
                "the engine could not allocate a dictionary key".to_owned(),
            ));
        }
        self.keys.borrow_mut().insert(key.to_vec(), ptr);
        Ok(ptr)
    }

    /// Wrap a handle the session just issued.
    fn own(&self, handle: u32) -> WebHandle<'a> {
        WebHandle::owned(self.engine, self.session, handle)
    }
}

impl Drop for WebGraph<'_> {
    fn drop(&mut self) {
        for ptr in self.keys.borrow().values() {
            // `free`, not `wipe_and_free`: a dictionary key is a name out of the specification,
            // not anything a person typed. The wipe exists for passwords, and spending it here
            // would say these were secret.
            self.engine.bridge().free(*ptr);
        }
    }
}

impl<'a> ObjectGraph for WebGraph<'a> {
    type Handle = WebHandle<'a>;

    fn page(&self, index: usize) -> Result<Self::Handle> {
        let pages = self.session.page_count()?;
        let at = u64::try_from(index)
            .map_err(|_| Error::Internal("page index does not fit in u64".to_owned()))?;
        if at >= pages {
            return Err(Error::InvalidArgument(
                "page is not in the document".to_owned(),
            ));
        }
        let n = u32::try_from(index)
            .map_err(|_| Error::Internal("page index does not fit in u32".to_owned()))?;
        let handle = self.engine.bridge().get_page_n(self.session.data(), n);
        // OWNED BEFORE THE DRAIN. qpdf allocates a handle on the error path too --
        // `trap_oh_errors`' fallback is `return_uninitialized`, which calls `new_object` -- so
        // draining first and returning past the raw `u32` leaks one `oh_cache` entry per
        // failure. Wrapping first makes every path release. `web/reorder.rs` uses the same
        // shape; found by security review.
        let owned = self.own(handle);
        self.drained()?;
        Ok(owned)
    }

    fn key(&self, of: &Self::Handle, key: &[u8]) -> Result<Self::Handle> {
        let ptr = self.key_ptr(key)?;
        // OWNED BEFORE THE DRAIN, which `WebHandle::key` does by construction: qpdf allocates
        // a handle on the error path too -- `trap_oh_errors`' fallback is
        // `return_uninitialized`, which calls `new_object` -- so returning past a raw `u32`
        // leaks one `oh_cache` entry per failure. Found by security review; the type holds it.
        let owned = of.key(ptr);
        of.drained()?;
        Ok(owned)
    }

    fn type_code(&self, of: &Self::Handle) -> Result<i32> {
        let code = of.type_code();
        of.drained()?;
        Ok(code)
    }

    fn name(&self, of: &Self::Handle) -> Result<Vec<u8>> {
        let ptr = of.name();
        of.drained()?;
        if ptr == QpdfPtr::NULL {
            return Ok(Vec::new());
        }
        Ok(self.engine.bridge().copy_c_string(ptr))
    }

    fn unparse(&self, of: &Self::Handle) -> Result<Vec<u8>> {
        let ptr = of.unparse_resolved();
        of.drained()?;
        if ptr == QpdfPtr::NULL {
            return Ok(Vec::new());
        }
        // COPIED IMMEDIATELY. The pointer belongs to the engine and dies on the next call that
        // returns one — including the next `unparse`, which is exactly what a loop over a page's
        // keys does.
        Ok(self.engine.bridge().copy_c_string(ptr))
    }

    fn remove_key(&self, of: &Self::Handle, key: &[u8]) -> Result<()> {
        let ptr = self.key_ptr(key)?;
        of.remove_key(ptr);
        of.drained()
    }

    fn array_len(&self, of: &Self::Handle) -> Result<i32> {
        let len = of.array_len();
        of.drained()?;
        Ok(len)
    }

    fn array_item(&self, of: &Self::Handle, at: i32) -> Result<Self::Handle> {
        let handle = of.array_item(at);
        // ALREADY OWNED: the accessor returns a `WebHandle`, so the "wrap before draining"
        // rule this used to spell out is held by the type. qpdf allocates a handle on the
        // error path too, and returning past a raw `u32` leaked one `oh_cache` entry per
        // failure; found by security review.
        of.drained()?;
        Ok(handle)
    }

    fn erase_item(&self, of: &Self::Handle, at: i32) -> Result<()> {
        of.erase_item(at);
        of.drained()
    }

    fn stream_dict(&self, of: &Self::Handle) -> Result<Self::Handle> {
        let handle = of.stream_dict();
        // ALREADY OWNED: the accessor returns a `WebHandle`, so the "wrap before draining"
        // rule this used to spell out is held by the type. qpdf allocates a handle on the
        // error path too, and returning past a raw `u32` leaked one `oh_cache` entry per
        // failure; found by security review.
        of.drained()?;
        Ok(handle)
    }

    fn page_content(&self, page: &Self::Handle) -> Result<Vec<u8>> {
        let data = page.page_content();
        page.drained()?;
        // `None` HERE IS NOT A DECODE FAILURE. The bridge returns it only when the engine
        // could not allocate its scratch words; a qpdf throw is latched and `drained` above has
        // already returned it. So this is the module out of memory rather than anything about
        // the document, and it is `Internal` rather than `Malformed`.
        data.ok_or_else(|| {
            Error::Internal(
                "the qpdf module could not allocate to read a page's content".to_owned(),
            )
        })
    }

    fn stream_data(&self, of: &Self::Handle) -> Result<Option<Vec<u8>>> {
        let data = of.stream_data();
        of.drained()?;
        // `None` ALREADY MEANS "could not decode" here, and the bridge does not distinguish that
        // from an error because the policy treats them the same way and must. See the bridge
        // method: undecoded bytes are still compressed, and reading names out of them is an
        // under-approximation, which deletes a resource the page draws with.
        Ok(data)
    }

    fn identity(&self, of: &Self::Handle) -> Result<(i32, i32)> {
        let packed = of.object();
        // DRAINED AND RETURNED, never swallowed. Both halves read as 0 on an internal failure and
        // `(0, 0)` equals `(0, 0)`, so a swallowed error here is two unrelated objects comparing
        // equal — which for pruning means keeping what should have gone. `handle.rs` has the
        // native half of this argument.
        self.drained()?;
        let number = i32::try_from(packed >> 32)
            .map_err(|_| Error::Internal("an object number does not fit in i32".to_owned()))?;
        let generation = i32::try_from(packed & 0xffff_ffff)
            .map_err(|_| Error::Internal("a generation does not fit in i32".to_owned()))?;
        Ok((number, generation))
    }

    fn integer_value(&self, of: &Self::Handle) -> Result<i64> {
        let value = of.int_value();
        self.drained()?;
        Ok(value)
    }
}

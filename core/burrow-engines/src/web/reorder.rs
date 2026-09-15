//! The web implementation of [`PageReorderer`], over the JS bridge.
//!
//! **The same Rust as the native path, against a different seam.** `qpdf/reorder.rs` calls
//! `ffi::qpdf_remove_page`; this calls `bridge.remove_page`. The algorithm between — take
//! every page's handle up front, walk the target positions, move only the pages that are not
//! already where they belong — is duplicated deliberately rather than shared, for the reason
//! `web/mod.rs` gives: the two are separate implementations of one trait, held together by the
//! differential harness. Code that *cannot* diverge cannot be caught diverging either.
//!
//! # Handles are released by hand, and there are `n` of them at once
//!
//! Across the bridge a handle is a `u32` with nothing to hang a `Drop` on, so release is a
//! discipline rather than a type — `web/rotate.rs`'s header sets that out and this module
//! inherits it. Reorder makes it sharper in one way: `rotate` holds **one** page handle at a
//! time, and this holds **every page's** for the whole permutation, because a page's index
//! changes as its neighbours move and only its identity is stable.
//!
//! So the discipline here is not "release before returning" at each step but: the vector of
//! handles is owned by one guard, [`Pages`], whose `Drop` releases all of them. Every `?` in
//! the permutation returns past that guard, not past a bare release call — which is what the
//! native path gets from `ObjectHandle` and what this module has to build.
//!
//! # A handle is not an identity, and that is the whole reason `oh_object` exists
//!
//! `qpdf_oh oh = ++qpdf->next_oh` — a fresh handle on every call, so two handles to the same
//! page never compare equal. Comparing them made every native permutation fail with
//! `qpdf_e_pages`, the identity included. Identity is object number plus generation, which
//! crosses the bridge as one packed value precisely so the halves cannot be used apart: two
//! objects may share a number across generations. ADR 0013's handle-identity amendment.
//!
//! And it can fail **equal**: both reads return 0 on failure, so `(0, 0) == (0, 0)` reads as
//! "already in place" and the page is not moved. The error is drained after every comparison
//! rather than at the end, for that reason.
//!
//! **What the drain buys, stated precisely**, because the first version of this paragraph
//! claimed more. `trap_errors` only ever *sets* `qpdf->error`; nothing but `qpdf_get_error`
//! clears it. So a failure here is caught eventually whatever this module does — by the next
//! page's `page_handle`, or by the drain after the write. What the immediate drain prevents is
//! the case where there is no "next": a failed read on the **last** comparison, where without
//! it the loop ends having moved nothing and a document is written as though nothing needed
//! moving. `a_failed_identity_read_refuses_rather_than_skipping_the_page` puts the failure
//! exactly there, because with the failure anywhere earlier the test cannot tell the drain
//! from its absence — measured, twice, by deleting the drain and watching the test stay green.
//!
//! # Remove before inserting, or the page is copied instead of moved
//!
//! `Pages::insert` contains `if (pageobj_to_pages_pos.contains(newpage)) { newpage =
//! makeIndirectObject(newpage.copy()); }`. Inserting a page the document still holds
//! **duplicates the object**; removing first erases the ObjGen, so the move is a move. The
//! order is not a style choice and it is not checkable at the bridge.

use std::sync::Arc;

use burrow_types::{Deadline, Error, Limits, Permutation, Result, Stage};

use super::qpdf::{Session, WebQpdf};
use crate::{OpenOptions, PageReorderer};

/// A document held open for a reordering, over the bridge.
pub struct WebReorderable {
    session: Session,
    pages: u64,
    /// The engine heap before the document was opened. `max_memory_bytes` **detects rather
    /// than bounds** (ADR 0007), and detecting it needs a before.
    heap_before: u64,
    /// The ceilings the document was opened under, so a later caller cannot loosen them.
    limits: Limits,
    /// Set once a permutation has failed partway through.
    ///
    /// # Why a failed reorder poisons its source
    ///
    /// A permutation moves a page by REMOVING it and putting it back. If the removal succeeds
    /// and the insertion fails, the document in the engine has one page fewer than it started
    /// with — and `reorder` takes `&self` and `&Source`, so nothing in the type system stops a
    /// caller trying again and writing out a document that is silently short a page. Losing a
    /// page quietly is the one thing this operation's invariant forbids.
    ///
    /// No caller reaches it today: `burrow_ops::reorder` returns on the first error and the
    /// wasm entry point opens once. That is an argument for the flag being cheap, not for it
    /// being unnecessary — "no caller does this yet" is a property of today's callers, and the
    /// trait is public. Found by security review.
    poisoned: core::cell::Cell<bool>,
}

/// Every page's handle, released together.
///
/// # Why a guard rather than releasing at each step
///
/// The permutation holds `n` handles for its whole duration — it cannot hold one at a time,
/// because a page's *index* changes as its neighbours move and its handle is the only stable
/// reference to it. So the shape `rotate.rs` uses (take one, release it, `?` after) does not
/// apply: there is no single point where releasing is correct except the end.
///
/// A guard is what the native path gets for free from `ObjectHandle`'s `Drop`. Building it
/// here means every `?` in [`permute`] returns past a release of **all** the handles rather
/// than leaking the ones taken so far — which is the failure a hand-written release loop has
/// on exactly the error paths nobody exercises.
struct Pages<'a> {
    engine: &'a WebQpdf,
    data: super::bridge::QpdfPtr,
    handles: Vec<u32>,
}

impl Drop for Pages<'_> {
    fn drop(&mut self) {
        for &handle in &self.handles {
            self.engine.bridge().oh_release(self.data, handle);
        }
    }
}

impl PageReorderer for WebQpdf {
    type Source = WebReorderable;

    fn name(&self) -> &'static str {
        "qpdf-wasm"
    }

    fn open(&self, bytes: Box<[u8]>, options: &OpenOptions<'_>) -> Result<Self::Source> {
        let limits = options.limits;

        // EVERY CEILING THAT APPLIES BEFORE THE ENGINE, in one call: the byte count,
        // the size estimate, the structural pre-scan. Shared rather than spelled out
        // here, so a path that pre-scans without estimating is not writable --- see
        // `crate::estimate::before_open`, and #26 for what the drift cost last time.
        crate::estimate::before_open(&bytes, &limits)?;

        let clock = Arc::clone(&options.clock);
        let deadline = Deadline::start(clock.as_ref(), &limits);
        deadline.checkpoint(clock.as_ref())?;

        // Read before the open, so the measured check covers the `copy_in` as well as the
        // read -- the same window `rotate`'s open measures, and for the same reason.
        let heap_before = self.bridge().heap_bytes();

        // Recovery OFF, as every other operation has it.
        let session = Session::open(self, &bytes, options.password, false)?;
        let pages = session.page_count()?;
        Limits::check(Stage::PageCount, "max_pages", pages, limits.max_pages)?;

        if let Some(error) = session.take_error() {
            return Err(error);
        }

        crate::estimate::check_measured_memory(
            Some(heap_before),
            Some(self.bridge().heap_bytes()),
            &limits,
        )?;

        Ok(WebReorderable {
            session,
            pages,
            heap_before,
            limits,
            poisoned: core::cell::Cell::new(false),
        })
    }

    fn pages(&self, source: &Self::Source) -> Result<u64> {
        Ok(source.pages)
    }

    fn rotations(
        &self,
        source: &Self::Source,
        options: &OpenOptions<'_>,
        deadline: &Deadline,
    ) -> Result<Vec<i64>> {
        // THE SAME WALK `web/rotate.rs` DOES, against the document this is about to permute.
        // The two key strings are copied into the engine heap ONCE, by the same `Keys` guard
        // rotate uses, so the walk cannot fail on an allocation part-way through and leave a
        // handle behind -- the defect that module's header records.
        let keys = super::rotate::Keys::copy_in(self)?;
        let capacity = usize::try_from(source.pages)
            .map_err(|_| Error::Internal("page count does not fit in usize".to_owned()))?;
        let mut rotations = Vec::with_capacity(capacity);

        // PER PAGE, as the native sweep checkpoints and for the same measured reason: this is
        // one call from outside and a `/Parent` climb per page from inside, so unchecked it
        // sits outside every deadline. Against the CALLER'S deadline -- see the trait's docs.
        let clock = Arc::clone(&options.clock);

        for index in 0..source.pages {
            // BEFORE THE HANDLE IS ISSUED, so a refusal cannot leave one behind.
            deadline.checkpoint(clock.as_ref())?;
            let page = self.page_handle(&source.session, index, source.pages)?;
            // EVERY PATH RELEASES, as everywhere else across this bridge: `?` inside the loop
            // would return past the release.
            // RECORDED, NOT JUDGED -- see the native sweep's comment and the trait's docs.
            let outcome = super::rotate::declared_rotation(self, &source.session, &keys, page);
            self.bridge().oh_release(source.session.data(), page);
            rotations.push(outcome?.unwrap_or(0));
        }
        Ok(rotations)
    }

    fn reorder(
        &self,
        source: &Self::Source,
        order: &Permutation,
        options: &OpenOptions<'_>,
    ) -> Result<Vec<u8>> {
        // `options` carries the CLOCK; the CEILINGS come from `source.limits` -- the ceiling
        // that counts is the one the document was opened under.
        let clock = Arc::clone(&options.clock);
        let deadline = Deadline::start(clock.as_ref(), &source.limits);

        // THE PERMUTATION IS FOR THIS DOCUMENT. `Permutation` established that the order names
        // every page of SOME document exactly once; that it is this one is the check that
        // belongs here, where the page count is known.
        let length = u64::try_from(order.len())
            .map_err(|_| Error::Internal("page order length does not fit in u64".to_owned()))?;
        if length != source.pages {
            return Err(Error::InvalidArgument(format!(
                "the page order is for a document of {length} pages, and this one has {}",
                source.pages
            )));
        }

        // NO `max_pages` CHECK HERE, deliberately, matching the native path. One was written
        // there and code review measured that it could never fire: the length is required to
        // equal `source.pages` immediately above, and `open` already refused a document over
        // the ceiling under these same `Limits`. A ceiling that cannot fire reads as coverage.
        // `rotate` keeps its equivalent because a rotation's page LIST is caller-chosen and not
        // pinned to the page count, so there it is reachable.
        //
        // AN OPTIMISATION, AND ONLY AN OPTIMISATION -- the loop below already does nothing for
        // the identity, because every comparison matches. What preserves the page tree is that
        // nothing MOVED, not this branch; ADR 0021 records the measurement that established
        // the difference, and the native module carries the same note.
        // A FAILED PERMUTATION LEFT THE DOCUMENT SHORT. See `poisoned`: a removal that
        // succeeded with an insertion that did not leaves a page out of the tree, and writing
        // that out is silent loss.
        if source.poisoned.get() {
            return Err(Error::Internal(
                "this document was left part-way through a failed reordering and cannot be \
                 written; open it again"
                    .to_owned(),
            ));
        }

        if !order.is_identity() {
            // POISON ON ANY FAILURE, not only on a failed insertion: a deadline refusal
            // partway through the loop leaves the document equally half-permuted.
            if let Err(error) = self.permute(source, order, &deadline, clock.as_ref()) {
                source.poisoned.set(true);
                return Err(error);
            }
        }

        deadline.checkpoint(clock.as_ref())?;

        let init = self.bridge().init_write_memory(source.session.data());
        if crate::codes::qpdf::has_errors(init) {
            return Err(source.session.take_error().unwrap_or_else(|| {
                Error::Io("qpdf could not prepare an in-memory write".to_owned())
            }));
        }

        // AFTER `init_write_memory`, never before: the writer does not exist until that call
        // succeeds, and this dereferences it.
        self.bridge()
            .set_deterministic_id(source.session.data(), true);

        let wrote = self.bridge().write(source.session.data());
        if crate::codes::qpdf::has_errors(wrote) {
            return Err(source.session.take_error().unwrap_or_else(|| {
                Error::Malformed("qpdf: the output could not be written".to_owned())
            }));
        }
        if let Some(error) = source.session.take_error() {
            return Err(error);
        }

        let len = self.bridge().get_buffer_length(source.session.data());
        let ptr = self.bridge().get_buffer(source.session.data());
        // BOTH are checked, not one -- see `web/rotate.rs` for why the length alone is not
        // enough and the pointer alone is not either.
        if ptr.is_null() || len == 0 {
            return Err(Error::Io("qpdf produced no output".to_owned()));
        }
        let output = self.bridge().copy_out(ptr, len);

        crate::estimate::check_measured_memory(
            Some(source.heap_before),
            Some(self.bridge().heap_bytes()),
            &source.limits,
        )?;

        Ok(output)
    }
}

impl WebQpdf {
    /// Move each page to the position the order gives it.
    ///
    /// Split out so the handle guard has a scope: every `?` below returns past [`Pages`]'
    /// `Drop`, which releases all `n` handles.
    fn permute(
        &self,
        source: &WebReorderable,
        order: &Permutation,
        deadline: &Deadline,
        clock: &dyn burrow_types::Clock,
    ) -> Result<()> {
        let data = source.session.data();

        // EVERY PAGE'S HANDLE, TAKEN ONCE, UP FRONT. A page's index changes as its neighbours
        // move -- `get_page_n(3)` means something different after the first swap -- so the
        // loop works with identities rather than positions. Bounded by `max_pages`, checked
        // at open.
        let mut originals = Pages {
            engine: self,
            data,
            handles: Vec::with_capacity(order.len()),
        };
        for index in 0..source.pages {
            // CHECKPOINTED TOO. This loop is two bridge round trips per page before the
            // permutation proper begins, so without it `max_duration_ms` gets its first look
            // only after 2n crossings -- on a document at `max_pages` that is the ceiling
            // being unenforced for the part of the operation that runs first. Found by
            // security review.
            deadline.checkpoint(clock)?;
            let handle = self.page_handle(&source.session, index, source.pages)?;
            originals.handles.push(handle);
        }

        for (target, &wanted) in order.order().iter().enumerate() {
            // A page boundary, which is the granularity `Limits` promises. Each iteration is
            // a handful of bridge round trips and qpdf offers no cancellation hook.
            deadline.checkpoint(clock)?;

            let at = u64::try_from(target)
                .map_err(|_| Error::Internal("page index does not fit in u64".to_owned()))?;
            let wanted_index = usize::try_from(wanted)
                .map_err(|_| Error::Internal("page index does not fit in usize".to_owned()))?;
            let Some(&wanted_page) = originals.handles.get(wanted_index) else {
                // Unreachable: `Permutation` refused anything out of range and the length was
                // checked against this document above. A typed error rather than an index
                // panic, because library code here does not panic.
                return Err(Error::Internal(
                    "a validated page order named a page that is not in the document".to_owned(),
                ));
            };

            // WHERE IS IT NOW? Asked of the document rather than tracked in a local, because
            // the document is the thing being changed and a local would be a second model of
            // it that could disagree.
            let current = self.page_handle(&source.session, at, source.pages)?;
            let outcome = self.move_into_place(source, wanted_page, current);
            self.bridge().oh_release(data, current);
            outcome?;
        }

        Ok(())
    }

    /// Put `wanted_page` where `current` is, if it is not already there.
    ///
    /// Split out so `permute` can release `current` on every path: a `?` in here would return
    /// past that release, which is the leak `web/rotate.rs`'s header is about.
    fn move_into_place(
        &self,
        source: &WebReorderable,
        wanted_page: u32,
        current: u32,
    ) -> Result<()> {
        let data = source.session.data();

        // OBJECT IDENTITY, NOT HANDLE IDENTITY, and the error is drained immediately after.
        // Both halves return 0 on failure, so an undrained failure reads as "already in
        // place" -- the answer that means *do nothing* -- and the page is silently not moved.
        let here = self.bridge().oh_object(data, current);
        let there = self.bridge().oh_object(data, wanted_page);
        if let Some(error) = source.session.take_error() {
            return Err(error);
        }
        if here == there {
            // Already in place. The identity permutation reaches this on every page, and a
            // partial reorder reaches it on the pages nobody moved.
            return Ok(());
        }

        // OUT, THEN BACK IN BEFORE THE PAGE THAT IS THERE NOW. The handle stays valid across
        // the removal -- `Pages::erase` takes it out of `/Kids` and leaves the object alone --
        // and at least one page is in the tree throughout, which "remove them all, add them
        // back" would not give. Removing FIRST is also what makes this a move rather than a
        // copy; see `QpdfBridge::add_page_at`.
        let removed = self.bridge().remove_page(data, wanted_page);
        if crate::codes::qpdf::has_errors(removed) {
            return Err(source.session.take_error().unwrap_or_else(|| {
                Error::Malformed("qpdf: a page could not be taken out of the order".to_owned())
            }));
        }
        if let Some(error) = source.session.take_error() {
            return Err(error);
        }

        // `true` is `before`: the page lands at the target position rather than one past it.
        // The document is passed as both source and destination because the page is already
        // its own -- this is a move, not a copy.
        let added = self
            .bridge()
            .add_page_at(data, data, wanted_page, true, current);
        if crate::codes::qpdf::has_errors(added) {
            return Err(source.session.take_error().unwrap_or_else(|| {
                Error::Malformed("qpdf: a page could not be put back in the order".to_owned())
            }));
        }
        if let Some(error) = source.session.take_error() {
            return Err(error);
        }

        Ok(())
    }

    /// The handle for page `index`, bounds-checked first.
    fn page_handle(&self, session: &Session, index: u64, pages: u64) -> Result<u32> {
        if index >= pages {
            return Err(Error::InvalidArgument(
                "page is not in the document".to_owned(),
            ));
        }
        let n = u32::try_from(index)
            .map_err(|_| Error::Internal("page index does not fit in u32".to_owned()))?;
        let page = self.bridge().get_page_n(session.data(), n);
        if let Some(error) = session.take_error() {
            // RELEASE ONLY WHAT WAS ISSUED. qpdf numbers handles from 1 (`++qpdf->next_oh`),
            // so 0 means no handle was created -- and releasing it is not harmless in the one
            // place it matters: `qpdf_oh_release` erases from a map, so a release of an id
            // that was never issued would cancel out a future leak of the same id rather than
            // doing nothing. The fake refuses it outright, which is how this was found.
            if page != 0 {
                self.bridge().oh_release(session.data(), page);
            }
            return Err(error);
        }
        Ok(page)
    }
}

/// Reading a document back to check what an operation produced (ADR 0022).
///
/// Here rather than beside `web/rotate.rs` only because this file is the newest; it serves
/// every web operation. The implementation is `PageRotator`'s three calls with a different
/// purpose, exactly as the native one is.
impl crate::OutputReader for WebQpdf {
    type Read = super::rotate::WebRotatable;

    fn fresh(&self) -> Self {
        // A NEW ENGINE VALUE AND A NEW `qpdf_data`, over the SAME WebAssembly module.
        //
        // That is the whole of what "fresh" can mean here, and the limit is worth stating at
        // the place it bites: `WebQpdf` holds an `Arc<dyn QpdfBridge>`, and the bridge is one
        // module with one linear memory. Cloning the `Arc` gives a separate document handle
        // and a separate parse; it does not give a separate heap. A module whose heap is
        // corrupt corrupts the writer and the reader alike.
        //
        // The alternative is a fresh worker per operation -- 6.8 MB re-fetched and three
        // modules recompiled -- which ADR 0022 measures and declines. Natively the two handles
        // share nothing but the process allocator, so the same code is stronger there; the
        // asymmetry is real and recorded rather than smoothed over.
        //
        // CLONE, NEVER `WebQpdf::new`. The first version constructed, and `new` builds a fresh
        // `installed: OnceLock`, so the witness re-entered `install()` and called
        // `logger_create()` a second time -- a `qpdflogger_handle` plus three `Pl_Discard`
        // pipelines leaked per operation into a heap that never shrinks, which is the exact
        // defect `web/qpdf.rs`'s "one logger, not one per operation" header records and the
        // `installed` field exists to prevent. Found by security review. The `Clone` shares
        // the bridge and the `OnceLock`, which is what must be shared, and shares no document:
        // the fresh `qpdf_data` comes from `open_output`, not from this value.
        self.clone()
    }

    fn open_output(&self, bytes: &[u8], options: &crate::OpenOptions<'_>) -> Result<Self::Read> {
        // Recovery off, as every other open has it: reading our own output back with
        // reconstruction enabled would let a badly-written document be repaired on the way in
        // and pass.
        crate::PageRotator::open(self, bytes.to_vec().into_boxed_slice(), options)
    }

    fn page_count(&self, read: &Self::Read) -> Result<u64> {
        crate::PageRotator::pages(self, read)
    }

    fn rotations(
        &self,
        read: &Self::Read,
        options: &OpenOptions<'_>,
        deadline: &Deadline,
    ) -> Result<Vec<i64>> {
        // ONE SWEEP IMPLEMENTATION, for the reason the native one records: `Read` is a
        // rotatable source, so this is the same walk under a different name.
        crate::PageRotator::rotations(self, read, options, deadline)
    }
}

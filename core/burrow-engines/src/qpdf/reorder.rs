//! The qpdf implementation of [`PageReorderer`].
//!
//! # In place, not built
//!
//! [ADR 0021](../../../../docs/adr/0021-how-reorder-permutes-a-page-tree.md). Every input page
//! appears in the output, so nothing is excluded and [ADR 0019](../../../../docs/adr/0019-how-split-builds-its-outputs.md)
//! §2's subsetting rule has nothing to bite on. Building into a blank destination — the route
//! `extract` must take — would drop the outline, the attachments and the `/AcroForm`, which is
//! the correct trade when carrying them would be a leak and pure loss when it would not.
//!
//! # The permutation, and why it is not "remove them all and add them back"
//!
//! The obvious algorithm empties the document and refills it. qpdf refuses to *open* a
//! document with no pages — the corpus records `no-pages` as `Malformed` — and while nothing
//! says it refuses a transient one, "nothing says it refuses" is not a guarantee to build on.
//!
//! So the pages are moved one at a time, and at least one page is in the tree throughout. The
//! loop walks the target positions in order; at each one it takes the page that belongs there
//! out of wherever it is and puts it back immediately before the page currently occupying that
//! position. Pages already in place are left alone, so the identity permutation moves nothing.
//!
//! **A handle survives its own removal**, which is what makes this possible: `QPDF::removePage`
//! is `m->pages.erase(page)` and `Pages::erase` does `kids.eraseItem(pos)` — the page comes out
//! of `/Kids` and the object is untouched. Read from `libqpdf/QPDF_pages.cc`.
//!
//! # What it costs the document, measured
//!
//! Both calls route through code that flattens the page tree, after
//! `pushInheritedAttributesToPage`. So an inherited `/Rotate` becomes an explicit one on every
//! page — what each page displays is preserved — and intermediate `/Pages` nodes are gone. On
//! a two-level six-page fixture: 16 objects in, 14 out, one `/Rotate` in, six out. ADR 0021
//! has the table, and `reorder_keeps_every_page.rs` is the test that holds it.

use std::sync::Arc;

use burrow_types::{Deadline, Error, Limits, Permutation, Result};

use super::handle::ObjectHandle;
use super::{Document, Qpdf, ffi};
use crate::{OpenOptions, PageReorderer};

/// A document held open for a reordering.
pub struct QpdfReorderable {
    document: Document,
    pages: u64,
    /// The resident set before the document was opened. `max_memory_bytes` **detects rather
    /// than bounds** (ADR 0007), and detecting it needs a before.
    rss_before: Option<u64>,
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

impl PageReorderer for Qpdf {
    type Source = QpdfReorderable;

    fn name(&self) -> &'static str {
        "qpdf"
    }

    fn open(&self, bytes: Box<[u8]>, options: &OpenOptions<'_>) -> Result<Self::Source> {
        // Every ceiling, in one place, shared with the other operations that open a document
        // this way -- see `open_document` for why this is not written out here.
        let (document, pages, rss_before, _deadline) = super::open_document(bytes, options)?;
        Ok(QpdfReorderable {
            document,
            pages,
            rss_before,
            limits: options.limits,
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
        // THE SAME WALK `rotate` DOES, against the document this is about to permute. Reusing
        // `rotate.rs`'s helpers rather than repeating the `/Parent` climb: the depth ceiling
        // and the type assertion live there, and a second copy of a walk over hostile input
        // is a second place to get them wrong.
        let capacity = usize::try_from(source.pages)
            .map_err(|_| Error::Internal("page count does not fit in usize".to_owned()))?;
        let mut rotations = Vec::with_capacity(capacity);

        // PER PAGE, like `rotate`'s own walk and for the same measured reason: on the default
        // `max_pages` of 10,000 with a deep page tree this loop is millions of FFI calls, and
        // without a checkpoint it sits outside every deadline. Security review measured it at
        // 144 ms against 12 ms of edit-and-write on a 10,000-page document -- twelve times the
        // work the cooperative limit was covering.
        //
        // THE CALLER'S DEADLINE, NOT A NEW ONE -- see the trait's docs.
        let clock = Arc::clone(&options.clock);

        for index in 0..source.pages {
            deadline.checkpoint(clock.as_ref())?;
            let page = page_handle(&source.document, index, source.pages)?;
            // RECORDED, NOT JUDGED. `effective_rotation` refuses a `/Rotate` that is not a
            // multiple of 90, and a reorder names no page to turn -- so normalising here
            // failed a whole operation over a page it was only ever going to move.
            rotations.push(super::rotate::declared_rotation(&source.document, &page)?.unwrap_or(0));
        }
        Ok(rotations)
    }

    fn reorder(
        &self,
        source: &Self::Source,
        order: &Permutation,
        options: &OpenOptions<'_>,
    ) -> Result<Vec<u8>> {
        // THERE IS NO `max_pages` CHECK IN THE WEB COUNTERPART EITHER, and for the same reason
        // recorded below: it cannot fire. Said in both modules rather than only here, because the
        // next person writing a web operation will read `web/rotate.rs` — which DOES keep its
        // check, legitimately — and take the absence for an omission.
        //
        // `options` carries the CLOCK; the CEILINGS come from `source.limits`, for the reason
        // `rotate` records: the ceiling that counts is the one the document was opened under,
        // and a caller passing laxer options to a later call must not be able to raise it.
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
        // THERE IS NO `max_pages` CHECK HERE, and that is deliberate rather than an omission.
        // One was written, and code review measured that it could never fire: `length` was
        // just required to equal `source.pages`, and `open_document` already refused
        // `source.pages > max_pages` under these same `Limits`. A ceiling that cannot fire
        // reads as coverage, which `CLAUDE.md` calls worse than no check at all.
        //
        // `rotate.rs` keeps its equivalent because there it IS reachable: a rotation's page
        // list is a caller-chosen selection that is not pinned to the page count, so it can be
        // longer than the document. A permutation's cannot.
        //
        // The ceilings that apply here are `open_document`'s, on the document, and
        // `burrow_ops::reorder`'s, on both the document and the order's length before the
        // engine is reached.

        // AN OPTIMISATION, AND ONLY AN OPTIMISATION. The loop below already does nothing for
        // the identity: every page is where it belongs, so every comparison matches and every
        // iteration `continue`s. This saves `n` comparisons and `n` engine calls on the common
        // case of a person dragging a page and putting it back.
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
            // POISON ON ANY FAILURE -- see the web path for the reasoning, which is identical.
            if let Err(error) = permute(source, order, &deadline, clock.as_ref()) {
                source.poisoned.set(true);
                return Err(error);
            }
        }
        // WHAT PRESERVES THE PAGE TREE IS THAT NOTHING MOVED, not this branch. An earlier
        // version of this comment said the short-circuit was the mechanism, and code review
        // measured otherwise: replacing the condition with `if true` leaves the whole suite
        // green, because `qpdf_get_page_n` does not flatten and `qpdf_remove_page` /
        // `qpdf_add_page_at` are never reached for the identity. Deleting the branch entirely
        // would change the cost and not the output.
        //
        // The asymmetry itself is real and is ADR 0021's: any permutation that actually moves
        // a page routes through `Pages::erase`/`insert`, which flatten the tree and push
        // inherited attributes down first; one that moves nothing leaves the document with
        // whatever structure it arrived with. It is recorded because it is surprising, and
        // because it caught a test that assumed one shape.

        let output = super::extract::write_out(&source.document, &source.document)?;

        // `max_memory_bytes` DETECTS rather than bounds (ADR 0007). Checked after the write,
        // where the output buffer is at its largest.
        crate::estimate::check_measured_memory(
            source.rss_before,
            crate::rss::resident_bytes(),
            &source.limits,
        )?;

        Ok(output)
    }
}

/// Move each page to the position the order gives it.
///
/// The handles for every source page are taken **once, up front**, because a page's index
/// changes as its neighbours move: `get_page_n(3)` means something different after the first
/// swap. Taking them first means the loop works with identities rather than positions.
fn permute(
    source: &QpdfReorderable,
    order: &Permutation,
    deadline: &Deadline,
    clock: &dyn burrow_types::Clock,
) -> Result<()> {
    let document = &source.document;

    // Every page, in the document's CURRENT order. Held for the whole permutation -- which is
    // one handle per page, the thing `handle.rs` is careful about. Bounded by `max_pages`,
    // checked before this is reached.
    let mut originals = Vec::with_capacity(order.len());
    for index in 0..source.pages {
        // CHECKPOINTED, for the reason the web path records: this loop runs before the
        // permutation and is `n` engine calls, so without it `max_duration_ms` is unenforced
        // for the first part of the operation.
        deadline.checkpoint(clock)?;
        originals.push(page_handle(document, index, source.pages)?);
    }

    for (target, &wanted) in order.order().iter().enumerate() {
        // A page boundary, which is the granularity `Limits` promises. Each iteration is a
        // handful of engine calls and qpdf offers no cancellation hook (ADR 0007).
        deadline.checkpoint(clock)?;

        let at = u64::try_from(target)
            .map_err(|_| Error::Internal("page index does not fit in u64".to_owned()))?;
        let Some(wanted_page) = originals.get(
            usize::try_from(wanted)
                .map_err(|_| Error::Internal("page index does not fit in usize".to_owned()))?,
        ) else {
            // Unreachable: `Permutation` refused anything out of range and the length was
            // checked against this document above. A typed error rather than an index panic,
            // because library code here does not panic.
            return Err(Error::Internal(
                "a validated page order named a page that is not in the document".to_owned(),
            ));
        };

        // WHERE IS IT NOW? Asked of the document rather than tracked in a local, because the
        // document is the thing being changed and a local would be a second model of it that
        // could disagree. One extra engine call per page, on a path that is already engine
        // calls.
        let current = page_handle(document, at, source.pages)?;
        // OBJECT IDENTITY, NOT HANDLE IDENTITY. `qpdf_get_page_n` issues a fresh handle every
        // call, so comparing `raw()` says "different" for two handles to the same page -- which
        // made every permutation remove a page and try to insert it before itself. See
        // `ffi::qpdf_oh_get_object_id`.
        if current.object(document)? == wanted_page.object(document)? {
            // Already in place. The identity permutation reaches this on every page, and a
            // partial reorder reaches it on the pages nobody moved.
            continue;
        }

        // OUT, THEN BACK IN BEFORE THE PAGE THAT IS THERE NOW. The handle stays valid across
        // the removal -- `Pages::erase` takes it out of `/Kids` and leaves the object alone --
        // and at least one page is in the tree throughout, which "remove them all, add them
        // back" would not give.
        //
        // SAFETY: `document.data` is a live handle and `wanted_page` is one of its page
        // handles. Routes through `trap_errors`.
        let removed = unsafe { ffi::qpdf_remove_page(document.data, wanted_page.raw()) };
        if ffi::has_errors(removed) {
            return Err(document.take_error().unwrap_or_else(|| {
                Error::Malformed("qpdf: a page could not be taken out of the order".to_owned())
            }));
        }
        if let Some(error) = document.take_error() {
            return Err(error);
        }

        // `QPDF_TRUE` is `before`: the page lands at `target`, rather than one past it. The
        // document is passed as both source and destination because the page is already its
        // own -- this is a move, not a copy.
        //
        // SAFETY: both handles belong to `document`, which is passed as both arguments.
        // Routes through `trap_errors`.
        let added = unsafe {
            ffi::qpdf_add_page_at(
                document.data,
                document.data,
                wanted_page.raw(),
                ffi::QPDF_TRUE,
                current.raw(),
            )
        };
        if ffi::has_errors(added) {
            return Err(document.take_error().unwrap_or_else(|| {
                Error::Malformed("qpdf: a page could not be put back in the order".to_owned())
            }));
        }
        if let Some(error) = document.take_error() {
            return Err(error);
        }
    }

    Ok(())
}

/// The handle for page `index`, bounds-checked first.
pub(super) fn page_handle(document: &Document, index: u64, pages: u64) -> Result<ObjectHandle<'_>> {
    if index >= pages {
        return Err(Error::InvalidArgument(
            "page is not in the document".to_owned(),
        ));
    }
    let n = usize::try_from(index)
        .map_err(|_| Error::Internal("page index does not fit in usize".to_owned()))?;

    // SAFETY: `n` is below the document's page count, checked above.
    let page = unsafe { ObjectHandle::page(document, n) };
    if let Some(error) = document.take_error() {
        return Err(error);
    }
    Ok(page)
}

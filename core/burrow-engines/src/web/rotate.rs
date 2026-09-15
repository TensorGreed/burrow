//! The web implementation of [`PageRotator`], over the JS bridge.
//!
//! **The same Rust as the native path, against a different seam.** `qpdf/rotate.rs` calls
//! `ffi::qpdf_oh_get_key`; this calls `bridge.oh_get_key`. Everything between — the walk up
//! `/Parent`, the depth ceiling, the type assertion before any value is read, the per-page
//! write, which errors are which — is duplicated deliberately rather than shared, for the
//! reason `web/mod.rs` gives: the two are separate implementations of one trait and the
//! differential harness is what holds them together. Code shared between them could not
//! diverge; code that *cannot* diverge cannot be caught diverging either.
//!
//! # Handles are released by hand here, and that is the real difference
//!
//! The native path has [`ObjectHandle`](crate::qpdf), which releases on drop and carries a
//! lifetime tying it to its document. Across the bridge a handle is a `u32` and there is
//! nothing to attach a `Drop` to — the value qpdf gives back is an index into a cache in
//! *another heap*. So release is a discipline here rather than a type, and the discipline is
//! stated once: **every function in this module that obtains a handle releases it before
//! returning, on every path including the error paths.**
//!
//! That claim was false when it was first written, in the one way a hand-written discipline
//! always fails: two `?`s on an allocation returned past a live handle. Both are gone, and not
//! by adding two more release calls — the two keys the walk reads are now copied into the
//! engine heap once, in [`Keys`], so the walk cannot fail on allocation at all. Fewer paths
//! beats more releases, and it removed roughly 1.3 million malloc/free pairs on a document at
//! `max_pages` with a deep tree.
//!
//! And the claim is measured rather than asserted: `web/tests.rs` counts live handles after a
//! rotation, after a refusal, and after the depth-exhausted walk. That mattered more than it
//! sounds — the counter existed, was written by three call sites, and was read by nothing, so
//! it reported "no leak" for every input. The failure it prevents is invisible otherwise:
//! qpdf's cache only grows, nothing in its C API reports how many are live, and
//! `max_memory_bytes` **detects rather than bounds** (ADR 0007) at operation boundaries.
//!
//! # The key strings are copied in, and never come from a document
//!
//! `qpdf_oh_get_key` takes a `char const*`, so the key has to exist in the module's heap.
//! Rust copies `/Rotate` and `/Parent` in and passes the address; nothing on the JS side
//! builds a key, and no key is ever derived from file content — a document naming its own
//! keys is not a thing this boundary permits.

use std::sync::Arc;

use burrow_types::{Deadline, Error, Limits, Result, Rotation, Stage};

use super::bridge::QpdfPtr;
use super::qpdf::{Session, WebQpdf};
use crate::codes::qpdf::object_type;
use crate::{OpenOptions, PageRotator};

/// `/Rotate`, NUL-terminated, ready to copy into the engine heap.
const ROTATE_KEY: &[u8] = b"/Rotate\0";

/// `/Parent`, likewise.
const PARENT_KEY: &[u8] = b"/Parent\0";

/// How far up the page tree the inheritance walk will go before refusing.
///
/// Matches the native path's ceiling exactly, and for the same reason: a page tree is a tree,
/// so a legitimate document cannot need more, and `/Parent` pointing at a descendant is a
/// two-line edit to any PDF that would otherwise spin this loop forever. The two constants are
/// separate on purpose — the differential harness exists to catch the paths diverging, and a
/// shared constant is one fewer thing it could catch.
const MAX_PAGE_TREE_DEPTH: u32 = 64;

/// A document held open for a rotation, on the web.
pub struct WebRotatable {
    session: Session,
    /// `/Rotate` and `/Parent`, copied into the engine heap ONCE.
    ///
    /// Two reasons, and the second is the one that matters. Allocating a key per read made
    /// `copy_key` fallible inside the walk, and two of its `?`s returned past a live object
    /// handle -- reachable when the module hits its 2 GiB cap, which a crafted input can
    /// drive. Hoisting the allocation makes the walk infallible on allocation, which removes
    /// both leak paths structurally rather than by adding two more release calls.
    ///
    /// And it is 1.3 million fewer malloc/free pairs on a document at `max_pages` with a deep
    /// tree: 10,000 pages x 63 ancestors x 2 keys, against two. Found by security review.
    keys: Keys,
    pages: u64,
    /// The engine heap before the document was opened. `max_memory_bytes` **detects rather
    /// than bounds** (ADR 0007), and detecting it needs a before.
    heap_before: u64,
    /// The ceilings the document was opened under, so a later call cannot loosen them.
    limits: Limits,
}

/// The two dictionary keys rotate reads, resident in the engine heap.
///
/// Freed by [`Drop`] rather than by the code that uses them, so no path can return past them.
pub(super) struct Keys {
    rotate: QpdfPtr,
    parent: QpdfPtr,
    bridge: Arc<dyn super::bridge::QpdfBridge>,
}

impl Drop for Keys {
    fn drop(&mut self) {
        self.bridge.free(self.rotate);
        self.bridge.free(self.parent);
    }
}

impl PageRotator for WebQpdf {
    type Source = WebRotatable;

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

        // READ BEFORE THE OPEN, so the measured check covers the `copy_in` into the engine
        // heap as well as the read itself. A slightly wider window than native measures, and
        // the honest one: the copy is real memory the operation caused.
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

        // ALLOCATED ONCE, freed by `Keys`' `Drop`. See the field's own comment.
        let keys = Keys::copy_in(self)?;

        Ok(WebRotatable {
            session,
            keys,
            pages,
            heap_before,
            limits,
        })
    }

    fn pages(&self, source: &Self::Source) -> Result<u64> {
        Ok(source.pages)
    }

    fn effective_rotation(&self, source: &Self::Source, index: u64) -> Result<Rotation> {
        let page = page_handle(self, &source.session, index, source.pages)?;
        let rotation = effective_rotation(self, &source.session, &source.keys, page);
        self.bridge().oh_release(source.session.data(), page);
        rotation
    }

    fn rotations(
        &self,
        source: &Self::Source,
        options: &OpenOptions<'_>,
        deadline: &Deadline,
    ) -> Result<Vec<i64>> {
        let capacity = usize::try_from(source.pages)
            .map_err(|_| Error::Internal("page count does not fit in usize".to_owned()))?;
        let mut rotations = Vec::with_capacity(capacity);

        // PER PAGE, as the native sweep checkpoints and for the same measured reason -- and
        // against the CALLER'S deadline, not a new one. See the trait's docs.
        let clock = Arc::clone(&options.clock);

        for index in 0..source.pages {
            // BEFORE THE HANDLE IS ISSUED, so a refusal cannot leave one behind.
            deadline.checkpoint(clock.as_ref())?;
            let page = page_handle(self, &source.session, index, source.pages)?;
            // EVERY PATH RELEASES: `?` inside the loop would return past the release.
            // RECORDED, NOT JUDGED -- see the trait's docs.
            let outcome = declared_rotation(self, &source.session, &source.keys, page);
            self.bridge().oh_release(source.session.data(), page);
            rotations.push(outcome?.unwrap_or(0));
        }
        Ok(rotations)
    }

    fn rotate(
        &self,
        source: &Self::Source,
        pages: &[u64],
        rotation: Rotation,
        options: &OpenOptions<'_>,
    ) -> Result<Vec<u8>> {
        // `options` carries the CLOCK; the CEILINGS come from `source.limits` -- the ceiling
        // that counts is the one the document was opened under, and a caller passing laxer
        // options to a later call must not be able to raise it after the fact.
        let clock = Arc::clone(&options.clock);
        let deadline = Deadline::start(clock.as_ref(), &source.limits);

        if pages.is_empty() {
            return Err(Error::InvalidArgument("no pages to rotate".to_owned()));
        }
        let count = u64::try_from(pages.len())
            .map_err(|_| Error::Internal("page count does not fit in u64".to_owned()))?;
        Limits::check(
            Stage::PageCount,
            "max_pages",
            count,
            source.limits.max_pages,
        )?;

        let mut seen = std::collections::BTreeSet::new();
        for &index in pages {
            if index >= source.pages {
                return Err(Error::InvalidArgument(
                    "page is not in the document".to_owned(),
                ));
            }
            if !seen.insert(index) {
                return Err(Error::InvalidArgument(
                    "the same page is named more than once".to_owned(),
                ));
            }
        }

        for &index in pages {
            // The page boundary `Limits` promises. One check per page: the walk below is up
            // to `MAX_PAGE_TREE_DEPTH` ancestors and several bridge round trips.
            deadline.checkpoint(clock.as_ref())?;

            let page = page_handle(self, &source.session, index, source.pages)?;
            // EVERY PATH RELEASES. Written as a closure-and-release rather than `?` on each
            // step, because `?` here would return past the release -- which is exactly the
            // leak this module's header is about, and the native path's `Drop` is what makes
            // it impossible over there.
            let outcome = self.rotate_one_page(source, page, rotation);
            self.bridge().oh_release(source.session.data(), page);
            outcome?;
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
        // BOTH are checked, not one. qpdf returns a null buffer when the writer has none and
        // reports the length from a separate accessor, so a caller trusting only the length
        // would read from null and one trusting only the pointer would copy zero bytes and
        // call it a document.
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
    /// Read one page's effective rotation and write the turned value back onto the page.
    ///
    /// Split out so `rotate` can release the page handle on every path: a `?` inside the loop
    /// would return past the release.
    fn rotate_one_page(&self, source: &WebRotatable, page: u32, rotation: Rotation) -> Result<()> {
        // READ THE EFFECTIVE VALUE, not the page's own. A page inheriting 90 that is turned
        // another 90 must end at 180; reading only its own dictionary would see nothing,
        // write 90, and quietly UNDO an inherited quarter turn.
        let current = effective_rotation(self, &source.session, &source.keys, page)?;
        let wanted = rotation.after(current);

        let value = self
            .bridge()
            .oh_new_integer(source.session.data(), wanted.degrees());
        if let Some(error) = source.session.take_error() {
            self.bridge().oh_release(source.session.data(), value);
            return Err(error);
        }

        // ON THE PAGE. Never on the ancestor `current` may have come from -- that node can be
        // the parent of every page in the document.
        self.bridge()
            .oh_replace_key(source.session.data(), page, source.keys.rotate, value);
        let error = source.session.take_error();
        self.bridge().oh_release(source.session.data(), value);
        match error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl Keys {
    /// Copy both keys into the engine heap, once.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] if the engine heap could not allocate. Two compile-time constants,
    /// so a failure here is the module out of memory rather than anything about a document.
    pub(super) fn copy_in(engine: &WebQpdf) -> Result<Self> {
        let bridge = Arc::clone(engine.bridge());
        let rotate = bridge.copy_in(ROTATE_KEY);
        if rotate.is_null() {
            return Err(Error::Internal(
                "the qpdf module could not allocate".to_owned(),
            ));
        }
        let parent = bridge.copy_in(PARENT_KEY);
        if parent.is_null() {
            // The first one is already in the heap, and this returns past it -- so it is freed
            // here rather than left to a `Drop` that does not exist yet.
            bridge.free(rotate);
            return Err(Error::Internal(
                "the qpdf module could not allocate".to_owned(),
            ));
        }
        Ok(Self {
            rotate,
            parent,
            bridge,
        })
    }
}

/// The handle for page `index`, bounds-checked first.
///
/// The caller releases it.
pub(super) fn page_handle(
    engine: &WebQpdf,
    session: &Session,
    index: u64,
    pages: u64,
) -> Result<u32> {
    if index >= pages {
        return Err(Error::InvalidArgument(
            "page is not in the document".to_owned(),
        ));
    }
    let n = u32::try_from(index)
        .map_err(|_| Error::Internal("page index does not fit in u32".to_owned()))?;
    let page = engine.bridge().get_page_n(session.data(), n);
    if let Some(error) = session.take_error() {
        // ONLY WHAT WAS ISSUED -- qpdf numbers handles from 1, so 0 means none was created.
        // See `web/reorder.rs`'s `page_handle` for why releasing an unissued id is worse than
        // a no-op. The same shape, fixed in both rather than only where it was found.
        if page != 0 {
            engine.bridge().oh_release(session.data(), page);
        }
        return Err(error);
    }
    Ok(page)
}

/// The rotation `page` displays at, following `/Rotate` up the page tree.
///
/// Releases every handle it takes; `page` belongs to the caller.
///
/// # Errors
///
/// [`Error::Malformed`] if the value is not an integer multiple of 90. Callers that only need
/// to **record** what a page displays at want [`declared_rotation`] instead; the native
/// module's copy of this pair carries the reasoning.
pub(super) fn effective_rotation(
    engine: &WebQpdf,
    session: &Session,
    keys: &Keys,
    page: u32,
) -> Result<Rotation> {
    match declared_rotation(engine, session, keys, page)? {
        None => Ok(Rotation::None),
        Some(degrees) => Rotation::from_degrees(degrees)
            .map_err(|_| Error::Malformed("/Rotate is not a multiple of 90".to_owned())),
    }
}

/// The `/Rotate` `page` inherits, **as written**, without deciding whether it is in spec.
///
/// `None` means no ancestor carries the key. See the native `declared_rotation` for why the
/// witness records an out-of-spec value rather than refusing it: an ADR 0022 promise about a
/// page nobody named must not fail the operation.
///
/// Releases every handle it takes; `page` belongs to the caller.
pub(super) fn declared_rotation(
    engine: &WebQpdf,
    session: &Session,
    keys: &Keys,
    page: u32,
) -> Result<Option<i64>> {
    let data = session.data();

    let own = read_rotate(engine, session, keys, page)?;
    if let Some(degrees) = own {
        return Ok(Some(degrees));
    }

    let mut current = engine.bridge().oh_get_key(data, page, keys.parent);
    if let Some(error) = session.take_error() {
        engine.bridge().oh_release(data, current);
        return Err(error);
    }

    for _ in 0..MAX_PAGE_TREE_DEPTH {
        let parent_type = engine.bridge().oh_get_type_code(data, current);
        if let Some(error) = session.take_error() {
            engine.bridge().oh_release(data, current);
            return Err(error);
        }
        if parent_type == object_type::NULL {
            // The top of a well-formed tree: `/Parent` absent reads as a null object.
            engine.bridge().oh_release(data, current);
            return Ok(None);
        }
        if parent_type != object_type::DICTIONARY {
            // A `/Parent` pointing at something that cannot be a page-tree node. Malformed
            // rather than folded in with the line above: "the walk ended" and "the walk hit
            // something that should not be there" are different facts.
            engine.bridge().oh_release(data, current);
            return Err(Error::Malformed(
                "a page tree node's /Parent is not a dictionary".to_owned(),
            ));
        }

        match read_rotate(engine, session, keys, current) {
            Ok(Some(degrees)) => {
                engine.bridge().oh_release(data, current);
                return Ok(Some(degrees));
            }
            Ok(None) => {}
            Err(error) => {
                engine.bridge().oh_release(data, current);
                return Err(error);
            }
        }

        let parent = engine.bridge().oh_get_key(data, current, keys.parent);
        // The child is released as soon as its parent is in hand: the walk holds one handle
        // at a time, which is what keeps a deep tree from filling the cache.
        engine.bridge().oh_release(data, current);
        if let Some(error) = session.take_error() {
            engine.bridge().oh_release(data, parent);
            return Err(error);
        }
        current = parent;
    }

    engine.bridge().oh_release(data, current);
    Err(Error::Malformed(
        "the page tree is deeper than this engine will walk".to_owned(),
    ))
}

/// `Some(degrees)` if `node` has a readable `/Rotate`, `None` if the key is absent.
///
/// **The type is asserted before the value is read.** qpdf returns 0 for every non-integer
/// rather than raising, so without this a `/Rotate /Ninety` would read as "no rotation" and
/// emit a page turned the wrong way. Releases the handle it takes.
///
/// Whether the number is a multiple of 90 is **not** decided here; see [`declared_rotation`].
fn read_rotate(engine: &WebQpdf, session: &Session, keys: &Keys, node: u32) -> Result<Option<i64>> {
    let data = session.data();
    let value = engine.bridge().oh_get_key(data, node, keys.rotate);
    if let Some(error) = session.take_error() {
        engine.bridge().oh_release(data, value);
        return Err(error);
    }

    let type_code = engine.bridge().oh_get_type_code(data, value);
    if let Some(error) = session.take_error() {
        engine.bridge().oh_release(data, value);
        return Err(error);
    }

    let outcome = match type_code {
        object_type::NULL => Ok(None),
        object_type::INTEGER => {
            let degrees = engine.bridge().oh_get_int_value(data, value);
            match session.take_error() {
                Some(error) => Err(error),
                None => Ok(Some(degrees)),
            }
        }
        _ => Err(Error::Malformed("/Rotate is not an integer".to_owned())),
    };

    engine.bridge().oh_release(data, value);
    outcome
}

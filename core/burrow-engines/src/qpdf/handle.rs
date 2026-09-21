//! Object handles that release themselves, and a way to prove they did.
//!
//! # The leak the existing ceilings cannot see
//!
//! `qpdf_oh_*` handles are entries in a `std::map` on the `qpdf_data`, keyed by a counter that
//! never reuses an id (`qpdf-c.cc`: `qpdf_oh oh = ++qpdf->next_oh; qpdf->oh_cache[oh] = qoh;`).
//! The map grows on every handle and shrinks only on `qpdf_oh_release`. Nothing in qpdf's C
//! API reports how many are live.
//!
//! Rotate walks each page's ancestors to read an inherited `/Rotate`, so a naive
//! implementation allocates a handle per node per page and releases none: a 1000-page document
//! with a three-deep page tree leaves four thousand `QPDFObjectHandle`s alive until the
//! document is dropped. `max_memory_bytes` would not catch it — it **detects rather than
//! bounds** (ADR 0007), it samples RSS at operation boundaries, and this is steady growth well
//! under a ceiling until suddenly it is not.
//!
//! # So release is not a discipline, it is a type
//!
//! [`ObjectHandle`] owns its handle and releases on drop. Every route from qpdf to a handle in
//! the `qpdf` module returns one, so "remember to release" is not something a reader has to
//! check.
//!
//! # And the proof is a count, because a type is not evidence
//!
//! A wrapper that releases is worth nothing if a path exists that does not use it. [`live`]
//! reports how many `ObjectHandle`s this thread has alive right now, and `rotate`'s tests
//! assert it returns to its baseline after an operation — so the claim is measured on the real
//! code path rather than argued from the type. Test-only, and compiled out of release builds:
//! it is evidence, not a runtime feature.
//!
//! The counter only sees handles that go through this type, so "every route is wrapped" has to
//! be true for it to mean anything. It was not when this module was written: `extract.rs` and
//! `assemble.rs` took page handles raw and never released them — one cache entry per page for
//! the life of the document. Code review measured the claim against the code, and this is the
//! second half of the fix: [`the_handle_api_is_reachable_only_from_here`] reads the sibling
//! modules' source and fails if any of them names a `qpdf_oh_*` function.
//!
//! A test rather than a visibility. Rust cannot restrict a `pub` to a sibling module, and
//! moving the declarations into this file would hide them from
//! `tools/check-qpdf-trapped.py`, which parses `ffi.rs` to decide which qpdf functions burrow
//! calls — trading a leak for a hole in the exception-safety check is not a trade worth
//! making.

use core::ffi::{c_char, c_int};
use core::marker::PhantomData;

use burrow_types::{Error, Result};

use super::{Document, ffi};

// How many [`ObjectHandle`]s are alive on this thread.
//
// Test support. Counting is compiled out of release builds entirely.
//
// **Per thread, not process-wide**, and that is not a simplification. `libtest` runs tests in
// parallel threads, so a global counter reads other tests' handles as this test's leak: the
// first version of this was an `AtomicU64` and both leak assertions failed on a baseline of 4
// that belonged to whatever else was running. A thread-local is also sound here rather than
// merely convenient — `ObjectHandle` holds a raw pointer and is not `Send`, so a handle is
// always created and dropped on one thread.
#[cfg(test)]
thread_local! {
    static LIVE: core::cell::Cell<u64> = const { core::cell::Cell::new(0) };
}

/// How many [`ObjectHandle`]s this thread has alive right now.
///
/// The number a leak test compares against a baseline taken before the operation. It counts
/// *this crate's* handles, which is the set this crate is responsible for releasing; a handle
/// qpdf holds internally is qpdf's business.
#[cfg(test)]
pub(crate) fn live() -> u64 {
    LIVE.with(core::cell::Cell::get)
}

/// A qpdf object handle, released when it goes out of scope.
///
/// Handles are **per-document**: one from document A means something else in document B. The
/// owning `qpdf_data` is carried alongside so release cannot be aimed at the wrong document.
///
/// # The lifetime is the invariant, not the comment
///
/// `'a` ties the handle to the [`Document`] it came from, so releasing it after the document
/// has been cleaned up is a compile error rather than a rule. It began as a SAFETY comment
/// saying "`data` must be a live `qpdf_data` that outlives this value" with nothing enforcing
/// it — and in one test the correctness of that depended on the order two `let` bindings
/// happened to be written in. Code review pointed out that reordering those two lines would
/// have released a handle into a cleaned-up document. Found by security review.
pub(super) struct ObjectHandle<'a> {
    data: ffi::QpdfData,
    handle: ffi::QpdfObjectHandle,
    /// Borrows the document, so the handle cannot outlive it.
    owner: PhantomData<&'a Document>,
}

impl<'a> ObjectHandle<'a> {
    /// Take ownership of a handle `document` has just returned.
    ///
    /// # Safety
    ///
    /// `handle` must be a handle that `document` issued and that nothing else will release.
    /// That the document outlives the handle is no longer a caller's obligation: `'a` says it.
    pub(super) unsafe fn owned(document: &'a Document, handle: ffi::QpdfObjectHandle) -> Self {
        let data = document.data;
        #[cfg(test)]
        LIVE.with(|live| live.set(live.get() + 1));
        Self {
            data,
            handle,
            owner: PhantomData,
        }
    }

    /// The object's type code — `ot_integer`, `ot_dictionary`, … — read before any value is.
    ///
    /// Trapped through `do_with_oh`. See `ffi.rs`: these accessors return defaults rather than
    /// raising, so this is what separates "the key is an integer" from "the key is a name and
    /// the integer accessor said 0".
    pub(super) fn type_code(&self) -> core::ffi::c_int {
        // SAFETY: `self.data` is a live document and `self.handle` is one of its handles, by
        // this type's invariant. Routes through `trap_errors`.
        unsafe { ffi::qpdf_oh_get_type_code(self.data, self.handle) }
    }

    /// The value at `key`, as an owned handle.
    ///
    /// Returns a handle to a null object if this is not a dictionary or the key is absent —
    /// qpdf does not raise for either, which is why every caller checks [`Self::type_code`]
    /// before reading a value out.
    pub(super) fn key(&self, document: &'a Document, key: *const c_char) -> Self {
        // SAFETY: as `type_code`. The returned handle belongs to `self.data`, and wrapping it
        // here is what makes it released exactly once. Routes through `trap_errors`.
        let handle = unsafe { ffi::qpdf_oh_get_key(self.data, self.handle, key) };
        // SAFETY: `handle` was just issued by `self.data`, which is `document`'s -- the caller
        // passes the same document this handle came from, and `'a` holds it alive.
        unsafe { Self::owned(document, handle) }
    }

    /// This object's integer value.
    ///
    /// **Only meaningful once [`Self::type_code`] has said it is an integer**: qpdf returns 0
    /// for every other type rather than raising.
    pub(super) fn integer_value(&self) -> i64 {
        // SAFETY: as `type_code`. Routes through `trap_errors`.
        unsafe { ffi::qpdf_oh_get_int_value(self.data, self.handle) }
    }

    /// Set `key` on this dictionary to `item`.
    pub(super) fn replace_key(&self, key: *const c_char, item: &Self) {
        // SAFETY: as `type_code`; `item` belongs to the same document as `self`, which the
        // caller establishes by obtaining both from it. Routes through `trap_errors` via
        // `do_with_oh_void` -> `do_with_oh` -> `trap_oh_errors`.
        unsafe { ffi::qpdf_oh_replace_key(self.data, self.handle, key, item.handle) }
    }

    /// Replace this stream's body with `bytes`, and report qpdf's verdict.
    ///
    /// # It drains the error slot itself, and that is the whole point of the wrapper
    ///
    /// `qpdf_oh_replace_stream_data` returns `void`. Like every other void write in the C API
    /// it reports failure only by latching on the `qpdf_data`, so calling it and carrying on is
    /// an unchecked write — and for redaction an unchecked write is a page that reports a
    /// removal it did not make (ADR 0029 §6). Folding [`Document::take_error`] in here means a
    /// caller cannot forget it, the same reason `Lexer::next_token` skips an inline image
    /// rather than asking its caller to.
    ///
    /// # It takes no `Document`, and that is a correctness decision
    ///
    /// The first version took `&Document` alongside `&self` so it could reach the error slot.
    /// Security review found the leak in it: `ObjectHandle` carries no document identity, and
    /// nothing stopped a caller passing a **different** document — whereupon qpdf latches the
    /// failure on `self.data`'s slot, the wrapper drains the other document's empty one, and
    /// the function returns `Ok(())` **on a write that did not happen**. That is the exact
    /// sentence this milestone exists to prevent.
    ///
    /// So the slot is reached through `self.data`, which is by construction the document that
    /// issued this handle. `filter` and `decode_parms` are checked against it too: qpdf handles
    /// are bare `++next_oh` counters (`qpdf-c.cc:833-838`), so a handle from another document
    /// would very likely *collide* with a live one here and be written as `/Filter` with no
    /// error at all. `core/CLAUDE.md`'s rule that a handle is not an identity, one level up.
    ///
    /// **qpdf copies `bytes` before returning** — `qpdf-c.h:942-944` — so the slice does not
    /// need to outlive the call.
    ///
    /// # `Err` does not tell you whether the stream was written
    ///
    /// `qpdf_oh_replace_stream_data` is three nested trapped calls, not one: inside the outer
    /// `do_with_oh_void` it resolves `filter` and `decode_parms` through their own `do_with_oh`
    /// (`qpdf-c.cc:1778-1796`), and an inner failure latches on the **same** slot. So an `Err`
    /// here may mean the write never happened, or that it happened and a later step failed.
    /// A caller must not treat `Err` as "the document is untouched" — for redaction, retrying
    /// or falling back on that assumption is operating on state it believes unmodified.
    ///
    /// # Errors
    ///
    /// - [`Error::Internal`] if `filter` or `decode_parms` belongs to another document.
    /// - Whatever qpdf latched, mapped at the engine boundary as everywhere else.
    pub(super) fn replace_stream_data(
        &self,
        bytes: &[u8],
        filter: &Self,
        decode_parms: &Self,
    ) -> Result<()> {
        if filter.data != self.data || decode_parms.data != self.data {
            return Err(Error::Internal(
                "qpdf: a stream's filter came from another document".to_owned(),
            ));
        }
        // SAFETY: `self.data` is live and `self.handle` was issued by it; the check above
        // establishes that `filter` and `decode_parms` were issued by the same document.
        // `bytes` is read for `len` bytes and copied by qpdf before the call returns. Routes
        // through `trap_errors` via `do_with_oh_void` -> `do_with_oh` -> `trap_oh_errors`
        // (engines/qpdf-trapped-functions.txt:82).
        unsafe {
            ffi::qpdf_oh_replace_stream_data(
                self.data,
                self.handle,
                bytes.as_ptr(),
                bytes.len(),
                filter.handle,
                decode_parms.handle,
            );
        }
        // THE SLOT THAT BELONGS TO THIS HANDLE'S DOCUMENT. Reached through `self.data` rather
        // than through a `Document` a caller chose; see above for what that cost.
        // SAFETY: `self.data` is live for as long as this handle borrows its document.
        unsafe { Document::take_error_on(self.data) }.map_or(Ok(()), Err)
    }

    /// The null object, in `document`.
    ///
    /// It exists for [`Self::replace_stream_data`]'s two object arguments: a null is how the C
    /// API says "no filter".
    pub(super) fn new_null(document: &'a Document) -> Self {
        // SAFETY: `document.data` is live. Untrapped and argued in
        // `engines/qpdf-untrapped-accepted.toml`: it constructs the null object and never
        // touches the document.
        let handle = unsafe { ffi::qpdf_oh_new_null(document.data) };
        // SAFETY: `handle` was just issued by `document.data`.
        unsafe { Self::owned(document, handle) }
    }

    /// A new integer object in this handle's document.
    pub(super) fn new_integer(document: &'a Document, value: i64) -> Self {
        // SAFETY: `document.data` is live. Untrapped and argued in
        // `engines/qpdf-untrapped-accepted.toml`: it constructs an integer from a number this
        // crate chose and never touches the document.
        let handle = unsafe { ffi::qpdf_oh_new_integer(document.data, value) };
        // SAFETY: `handle` was just issued by `document.data`.
        unsafe { Self::owned(document, handle) }
    }

    /// The handle for page `n`, as an owned handle.
    ///
    /// Here rather than in `rotate.rs` so that **every** route from qpdf to a handle produces
    /// an `ObjectHandle`. `extract.rs` and `assemble.rs` call `qpdf_get_page_n` directly and
    /// hold the raw `qpdf_oh` without releasing it; code review measured that against this
    /// module's claim that "remember to release" is not something a reader has to check, and
    /// the claim was false. They use this now too.
    ///
    /// # Safety
    ///
    /// `n` must be below the document's page count.
    pub(super) unsafe fn page(document: &'a Document, n: usize) -> Self {
        // SAFETY: `document.data` is a live handle whose document read successfully, and `n`
        // is below its page count by this function's contract. Routes through `trap_errors`.
        let handle = unsafe { ffi::qpdf_get_page_n(document.data, n) };
        // SAFETY: `handle` was just issued by `document.data`.
        unsafe { Self::owned(document, handle) }
    }

    /// The object this handle refers to, as `(number, generation)`.
    ///
    /// **A handle is not an identity.** `qpdf_get_page_n` issues a new one on every call, so
    /// two handles to the same page compare unequal — `reorder` compared them and every
    /// permutation failed, including the identity. This is what identity means in a PDF.
    ///
    /// # `Result`, because the fallback compares EQUAL
    ///
    /// Unlike [`Self::type_code`] and [`Self::integer_value`], this one may not fail open.
    /// `qpdf-c.cc` implements both reads as `do_with_oh<int>(qpdf, oh, return_T<int>(0), …)`:
    /// on any internal failure `trap_oh_errors` returns the fallback **0** and latches the
    /// exception in `qpdf->error`. So two failing reads both yield `(0, 0)` — and `(0, 0)`
    /// equals `(0, 0)`.
    ///
    /// For every caller of this method, "equal" is the answer that means *do nothing*:
    /// `reorder` reads it as "the page is already in place" and skips the move; `split`'s
    /// pruning and M2's redaction will read it as "this object belongs here" and keep it. A
    /// silent `Ok` with the work not done is the failure mode this project treats most
    /// seriously, so the error is drained and returned rather than swallowed. `0` is also a
    /// legitimate object id for a direct object, so the sentinel is not even distinguishable
    /// from a real answer.
    ///
    /// Found by security review. It was not shown to be reachable on any input; it is fixed
    /// because the direction it fails in is the one that produces a valid-looking wrong file.
    ///
    /// # Errors
    ///
    /// Whatever qpdf latched while reading the object's number or generation, mapped by code.
    pub(super) fn object(&self, document: &Document) -> Result<(c_int, c_int)> {
        // SAFETY: `self.data` is a live document and `self.handle` is one of its handles, by
        // this type's invariant. Both route through `trap_errors` via `do_with_oh`.
        let id = unsafe { ffi::qpdf_oh_get_object_id(self.data, self.handle) };
        let generation = unsafe { ffi::qpdf_oh_get_generation(self.data, self.handle) };
        // DRAINED AFTER BOTH READS, not between them: one latched error is one error whichever
        // of the two raised it, and leaving it latched would surface it later against an
        // unrelated page or against the write. The caller passes the document this handle came
        // from -- the same contract `key` has.
        if let Some(error) = document.take_error() {
            return Err(error);
        }
        Ok((id, generation))
    }

    // ------------------------------------------------------------ split's pruning
    //
    // Everything below exists for ADR 0019 §2b: reading a page apart far enough to remove what
    // belongs to pages the output does not contain. They are here rather than in `prune.rs`
    // for this module's standing reason -- every route from qpdf to a handle returns an
    // `ObjectHandle`, and `the_handle_api_is_reachable_only_from_here` measures it.

    /// This object's own syntax, with its CHILDREN left as `N G R`.
    ///
    /// `qpdf_oh_unparse_resolved`, not `qpdf_oh_unparse`: the plain one returns `"N G R"` for an
    /// indirect object, and every page in a real document is indirect — so it would hand back a
    /// reference rather than a dictionary. See the declaration for the measurement.
    ///
    /// **The route to a dictionary's keys**, because qpdf's own key iterator may not be called:
    /// `qpdf_oh_begin_dict_key_iter` reaches `trap_errors` only through an assignment and its
    /// two companions do not go near it, so none of the three is on the trapped list. See
    /// `crate::pdfsyntax::dict`, which reads what this returns.
    ///
    /// Returns an empty vector when qpdf hands back null, which it does for a released or
    /// uninitialised handle. **No caller treats that as "no keys"**: every one of them feeds it
    /// to `pdfsyntax::top_level_keys`, which fails on the first token because an empty slice is
    /// not a dictionary — so an unreadable object is a refusal rather than an object that
    /// appears to have nothing in it. That is the direction this whole module needs: a
    /// dictionary read as empty is a dictionary nothing gets removed from.
    pub(super) fn unparse(&self) -> Vec<u8> {
        // SAFETY: `self.data` is a live document and `self.handle` is one of its handles, by
        // this type's invariant. Routes through `trap_errors` via `do_with_oh`.
        let text = unsafe { ffi::qpdf_oh_unparse_resolved(self.data, self.handle) };
        // COPIED OUT IMMEDIATELY. The pointer belongs to the `qpdf_data` and is invalidated by
        // the next call that returns one -- including the next `unparse`, which is exactly what
        // a loop over a page's keys does.
        copy_c_string(text)
    }

    /// This name object's value, canonicalised and including its leading `/`.
    ///
    /// Empty for anything that is not a name, which qpdf returns rather than raising -- so the
    /// caller checks [`Self::type_code`] first, as everywhere else here.
    pub(super) fn name(&self) -> Vec<u8> {
        // SAFETY: as `unparse`.
        let text = unsafe { ffi::qpdf_oh_get_name(self.data, self.handle) };
        copy_c_string(text)
    }

    /// Remove `key` from this dictionary.
    ///
    /// Removing a key that is not there is not an error, which is what lets the page-key rule
    /// be "remove everything the allowlist does not name" rather than a diff.
    pub(super) fn remove_key(&self, key: *const c_char) {
        // SAFETY: as `unparse`. Routes through `trap_errors` via `do_with_oh_void` ->
        // `do_with_oh` -> `trap_oh_errors`.
        unsafe { ffi::qpdf_oh_remove_key(self.data, self.handle, key) }
    }

    /// How many items this array has, or 0 for anything that is not an array.
    pub(super) fn array_len(&self) -> c_int {
        // SAFETY: as `unparse`.
        unsafe { ffi::qpdf_oh_get_array_n_items(self.data, self.handle) }
    }

    /// The item at `n`, as an owned handle. Out of range yields a null object.
    pub(super) fn array_item(&self, document: &'a Document, n: c_int) -> Self {
        // SAFETY: as `unparse`. The returned handle belongs to `self.data`, which is
        // `document`'s -- the caller passes the document this handle came from.
        let handle = unsafe { ffi::qpdf_oh_get_array_item(self.data, self.handle, n) };
        // SAFETY: `handle` was just issued by `self.data`.
        unsafe { Self::owned(document, handle) }
    }

    /// Remove the item at `at` from this array.
    ///
    /// **Everything after `at` shifts down**, so a caller filtering an array walks it
    /// backwards. Forwards, erasing item 2 of 5 makes the old item 3 the new item 2 and the
    /// loop skips it -- which, for the annotation filter this exists for, means an annotation
    /// belonging to an excluded page is never examined and rides into the output.
    pub(super) fn erase_item(&self, at: c_int) {
        // SAFETY: as `unparse`.
        unsafe { ffi::qpdf_oh_erase_item(self.data, self.handle, at) }
    }

    /// This stream's dictionary, as an owned handle.
    ///
    /// `ot_stream` is a distinct type from `ot_dictionary`, so a Form XObject's `/Resources` is
    /// unreachable without this.
    pub(super) fn stream_dict(&self, document: &'a Document) -> Self {
        // SAFETY: as `array_item`.
        let handle = unsafe { ffi::qpdf_oh_get_dict(self.data, self.handle) };
        // SAFETY: `handle` was just issued by `self.data`.
        unsafe { Self::owned(document, handle) }
    }

    /// This page's content streams, concatenated and **decoded**.
    ///
    /// What the resource-name filter reads. In the file the names are inside a flate stream, so
    /// a scan of the raw bytes would find none of them and prune every resource the page uses.
    ///
    /// # Errors
    ///
    /// Whatever qpdf latched. A page with no `/Contents` is `Ok(empty)` rather than an error:
    /// it is a legitimate page that draws nothing and therefore uses no resources.
    pub(super) fn page_content(&self, document: &Document) -> Result<Vec<u8>> {
        let mut buffer: *mut u8 = core::ptr::null_mut();
        let mut length: usize = 0;
        // SAFETY: `self.data` is a live document and `self.handle` one of its handles. Both
        // out-parameters point at live locals for the duration of the call. Routes through
        // `trap_errors` directly.
        let status = unsafe {
            ffi::qpdf_oh_get_page_content_data(
                self.data,
                self.handle,
                &raw mut buffer,
                &raw mut length,
            )
        };
        take_malloced_buffer(document, status, buffer, length)
    }

    /// This stream's data, decoded, or `None` if qpdf could not decode it.
    ///
    /// **`None` is not "empty" and a caller may not treat it as such.** It means the bytes came
    /// back still compressed, and lexing compressed bytes for resource names yields a handful
    /// of accidents rather than the names that are there -- the under-approximation that
    /// deletes a resource the page draws with. `prune.rs` turns it into a refusal.
    ///
    /// # Errors
    ///
    /// Whatever qpdf latched while reading the stream.
    pub(super) fn stream_data(&self, document: &Document) -> Result<Option<Vec<u8>>> {
        let mut filtered: ffi::QpdfBool = ffi::QPDF_FALSE;
        let mut buffer: *mut u8 = core::ptr::null_mut();
        let mut length: usize = 0;
        // SAFETY: as `page_content`; all three out-parameters point at live locals.
        let status = unsafe {
            ffi::qpdf_oh_get_stream_data(
                self.data,
                self.handle,
                crate::codes::qpdf::decode_level::SPECIALIZED,
                &raw mut filtered,
                &raw mut buffer,
                &raw mut length,
            )
        };
        let data = take_malloced_buffer(document, status, buffer, length)?;
        if filtered == ffi::QPDF_FALSE {
            return Ok(None);
        }
        Ok(Some(data))
    }

    /// Hand the raw handle to a qpdf call that takes one.
    ///
    /// Narrow on purpose: `qpdf_add_page` and `qpdf_remove_page` take a handle and are not
    /// worth a wrapper method each. The value is copied out for the duration of one call and
    /// never stored — storing it is what this type exists to prevent.
    pub(super) const fn raw(&self) -> ffi::QpdfObjectHandle {
        self.handle
    }
}

/// A NUL-terminated string qpdf owns, copied out before the next call invalidates it.
///
/// Every `char const*` in qpdf's C API points into storage on the `qpdf_data` that the next
/// call returning one overwrites (`qpdf-c.h:796-800` says so for the key iterator and the same
/// holds for `unparse` and `get_name`). Holding one across a second call is a use-after-free
/// that looks like a wrong answer, so there is one function that copies and no way to get the
/// pointer out of this module.
fn copy_c_string(text: *const c_char) -> Vec<u8> {
    if text.is_null() {
        return Vec::new();
    }
    // SAFETY: `text` is non-null and qpdf guarantees a NUL-terminated string at it, valid
    // until the next call on the same `qpdf_data` that returns one. This reads it and copies
    // before returning, so nothing outlives that window.
    unsafe { core::ffi::CStr::from_ptr(text) }
        .to_bytes()
        .to_vec()
}

/// Copy a `malloc`ed buffer out of qpdf and free it, whatever the outcome.
///
/// The two stream readers are the only qpdf functions this crate calls whose buffer is the
/// **caller's** to free rather than qpdf's. Every path through here frees it -- including the
/// error path, which is the one that would otherwise leak the decompressed size of a stream
/// per failed page.
fn take_malloced_buffer(
    document: &Document,
    status: ffi::QpdfErrorCode,
    mut buffer: *mut u8,
    length: usize,
) -> Result<Vec<u8>> {
    // READ THE BYTES BEFORE THE ERROR IS DRAINED, and free before returning either way.
    let data = if buffer.is_null() || length == 0 {
        Vec::new()
    } else {
        // SAFETY: qpdf reports `length` readable bytes at `buffer`, which it allocated with
        // `malloc`. Copied out immediately and not stored.
        unsafe { core::slice::from_raw_parts(buffer, length) }.to_vec()
    };
    if !buffer.is_null() {
        // SAFETY: `buffer` was allocated by qpdf with `malloc` and has not been freed. The
        // function nulls it out, and nothing reads it afterwards. Untrapped and argued in
        // `engines/qpdf-untrapped-accepted.toml`: its body is a `free`.
        unsafe { ffi::qpdf_oh_free_buffer(&raw mut buffer) }
    }

    // THE ERROR BIT, never `!= 0`: a warning here is an ordinary outcome, and a page whose
    // content stream qpdf grumbled about still has content worth reading.
    if ffi::has_errors(status) {
        return Err(document.take_error().unwrap_or_else(|| {
            Error::Malformed("qpdf: a stream's data could not be read".to_owned())
        }));
    }
    if let Some(error) = document.take_error() {
        return Err(error);
    }
    Ok(data)
}

impl Drop for ObjectHandle<'_> {
    fn drop(&mut self) {
        // SAFETY: `self.data` is live for at least as long as this value, and `self.handle` is
        // one of its handles that nothing else releases. Untrapped, and argued in
        // `engines/qpdf-untrapped-accepted.toml`: the whole body is a map erase.
        unsafe { ffi::qpdf_oh_release(self.data, self.handle) }
        #[cfg(test)]
        LIVE.with(|live| live.set(live.get() - 1));
    }
}

#[cfg(test)]
mod tests {
    /// The `qpdf_oh_*` API is called from this module and nowhere else.
    ///
    /// `handle.rs` claims "every route from qpdf to a handle in the `qpdf` module returns an
    /// `ObjectHandle`". That claim was false when it was written, and the compiler cannot
    /// enforce it — so it is measured here, on the source, and the test names what it
    /// examined rather than reporting a bare pass.
    #[test]
    fn the_handle_api_is_reachable_only_from_here() {
        // Every sibling that could reach `ffi`, by name. `include_str!` needs a literal, so
        // the contents are listed; what is NOT listed is how many there should be.
        let siblings: [(&str, &str); 8] = [
            ("assemble.rs", include_str!("assemble.rs")),
            ("compress.rs", include_str!("compress.rs")),
            ("extract.rs", include_str!("extract.rs")),
            ("limits.rs", include_str!("limits.rs")),
            ("mod.rs", include_str!("mod.rs")),
            ("prune.rs", include_str!("prune.rs")),
            ("reorder.rs", include_str!("reorder.rs")),
            ("rotate.rs", include_str!("rotate.rs")),
        ];

        // THE LIST IS COMPARED AGAINST THE DIRECTORY, not against a number. It used to assert
        // `siblings.len() == 5` against a hand-written five, and `reorder.rs` -- which calls
        // `ffi::qpdf_remove_page` and `ffi::qpdf_add_page_at` directly -- was added without
        // being added here. The count still matched, the scan still passed, and the claim
        // underneath it ("the set of modules calling the qpdf C API directly") was false while
        // the assertion was green. Exactly the shape `CLAUDE.md` calls a check that silently
        // examines nothing. Found by security review.
        //
        // Reading the directory means a new module is a FAILURE rather than an omission.
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/qpdf");
        let mut on_disk: Vec<String> = std::fs::read_dir(&directory)
            .expect("the qpdf module directory must be readable")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".rs"))
            // `handle.rs` is the subject, and `ffi.rs` is the declaration site the rule is
            // about reaching -- neither is a sibling that could bypass this module.
            .filter(|name| name != "handle.rs" && name != "ffi.rs")
            // Test modules are `#[cfg(test)]` and may reach `ffi` to build a fixture.
            .filter(|name| !name.ends_with("_tests.rs") && name != "tests.rs")
            .collect();
        on_disk.sort();
        let listed: Vec<String> = siblings
            .iter()
            .map(|(name, _)| (*name).to_owned())
            .collect();
        assert_eq!(
            listed, on_disk,
            "a module was added to src/qpdf/ without being added to this scan, so the qpdf_oh              check below examined everything except the newest code"
        );

        // THE RULE, as a function, so it can be probed before it is trusted.
        let is_a_call =
            |line: &str| !line.trim_start().starts_with("//") && line.contains("ffi::qpdf_oh_");

        // ITS OWN FIXTURE AND NEAR-MISS, checked here rather than assumed. Skipping comment
        // lines is what makes this rule usable, and it is also the way it could be made to
        // match nothing -- a predicate that skipped everything would report a clean scan over
        // any source at all.
        assert!(
            is_a_call("        let id = unsafe { ffi::qpdf_oh_get_object_id(d, h) };"),
            "the scan no longer recognises a real call, so it would pass any file"
        );
        assert!(
            !is_a_call("        // see `ffi::qpdf_oh_get_object_id`."),
            "the scan flags prose, so it cannot be satisfied without deleting the comments \
             that explain the rule"
        );

        let mut offenders = Vec::new();
        for (name, source) in siblings {
            for (number, line) in source.lines().enumerate() {
                // `ffi::qpdf_oh_` is the call shape. A mention in a COMMENT is fine and is
                // wanted -- `ffi.rs`'s own prose discusses these functions at length, and
                // `reorder.rs` has to name `qpdf_oh_get_object_id` to explain why it compares
                // object numbers rather than handles.
                //
                // The comment above said that before the code did it. Nothing had noticed,
                // because until `reorder.rs` joined the scan no sibling had ever mentioned the
                // API in prose -- so the rule read as correct while being strictly stricter
                // than stated. It fired the moment the scan was widened.
                if is_a_call(line) {
                    offenders.push(format!("{name}:{}: {}", number + 1, line.trim()));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "the qpdf_oh_* API is called outside handle.rs, so a handle can be taken without \
             being released and the leak test's baseline means nothing:\n  {}",
            offenders.join("\n  ")
        );

        // AND THE SCAN LOOKED AT SOMETHING. A pattern that matches nothing passes everything,
        // and this one would pass an empty file list just as happily.
        // `ffi::`, but NOT `core::ffi::` or `std::ffi::`. The predicate was a bare substring
        // match, and `prune.rs` imports `core::ffi::c_int` -- so the module that reaches qpdf
        // ONLY through `ObjectHandle`, which is this file's whole thesis, was reported as
        // calling the C API directly. A check that counts a standard-library import as a
        // foreign-function call is one whose list grows for reasons unrelated to its rule, and
        // a list like that is one people learn to update without reading.
        let reaches_the_c_api = |source: &str| {
            source.match_indices("ffi::").any(|(at, _)| {
                !source[..at].ends_with("core::") && !source[..at].ends_with("std::")
            })
        };

        // ITS OWN FIXTURE AND NEAR-MISS, like the call predicate above.
        assert!(
            reaches_the_c_api("let added = unsafe { ffi::qpdf_add_page(dest.data) };"),
            "the scan no longer recognises a real C API call, so its list means nothing"
        );
        assert!(
            !reaches_the_c_api("use core::ffi::c_int;"),
            "the scan counts a standard-library import as a C API call"
        );
        assert!(
            !reaches_the_c_api("use std::ffi::CStr;"),
            "the scan counts a standard-library import as a C API call"
        );

        let reach_ffi: Vec<&str> = siblings
            .iter()
            .filter(|(_, source)| reaches_the_c_api(source))
            .map(|(name, _)| *name)
            .collect();
        // `rotate.rs` reaches qpdf only through `ObjectHandle` and `open_document`, so it names
        // `ffi` nowhere -- which is the outcome this test is for, not a gap in it. The others
        // call the C API directly and are the ones worth scanning; `reorder.rs` is among them
        // because it calls `qpdf_remove_page` and `qpdf_add_page_at`, neither of which takes
        // or returns an object handle it could leak.
        //
        // `prune.rs` is the newest and the most handle-heavy module in the crate -- it reads
        // keys, walks arrays, opens streams -- and it reaches qpdf's C API nowhere at all. That
        // is the whole point of this file existing: the module that would leak the most handles
        // is the one that cannot reach the API that issues them. It DOES name `core::ffi::c_int`,
        // which is what the predicate above had to learn to tell apart.
        assert_eq!(
            reach_ffi,
            vec![
                "assemble.rs",
                "extract.rs",
                "limits.rs",
                "mod.rs",
                "reorder.rs"
            ],
            "the set of modules calling the qpdf C API directly has changed; check whether the \
             new one takes object handles"
        );
    }
}

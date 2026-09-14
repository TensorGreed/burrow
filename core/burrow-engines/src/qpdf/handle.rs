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

use burrow_types::Result;

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

    /// Hand the raw handle to a qpdf call that takes one.
    ///
    /// Narrow on purpose: `qpdf_add_page` and `qpdf_remove_page` take a handle and are not
    /// worth a wrapper method each. The value is copied out for the duration of one call and
    /// never stored — storing it is what this type exists to prevent.
    pub(super) const fn raw(&self) -> ffi::QpdfObjectHandle {
        self.handle
    }
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
        let siblings: [(&str, &str); 6] = [
            ("assemble.rs", include_str!("assemble.rs")),
            ("extract.rs", include_str!("extract.rs")),
            ("limits.rs", include_str!("limits.rs")),
            ("mod.rs", include_str!("mod.rs")),
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
        let reach_ffi: Vec<&str> = siblings
            .iter()
            .filter(|(_, source)| source.contains("ffi::"))
            .map(|(name, _)| *name)
            .collect();
        // `rotate.rs` reaches qpdf only through `ObjectHandle` and `open_document`, so it names
        // `ffi` nowhere -- which is the outcome this test is for, not a gap in it. The others
        // call the C API directly and are the ones worth scanning; `reorder.rs` is among them
        // because it calls `qpdf_remove_page` and `qpdf_add_page_at`, neither of which takes
        // or returns an object handle it could leak.
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

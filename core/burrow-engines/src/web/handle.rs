//! A qpdf object handle across the bridge, bound to the document that issued it.
//!
//! The web twin of `crate::qpdf::handle`, and for the same two reasons.
//!
//! # A handle and a `qpdf_data` that could disagree
//!
//! [`QpdfBridge`](super::bridge::QpdfBridge) takes them as **separate arguments** — `data:
//! QpdfPtr, oh: u32` — because that is what wasm-bindgen can carry, exactly as `qpdf/ffi.rs`
//! takes them separately because that is what the C API is. On the native side
//! `ObjectHandle` closes the gap; on this side nothing did, and the bridge's own comment said
//! so: *"on the web that is the caller's discipline rather than a type's — `web/rotate.rs` owns
//! it, and the native path's `ObjectHandle` is the model."*
//!
//! What that costs is not theoretical. #130 found it on the native side, where the type at
//! least existed: a wrapper taking a `&Document` beside `&self` let the two be different
//! documents, and the error slot was drained from the wrong one — returning `Ok` on a write
//! that did not happen. Six more methods had the same shape. Here **every** bridge call has it,
//! because there is no pairing at all.
//!
//! qpdf handles are bare `++qpdf->next_oh` counters, so a handle from one document does not
//! fail against another — it very likely **collides** with a live handle there and answers
//! about the wrong object.
//!
//! # Releasing, which three files solved three ways
//!
//! `qpdf_oh_release` is a map erase and not calling it is the hazard: the cache only grows and
//! `next_oh` never reuses, so a per-page walk that took a handle per ancestor and released none
//! would leak one `QPDFObjectHandle` per node for as long as the document is open — growth
//! `max_memory_bytes` cannot see, because it samples RSS at operation boundaries and detects
//! rather than bounds (ADR 0007).
//!
//! Before this module there were three answers to that in three files: `prune.rs` had a
//! `WebHandle` with a `Drop`, `reorder.rs` had a `Pages` guard owning a `Vec<u32>`, and
//! `rotate.rs`, `extract.rs` and `compress.rs` released by hand at every exit. This is the one
//! answer, and `prune.rs`'s is the one it is built from.

use burrow_types::Result;

use super::WebQpdf;
use super::bridge::QpdfPtr;
use super::qpdf::Session;

/// A qpdf object handle across the bridge, released when it goes out of scope.
///
/// The `u32` qpdf issued, the session that issued it, and what is needed to give it back.
/// **The pair is the point**: a method here never takes a `QpdfPtr` naming a document, so there
/// is none to get wrong.
///
/// **What that does not cover, stated rather than implied.** Four call sites still hand a
/// `raw()` handle to a bridge method alongside a document — `add_page` and `add_page_at` take
/// TWO documents, and `remove_page` takes a bare `oh`, so there is no `WebHandle` shape for
/// them. All four pair correctly and none is checked by a rule. An earlier version of this
/// paragraph said the mismatch was "gone"; it is *unexpressible in this type's methods*, which
/// is a smaller claim and the true one.
pub(super) struct WebHandle<'a> {
    engine: &'a WebQpdf,
    /// The session that issued `handle`, borrowed so the handle cannot outlive it.
    ///
    /// # The lifetime is the invariant, not the comment
    ///
    /// The first version of this struct stored a **copy** of the `QpdfPtr` and borrowed only
    /// the engine, so `'a` said nothing about the session. `Session::drop` calls
    /// `qpdf_cleanup`, and this type's `Drop` calls `qpdf_oh_release` — so a handle outliving
    /// its session released against a freed `qpdf_data`, a wild write inside the wasm heap.
    /// Both reviews compiled a function that did it.
    ///
    /// It was not reachable from a file: the one place a handle is taken over a local session
    /// is `extract.rs`'s `blank`, and declaration order happened to save it. That is an
    /// accident of declaration order, not a guarantee, and the whole point of this type is that
    /// the rule is held by the type. The native `ObjectHandle` says the same thing about its
    /// own `PhantomData<&'a Document>`, and this header claimed parity with it while missing
    /// exactly this half.
    session: &'a Session,
    handle: u32,
}

impl<'a> WebHandle<'a> {
    /// Take ownership of a handle the bridge has just issued for `data`.
    ///
    /// The caller establishes that `handle` came from `data` and that nothing else will release
    /// it — which is why this is the only way in, and why the derived accessors below do not
    /// ask again.
    pub(super) const fn owned(engine: &'a WebQpdf, session: &'a Session, handle: u32) -> Self {
        Self {
            engine,
            session,
            handle,
        }
    }

    /// The bare `u32`, to hand **to** the bridge and for nothing else.
    ///
    /// The native `ObjectHandle::raw` carries the same warning and it is the same one:
    /// `core/CLAUDE.md`'s rule that a handle is not an identity. Two handles to the same object
    /// never compare equal, so this value may be passed and may not be compared, collected,
    /// searched for or matched.
    pub(super) const fn raw(&self) -> u32 {
        self.handle
    }

    /// A handle this one's document has just issued, bound to that same document.
    ///
    /// Every derived accessor goes through here, so a derived handle cannot be attributed to
    /// another document — the mistake `qpdf/handle.rs`'s `sibling` exists to make
    /// unexpressible, in the place where nothing was stopping it.
    const fn sibling(&self, handle: u32) -> Self {
        Self {
            engine: self.engine,
            session: self.session,
            handle,
        }
    }

    /// The `qpdf_data` this handle belongs to.
    fn data(&self) -> QpdfPtr {
        self.session.data()
    }

    /// Whatever this handle's document has latched, as an error.
    ///
    /// **After every call, without exception.** qpdf reports failure by latching, so a missed
    /// drain surfaces later against something unrelated. Reached through `self.data()` rather
    /// than through a session a caller passes, for the reason the module header gives.
    /// # Errors
    ///
    /// Whatever qpdf latched, mapped at the boundary as everywhere else.
    ///
    /// **Delegated rather than reimplemented.** The first version copied
    /// [`Session::take_error`]'s three bridge calls line for line, which is two drains that
    /// would diverge the moment one grew a step. Holding the session is what makes delegating
    /// possible, and is the same change that closed the outlives hole above.
    pub(super) fn drained(&self) -> Result<()> {
        self.session.take_error().map_or(Ok(()), Err)
    }

    /// `qpdf_oh_get_key`. Resolves an indirect object, so this is the parser on file bytes.
    pub(super) fn key(&self, key: QpdfPtr) -> Self {
        self.sibling(
            self.engine
                .bridge()
                .oh_get_key(self.data(), self.handle, key),
        )
    }

    /// `qpdf_oh_get_array_item`.
    pub(super) fn array_item(&self, at: i32) -> Self {
        self.sibling(
            self.engine
                .bridge()
                .oh_get_array_item(self.data(), self.handle, at),
        )
    }

    /// `qpdf_oh_get_dict` — a stream's dictionary. A Form XObject is not a dictionary.
    pub(super) fn stream_dict(&self) -> Self {
        self.sibling(self.engine.bridge().oh_get_dict(self.data(), self.handle))
    }

    /// `qpdf_oh_get_type_code`, read before any value is.
    pub(super) fn type_code(&self) -> i32 {
        self.engine
            .bridge()
            .oh_get_type_code(self.data(), self.handle)
    }

    /// `qpdf_oh_get_int_value`. Meaningful only once the type code has said it is an integer.
    pub(super) fn int_value(&self) -> i64 {
        self.engine
            .bridge()
            .oh_get_int_value(self.data(), self.handle)
    }

    /// `qpdf_oh_get_object_id` and `_generation`, packed as qpdf's `(id << 32) | generation`.
    ///
    /// **This is the identity**, and the only thing about a handle that may be compared.
    pub(super) fn object(&self) -> u64 {
        self.engine.bridge().oh_object(self.data(), self.handle)
    }

    /// `qpdf_oh_get_array_n_items`.
    pub(super) fn array_len(&self) -> i32 {
        self.engine
            .bridge()
            .oh_get_array_n_items(self.data(), self.handle)
    }

    /// `qpdf_oh_unparse_resolved`, as a pointer into the engine heap.
    pub(super) fn unparse_resolved(&self) -> QpdfPtr {
        self.engine
            .bridge()
            .oh_unparse_resolved(self.data(), self.handle)
    }

    /// `qpdf_oh_get_name`, as a pointer into the engine heap.
    pub(super) fn name(&self) -> QpdfPtr {
        self.engine.bridge().oh_get_name(self.data(), self.handle)
    }

    /// `qpdf_oh_replace_key`. Sets `key` on this dictionary to `item`.
    ///
    /// **The one pairing this type still permits, and it is checked rather than argued.**
    /// `self.data()` is paired with `item.handle`, and two handles over two sessions unify
    /// freely — so this is the #130 shape spelled `&Self` instead of `QpdfPtr`, which the scan
    /// cannot see. Security review found it; the single call site pairs correctly today.
    pub(super) fn replace_key(&self, key: QpdfPtr, item: &Self) {
        debug_assert_eq!(
            self.data(),
            item.data(),
            "a value from another document would be written as this key"
        );
        self.engine
            .bridge()
            .oh_replace_key(self.data(), self.handle, key, item.handle);
    }

    /// `qpdf_oh_remove_key`.
    pub(super) fn remove_key(&self, key: QpdfPtr) {
        self.engine
            .bridge()
            .oh_remove_key(self.data(), self.handle, key);
    }

    /// `qpdf_oh_erase_item`.
    pub(super) fn erase_item(&self, at: i32) {
        self.engine
            .bridge()
            .oh_erase_item(self.data(), self.handle, at);
    }

    /// `qpdf_oh_get_page_content_data`, copied out and freed on the JS side.
    pub(super) fn page_content(&self) -> Option<Vec<u8>> {
        self.engine
            .bridge()
            .oh_page_content(self.data(), self.handle)
    }

    /// `qpdf_oh_get_stream_data`, copied out and freed on the JS side.
    pub(super) fn stream_data(&self) -> Option<Vec<u8>> {
        self.engine
            .bridge()
            .oh_stream_data(self.data(), self.handle)
    }
}

impl Drop for WebHandle<'_> {
    fn drop(&mut self) {
        // ONLY WHAT WAS ISSUED. qpdf numbers handles from 1 (`++qpdf->next_oh`), so 0 means no
        // handle was created — and releasing it is not harmless in the one place it matters:
        // `qpdf_oh_release` erases from a map, so a release of an id that was never issued would
        // cancel out a future leak of the same id rather than doing nothing. The fake refuses
        // the call outright, which is how it was found.
        //
        // THE COMPARISON IS BOUND TO A NAME so the hatch can sit on it. `check-handle-identity`
        // reads one line, and rustfmt will not keep a trailing comment after an `if`'s brace --
        // it moves it inside the block, where the checker no longer sees it and the violation
        // comes back. What the hatch opens is narrow: this compares against the LITERAL 0, not
        // against another handle, and 0 is readable precisely because qpdf numbers from 1.
        let issued = self.handle != 0; // handle-identity-ok: 0 is "no handle", not an object
        if issued {
            self.engine.bridge().oh_release(self.data(), self.handle);
        }
    }
}

/// Every function signature in this file, whitespace-collapsed onto one line.
///
/// **Signatures, not lines.** The first version of the scan below read lines, and rustfmt wraps
/// a signature past 100 columns onto several — at which point the first line holds neither
/// `&self` nor `QpdfPtr` and the offender is invisible. Both reviews planted exactly that and
/// watched the test stay green; `rotate.rs` already has a wrapped signature, and #131 writing
/// longer method names is what produces more.
#[cfg(test)]
fn signatures(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut building: Option<String> = None;
    for line in source.lines() {
        let trimmed = line.trim();
        let mut current = match building.take() {
            Some(mut carried) => {
                carried.push(' ');
                carried.push_str(trimmed);
                carried
            }
            None if trimmed.contains("fn ") => trimmed.to_owned(),
            None => continue,
        };
        // Balanced parentheses mean the parameter list is complete.
        let opens = current.matches('(').count();
        let closes = current.matches(')').count();
        if opens == 0 || opens > closes {
            building = Some(current);
            continue;
        }
        current = current.split_whitespace().collect::<Vec<_>>().join(" ");
        out.push(current);
    }
    out
}

/// Whether one signature pairs a handle with a document it did not come from.
///
/// **One predicate, used by the scan AND by its probes.** The first version wrote the rule twice
/// — once in the scan and once in a `catches` closure — and the copy omitted the prefix gate and
/// two exceptions, so all four probes passed over a rule that no longer matched anything. That
/// is `CLAUDE.md`'s "a rule that matches nothing passes everything", arriving through the probe
/// that was supposed to prevent it.
#[cfg(test)]
fn offends(signature: &str) -> bool {
    if !signature.contains("fn ") || !signature.contains("&self") {
        return false;
    }
    // A `-> QpdfPtr` is a RETURN, not a second document: `unparse_resolved` and `name` hand back
    // a pointer into the engine heap for the caller to copy out.
    let parameters = signature.split("->").next().unwrap_or(signature);
    // A `QpdfPtr` NAMED `key` is a NUL-terminated string in the engine heap, not a document.
    // Keyed on the parameter's name rather than on the method's, so renaming a method cannot
    // move it in or out of the exception — which the first version's list of three method names
    // could not survive.
    parameters.replace("key: QpdfPtr", "").contains("QpdfPtr")
}

#[cfg(test)]
mod tests {
    use super::{offends, signatures};

    /// No method here pairs a handle with a document it did not come from.
    ///
    /// The web counterpart of `qpdf::handle`'s
    /// `no_method_takes_both_a_handle_and_a_document`, and the stricter of the two: over the
    /// bridge there is no `Document` type to name, so what is refused is a `QpdfPtr` alongside
    /// `&self` — a handle already paired with one.
    ///
    /// `owned` is excluded because it takes no `&self`: it is the only way in, and takes the
    /// pair precisely so nothing else has to. The three key-string parameters are excluded by
    /// being NAMED `key`. An earlier version claimed `owned` was "named" as an exception and it
    /// was not — it was excluded by being a `const fn`, which the prefix gate never saw, and a
    /// new `const fn` offender joined the exception for free. Both reviews planted one.
    #[test]
    fn no_method_pairs_a_handle_with_a_separate_document() {
        // THE PRODUCTION API ONLY. Everything from the first `#[cfg(test)]` is this test and
        // its helpers, whose probe STRING LITERALS are deliberately offending signatures -- the
        // first run of this version reported all three of them as real offenders.
        let source = include_str!("handle.rs");
        let api = source
            .split_once("#[cfg(test)]")
            .map_or(source, |(before, _)| before);
        let found = signatures(api);

        let offenders: Vec<&String> = found.iter().filter(|s| offends(s)).collect();
        assert!(
            offenders.is_empty(),
            "a method pairs a handle with a separate document, so the two can disagree: \
             {offenders:?}"
        );

        // THE PROBES CALL THE RULE ITSELF, not a copy of it. Each of the first three is a shape
        // that slipped past the previous version, planted and measured by review.
        assert!(offends(
            "pub(super) fn thing(&self, data: QpdfPtr) -> u32 {"
        ));
        assert!(
            offends("pub(super) const fn thing(&self, data: QpdfPtr) -> u32 {"),
            "a `const fn` offender must be caught: three of this type's own methods are `const`"
        );
        assert!(
            offends("pub(super) fn wrapped( &self, data: QpdfPtr, ) -> u32 {"),
            "a rustfmt-wrapped signature must be caught once it is joined"
        );
        assert!(!offends("pub(super) fn name(&self) -> QpdfPtr {"));
        assert!(!offends("pub(super) fn key(&self, key: QpdfPtr) -> Self {"));
        assert!(!offends("pub(super) fn type_code(&self) -> i32 {"));
        assert!(!offends(
            "pub(super) const fn owned(engine: &'a WebQpdf, session: &'a Session, handle: u32) -> Self {"
        ));

        // AND THE JOINER WORKS, which the probes above cannot show on their own: a wrapped
        // signature in the real file must arrive as one string.
        let wrapped = signatures(
            "    pub(super) fn wide(\n        &self,\n        at: i32,\n    ) -> Self {\n",
        );
        assert_eq!(
            wrapped.len(),
            1,
            "the joiner did not rejoin one signature: {wrapped:?}"
        );
        assert!(wrapped[0].contains("&self") && wrapped[0].contains("at: i32"));

        // GATED ON THE EXACT COUNT, not a floor. A `>=` gate would not notice a method leaving
        // the scanned set -- which is how the `const fn` hole stayed invisible.
        let scanned = found.iter().filter(|s| s.contains("fn ")).count();
        assert_eq!(
            scanned, EXPECTED_SIGNATURES,
            "this file has {scanned} signatures and the scan expects {EXPECTED_SIGNATURES}; \
             update the constant deliberately when adding or removing one"
        );
    }

    /// Every `fn` in this file's PRODUCTION half -- the scan stops at the first `#[cfg(test)]`.
    ///
    /// Hard-coded so that adding a method is a deliberate edit here rather than a silent change
    /// in what the scan covers. `CLAUDE.md`: gate on the expected count where it is knowable.
    const EXPECTED_SIGNATURES: usize = 20;
}

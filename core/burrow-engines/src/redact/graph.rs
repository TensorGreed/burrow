//! The seam redaction's policy is written against (#191).
//!
//! Two traits, implemented once per platform: [`PdfDocument`] issues handles, and [`PdfObject`]
//! is a handle. The policy above them — the steps, the read-back witness, form sharing,
//! resources, optional content and the region's frame — is written **once**, over these, and the
//! platforms supply marshalling and nothing else. ADR 0029's #191 amendment records why: a
//! divergence between two redactions is a leak on one platform and not the other, and two copies
//! of a rule are how two answers begin. This is `prune::graph::ObjectGraph`'s precedent, applied a
//! second time.
//!
//! # The handle carries the verbs, and that is the part that is not `ObjectGraph`'s
//!
//! `ObjectGraph` puts every verb on the graph: `graph.key(&handle, key)`. That is a document
//! **beside** a handle, and nothing makes the two the same document. It is the shape #130 found on
//! the native side — a wrapper taking `&Document` beside `&self` drained the error slot of the
//! wrong document and returned `Ok` on a write that did not happen — and the shape #147 removed
//! from the web side by making a handle and the session that issued it one value. A trait with
//! that signature would have re-opened, for both platforms at once, the gap both of those closed
//! for one.
//!
//! So every verb here is on the handle, and each implementation reaches the engine and drains the
//! error **through the document the handle came from**, which it holds. The policy is never given
//! a document to pair a handle with: [`PdfDocument`] only issues handles, and a new value is made
//! *beside* an existing handle ([`PdfObject::null_beside`], [`PdfObject::integer_beside`]) rather
//! than by a document the caller names. The functions in the policy that took a document only to
//! drain it do not take one any more, which is the guarantee expressed as an absent parameter.
//!
//! # And a handle cannot outlive its document
//!
//! [`PdfDocument::Object`] is generic over the borrow of the document that issued it, so **the
//! policy** cannot hold a handle past that borrow: generic code that drops a document while holding
//! one of its handles does not compile (E0505, measured by the security review of this change).
//!
//! **What the trait cannot do is make an implementation's handle carry that borrow.** An
//! `Object<'a>` that names a type holding no lifetime compiles, and its handle then outlives the
//! document — measured the same way. So the half of the guarantee that stops a handle outliving
//! its document **per implementation** is each implementation's own type: the native
//! `ObjectHandle<'a>` carries `PhantomData<&'a Document>`, and the web `WebObject<'a>` borrows its
//! document and wraps a `WebHandle<'a>`, which borrows its session since #147. Both need it for the same reason: the handle's `Drop`
//! releases it into the document, and releasing into a document that has been cleaned up is a
//! use-after-free natively and a wild write in the wasm heap on the web — which is what #147's
//! reviews compiled before the web handle borrowed its session.
//!
//! # What the types cannot hold, and what the implementations do instead
//!
//! Two verbs take a second handle: [`PdfObject::replace_stream_data`] and
//! [`PdfObject::set_array_item`]. Two handles of the same type
//! from two documents unify freely — the lifetime says each outlives nothing, not that they share
//! a document — so this is the one pairing the traits can express. **Every implementation checks
//! it and returns an error**, never a debug assertion: a value from another document is a handle
//! id that very likely names a live, unrelated object in this one (qpdf numbers handles
//! `++next_oh`), and the write would land on it.
//!
//! # The drain is the policy's in this step, through the handle, and that is deliberate
//!
//! qpdf reports failure by latching an error on the document, which a caller drains. ADR 0029's
//! #191 amendment asks for the drain to move into each method, as `ObjectGraph`'s does. **This
//! step does not do that, and the reason is measured.** The move is required to change no
//! behaviour, and draining inside every accessor does change it. `getKey` on an object with no
//! owning document raises and latches an error; on one fixture the native policy asks exactly that
//! of an absent `/XObject` and then refuses by its own rule before any drain, and a drain inside
//! `key` reports the engine's error instead. The
//! `redaction_defences` suite caught it (`a_do_naming_nothing_the_resources_hold_is_refused`),
//! where the corpus golden could not.
//!
//! So these accessors mirror `qpdf::handle::ObjectHandle`'s as they are — the ones that did not
//! drain do not, the four that did ([`PdfObject::object`], [`PdfObject::page_content`],
//! [`PdfObject::stream_data`], [`PdfObject::replace_stream_data`]) still do, and so does
//! [`PdfDocument::page`], because every page lookup in the policy was followed by one — and the
//! policy drains where it always drained, through [`PdfObject::drained`]. The engine calls are
//! then the same calls in the same order, **with two exceptions**: `PdfDocument::page` reads the
//! page count to bounds-check, which is a cached read that neither clears nor sets the error slot,
//! and one null handle is released a few lines earlier than it was, which is a map erase. That is
//! the argument for "no behaviour change" that does not rest on a corpus. Moving the drain into
//! the methods is its own change, with its own measurement, after this one.
//!
//! What the policy may not do, and cannot: drain through a document it names. `drained` is on the
//! handle and reaches the document that issued it.
//!
//! # Accessors do not raise on a type mismatch
//!
//! qpdf returns a null object for a key on a non-dictionary and `0` for the integer value of a
//! name, on both platforms, because it is the same qpdf. So the policy asks
//! [`PdfObject::type_code`] **before** it reads a value, exactly as it always has.

use core::ffi::c_int;

use burrow_types::{Deadline, Result};

use crate::name::Name;

/// A document redaction can read and edit, however the engine is reached.
///
/// Owned by the redaction that edits it. `redact::Steps` requires that dropping the steps drops
/// the document, so a failed step leaves nothing to emit from; an implementation of this trait is
/// the document, not a reference to one, for that reason.
pub(crate) trait PdfDocument {
    /// A handle to one object in this document, borrowing it.
    type Object<'a>: PdfObject
    where
        Self: 'a;

    /// How many pages the document has.
    ///
    /// # Errors
    ///
    /// Whatever the engine reports.
    fn page_count(&self) -> Result<u64>;

    /// The page at `index`, zero-based.
    ///
    /// **Bounds-checked here**, so the policy holds no `unsafe`: the native call underneath is
    /// undefined past the end. The policy still checks first where it has a better error to give.
    ///
    /// # Errors
    ///
    /// [`burrow_types::Error::InvalidArgument`] for an index the document does not have, and
    /// whatever the engine reports.
    fn page(&self, index: usize) -> Result<Self::Object<'_>>;

    /// Write the document out: object streams preserved, a deterministic `/ID`, and nothing a
    /// caller can loosen.
    ///
    /// # Errors
    ///
    /// Whatever the engine reports.
    fn write(&self) -> Result<Vec<u8>>;
}

/// A handle to one object, bound to the document that issued it.
///
/// Each method reaches the engine through that document. Unless it says it drains, a method does
/// not: an error it latches waits for the next [`Self::drained`], exactly as with the native
/// `ObjectHandle` this mirrors. See the module header for why, in this step.
pub(crate) trait PdfObject: Sized {
    /// Whatever this handle's document has latched, as an error. **Through this handle's own
    /// document**, never one a caller names (#130).
    ///
    /// # Errors
    ///
    /// The latched error, mapped at the boundary as everywhere else.
    fn drained(&self) -> Result<()>;

    /// This object's type: the `enum qpdf_object_type_e` ordinal, as
    /// `codes::qpdf::object_type` names them. Asked before any value is read.
    fn type_code(&self) -> c_int;

    /// The value at `key`. A null object when the key is absent or this is not a dictionary;
    /// qpdf raises for neither, though it may latch.
    fn key(&self, key: &Name) -> Self;

    /// This name object's value, **with** its leading `/`.
    ///
    /// # Errors
    ///
    /// For anything that is not a name, whose value the engine gives as an empty string. Does
    /// not drain.
    fn name(&self) -> Result<Name>;

    /// This integer object's value. Meaningful only once [`Self::type_code`] has said it is an
    /// integer; every other type reads as `0`.
    fn integer_value(&self) -> i64;

    /// This object's own syntax, children left as `N G R`: the route to a dictionary's keys,
    /// because qpdf's key iterator is not trapped (ADR 0013 §1), and to a real's value, because
    /// no trapped accessor reads one.
    fn unparse(&self) -> Vec<u8>;

    /// How many items this array has, or 0 for anything that is not an array.
    fn array_len(&self) -> c_int;

    /// The item at `at`. Out of range is a null object, not an error.
    fn array_item(&self, at: c_int) -> Self;

    /// This stream's dictionary. A stream is not a dictionary, so a Form XObject's `/Resources`
    /// is unreachable without this.
    fn stream_dict(&self) -> Self;

    /// This page's content streams, concatenated and decoded. A page with no `/Contents` is an
    /// empty vector. **Drains.**
    ///
    /// # Errors
    ///
    /// Whatever the engine latched.
    fn page_content(&self) -> Result<Vec<u8>>;

    /// This stream's data, decoded, or `None` if the engine could not decode it. **Drains.**
    ///
    /// **`None` is not "empty"**: it means the bytes are still compressed, and the policy refuses
    /// rather than reading them.
    ///
    /// # Errors
    ///
    /// Whatever the engine latched.
    fn stream_data(&self) -> Result<Option<Vec<u8>>>;

    /// This object's identity: its number and generation. **Drains.**
    ///
    /// **The only thing about a handle that may be compared** (ADR 0013's handle-identity
    /// amendment). A direct object is `(0, 0)`.
    ///
    /// # Errors
    ///
    /// Whatever the engine latched. This one may not fail open: both halves read as `0` on an
    /// internal failure, and `(0, 0)` equals `(0, 0)`.
    fn object(&self) -> Result<(c_int, c_int)>;

    /// A new null object, in this handle's document.
    fn null_beside(&self) -> Self;

    /// A new integer object, in this handle's document.
    fn integer_beside(&self, value: i64) -> Self;

    /// Remove `key` from this dictionary. Removing an absent key is not an error.
    fn remove_key(&self, key: &Name);

    /// Replace the item at `at`, in place. This does **not** renumber, unlike
    /// [`Self::erase_item`].
    ///
    /// # Errors
    ///
    /// [`burrow_types::Error::Internal`] if `item` belongs to another document — see the module
    /// header. Does not drain.
    fn set_array_item(&self, at: c_int, item: &Self) -> Result<()>;

    /// Remove the item at `at`. **Everything after it shifts down**, so a filter walks backwards.
    fn erase_item(&self, at: c_int);

    /// Replace this stream's data with `bytes`, declared with `filter` and `decode_parms` —
    /// nulls, for plain bytes the engine re-compresses on write. **Drains.**
    ///
    /// # Errors
    ///
    /// [`burrow_types::Error::Internal`] if either handle belongs to another document, and
    /// whatever the engine latched.
    fn replace_stream_data(&self, bytes: &[u8], filter: &Self, decode_parms: &Self) -> Result<()>;
}

/// An engine that can open a document for redaction, and open its output again to read it back.
///
/// Both halves go through here so the read-back is the same engine's, with the same ceilings, as
/// the edit — and a **fresh** document: `ClearedWitness` requires that the document it reads was
/// never the one that wrote the bytes.
pub(crate) trait OpensForRedaction {
    /// The document this engine opens.
    type Document: PdfDocument;

    /// Open `bytes` under `options`, applying every ceiling that applies before the engine.
    ///
    /// Returns the document and a deadline started when the open began. The redaction spends
    /// that deadline; the read-back discards it and spends the operation's.
    ///
    /// # Errors
    ///
    /// Every ceiling `options.limits` names, and whatever the engine reports.
    fn open_for_redaction(
        &self,
        bytes: &[u8],
        options: &crate::OpenOptions<'_>,
    ) -> Result<(Self::Document, Deadline)>;
}

#[cfg(test)]
mod tests {
    /// `PdfObject`'s method signatures, each joined onto one line.
    ///
    /// The trait's block only: from its `pub(crate) trait PdfObject` line to the `}` that closes
    /// it at column zero. Joined because rustfmt wraps a long signature, and a scan reading lines
    /// would see neither half of a wrapped offender — the hole `web/handle.rs`'s scan had.
    fn object_signatures(source: &str) -> Vec<String> {
        let Some(start) = source.find("pub(crate) trait PdfObject") else {
            return Vec::new();
        };
        let block = &source[start..];
        let block = &block[..block.find("\n}\n").unwrap_or(block.len())];
        let mut out = Vec::new();
        let mut building: Option<String> = None;
        for line in block.lines().map(str::trim) {
            if line.starts_with("//") {
                continue;
            }
            let current = match building.take() {
                Some(mut carried) => {
                    carried.push(' ');
                    carried.push_str(line);
                    carried
                }
                None if line.starts_with("fn ") => line.to_owned(),
                None => continue,
            };
            // `;` ends a required method and `{` a provided one, whether or not its body shares
            // the line.
            if current.ends_with(';') || current.contains('{') {
                out.push(current.split_whitespace().collect::<Vec<_>>().join(" "));
            } else {
                building = Some(current);
            }
        }
        out
    }

    /// Whether a signature takes a document, or anything that names one, beside the handle.
    fn offends(signature: &str) -> bool {
        let parameters = signature.split("->").next().unwrap_or(signature);
        ["Document", "Session", "QpdfPtr", "QpdfData", ": &D"]
            .iter()
            .any(|shape| parameters.contains(shape))
    }

    /// No `PdfObject` method takes a document beside the handle.
    ///
    /// The module header's claim that the policy is never given a document to pair a handle with
    /// holds only while this trait never asks for one. `qpdf::handle`'s scan holds the native
    /// accessors to it and `web::handle`'s the web ones; this is the trait both implement, which
    /// neither of those reads (security review of #191).
    #[test]
    fn no_object_method_takes_a_document_beside_the_handle() {
        let found = object_signatures(include_str!("graph.rs"));
        let offenders: Vec<&String> = found.iter().filter(|s| offends(s)).collect();
        assert!(
            offenders.is_empty(),
            "a PdfObject method takes a document beside the handle, so the two can disagree: \
             {offenders:?}"
        );

        // THE PROBES CALL THE RULE ITSELF.
        assert!(offends(
            "fn key(&self, document: &Document, key: &Name) -> Self;"
        ));
        assert!(offends("fn key(&self, into: &D, key: &Name) -> Self;"));
        assert!(offends(
            "fn drained(&self, session: &'a Session) -> Result<()>;"
        ));
        assert!(!offends("fn key(&self, key: &Name) -> Self;"));
        assert!(!offends("fn object(&self) -> Result<(c_int, c_int)>;"));
        assert!(!offends(
            "fn replace_stream_data(&self, bytes: &[u8], filter: &Self, decode_parms: &Self) -> Result<()>;"
        ));

        // AND THE JOINER REJOINS what rustfmt wraps, which the trait's own `replace_stream_data`
        // already is.
        let wrapped = object_signatures(
            "pub(crate) trait PdfObject: Sized {\n    fn wide(&self, at: c_int)\n    -> Result<()>;\n}\n",
        );
        assert_eq!(wrapped, ["fn wide(&self, at: c_int) -> Result<()>;"]);

        // GATED ON THE EXACT COUNT: a method leaving the scanned set must be a deliberate edit.
        assert_eq!(
            found.len(),
            EXPECTED_METHODS,
            "PdfObject has {} methods and the scan expects {EXPECTED_METHODS}: {found:#?}",
            found.len()
        );
    }

    /// Every method of `PdfObject`. Hard-coded so that adding one is an edit here too.
    const EXPECTED_METHODS: usize = 18;
}

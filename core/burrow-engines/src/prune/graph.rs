//! The seam the pruning policy is written against.
//!
//! One trait, implemented twice: once over a native `qpdf_data` and once over the web path's
//! session across the JS bridge. **The policy above it is written once**, which is the whole point
//! — see the module header for why this one is shared where `rotate`, `reorder` and `merge` are
//! deliberately not.
//!
//! # Every method returns `Result`, and that is not ceremony
//!
//! qpdf's C API reports failure by *latching* an error on the document, which a caller has to drain
//! after every call. The native pruning code did that inline, and a single missed drain surfaces
//! later against an unrelated page or against the write — a wrong answer attributed to the wrong
//! cause. Here the draining is the implementation's job and the policy never sees it, so "forgot to
//! drain" stops being a thing the policy can get wrong.
//!
//! # Accessors do not raise on a type mismatch, and the policy must still check
//!
//! qpdf returns a null object for a key on a non-dictionary and `0` for the integer value of a
//! name, rather than failing. That is true across the bridge too, because it is the same qpdf. So
//! [`ObjectGraph::type_code`] is asked **before** any value is read, exactly as the native rotate
//! path does — trapping answers crashes, not wrong answers (ADR 0013 §1).

use burrow_types::Result;

/// A PDF object graph that can be read and edited, however the engine is reached.
///
/// Handles are **per-document** and per-implementation: the native one wraps a `qpdf_oh` that
/// releases itself on drop, the web one is a bare `u32` across the bridge. The policy never
/// constructs one and never compares two — [`ObjectGraph::identity`] is the only thing that may be
/// compared, for the reason ADR 0013's handle-identity amendment records: `qpdf` issues a fresh
/// handle on every call, so two handles to one object never compare equal, and an always-false
/// comparison in pruning prunes nothing while producing a valid PDF.
pub trait ObjectGraph {
    /// A live reference to one object in this document.
    type Handle;

    /// The page at `index`, zero-based.
    ///
    /// # Errors
    ///
    /// Whatever the engine reports, including an index the document does not have.
    fn page(&self, index: usize) -> Result<Self::Handle>;

    /// The value at `key`, which is a canonicalised name **including its leading `/`**.
    ///
    /// Returns a handle to a null object when the key is absent or the receiver is not a
    /// dictionary; qpdf does not raise for either.
    ///
    /// # Errors
    ///
    /// Whatever the engine reports.
    fn key(&self, of: &Self::Handle, key: &[u8]) -> Result<Self::Handle>;

    /// This object's type: the `enum qpdf_object_type_e` ordinal, as `codes::qpdf::object_type`
    /// names the three the policy compares against.
    ///
    /// Asked before any value is read. See the module header.
    ///
    /// # Errors
    ///
    /// Whatever the engine reports.
    fn type_code(&self, of: &Self::Handle) -> Result<i32>;

    /// This name object's value, canonicalised, including its leading `/`.
    ///
    /// # Errors
    ///
    /// Whatever the engine reports.
    fn name(&self, of: &Self::Handle) -> Result<Vec<u8>>;

    /// This object's own syntax, with its **children** left as `N G R`.
    ///
    /// The route to a dictionary's keys, because qpdf's key iterator does not reach `trap_errors`
    /// and may not be called (ADR 0013 §1). `crate::pdfsyntax::dict` reads what this returns.
    ///
    /// # Errors
    ///
    /// Whatever the engine reports.
    fn unparse(&self, of: &Self::Handle) -> Result<Vec<u8>>;

    /// Remove `key` from this dictionary. Removing an absent key is not an error.
    ///
    /// # Errors
    ///
    /// Whatever the engine reports.
    fn remove_key(&self, of: &Self::Handle, key: &[u8]) -> Result<()>;

    /// How many items this array has, or 0 for anything that is not an array.
    ///
    /// # Errors
    ///
    /// Whatever the engine reports.
    fn array_len(&self, of: &Self::Handle) -> Result<i32>;

    /// The item at `at`. Out of range is a null object rather than an error.
    ///
    /// # Errors
    ///
    /// Whatever the engine reports.
    fn array_item(&self, of: &Self::Handle, at: i32) -> Result<Self::Handle>;

    /// Remove the item at `at`.
    ///
    /// **Everything after `at` shifts down**, which is why the policy walks an array backwards. A
    /// forward loop skips the item that takes the removed one's place — and for the annotation
    /// filter that is an annotation belonging to an excluded page never being examined.
    ///
    /// # Errors
    ///
    /// Whatever the engine reports.
    fn erase_item(&self, of: &Self::Handle, at: i32) -> Result<()>;

    /// This stream's dictionary.
    ///
    /// A stream is a distinct type from a dictionary, so a Form XObject's `/Resources` is
    /// unreachable without this.
    ///
    /// # Errors
    ///
    /// Whatever the engine reports.
    fn stream_dict(&self, of: &Self::Handle) -> Result<Self::Handle>;

    /// This page's content streams, concatenated and **decoded**.
    ///
    /// What the resource-name filter reads. In the file the names are inside a flate stream, so a
    /// scan of the raw bytes would find none of them and prune every resource the page uses.
    ///
    /// # Errors
    ///
    /// Whatever the engine reports. A page with no `/Contents` is an empty vector rather than an
    /// error: it is a legitimate page that draws nothing.
    fn page_content(&self, page: &Self::Handle) -> Result<Vec<u8>>;

    /// This stream's data, decoded, or `None` if the engine could not decode it.
    ///
    /// **`None` is not "empty" and the policy may not treat it as such.** It means the bytes came
    /// back still compressed, and lexing those for resource names yields accidents rather than the
    /// names that are there — the under-approximation that deletes a resource the page draws with.
    ///
    /// # Errors
    ///
    /// Whatever the engine reports.
    fn stream_data(&self, of: &Self::Handle) -> Result<Option<Vec<u8>>>;

    /// This object's identity: its number and generation.
    ///
    /// **The only thing that may be compared.** A direct object is `(0, 0)`, which the policy
    /// treats differently per call site — an inline `/Annots` array is never shared, and a stream
    /// that reports `(0, 0)` could not be identified and is followed rather than deduplicated.
    ///
    /// # Errors
    ///
    /// Whatever the engine latched. This one may **not** fail open: both reads return `0` on an
    /// internal failure, and `(0, 0)` equals `(0, 0)` — so a swallowed error here is two unrelated
    /// objects comparing equal.
    fn identity(&self, of: &Self::Handle) -> Result<(i32, i32)>;

    /// This integer object's value.
    ///
    /// Only meaningful once [`ObjectGraph::type_code`] has said it is an integer; every other type
    /// reads as `0`.
    ///
    /// # Errors
    ///
    /// Whatever the engine reports.
    fn integer_value(&self, of: &Self::Handle) -> Result<i64>;
}

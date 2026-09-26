//! The native half of redaction's seam: qpdf's C API, and nothing else (#191).
//!
//! **The policy lives in [`crate::redact`] and is written once.** This file is the marshalling —
//! each method calls the one `ObjectHandle` accessor it names, and drains exactly when that
//! accessor did before #191, so the engine sees the same calls in the same order, with the two
//! exceptions `crate::redact::graph`'s header states. The policy
//! drains through [`PdfObject::drained`], which reaches **the handle's own document**; it never
//! names a document to drain. See [`crate::redact::graph`] for why the verbs are on the handle
//! rather than on a graph beside it, and why the drain is not inside them yet.

use core::ffi::c_int;

use burrow_types::{Deadline, Error, Result};

use super::extract::{self, ObjectStreams};
use super::handle::ObjectHandle;
use super::{Document, Qpdf};
use crate::name::Name;
use crate::redact::graph::{OpensForRedaction, PdfDocument, PdfObject};

impl PdfDocument for Document {
    type Object<'a> = ObjectHandle<'a>;

    fn page_count(&self) -> Result<u64> {
        Self::page_count(self)
    }

    fn page(&self, index: usize) -> Result<Self::Object<'_>> {
        let pages = usize::try_from(Self::page_count(self)?)
            .map_err(|_| Error::Internal("a page count that does not fit in usize".to_owned()))?;
        if index >= pages {
            return Err(Error::InvalidArgument(
                "pdf: a page index past the end of the document".to_owned(),
            ));
        }
        // SAFETY: `index` is below the document's page count, read immediately above from this
        // document. Routes through `trap_errors`.
        let page = unsafe { ObjectHandle::page(self, index) };
        page.drained()?;
        Ok(page)
    }

    fn write(&self) -> Result<Vec<u8>> {
        // The document is its own source: redaction edits in place.
        extract::write_out(self, self, ObjectStreams::Preserve)
    }
}

/// Refuse a second handle from another document, before it reaches the C API.
fn same_document(this: &ObjectHandle<'_>, other: &ObjectHandle<'_>) -> Result<()> {
    if this.same_document(other) {
        Ok(())
    } else {
        Err(Error::Internal(
            "qpdf: a value from another document would be written into this one".to_owned(),
        ))
    }
}

impl PdfObject for ObjectHandle<'_> {
    fn drained(&self) -> Result<()> {
        Self::drained(self)
    }

    fn type_code(&self) -> c_int {
        Self::type_code(self)
    }

    fn key(&self, key: &Name) -> Self {
        Self::key(self, key)
    }

    fn name(&self) -> Result<Name> {
        Self::name(self)
    }

    fn integer_value(&self) -> i64 {
        Self::integer_value(self)
    }

    fn unparse(&self) -> Vec<u8> {
        Self::unparse(self)
    }

    fn array_len(&self) -> c_int {
        Self::array_len(self)
    }

    fn array_item(&self, at: c_int) -> Self {
        Self::array_item(self, at)
    }

    fn stream_dict(&self) -> Self {
        Self::stream_dict(self)
    }

    fn page_content(&self) -> Result<Vec<u8>> {
        // Drains itself: the buffer is freed and the slot read in `take_malloced_buffer`.
        Self::page_content(self)
    }

    fn stream_data(&self) -> Result<Option<Vec<u8>>> {
        // Drains itself, as `page_content`.
        Self::stream_data(self)
    }

    fn object(&self) -> Result<(c_int, c_int)> {
        // Drains itself, and returns `Result` rather than failing open. See `handle.rs`.
        Self::object(self)
    }

    fn null_beside(&self) -> Self {
        Self::null_beside(self)
    }

    fn integer_beside(&self, value: i64) -> Self {
        Self::integer_beside(self, value)
    }

    fn remove_key(&self, key: &Name) {
        Self::remove_key(self, key);
    }

    fn set_array_item(&self, at: c_int, item: &Self) -> Result<()> {
        // CHECKED, NOT ASSUMED. `ObjectHandle::set_array_item`'s SAFETY comment leaves this to
        // the caller, and the caller is now generic code the type system cannot hold to it.
        same_document(self, item)?;
        Self::set_array_item(self, at, item);
        Ok(())
    }

    fn erase_item(&self, at: c_int) {
        Self::erase_item(self, at);
    }

    fn replace_stream_data(&self, bytes: &[u8], filter: &Self, decode_parms: &Self) -> Result<()> {
        // `ObjectHandle::replace_stream_data` checks both and drains itself (#130).
        Self::replace_stream_data(self, bytes, filter, decode_parms)
    }
}

impl OpensForRedaction for Qpdf {
    type Document = Document;

    fn open_for_redaction(
        &self,
        bytes: &[u8],
        options: &crate::OpenOptions<'_>,
    ) -> Result<(Self::Document, Deadline)> {
        let (document, _, _, deadline) =
            super::open_document(bytes.to_vec().into_boxed_slice(), options)?;
        Ok((document, deadline))
    }
}

#[cfg(all(test, feature = "native-engines", burrow_native_engines))]
mod tests {
    use std::sync::Arc;

    use burrow_types::{Clock, Limits, ManualClock};

    use crate::codes::qpdf::object_type;
    use crate::minimal_pdf;
    use crate::name::Name;
    use crate::redact::graph::{PdfDocument, PdfObject};

    /// The drain inside `PdfDocument::page` reports what the document latched before it.
    ///
    /// # Why this needs its own test
    ///
    /// That one line replaced four drains that each followed a page lookup: in `page_handle`,
    /// the strip loop, `count_form_uses` and the read-back's `page`. The security review of #191
    /// deleted it and the whole native suite stayed green, golden included, because no fixture
    /// latches an error at any of those points.
    ///
    /// # The latch is a real one
    ///
    /// qpdf's `getKey` on an object with **no owning document** raises instead of warning, and
    /// the error latches: `QPDFObjectHandle::warn` throws when it has no `QPDF` to warn through.
    /// A free-standing null is such an object, so no hook is needed to make the document hold an
    /// error. It is the same class as the `/XObject` lookup that kept #191's drain out of the
    /// accessors (ADR 0029's 2026-09-25 amendment). **Not every null does it**, measured: one from
    /// an absent key, or from keying an array, carries its owner, and only warns.
    #[test]
    fn a_page_lookup_drains_what_the_document_latched_before_it() {
        let options = crate::OpenOptions::new(
            Limits::default(),
            Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
        );
        let (document, _, _, _) =
            super::super::open_document(minimal_pdf::pdf_with_ink().into(), &options)
                .expect("opens");
        {
            let page = PdfDocument::page(&document, 0).expect("page 0");
            // A NULL WITH NO OWNING DOCUMENT, which is what `qpdf_oh_new_null` makes. qpdf's
            // `typeWarning` raises for such an object rather than warning, and the error latches.
            let null = PdfObject::null_beside(&page);
            assert_eq!(PdfObject::type_code(&null), object_type::NULL);
            let _ = PdfObject::key(&null, &Name::literal(b"/Anything\0"));
        }

        let latched = PdfDocument::page(&document, 0);
        assert!(
            latched.is_err(),
            "the page lookup returned a handle over an error the document was holding"
        );

        // THE NEAR-MISS: the drain consumed the error, so the next lookup is clean. Without it
        // this passes for a lookup that refuses everything.
        PdfDocument::page(&document, 0).expect("nothing is latched any more");
    }

    /// `PdfObject::drained` reports what the handle's document latched, and consumes it.
    ///
    /// The policy's drains all funnel through this one method since #191: about sixty sites that
    /// were each `document.take_error()`. The code review of that change made it inert --
    /// `let _ = Self::drained(self); Ok(())` -- and every test stayed green, because no fixture
    /// latches an error at any of those sites. One site means one test can hold all of them.
    #[test]
    fn a_handle_drains_what_its_document_latched() {
        let options = crate::OpenOptions::new(
            Limits::default(),
            Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
        );
        let (document, _, _, _) =
            super::super::open_document(minimal_pdf::pdf_with_ink().into(), &options)
                .expect("opens");
        let page = PdfDocument::page(&document, 0).expect("page 0");
        PdfObject::drained(&page).expect("nothing is latched before the planted error");

        // Latched as in the page test above: a key of a null with no owning document.
        let null = PdfObject::null_beside(&page);
        let _ = PdfObject::key(&null, &Name::literal(b"/Anything\0"));

        assert!(
            PdfObject::drained(&page).is_err(),
            "a handle's drain returned Ok over an error its document was holding"
        );
        // Consumed, so a second drain is clean -- otherwise one latched error would fail every
        // later step against something unrelated.
        PdfObject::drained(&page).expect("the first drain consumed it");
    }

    /// `PdfDocument::page` refuses an index past the end rather than handing it to qpdf.
    ///
    /// The native lookup underneath is `unsafe` on such an index, and `redact`'s
    /// `forbid(unsafe_code)` rests on this check. Every caller checks first with an error that
    /// names its rule, so no redaction test reaches it: the code review planted `&& false` on it
    /// and the suite stayed green.
    #[test]
    fn a_page_past_the_end_is_refused_by_the_lookup_itself() {
        let options = crate::OpenOptions::new(
            Limits::default(),
            Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
        );
        let (document, pages, _, _) =
            super::super::open_document(minimal_pdf::pdf_with_pages(3).into(), &options)
                .expect("opens");
        assert_eq!(pages, 3);
        for past in [3, 4, usize::MAX] {
            let refused = PdfDocument::page(&document, past);
            assert!(
                matches!(refused, Err(burrow_types::Error::InvalidArgument(_))),
                "page {past} of 3 was not refused by the lookup: {:?}",
                refused.map(|_| ())
            );
        }
        // THE NEAR-MISS: the last page is a page.
        PdfDocument::page(&document, 2).expect("page 2 of 3 exists");
    }
}

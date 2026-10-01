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
use crate::codes::qpdf::object_type;
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

    fn object(&self, number: c_int, generation: c_int) -> Result<Self::Object<'_>> {
        let object = ObjectHandle::by_id(self, number, generation);
        object.drained()?;
        Ok(object)
    }

    fn write(&self) -> Result<Vec<u8>> {
        // The document is its own source: redaction edits in place.
        extract::write_out(self, self, ObjectStreams::Preserve)
    }

    fn repaired(&self) -> bool {
        Document::repaired(self)
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

    // TYPE-CHECKED BEFORE THE CALL, in `key`, `name` and `array_item` below. qpdf answers a
    // read of the wrong type -- or an array read out of range -- with a fallback *and a
    // warning*, and a warning at the write is refused as a repair (#224). Each guard has a
    // document that redacts with it and is refused without it: an integer `/Font` entry, an
    // XObject whose `/Subtype` is a number, and a `/W` array that ends mid-range.

    fn key(&self, key: &Name) -> Self {
        if Self::type_code(self) != object_type::DICTIONARY {
            return Self::null_beside(self);
        }
        Self::key(self, key)
    }

    fn name(&self) -> Result<Name> {
        if Self::type_code(self) != object_type::NAME {
            return Name::from_canonical(&[]);
        }
        Self::name(self)
    }

    fn integer_value(&self) -> i64 {
        // UNGUARDED, because unreachable: every caller has asked `type_code` for an integer.
        Self::integer_value(self)
    }

    fn unparse(&self) -> Vec<u8> {
        Self::unparse(self)
    }

    fn array_len(&self) -> c_int {
        // UNGUARDED, because unreachable: every caller -- `array_item`'s range check included --
        // asks it only of an array.
        Self::array_len(self)
    }

    fn array_item(&self, at: c_int) -> Self {
        if at < 0 || at >= PdfObject::array_len(self) {
            return Self::null_beside(self);
        }
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
        // A REPAIRED INPUT IS REFUSED (#224): see `repaired_by_the_engine`. After the page count
        // `open_document` takes, which is when qpdf flattens -- and repairs -- the page tree.
        if document.repaired() {
            return Err(crate::redact::repaired_by_the_engine());
        }
        Ok((document, deadline))
    }

    /// PDFium's size for the page, read without loading its content: `page_size` goes by index
    /// precisely so that no display list is built (#103).
    ///
    /// THE RENDERER'S OPEN STARTS ITS OWN DEADLINE from `options`, as the read-back's does
    /// (`witness.rs`): the redaction checkpoints just before this call, so the overshoot past the
    /// operation's budget is this open and one size read, and every ceiling applies to it.
    fn renderer_page_size(
        &self,
        bytes: &[u8],
        page: usize,
        options: &crate::OpenOptions<'_>,
    ) -> Result<Option<(f64, f64)>> {
        use crate::{DocumentEngine, PageRenderer};
        let renderer = crate::pdfium::Pdfium;
        let document = renderer.open(bytes.to_vec().into_boxed_slice(), options)?;
        let index = u64::try_from(page)
            .map_err(|_| Error::Internal("a page index that does not fit in u64".to_owned()))?;
        let (width, height) = renderer.page_size(&document, index)?;
        Ok(Some((f64::from(width), f64::from(height))))
    }
}

#[cfg(all(test, feature = "native-engines", burrow_native_engines))]
mod tests {
    use std::sync::Arc;

    use burrow_types::{Clock, Limits, ManualClock};

    use crate::codes::qpdf::object_type;
    use crate::minimal_pdf;
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
    /// qpdf's `getDict` on anything that is not a stream throws -- `as_stream` asserts the type
    /// -- and the error latches, owner or no owner. So a stream's dictionary asked of a null makes
    /// the document hold an error with no hook. Until #224 this used a key of a free-standing
    /// null, which raises only for a null with no owner; the reader no longer hands the engine a
    /// key of a non-dictionary at all, so that route is closed.
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
            // A STREAM'S DICTIONARY, ASKED OF A NULL: qpdf raises for it, and the error latches.
            // Not a key of the null, which this used until #224: the reader no longer hands a
            // key of a non-dictionary to the engine at all.
            let null = PdfObject::null_beside(&page);
            assert_eq!(PdfObject::type_code(&null), object_type::NULL);
            let _ = PdfObject::stream_dict(&null);
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

    /// The lookup by number and generation drains too (#227), as the page lookup does.
    ///
    /// The reference check asks it once per reference and reads the type straight after, so an
    /// error left latched before it would be reported against a reference that has nothing to do
    /// with it -- or, were neither to drain, by nothing at all.
    #[test]
    fn an_object_lookup_drains_what_the_document_latched_before_it() {
        let options = crate::OpenOptions::new(
            Limits::default(),
            Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
        );
        let (document, _, _, _) =
            super::super::open_document(minimal_pdf::pdf_with_ink().into(), &options)
                .expect("opens");
        {
            let page = PdfDocument::page(&document, 0).expect("page 0");
            let _ = PdfObject::stream_dict(&PdfObject::null_beside(&page));
        }
        assert!(
            PdfDocument::object(&document, 1, 0).is_err(),
            "the object lookup returned a handle over an error the document was holding"
        );
        // THE NEAR-MISS, and the lookup's own answer: object 1 is the catalog.
        let catalog = PdfDocument::object(&document, 1, 0).expect("nothing is latched any more");
        assert_eq!(PdfObject::type_code(&catalog), object_type::DICTIONARY);
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

        // Latched as in the page test above: a stream's dictionary, asked of a null.
        let null = PdfObject::null_beside(&page);
        let _ = PdfObject::stream_dict(&null);

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

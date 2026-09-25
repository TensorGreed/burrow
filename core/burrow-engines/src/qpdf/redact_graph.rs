//! The native half of redaction's seam: qpdf's C API, and nothing else (#191).
//!
//! **The policy lives in [`crate::redact`] and is written once.** This file is the marshalling —
//! each method calls the one `ObjectHandle` accessor it names, and drains exactly when that
//! accessor did before #191, so the engine sees the same calls in the same order. The policy
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

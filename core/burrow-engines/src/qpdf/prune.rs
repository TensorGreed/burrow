//! The native half of the pruning seam: qpdf's C API, and nothing else.
//!
//! **The policy lives in [`crate::prune`] and is written once.** This file is the marshalling —
//! a key becomes a NUL-terminated C string, a handle is an [`ObjectHandle`] that releases itself
//! on drop, and qpdf's latched error is drained after every call so the policy never has to.
//!
//! Its web twin is `crate::web::prune`, and the two are *expected* to be near-identical in shape:
//! that is what makes them auditable against each other. What must never be duplicated is the
//! decision about what to remove, which is why that lives one directory up. See `crate::prune`'s
//! header for why this one capability is shared where `rotate`, `reorder` and `merge` deliberately
//! are not.

use burrow_types::{Error, Result};

use super::Document;
use super::handle::ObjectHandle;
use crate::prune::graph::ObjectGraph;

/// A qpdf document, seen as an object graph.
///
/// Borrowed rather than owned: every handle the policy holds borrows the document, which is the
/// invariant `handle.rs`'s lifetime enforces — releasing a handle into a cleaned-up document is a
/// compile error rather than a rule.
pub(super) struct QpdfGraph<'a> {
    document: &'a Document,
}

impl<'a> QpdfGraph<'a> {
    /// See `document` as a graph.
    pub(super) const fn over(document: &'a Document) -> Self {
        Self { document }
    }

    /// Drain whatever qpdf latched, as an error.
    ///
    /// **After every call, without exception.** qpdf reports failure by latching rather than
    /// returning, so a missed drain surfaces later against an unrelated page or against the write —
    /// a wrong answer attributed to the wrong cause. Doing it here, once per method, is what lets
    /// the policy be written without a drain after every line.
    fn drained(&self) -> Result<()> {
        self.document.take_error().map_or(Ok(()), Err)
    }
}

/// A key as qpdf's C API wants it: NUL-terminated.
///
/// The policy hands over a canonicalised name with its leading `/` and no NUL — it has already
/// refused a name containing one, because a C string would end there and the call would act on a
/// *different, shorter* key. This only appends the terminator.
fn c_key(key: &[u8]) -> Vec<u8> {
    let mut owned = Vec::with_capacity(key.len() + 1);
    owned.extend_from_slice(key);
    owned.push(0);
    owned
}

impl<'a> ObjectGraph for QpdfGraph<'a> {
    type Handle = ObjectHandle<'a>;

    fn page(&self, index: usize) -> Result<Self::Handle> {
        let pages = self.document.page_count()?;
        let at = u64::try_from(index)
            .map_err(|_| Error::Internal("page index does not fit in u64".to_owned()))?;
        if at >= pages {
            return Err(Error::InvalidArgument(
                "page is not in the document".to_owned(),
            ));
        }
        // SAFETY: `index` is below the document's page count, checked immediately above.
        // Routes through `trap_errors`.
        let page = unsafe { ObjectHandle::page(self.document, index) };
        self.drained()?;
        Ok(page)
    }

    fn key(&self, of: &Self::Handle, key: &[u8]) -> Result<Self::Handle> {
        let key = c_key(key);
        let value = of.key(self.document, key.as_ptr().cast());
        self.drained()?;
        Ok(value)
    }

    fn type_code(&self, of: &Self::Handle) -> Result<i32> {
        let code = of.type_code();
        self.drained()?;
        Ok(code)
    }

    fn name(&self, of: &Self::Handle) -> Result<Vec<u8>> {
        let name = of.name();
        self.drained()?;
        Ok(name)
    }

    fn unparse(&self, of: &Self::Handle) -> Result<Vec<u8>> {
        let text = of.unparse();
        self.drained()?;
        Ok(text)
    }

    fn remove_key(&self, of: &Self::Handle, key: &[u8]) -> Result<()> {
        let key = c_key(key);
        of.remove_key(key.as_ptr().cast());
        self.drained()
    }

    fn array_len(&self, of: &Self::Handle) -> Result<i32> {
        let len = of.array_len();
        self.drained()?;
        Ok(len)
    }

    fn array_item(&self, of: &Self::Handle, at: i32) -> Result<Self::Handle> {
        let item = of.array_item(self.document, at);
        self.drained()?;
        Ok(item)
    }

    fn erase_item(&self, of: &Self::Handle, at: i32) -> Result<()> {
        of.erase_item(at);
        self.drained()
    }

    fn stream_dict(&self, of: &Self::Handle) -> Result<Self::Handle> {
        let dictionary = of.stream_dict(self.document);
        self.drained()?;
        Ok(dictionary)
    }

    fn page_content(&self, page: &Self::Handle) -> Result<Vec<u8>> {
        page.page_content(self.document)
    }

    fn stream_data(&self, of: &Self::Handle) -> Result<Option<Vec<u8>>> {
        of.stream_data(self.document)
    }

    fn identity(&self, of: &Self::Handle) -> Result<(i32, i32)> {
        // `object` drains the error itself, and returns `Result` rather than failing open: both
        // reads yield 0 on an internal failure and `(0, 0)` equals `(0, 0)`, so a swallowed error
        // here is two unrelated objects comparing equal. See `handle.rs`.
        of.object(self.document)
    }

    fn integer_value(&self, of: &Self::Handle) -> Result<i64> {
        let value = of.integer_value();
        self.drained()?;
        Ok(value)
    }
}

//! The web half of redaction's seam: the JS bridge, and nothing else (#191).
//!
//! **The policy lives in [`crate::redact`] and is written once.** This file is the marshalling,
//! and it is meant to be read beside its native twin, `crate::qpdf::redact_graph`: every method
//! makes the call the native one makes, and drains exactly when the native one drains. The engine
//! then sees the same calls in the same order on both platforms, which is what lets the
//! differential test hold the web to the native outcome byte for byte rather than argue it.
//!
//! # What is marshalling here and not natively
//!
//! A key is a `Name` natively and a pointer to a copy of it in the engine heap here, memoised per
//! distinct name for the life of the document, as `web/prune.rs` does. A string the engine hands
//! back is a pointer into its heap and is copied out at once. Neither decides anything.
//!
//! # A marshalling failure is latched, like the engine's own
//!
//! Copying a key into the engine heap can fail, where the native `Name` cannot, and
//! [`PdfObject::key`] returns no `Result`: natively it cannot fail, and qpdf reports its own
//! failures by latching an error for the next drain rather than by returning one. So a failed copy
//! does the same. It records the error on the document and hands back a handle to nothing, and
//! the next drain reports the failure before anything qpdf latched. The policy drains at the
//! points it always has, so it meets this the way it meets any other latched error.
//!
//! # Both of #147's guarantees, in this implementation's own type
//!
//! [`crate::redact::graph`] states what its traits hold and what they cannot. The half that is
//! each implementation's is held here by [`WebObject`], which borrows the document for as long as
//! it lives. Its [`WebHandle`] borrows the session too, so no handle outlives the `qpdf_data` it
//! releases into. And a handle reaches the engine and drains only through its own session.

use core::cell::RefCell;
use core::ffi::c_int;
use std::collections::BTreeMap;
use std::sync::Arc;

use burrow_types::{Deadline, Error, Limits, Result, Stage};

use super::WebQpdf;
use super::bridge::QpdfPtr;
use super::handle::WebHandle;
use super::qpdf::Session;
use crate::name::Name;
use crate::redact::graph::{OpensForRedaction, PdfDocument, PdfObject};

/// `qpdf_o_preserve`: keep the input's object streams as they are. qpdf's own default, and stated
/// at the write for the reason the native `extract::write_out` states it: it is the one write
/// parameter that differs between operations.
const QPDF_O_PRESERVE: u32 = 1;

/// A document opened for redaction on the web: the session, and what marshalling needs.
///
/// Owned by the redaction, as `redact::Steps` requires: dropping it drops the session, which
/// drains and cleans up the `qpdf_data` and wipes the input it read from.
pub(crate) struct WebRedactionDocument<'e> {
    engine: &'e WebQpdf,
    session: Session,
    /// Keys copied into the engine heap, one per distinct name, freed when the document drops.
    keys: RefCell<BTreeMap<Vec<u8>, QpdfPtr>>,
    /// A marshalling failure waiting for the next drain. See the module header.
    latched: RefCell<Option<Error>>,
}

impl WebRedactionDocument<'_> {
    /// A key in the engine heap, NUL-terminated, kept for the life of the document.
    ///
    /// `None` when the engine could not allocate it, with the failure latched.
    fn key_ptr(&self, key: &Name) -> Option<QpdfPtr> {
        if let Some(ptr) = self.keys.borrow().get(key.bytes()) {
            return Some(*ptr);
        }
        let ptr = self.engine.bridge().copy_in(key.bytes());
        if ptr.is_null() {
            self.latch(Error::Internal(
                "the qpdf module could not allocate a dictionary key".to_owned(),
            ));
            return None;
        }
        self.keys.borrow_mut().insert(key.bytes().to_vec(), ptr);
        Some(ptr)
    }

    /// Record a marshalling failure for the next drain, keeping the first.
    fn latch(&self, error: Error) {
        let mut slot = self.latched.borrow_mut();
        if slot.is_none() {
            *slot = Some(error);
        }
    }

    /// The marshalling failure first, then whatever qpdf latched.
    fn drained(&self) -> Result<()> {
        if let Some(error) = self.latched.borrow_mut().take() {
            return Err(error);
        }
        self.session.take_error().map_or(Ok(()), Err)
    }

    fn wrap<'a>(&'a self, handle: WebHandle<'a>) -> WebObject<'a> {
        WebObject {
            handle,
            document: self,
        }
    }
}

impl Drop for WebRedactionDocument<'_> {
    fn drop(&mut self) {
        for ptr in self.keys.borrow().values() {
            // `free`, not `wipe_and_free`: a dictionary key is a name out of a document or out of
            // the specification, not something a person typed. `web/prune.rs` makes the same
            // call for the same reason.
            self.engine.bridge().free(*ptr);
        }
    }
}

/// A handle into a [`WebRedactionDocument`], borrowing it.
pub(crate) struct WebObject<'a> {
    handle: WebHandle<'a>,
    document: &'a WebRedactionDocument<'a>,
}

impl<'e> PdfDocument for WebRedactionDocument<'e> {
    type Object<'a>
        = WebObject<'a>
    where
        Self: 'a;

    fn page_count(&self) -> Result<u64> {
        self.session.page_count()
    }

    fn page(&self, index: usize) -> Result<Self::Object<'_>> {
        let pages = usize::try_from(self.session.page_count()?)
            .map_err(|_| Error::Internal("a page count that does not fit in usize".to_owned()))?;
        if index >= pages {
            return Err(Error::InvalidArgument(
                "pdf: a page index past the end of the document".to_owned(),
            ));
        }
        let n = u32::try_from(index)
            .map_err(|_| Error::Internal("page index does not fit in u32".to_owned()))?;
        // OWNED BEFORE THE DRAIN: qpdf issues a handle on its error path too, and wrapping first
        // is what makes every route release it. `web/prune.rs` has the measurement.
        let page = WebHandle::owned(
            self.engine,
            &self.session,
            self.engine.bridge().get_page_n(self.session.data(), n),
        );
        self.drained()?;
        Ok(self.wrap(page))
    }

    fn write(&self) -> Result<Vec<u8>> {
        // THE NATIVE `extract::write_out`, CALL FOR CALL AND MESSAGE FOR MESSAGE. The
        // differential test compares refusals by their text, so a different message here would be
        // a divergence it reports, and it should.
        let bridge = self.engine.bridge();
        let data = self.session.data();
        let init = bridge.init_write_memory(data);
        if crate::codes::qpdf::has_errors(init) {
            return Err(self.session.take_error().unwrap_or_else(|| {
                Error::Io("qpdf could not prepare an in-memory write".to_owned())
            }));
        }
        // AFTER `init_write_memory`, never before: the writer does not exist until that call
        // succeeds, and both of these dereference it.
        bridge.set_deterministic_id(data, true);
        bridge.set_object_stream_mode(data, QPDF_O_PRESERVE);
        let wrote = bridge.write(data);
        if crate::codes::qpdf::has_errors(wrote) {
            return Err(self.session.take_error().unwrap_or_else(|| {
                Error::Malformed("qpdf: the output could not be written".to_owned())
            }));
        }
        if let Some(error) = self.session.take_error() {
            return Err(error);
        }
        let len = bridge.get_buffer_length(data);
        let ptr = bridge.get_buffer(data);
        // BOTH are checked, not one, as natively: a null buffer with a length, or a pointer with
        // none, is not a document.
        if ptr.is_null() || len == 0 {
            return Err(Error::Io("qpdf produced no output".to_owned()));
        }
        Ok(bridge.copy_out(ptr, len))
    }
}

impl<'a> WebObject<'a> {
    /// A handle this one's session just issued, beside this one.
    fn beside(&self, handle: WebHandle<'a>) -> Self {
        Self {
            handle,
            document: self.document,
        }
    }

    /// A handle to nothing, for a key that could not be copied into the engine heap.
    ///
    /// Handle 0 is never issued (qpdf numbers from 1) and never released. qpdf answers
    /// `ot_uninitialized` for its type, measured, so the policy's type checks read it as absent
    /// until its next drain, which reports the allocation before anything qpdf latched.
    fn nothing(&self) -> Self {
        self.beside(WebHandle::owned(
            self.document.engine,
            &self.document.session,
            0,
        ))
    }
}

impl PdfObject for WebObject<'_> {
    fn drained(&self) -> Result<()> {
        self.document.drained()
    }

    fn type_code(&self) -> c_int {
        self.handle.type_code()
    }

    fn key(&self, key: &Name) -> Self {
        match self.document.key_ptr(key) {
            Some(ptr) => self.beside(self.handle.key(ptr)),
            None => self.nothing(),
        }
    }

    fn name(&self) -> Result<Name> {
        // AS NATIVELY: the engine's answer, copied out, then read as a name. A non-name comes back
        // as an empty string, which has no slash, so this is `Err` for it -- and it does not drain.
        let ptr = self.handle.name();
        let text = if ptr.is_null() {
            Vec::new()
        } else {
            self.document.engine.bridge().copy_c_string(ptr)
        };
        Name::from_canonical(&text)
    }

    fn integer_value(&self) -> i64 {
        self.handle.int_value()
    }

    fn unparse(&self) -> Vec<u8> {
        let ptr = self.handle.unparse_resolved();
        if ptr.is_null() {
            return Vec::new();
        }
        // COPIED AT ONCE: the pointer dies on the next call that returns one.
        self.document.engine.bridge().copy_c_string(ptr)
    }

    fn array_len(&self) -> c_int {
        self.handle.array_len()
    }

    fn array_item(&self, at: c_int) -> Self {
        self.beside(self.handle.array_item(at))
    }

    fn stream_dict(&self) -> Self {
        self.beside(self.handle.stream_dict())
    }

    fn page_content(&self) -> Result<Vec<u8>> {
        // DRAINS, as the native accessor does.
        let data = self.handle.page_content();
        self.drained()?;
        // `None` is the module out of memory, not a decode failure: a qpdf throw was latched and
        // the drain above has already returned it. See `web/prune.rs`.
        data.ok_or_else(|| {
            Error::Internal(
                "the qpdf module could not allocate to read a page's content".to_owned(),
            )
        })
    }

    fn stream_data(&self) -> Result<Option<Vec<u8>>> {
        // DRAINS, as the native accessor does. `None` is "could not decode", which the policy
        // refuses rather than reads, exactly as natively.
        let data = self.handle.stream_data();
        self.drained()?;
        Ok(data)
    }

    fn object(&self) -> Result<(c_int, c_int)> {
        // DRAINS AND MAY NOT FAIL OPEN: both halves read as 0 on an internal failure, and (0, 0)
        // equals (0, 0).
        let packed = self.handle.object();
        self.drained()?;
        let number = c_int::try_from(packed >> 32)
            .map_err(|_| Error::Internal("an object number does not fit in c_int".to_owned()))?;
        let generation = c_int::try_from(packed & 0xffff_ffff)
            .map_err(|_| Error::Internal("a generation does not fit in c_int".to_owned()))?;
        Ok((number, generation))
    }

    fn null_beside(&self) -> Self {
        self.beside(self.handle.null_beside())
    }

    fn integer_beside(&self, value: i64) -> Self {
        self.beside(self.handle.integer_beside(value))
    }

    fn remove_key(&self, key: &Name) {
        if let Some(ptr) = self.document.key_ptr(key) {
            self.handle.remove_key(ptr);
        }
    }

    fn set_array_item(&self, at: c_int, item: &Self) -> Result<()> {
        self.handle.set_array_item(at, &item.handle)
    }

    fn erase_item(&self, at: c_int) {
        self.handle.erase_item(at);
    }

    fn replace_stream_data(&self, bytes: &[u8], filter: &Self, decode_parms: &Self) -> Result<()> {
        // DRAINS AFTER THE CALL, through `Session::replace_stream_data`, as natively -- and not
        // before it, which would drain a qpdf error the native path reports after its write. Only
        // the web's own marshalling latch is read first, so a key that failed to reach the engine
        // is not reported as this write's failure.
        if let Some(error) = self.document.latched.borrow_mut().take() {
            return Err(error);
        }
        self.handle
            .replace_stream_data(bytes, &filter.handle, &decode_parms.handle)
    }
}

/// The web engine, as redaction opens documents with it.
///
/// `Copy`, because the read-back takes a fresh copy of its engine: `ClearedWitness` requires that
/// the document it reads was never the one that wrote the bytes, and a fresh document of the same
/// engine is that.
#[derive(Clone, Copy)]
pub(crate) struct WebRedactor<'e>(pub(crate) &'e WebQpdf);

impl<'e> OpensForRedaction for WebRedactor<'e> {
    type Document = WebRedactionDocument<'e>;

    fn open_for_redaction(
        &self,
        bytes: &[u8],
        options: &crate::OpenOptions<'_>,
    ) -> Result<(Self::Document, Deadline)> {
        // THE NATIVE `open_document`, STEP FOR STEP: every ceiling before the engine, the deadline
        // started, the engine's memory read before the open so the measured check covers the copy
        // into its heap, recovery off, the page ceiling, a drain, and the measured check. The web
        // rotate path opens the same way.
        let limits: Limits = options.limits;
        crate::estimate::before_open(bytes, &limits)?;
        let clock = Arc::clone(&options.clock);
        let deadline = Deadline::start(clock.as_ref(), &limits);
        deadline.checkpoint(clock.as_ref())?;
        let engine = self.0;
        let heap_before = engine.bridge().heap_bytes();
        let session = Session::open(engine, bytes, options.password, false)?;
        let pages = session.page_count()?;
        Limits::check(Stage::PageCount, "max_pages", pages, limits.max_pages)?;
        if let Some(error) = session.take_error() {
            return Err(error);
        }
        crate::estimate::check_measured_memory(
            Some(heap_before),
            Some(engine.bridge().heap_bytes()),
            &limits,
        )?;
        Ok((
            WebRedactionDocument {
                engine,
                session,
                keys: RefCell::new(BTreeMap::new()),
                latched: RefCell::new(None),
            },
            deadline,
        ))
    }
}

/// Redaction on the web engine: the policy written once in [`crate::redact`], over this file's
/// half of its seam.
///
/// The same body as the native implementation, [`crate::redact::redact_page`], so the steps, the
/// sharing rules and the read-back through a fresh document are the same code on both platforms.
/// What differs is only what this file marshals. `burrow_ops::redact::page` adds the half every
/// operation shares, through [`crate::OutputReader`], which the web engine already implements.
impl crate::PageRedactor for WebQpdf {
    fn name(&self) -> &'static str {
        "qpdf-wasm"
    }

    fn redact_page(
        &self,
        bytes: &[u8],
        page: usize,
        redacted: &std::collections::BTreeSet<usize>,
        region: crate::pdfsyntax::region::Region,
        options: &crate::OpenOptions<'_>,
    ) -> Result<(Vec<u8>, crate::redact::Report)> {
        crate::redact::redact_page(&WebRedactor(self), bytes, page, redacted, region, options)
    }

    fn input_rotations(
        &self,
        bytes: &[u8],
        options: &crate::OpenOptions<'_>,
        deadline: &Deadline,
    ) -> Result<Vec<i64>> {
        // THE WEB'S OWN ROTATION SWEEP, which the conformance harness already holds to the
        // native one. The native implementation reads the same vector through its rotator's
        // sweep; neither has a second copy of it.
        let source = crate::PageRotator::open(self, bytes.to_vec().into_boxed_slice(), options)?;
        crate::PageRotator::rotations(self, &source, options, deadline)
    }
}

/// Crate-internal access for the native-backed tests in `qpdf::web_differential_tests`, which
/// drive this module over real qpdf. Nothing here is compiled outside a native test build.
#[cfg(all(
    test,
    feature = "native-engines",
    burrow_native_engines,
    target_os = "linux"
))]
pub(crate) mod testing {
    /// The web engine as redaction opens documents with it.
    pub(crate) const fn redactor(engine: &super::WebQpdf) -> super::WebRedactor<'_> {
        super::WebRedactor(engine)
    }
}

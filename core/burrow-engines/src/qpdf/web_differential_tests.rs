//! The web engine, driven over real qpdf, held to the native engine case for case (#191).
//!
//! # Why this exists, and what it can and cannot see
//!
//! Redaction's policy is written once, over `redact::graph`, so what differs between the
//! platforms is only what each half of the seam marshals: `qpdf::redact_graph` natively and
//! `web::redact` over the JS bridge. ADR 0029 makes a divergence between the two a redaction bug,
//! and this is the check that would see one.
//!
//! [`NativeBridge`] implements `web::QpdfBridge` over the native C API, one call for one call,
//! reproducing what `apps/web/src/worker/bridge-qpdf.js` does with each answer. `WebQpdf` over it
//! is then the web's Rust -- its session, its handles, its key cache, its drains, its write -- over
//! the same qpdf the native path links. **What that cannot see** is the JavaScript glue and
//! `qpdf.wasm` itself: a marshalling defect in `bridge-qpdf.js`, or a difference in how the wasm
//! build of qpdf behaves. The browser-level comparison needs a redaction entry point in a wasm
//! binding and a worker op, which #137 owns; it lands there.
//!
//! # What it compares
//!
//! Every case `tests/redaction/outcomes.tsv` records -- 463 at the time of writing, over 107
//! documents -- through `PageRedactor::redact_page` on both engines, and `input_rotations` on both.
//! Each pair must be equal: the sha256 of the emitted bytes and of the report, or the error in
//! full. And every web `Ok` must be the document the golden pinned, byte for byte.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout,
    reason = "test support over committed fixtures; the workspace lints are for library code"
)]

use core::ffi::{c_char, c_uint};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::{Arc, Mutex};

use burrow_types::{Clock, Limits, ManualClock};
use sha2::{Digest, Sha256};

use super::ffi;
use crate::pdfsyntax::region::Region;
use crate::web::{QpdfBridge, QpdfPtr};
use crate::{OpenOptions, PageRedactor};

/// What a bridge "pointer" names on this side.
enum Slot {
    /// Bytes the web side copied in: an input, a description, a password, a key.
    Bytes(Vec<u8>),
    /// A live `qpdf_data`.
    Data(usize),
    /// A `qpdf_error`, valid until the next error call on its document.
    Error(usize),
    /// A pointer into qpdf's own storage -- a string, or the written buffer -- valid until the
    /// next call that returns one. Copied out by the caller at once, as the JS side does.
    Borrowed(usize),
    /// The shared discarding logger `limits::install` created.
    Logger(usize),
}

/// `web::QpdfBridge` over the native qpdf C API.
///
/// The web side hands out 32-bit "pointers" because that is what a wasm heap offset is. Here each
/// one names a [`Slot`] in a table, and the table is behind a lock because the trait is
/// `Send + Sync`; a test uses it from one thread.
pub(super) struct NativeBridge {
    table: Mutex<(u32, BTreeMap<u32, Slot>)>,
    /// How many documents the web side has opened through this bridge. The differential reads it
    /// around every case, so an engine that did not go through the bridge cannot pass (code review
    /// of #191: a `WebQpdf::redact_page` delegating to the native engine passed the first version).
    opened: std::sync::atomic::AtomicU64,
}

impl NativeBridge {
    pub(super) fn new() -> Self {
        // THE PROCESS-GLOBAL SETUP THE NATIVE PATH DOES, done here once so both engines in one
        // test process run under the same limits and the same discarding logger.
        let _ = super::limits::install();
        Self {
            table: Mutex::new((0, BTreeMap::new())),
            opened: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Documents opened through this bridge so far.
    fn opened(&self) -> u64 {
        self.opened.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Copied-in buffers and live documents the table still holds. Zero once every session and
    /// every key cache the web side made has been dropped: their `Drop`s give them back.
    fn outstanding(&self) -> (usize, usize) {
        let table = self.table.lock().unwrap();
        let bytes = table
            .1
            .values()
            .filter(|s| matches!(s, Slot::Bytes(_)))
            .count();
        let data = table
            .1
            .values()
            .filter(|s| matches!(s, Slot::Data(_)))
            .count();
        (bytes, data)
    }

    fn put(&self, slot: Slot) -> QpdfPtr {
        let mut table = self.table.lock().unwrap();
        table.0 += 1;
        let id = table.0;
        table.1.insert(id, slot);
        QpdfPtr(id)
    }

    fn data(&self, ptr: QpdfPtr) -> ffi::QpdfData {
        match self.table.lock().unwrap().1.get(&ptr.0) {
            Some(Slot::Data(address)) => *address as ffi::QpdfData,
            _ => panic!("bridge pointer {} is not a qpdf_data", ptr.0),
        }
    }

    /// A pointer to bytes the web side copied in, or null for the null pointer.
    fn bytes_ptr(&self, ptr: QpdfPtr) -> *const c_char {
        if ptr.is_null() {
            return core::ptr::null();
        }
        match self.table.lock().unwrap().1.get(&ptr.0) {
            // The `Vec`'s heap buffer does not move when the map rebalances; only the `Vec`
            // header does. So the pointer is stable until the slot is freed.
            Some(Slot::Bytes(bytes)) => bytes.as_ptr().cast(),
            _ => panic!("bridge pointer {} is not copied-in bytes", ptr.0),
        }
    }

    fn borrowed(&self, raw: *const u8) -> QpdfPtr {
        if raw.is_null() {
            return QpdfPtr::NULL;
        }
        self.put(Slot::Borrowed(raw as usize))
    }

    /// A caller-owned `malloc`ed buffer out of qpdf, copied and freed -- `bridge-qpdf.js`'s
    /// `oh_page_content` and `oh_stream_data` shape, without their scratch words.
    fn take_buffer(mut buffer: *mut u8, length: usize) -> Vec<u8> {
        let data = if buffer.is_null() || length == 0 {
            Vec::new()
        } else {
            // SAFETY: qpdf reports `length` readable bytes at `buffer`, which it allocated.
            unsafe { core::slice::from_raw_parts(buffer, length) }.to_vec()
        };
        if !buffer.is_null() {
            // SAFETY: `buffer` was allocated by qpdf with `malloc` and is freed once.
            unsafe { ffi::qpdf_oh_free_buffer(&raw mut buffer) };
        }
        data
    }
}

fn qpdf_bool(value: bool) -> ffi::QpdfBool {
    if value {
        ffi::QPDF_TRUE
    } else {
        ffi::QPDF_FALSE
    }
}

// SAFETY, FOR EVERY `unsafe` BLOCK BELOW UNLESS IT SAYS OTHERWISE: each forwards one call to the
// qpdf C API with a `qpdf_data` the web side obtained from `init` and has not cleaned up (the
// table would have removed it, and `data` panics on a missing slot), object handles that document
// issued, and pointers to bytes held in the table for the duration of the call. These are the
// same preconditions `qpdf::handle` and `qpdf::mod` establish for the same calls.
impl QpdfBridge for NativeBridge {
    fn copy_in(&self, bytes: &[u8]) -> QpdfPtr {
        self.put(Slot::Bytes(bytes.to_vec()))
    }

    fn free(&self, ptr: QpdfPtr) {
        self.table.lock().unwrap().1.remove(&ptr.0);
    }

    fn wipe_and_free(&self, ptr: QpdfPtr, _len: u32) {
        if let Some(Slot::Bytes(mut bytes)) = self.table.lock().unwrap().1.remove(&ptr.0) {
            bytes.fill(0);
        }
    }

    fn init(&self) -> QpdfPtr {
        // SAFETY: `qpdf_init` takes nothing and returns an owned handle or null.
        let data = unsafe { ffi::qpdf_init() };
        if data.is_null() {
            return QpdfPtr::NULL;
        }
        self.opened
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.put(Slot::Data(data as usize))
    }

    fn cleanup(&self, data: QpdfPtr) {
        let mut raw = self.data(data);
        // SAFETY: as the block comment above; `qpdf_cleanup` nulls the handle it is given.
        unsafe { ffi::qpdf_cleanup(&raw mut raw) };
        self.table.lock().unwrap().1.remove(&data.0);
    }

    fn silence_errors(&self, data: QpdfPtr) {
        // SAFETY: see the block comment above the impl.
        unsafe { ffi::qpdf_silence_errors(self.data(data)) };
    }

    fn set_suppress_warnings(&self, data: QpdfPtr, value: bool) {
        // SAFETY: see the block comment above the impl.
        unsafe { ffi::qpdf_set_suppress_warnings(self.data(data), qpdf_bool(value)) };
    }

    fn set_logger(&self, data: QpdfPtr, logger: QpdfPtr) {
        let handle = match self.table.lock().unwrap().1.get(&logger.0) {
            Some(Slot::Logger(address)) => *address as ffi::QpdfLoggerHandle,
            _ => return,
        };
        // SAFETY: see the block comment above the impl; the logger is `limits::install`'s,
        // which outlives every document.
        unsafe { ffi::qpdf_set_logger(self.data(data), handle) };
    }

    fn set_attempt_recovery(&self, data: QpdfPtr, value: bool) {
        // SAFETY: see the block comment above the impl.
        unsafe { ffi::qpdf_set_attempt_recovery(self.data(data), qpdf_bool(value)) };
    }

    fn read_memory(
        &self,
        data: QpdfPtr,
        description: QpdfPtr,
        buffer: QpdfPtr,
        size: u64,
        password: QpdfPtr,
    ) -> i32 {
        let (description, buffer, password) = (
            self.bytes_ptr(description),
            self.bytes_ptr(buffer),
            self.bytes_ptr(password),
        );
        // SAFETY: see the block comment above the impl. The input stays in the table until the
        // session frees it, after `cleanup`, which is the order `qpdf_read_memory` requires.
        unsafe { ffi::qpdf_read_memory(self.data(data), description, buffer, size, password) }
    }

    fn has_error(&self, data: QpdfPtr) -> bool {
        // SAFETY: see the block comment above the impl.
        unsafe { ffi::qpdf_has_error(self.data(data)) != ffi::QPDF_FALSE }
    }

    fn get_error(&self, data: QpdfPtr) -> QpdfPtr {
        // SAFETY: see the block comment above the impl.
        let error = unsafe { ffi::qpdf_get_error(self.data(data)) };
        if error.is_null() {
            return QpdfPtr::NULL;
        }
        self.put(Slot::Error(error as usize))
    }

    fn get_error_code(&self, data: QpdfPtr, error: QpdfPtr) -> i32 {
        let raw = match self.table.lock().unwrap().1.get(&error.0) {
            Some(Slot::Error(address)) => *address as ffi::QpdfError,
            _ => core::ptr::null_mut(),
        };
        // SAFETY: see the block comment above the impl; `raw` came from `get_error` on this
        // document, or is null, which qpdf accepts.
        unsafe { ffi::qpdf_get_error_code(self.data(data), raw) }
    }

    fn get_num_pages(&self, data: QpdfPtr) -> i32 {
        // SAFETY: see the block comment above the impl.
        unsafe { ffi::qpdf_get_num_pages(self.data(data)) }
    }

    fn global_set_uint32(&self, param: i32, value: u32) -> i32 {
        // SAFETY: process-global and takes no document; `limits::install` makes the same call.
        unsafe { ffi::qpdf_global_set_uint32(param, value) }
    }

    fn logger_create(&self) -> QpdfPtr {
        // THE SHARED LOGGER, not a new one: natively every document is given the one
        // `limits::install` made, which already discards everything.
        self.put(Slot::Logger(super::limits::install() as usize))
    }

    fn logger_discard_all(&self, _logger: QpdfPtr, _destination: i32) {
        // Already discarding: `limits::install` set every destination when it made the logger.
    }

    fn get_page_n(&self, data: QpdfPtr, n: u32) -> u32 {
        // SAFETY: see the block comment above the impl. `qpdf_get_page_n` is trapped and
        // bounds-checks `n` itself; the web side checks first as well.
        unsafe { ffi::qpdf_get_page_n(self.data(data), usize::try_from(n).unwrap()) }
    }

    fn add_page(&self, data: QpdfPtr, source: QpdfPtr, page: u32, first: bool) -> i32 {
        // SAFETY: see the block comment above the impl.
        unsafe { ffi::qpdf_add_page(self.data(data), self.data(source), page, qpdf_bool(first)) }
    }

    fn remove_page(&self, data: QpdfPtr, page: u32) -> i32 {
        // SAFETY: see the block comment above the impl.
        unsafe { ffi::qpdf_remove_page(self.data(data), page) }
    }

    fn add_page_at(
        &self,
        data: QpdfPtr,
        source: QpdfPtr,
        page: u32,
        before: bool,
        refpage: u32,
    ) -> i32 {
        // SAFETY: see the block comment above the impl.
        unsafe {
            ffi::qpdf_add_page_at(
                self.data(data),
                self.data(source),
                page,
                qpdf_bool(before),
                refpage,
            )
        }
    }

    fn init_write_memory(&self, data: QpdfPtr) -> i32 {
        // SAFETY: see the block comment above the impl.
        unsafe { ffi::qpdf_init_write_memory(self.data(data)) }
    }

    fn set_deterministic_id(&self, data: QpdfPtr, value: bool) {
        // SAFETY: see the block comment above the impl; the web side calls this only after a
        // successful `init_write_memory`, which is the precondition natively too.
        unsafe { ffi::qpdf_set_deterministic_ID(self.data(data), qpdf_bool(value)) };
    }

    fn set_object_stream_mode(&self, data: QpdfPtr, mode: u32) {
        // SAFETY: as `set_deterministic_id`.
        unsafe { ffi::qpdf_set_object_stream_mode(self.data(data), c_uint::from(mode)) };
    }

    fn write(&self, data: QpdfPtr) -> i32 {
        // SAFETY: see the block comment above the impl.
        unsafe { ffi::qpdf_write(self.data(data)) }
    }

    fn get_buffer_length(&self, data: QpdfPtr) -> u32 {
        // SAFETY: see the block comment above the impl; after a successful write.
        let length = unsafe { ffi::qpdf_get_buffer_length(self.data(data)) };
        u32::try_from(length).unwrap()
    }

    fn get_buffer(&self, data: QpdfPtr) -> QpdfPtr {
        // SAFETY: see the block comment above the impl; after a successful write.
        let raw = unsafe { ffi::qpdf_get_buffer(self.data(data)) };
        self.borrowed(raw)
    }

    fn copy_out(&self, ptr: QpdfPtr, len: u32) -> Vec<u8> {
        match self.table.lock().unwrap().1.get(&ptr.0) {
            // SAFETY: a `Borrowed` slot is a pointer qpdf returned for `len` readable bytes, and
            // the caller copies before its next call.
            Some(Slot::Borrowed(address)) => unsafe {
                core::slice::from_raw_parts(*address as *const u8, len.try_into().unwrap())
            }
            .to_vec(),
            Some(Slot::Bytes(bytes)) => bytes[..usize::try_from(len).unwrap()].to_vec(),
            _ => panic!("bridge pointer {} cannot be copied out", ptr.0),
        }
    }

    fn oh_get_key(&self, data: QpdfPtr, oh: u32, key: QpdfPtr) -> u32 {
        let key = self.bytes_ptr(key);
        // SAFETY: see the block comment above the impl.
        unsafe { ffi::qpdf_oh_get_key(self.data(data), oh, key) }
    }

    fn oh_get_type_code(&self, data: QpdfPtr, oh: u32) -> i32 {
        // SAFETY: see the block comment above the impl.
        unsafe { ffi::qpdf_oh_get_type_code(self.data(data), oh) }
    }

    fn oh_get_int_value(&self, data: QpdfPtr, oh: u32) -> i64 {
        // SAFETY: see the block comment above the impl.
        unsafe { ffi::qpdf_oh_get_int_value(self.data(data), oh) }
    }

    fn oh_new_integer(&self, data: QpdfPtr, value: i64) -> u32 {
        // SAFETY: see the block comment above the impl.
        unsafe { ffi::qpdf_oh_new_integer(self.data(data), value) }
    }

    fn oh_new_null(&self, data: QpdfPtr) -> u32 {
        // SAFETY: see the block comment above the impl.
        unsafe { ffi::qpdf_oh_new_null(self.data(data)) }
    }

    fn oh_replace_stream_data(
        &self,
        data: QpdfPtr,
        stream: u32,
        bytes: &[u8],
        filter: u32,
        decode_parms: u32,
    ) -> bool {
        // SAFETY: see the block comment above the impl. qpdf copies `bytes` before returning.
        unsafe {
            ffi::qpdf_oh_replace_stream_data(
                self.data(data),
                stream,
                bytes.as_ptr(),
                bytes.len(),
                filter,
                decode_parms,
            );
        }
        // TRUE MEANS THE BYTES REACHED THE ENGINE, as on the JS side; qpdf's verdict is latched.
        true
    }

    fn oh_replace_key(&self, data: QpdfPtr, oh: u32, key: QpdfPtr, item: u32) {
        let key = self.bytes_ptr(key);
        // SAFETY: see the block comment above the impl.
        unsafe { ffi::qpdf_oh_replace_key(self.data(data), oh, key, item) };
    }

    fn oh_unparse_resolved(&self, data: QpdfPtr, oh: u32) -> QpdfPtr {
        // SAFETY: see the block comment above the impl.
        let text = unsafe { ffi::qpdf_oh_unparse_resolved(self.data(data), oh) };
        self.borrowed(text.cast())
    }

    fn oh_get_name(&self, data: QpdfPtr, oh: u32) -> QpdfPtr {
        // SAFETY: see the block comment above the impl.
        let text = unsafe { ffi::qpdf_oh_get_name(self.data(data), oh) };
        self.borrowed(text.cast())
    }

    fn oh_remove_key(&self, data: QpdfPtr, oh: u32, key: QpdfPtr) {
        let key = self.bytes_ptr(key);
        // SAFETY: see the block comment above the impl.
        unsafe { ffi::qpdf_oh_remove_key(self.data(data), oh, key) };
    }

    fn oh_get_array_n_items(&self, data: QpdfPtr, oh: u32) -> i32 {
        // SAFETY: see the block comment above the impl.
        unsafe { ffi::qpdf_oh_get_array_n_items(self.data(data), oh) }
    }

    fn oh_get_array_item(&self, data: QpdfPtr, oh: u32, at: i32) -> u32 {
        // SAFETY: see the block comment above the impl.
        unsafe { ffi::qpdf_oh_get_array_item(self.data(data), oh, at) }
    }

    fn oh_erase_item(&self, data: QpdfPtr, oh: u32, at: i32) {
        // SAFETY: see the block comment above the impl.
        unsafe { ffi::qpdf_oh_erase_item(self.data(data), oh, at) };
    }

    fn oh_set_array_item(&self, data: QpdfPtr, oh: u32, at: i32, item: u32) {
        // SAFETY: see the block comment above the impl.
        unsafe { ffi::qpdf_oh_set_array_item(self.data(data), oh, at, item) };
    }

    fn oh_get_dict(&self, data: QpdfPtr, oh: u32) -> u32 {
        // SAFETY: see the block comment above the impl.
        unsafe { ffi::qpdf_oh_get_dict(self.data(data), oh) }
    }

    fn oh_page_content(&self, data: QpdfPtr, page: u32) -> Option<Vec<u8>> {
        let mut buffer: *mut u8 = core::ptr::null_mut();
        let mut length: usize = 0;
        // SAFETY: see the block comment above the impl; both out-parameters are live locals,
        // zeroed first as `bridge-qpdf.js` zeroes its scratch words.
        let _status = unsafe {
            ffi::qpdf_oh_get_page_content_data(
                self.data(data),
                page,
                &raw mut buffer,
                &raw mut length,
            )
        };
        // THE STATUS IS NOT READ, as on the JS side: a throw is latched for the drain.
        Some(Self::take_buffer(buffer, length))
    }

    fn oh_stream_data(&self, data: QpdfPtr, oh: u32) -> Option<Vec<u8>> {
        let mut filtered: ffi::QpdfBool = ffi::QPDF_FALSE;
        let mut buffer: *mut u8 = core::ptr::null_mut();
        let mut length: usize = 0;
        // SAFETY: as `oh_page_content`; all three out-parameters are live, zeroed locals.
        let _status = unsafe {
            ffi::qpdf_oh_get_stream_data(
                self.data(data),
                oh,
                crate::codes::qpdf::decode_level::SPECIALIZED,
                &raw mut filtered,
                &raw mut buffer,
                &raw mut length,
            )
        };
        let bytes = Self::take_buffer(buffer, length);
        // `None` WHEN qpdf DID NOT DECODE IT, error or not, as on the JS side.
        (filtered != ffi::QPDF_FALSE).then_some(bytes)
    }

    fn copy_c_string(&self, ptr: QpdfPtr) -> Vec<u8> {
        match self.table.lock().unwrap().1.get(&ptr.0) {
            // SAFETY: a `Borrowed` string slot is a NUL-terminated string qpdf returned, and the
            // caller copies before its next call on the document.
            Some(Slot::Borrowed(address)) => {
                unsafe { core::ffi::CStr::from_ptr(*address as *const c_char) }
                    .to_bytes()
                    .to_vec()
            }
            _ => Vec::new(),
        }
    }

    fn oh_object(&self, data: QpdfPtr, oh: u32) -> u64 {
        let data = self.data(data);
        // SAFETY: see the block comment above the impl.
        let (id, generation) = unsafe {
            (
                ffi::qpdf_oh_get_object_id(data, oh),
                ffi::qpdf_oh_get_generation(data, oh),
            )
        };
        // PACKED AS `bridge-qpdf.js` PACKS IT: each half as the unsigned 32 bits it is.
        (u64::from(u32::from_ne_bytes(id.to_ne_bytes())) << 32)
            | u64::from(u32::from_ne_bytes(generation.to_ne_bytes()))
    }

    fn oh_release(&self, data: QpdfPtr, oh: u32) {
        // SAFETY: see the block comment above the impl.
        unsafe { ffi::qpdf_oh_release(self.data(data), oh) };
    }

    fn heap_bytes(&self) -> u64 {
        // No engine heap to measure: the native qpdf allocates from the process. Zero before and
        // after makes the measured-memory check a no-op on this side, which is stated rather than
        // hidden -- no case in the corpus is refused by it natively either.
        0
    }
}

// ------------------------------------------------------------------------------------------------
// The differential.
// ------------------------------------------------------------------------------------------------

const PDFBUILD_REGION: Region = Region {
    left: 30.0,
    top: 68.0,
    width: 340.0,
    height: 44.0,
};
const WHOLE_PAGE: Region = Region {
    left: 0.0,
    top: 0.0,
    width: 5000.0,
    height: 5000.0,
};
const BAND: Region = Region {
    left: 0.0,
    top: 300.0,
    width: 5000.0,
    height: 120.0,
};

fn repository() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// The manifest's `region` per `file`, as the golden reads it.
fn manifest_regions() -> BTreeMap<String, Region> {
    let text = std::fs::read_to_string(repository().join("tests/redaction/manifest.toml")).unwrap();
    let value_of = |line: &str, key: &str| -> Option<String> {
        let (name, value) = line.split_once('=')?;
        (name.trim() == key).then(|| value.trim().to_owned())
    };
    let mut out = BTreeMap::new();
    let mut region = None;
    for line in text.lines().map(str::trim) {
        if line == "[[fixture]]" {
            region = None;
        } else if let Some(value) = value_of(line, "region") {
            let numbers: Vec<f64> = value
                .trim_start_matches('[')
                .trim_end_matches(']')
                .split(',')
                .map(|n| n.trim().parse().unwrap())
                .collect();
            region = Some(Region {
                left: numbers[0],
                top: numbers[1],
                width: numbers[2],
                height: numbers[3],
            });
        } else if let Some(file) = value_of(line, "file")
            && let Some(found) = region.take()
        {
            out.insert(format!("tests/redaction/{}", file.trim_matches('"')), found);
        }
    }
    out
}

/// One outcome, as a line both sides can be compared by.
fn outcome(result: &burrow_types::Result<(Vec<u8>, crate::redact::Report)>) -> String {
    match result {
        Ok((bytes, report)) => format!(
            "OK {} report {}",
            hex(bytes),
            hex(format!("{report:?}").as_bytes())
        ),
        Err(error) => format!("ERR {}", format!("{error:?}").replace(['\n', '\t'], " ")),
    }
}

/// Every way two outcome lists differ, by case.
fn divergences(native: &[(String, String)], web: &[(String, String)]) -> Vec<String> {
    let web: BTreeMap<&String, &String> = web.iter().map(|(k, v)| (k, v)).collect();
    let mut out = Vec::new();
    for (case, left) in native {
        match web.get(case) {
            Some(right) if *right == left => {}
            Some(right) => out.push(format!("{case}\n    native {left}\n    web    {right}")),
            None => out.push(format!("{case}: no web outcome")),
        }
    }
    if web.len() != native.len() {
        out.push(format!(
            "{} native outcomes and {} web",
            native.len(),
            web.len()
        ));
    }
    out
}

#[test]
fn the_web_engine_produces_the_native_outcome_on_every_recorded_case() {
    let golden =
        std::fs::read_to_string(repository().join("tests/redaction/outcomes.tsv")).unwrap();
    let regions = manifest_regions();
    let options = OpenOptions::new(
        Limits::default(),
        Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
    );
    let native = super::Qpdf;
    let bridge = Arc::new(NativeBridge::new());
    let web = crate::web::WebQpdf::new(Arc::clone(&bridge) as Arc<dyn QpdfBridge>);

    let mut inputs: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut native_out = Vec::new();
    let mut web_out = Vec::new();
    let mut pinned_mismatches = Vec::new();
    let mut unbridged = Vec::new();
    let mut pre_engine = 0usize;
    let (mut cases, mut agreed_ok) = (0usize, 0usize);
    for line in golden.lines().filter(|l| l.starts_with("CASE\t")) {
        let fields: Vec<&str> = line.split('\t').collect();
        let [_, name, page, covered, label, recorded] = fields[..] else {
            panic!("a golden line that is not six fields: {line}");
        };
        let bytes = inputs
            .entry(name.to_owned())
            .or_insert_with(|| std::fs::read(repository().join(name)).unwrap());
        let page: usize = page.parse().unwrap();
        let covered: std::collections::BTreeSet<usize> =
            covered.split(',').map(|p| p.parse().unwrap()).collect();
        let region = match label {
            "manifest" => regions[name],
            "pdfbuild" => PDFBUILD_REGION,
            "whole" => WHOLE_PAGE,
            "band" => BAND,
            other => panic!("a golden case with an unknown region label `{other}`"),
        };
        let key = format!("{name} page {page} covering {covered:?} over {label}");
        // EACH ENGINE, AND ONLY IT, IS ASKED. The native engine must not touch the bridge, and
        // the web engine must open at least one document through it -- the input, and then a
        // fresh one for the read-back -- UNLESS a ceiling that applies before any engine refused
        // it. The bomb fixtures are refused by the structural pre-scan on both engines without an
        // engine being touched; this comment first said every case reached `init`, and the check
        // below found twelve that do not. Without the check, a web engine delegating to the native
        // one agreed on every case.
        let before = bridge.opened();
        let left = outcome(&native.redact_page(bytes, page, &covered, region, &options));
        let between = bridge.opened();
        let right = outcome(&web.redact_page(bytes, page, &covered, region, &options));
        let after = bridge.opened();
        let before_the_engine = ["stage: InputSize", "stage: SizeEstimate", "stage: Prescan"]
            .iter()
            .any(|stage| right.contains(stage));
        pre_engine += usize::from(before_the_engine);
        if between != before || (after == between && !before_the_engine) {
            unbridged.push(format!(
                "{key}: native opened {} through the bridge, web {}",
                between - before,
                after - between
            ));
        }
        // AND THE PINNED OUTCOME, IN FULL: the web must be what the golden recorded, not merely
        // what native produced today. Engine-level outcomes equal the golden's operation-level ones
        // on every case, errors included (measured when this was written), so no case is exempt.
        if right != recorded {
            pinned_mismatches.push(format!("{key}\n    pinned {recorded}\n    web    {right}"));
        }
        agreed_ok += usize::from(right.starts_with("OK "));
        native_out.push((key.clone(), left));
        web_out.push((key, right));
        cases += 1;
    }
    // THE ROTATION VECTOR, which `burrow_ops::redact::page` reads from each engine as the promise.
    let deadline = burrow_types::Deadline::start(options.clock.as_ref(), &options.limits);
    for (name, bytes) in &inputs {
        let show = |r: burrow_types::Result<Vec<i64>>| match r {
            Ok(v) => format!("OK {v:?}"),
            Err(e) => format!("ERR {e:?}"),
        };
        native_out.push((
            format!("{name} rotations"),
            show(native.input_rotations(bytes, &options, &deadline)),
        ));
        web_out.push((
            format!("{name} rotations"),
            show(web.input_rotations(bytes, &options, &deadline)),
        ));
    }

    println!(
        "web differential: {cases} cases and {} rotation vectors over {} documents; \
         {agreed_ok} redactions; every outcome compared with the pinned one; {pre_engine} refused \
         before any engine was asked",
        inputs.len(),
        inputs.len()
    );
    let found = divergences(&native_out, &web_out);
    assert!(
        found.is_empty(),
        "{} of {} outcomes diverge between the native and the web engine:\n{}",
        found.len(),
        native_out.len(),
        found.join("\n")
    );
    assert!(
        unbridged.is_empty(),
        "{} cases were not asked of the engine they claim to compare:\n{}",
        unbridged.len(),
        unbridged.join("\n")
    );
    assert!(
        pinned_mismatches.is_empty(),
        "{} web outcomes are not the pinned ones:\n{}",
        pinned_mismatches.len(),
        pinned_mismatches.join("\n")
    );
    // THE COUNT THE GOLDEN SAYS, so a narrower run cannot read as the whole one.
    let recorded_cases = golden.lines().filter(|l| l.starts_with("CASE\t")).count();
    let recorded_inputs = golden.lines().filter(|l| l.starts_with("INPUT\t")).count();
    let recorded_ok = golden
        .lines()
        .filter(|l| l.starts_with("CASE\t") && l.contains("\tOK "))
        .count();
    assert_eq!(cases, recorded_cases);
    assert_eq!(inputs.len(), recorded_inputs);
    assert_eq!(
        agreed_ok, recorded_ok,
        "the web redacted a different number of cases than pinned"
    );

    // NOTHING LEFT IN THE ENGINE HEAP: every session was cleaned up and every copied-in buffer --
    // inputs, passwords, keys -- freed. `WebRedactionDocument`'s and `Session`'s `Drop`s are what
    // this holds to account; measured at zero of each when it was added (code review of #191).
    drop(web);
    assert_eq!(
        bridge.outstanding(),
        (0, 0),
        "(copied-in buffers, live documents) left behind"
    );
}

/// The comparison sees a divergence of each kind, so a clean run is a measurement.
#[test]
fn the_differential_names_every_kind_of_divergence() {
    let native = vec![
        ("a".to_owned(), "OK 11 report 22".to_owned()),
        ("b".to_owned(), "ERR Malformed(\"x [r]: y\")".to_owned()),
    ];
    assert!(divergences(&native, &native).is_empty());
    let mut changed = native.clone();
    changed[0].1 = "OK 12 report 22".to_owned();
    assert_eq!(divergences(&native, &changed).len(), 1);
    let mut reworded = native.clone();
    reworded[1].1 = "ERR Malformed(\"x [r]: z\")".to_owned();
    assert_eq!(divergences(&native, &reworded).len(), 1);
    assert_eq!(
        divergences(&native, &native[..1]).len(),
        2,
        "a missing case and the count"
    );
}

/// And it can see a real one: the same case over a different region is a different outcome.
///
/// A check on the instrument, not on the engine: two regions giving two outputs shows the
/// comparison is not comparing constants. **It does not show which engine ran** -- native passes it
/// too, which the code review of #191 measured. That is the bridge's `opened` count, read around
/// every case in the test above.
#[test]
fn the_web_engine_is_really_asked() {
    let bytes =
        std::fs::read(repository().join("tests/redaction/generated/01-plain-tj.pdf")).unwrap();
    let options = OpenOptions::new(
        Limits::default(),
        Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
    );
    let web = crate::web::WebQpdf::new(Arc::new(NativeBridge::new()));
    let covered = std::collections::BTreeSet::from([0]);
    let cleared = outcome(&web.redact_page(&bytes, 0, &covered, PDFBUILD_REGION, &options));
    let elsewhere = outcome(&web.redact_page(&bytes, 0, &covered, BAND, &options));
    assert!(
        cleared.starts_with("OK "),
        "the fixture must redact on the web: {cleared}"
    );
    assert_ne!(
        cleared, elsewhere,
        "two regions gave the same output, so nothing was measured"
    );
}

/// A [`NativeBridge`] with two knobs the native engine has no counterpart for.
///
/// - `refuse`: the engine heap refuses every `copy_in` once armed -- what a browser tab near its
///   memory ceiling does. Armed after a document is open, what fails is a key the redaction copies
///   in, which is the one marshalling step natively has no counterpart.
/// - `heap_step`: each `heap_bytes` read reports that much more than the last, so the web open's
///   measured-memory check can fire. [`NativeBridge`] reports zero, which makes it unobservable.
struct RefusingHeap {
    inner: NativeBridge,
    armed: std::sync::atomic::AtomicBool,
    heap: std::sync::atomic::AtomicU64,
    heap_step: u64,
}

impl RefusingHeap {
    fn new(heap_step: u64) -> Self {
        Self {
            inner: NativeBridge::new(),
            armed: std::sync::atomic::AtomicBool::new(false),
            heap: std::sync::atomic::AtomicU64::new(0),
            heap_step,
        }
    }
}

impl QpdfBridge for RefusingHeap {
    fn copy_in(&self, bytes: &[u8]) -> QpdfPtr {
        if self.armed.load(std::sync::atomic::Ordering::SeqCst) {
            return QpdfPtr::NULL;
        }
        self.inner.copy_in(bytes)
    }
    fn free(&self, ptr: QpdfPtr) {
        self.inner.free(ptr)
    }
    fn wipe_and_free(&self, ptr: QpdfPtr, len: u32) {
        self.inner.wipe_and_free(ptr, len)
    }
    fn init(&self) -> QpdfPtr {
        self.inner.init()
    }
    fn cleanup(&self, data: QpdfPtr) {
        self.inner.cleanup(data)
    }
    fn silence_errors(&self, data: QpdfPtr) {
        self.inner.silence_errors(data)
    }
    fn set_suppress_warnings(&self, data: QpdfPtr, value: bool) {
        self.inner.set_suppress_warnings(data, value)
    }
    fn set_logger(&self, data: QpdfPtr, logger: QpdfPtr) {
        self.inner.set_logger(data, logger)
    }
    fn set_attempt_recovery(&self, data: QpdfPtr, value: bool) {
        self.inner.set_attempt_recovery(data, value)
    }
    fn read_memory(
        &self,
        data: QpdfPtr,
        description: QpdfPtr,
        buffer: QpdfPtr,
        size: u64,
        password: QpdfPtr,
    ) -> i32 {
        self.inner
            .read_memory(data, description, buffer, size, password)
    }
    fn has_error(&self, data: QpdfPtr) -> bool {
        self.inner.has_error(data)
    }
    fn get_error(&self, data: QpdfPtr) -> QpdfPtr {
        self.inner.get_error(data)
    }
    fn get_error_code(&self, data: QpdfPtr, error: QpdfPtr) -> i32 {
        self.inner.get_error_code(data, error)
    }
    fn get_num_pages(&self, data: QpdfPtr) -> i32 {
        self.inner.get_num_pages(data)
    }
    fn global_set_uint32(&self, param: i32, value: u32) -> i32 {
        self.inner.global_set_uint32(param, value)
    }
    fn logger_create(&self) -> QpdfPtr {
        self.inner.logger_create()
    }
    fn logger_discard_all(&self, logger: QpdfPtr, destination: i32) {
        self.inner.logger_discard_all(logger, destination)
    }
    fn get_page_n(&self, data: QpdfPtr, n: u32) -> u32 {
        self.inner.get_page_n(data, n)
    }
    fn add_page(&self, data: QpdfPtr, source: QpdfPtr, page: u32, first: bool) -> i32 {
        self.inner.add_page(data, source, page, first)
    }
    fn remove_page(&self, data: QpdfPtr, page: u32) -> i32 {
        self.inner.remove_page(data, page)
    }
    fn add_page_at(
        &self,
        data: QpdfPtr,
        source: QpdfPtr,
        page: u32,
        before: bool,
        refpage: u32,
    ) -> i32 {
        self.inner.add_page_at(data, source, page, before, refpage)
    }
    fn init_write_memory(&self, data: QpdfPtr) -> i32 {
        self.inner.init_write_memory(data)
    }
    fn set_deterministic_id(&self, data: QpdfPtr, value: bool) {
        self.inner.set_deterministic_id(data, value)
    }
    fn set_object_stream_mode(&self, data: QpdfPtr, mode: u32) {
        self.inner.set_object_stream_mode(data, mode)
    }
    fn write(&self, data: QpdfPtr) -> i32 {
        self.inner.write(data)
    }
    fn get_buffer_length(&self, data: QpdfPtr) -> u32 {
        self.inner.get_buffer_length(data)
    }
    fn get_buffer(&self, data: QpdfPtr) -> QpdfPtr {
        self.inner.get_buffer(data)
    }
    fn copy_out(&self, ptr: QpdfPtr, len: u32) -> Vec<u8> {
        self.inner.copy_out(ptr, len)
    }
    fn oh_get_key(&self, data: QpdfPtr, oh: u32, key: QpdfPtr) -> u32 {
        self.inner.oh_get_key(data, oh, key)
    }
    fn oh_get_type_code(&self, data: QpdfPtr, oh: u32) -> i32 {
        self.inner.oh_get_type_code(data, oh)
    }
    fn oh_get_int_value(&self, data: QpdfPtr, oh: u32) -> i64 {
        self.inner.oh_get_int_value(data, oh)
    }
    fn oh_new_integer(&self, data: QpdfPtr, value: i64) -> u32 {
        self.inner.oh_new_integer(data, value)
    }
    fn oh_new_null(&self, data: QpdfPtr) -> u32 {
        self.inner.oh_new_null(data)
    }
    fn oh_replace_stream_data(
        &self,
        data: QpdfPtr,
        stream: u32,
        bytes: &[u8],
        filter: u32,
        decode_parms: u32,
    ) -> bool {
        self.inner
            .oh_replace_stream_data(data, stream, bytes, filter, decode_parms)
    }
    fn oh_replace_key(&self, data: QpdfPtr, oh: u32, key: QpdfPtr, item: u32) {
        self.inner.oh_replace_key(data, oh, key, item)
    }
    fn oh_unparse_resolved(&self, data: QpdfPtr, oh: u32) -> QpdfPtr {
        self.inner.oh_unparse_resolved(data, oh)
    }
    fn oh_get_name(&self, data: QpdfPtr, oh: u32) -> QpdfPtr {
        self.inner.oh_get_name(data, oh)
    }
    fn oh_remove_key(&self, data: QpdfPtr, oh: u32, key: QpdfPtr) {
        self.inner.oh_remove_key(data, oh, key)
    }
    fn oh_get_array_n_items(&self, data: QpdfPtr, oh: u32) -> i32 {
        self.inner.oh_get_array_n_items(data, oh)
    }
    fn oh_get_array_item(&self, data: QpdfPtr, oh: u32, at: i32) -> u32 {
        self.inner.oh_get_array_item(data, oh, at)
    }
    fn oh_erase_item(&self, data: QpdfPtr, oh: u32, at: i32) {
        self.inner.oh_erase_item(data, oh, at)
    }
    fn oh_set_array_item(&self, data: QpdfPtr, oh: u32, at: i32, item: u32) {
        self.inner.oh_set_array_item(data, oh, at, item)
    }
    fn oh_get_dict(&self, data: QpdfPtr, oh: u32) -> u32 {
        self.inner.oh_get_dict(data, oh)
    }
    fn oh_page_content(&self, data: QpdfPtr, page: u32) -> Option<Vec<u8>> {
        self.inner.oh_page_content(data, page)
    }
    fn oh_stream_data(&self, data: QpdfPtr, oh: u32) -> Option<Vec<u8>> {
        self.inner.oh_stream_data(data, oh)
    }
    fn copy_c_string(&self, ptr: QpdfPtr) -> Vec<u8> {
        self.inner.copy_c_string(ptr)
    }
    fn oh_object(&self, data: QpdfPtr, oh: u32) -> u64 {
        self.inner.oh_object(data, oh)
    }
    fn oh_release(&self, data: QpdfPtr, oh: u32) {
        self.inner.oh_release(data, oh)
    }
    fn heap_bytes(&self) -> u64 {
        self.heap
            .fetch_add(self.heap_step, std::sync::atomic::Ordering::SeqCst)
    }
}

/// A key the engine heap refuses is an error the policy sees, never a quiet null it acts on.
///
/// Natively a key cannot fail to reach qpdf, so no native test and no corpus case can ask this.
/// The web latches the failure and the next drain reports it (`web::redact`'s header). Without
/// the latch the key lookup answers a handle to nothing, the page reads as having no `/Contents`,
/// and the redaction removes nothing -- a region "cleared" by never being looked at.
#[test]
fn a_key_the_engine_heap_refuses_fails_the_redaction_by_name() {
    use crate::redact::graph::{OpensForRedaction, PdfDocument, PdfObject};

    let bytes =
        std::fs::read(repository().join("tests/redaction/generated/01-plain-tj.pdf")).unwrap();
    let options = OpenOptions::new(
        Limits::default(),
        Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
    );
    let bridge = Arc::new(RefusingHeap::new(0));
    let web = crate::web::WebQpdf::new(Arc::clone(&bridge) as Arc<dyn QpdfBridge>);
    let redactor = crate::web::redact_testing::redactor(&web);
    let (document, _) = redactor
        .open_for_redaction(&bytes, &options)
        .expect("opens before arming");
    let page = document.page(0).expect("page 0");

    // THE NEAR-MISS FIRST: unarmed, a key reaches the engine and the drain is clean.
    let _ = page.key(&crate::name::Name::literal(b"/Contents\0"));
    page.drained().expect("an unarmed key latches nothing");

    bridge
        .armed
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let absent = page.key(&crate::name::Name::literal(b"/MediaBox\0"));
    // A HANDLE TO NOTHING, measured: qpdf answers `ot_uninitialized` (0) for an id it never
    // issued -- not a null (2), which is what this assertion first expected. Either way it is no
    // dictionary and no array, so the policy's type checks read it as absent until the drain.
    assert_eq!(
        absent.type_code(),
        0,
        "a refused key answers a handle to nothing"
    );
    match page.drained() {
        Err(burrow_types::Error::Internal(message)) => {
            assert!(
                message.contains("could not allocate a dictionary key"),
                "{message}"
            );
        }
        other => panic!("the next drain must report the refused key: {other:?}"),
    }
    // AND THEN qpdf's OWN, measured: asking the handle to nothing for its type made qpdf latch
    // an error of its own, which the next drain reports. The policy stops at the first `Err`, so
    // it is the marshalling failure that names the refusal; after both, nothing is left.
    assert!(
        page.drained().is_err(),
        "qpdf latched its own error for the unknown handle"
    );
    page.drained().expect("both consumed");
}

/// The web handle's two writes that take a second handle refuse one from another document.
///
/// The native twin is `qpdf::write_path_tests::an_array_item_from_another_document_is_refused_...`.
/// Over the bridge the pairing is a `qpdf_data` beside an `oh`, and qpdf numbers handles from 1 in
/// every document, so a foreign handle collides with a live one here rather than failing.
#[test]
fn a_handle_from_another_web_document_is_refused_by_both_writes() {
    use crate::redact::graph::{OpensForRedaction, PdfDocument, PdfObject};

    let bytes =
        std::fs::read(repository().join("tests/redaction/generated/01-plain-tj.pdf")).unwrap();
    let options = OpenOptions::new(
        Limits::default(),
        Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
    );
    let web = crate::web::WebQpdf::new(Arc::new(NativeBridge::new()));
    let redactor = crate::web::redact_testing::redactor(&web);
    let (a, _) = redactor
        .open_for_redaction(&bytes, &options)
        .expect("opens");
    let (b, _) = redactor
        .open_for_redaction(&bytes, &options)
        .expect("opens");
    let (page_a, page_b) = (a.page(0).expect("page"), b.page(0).expect("page"));
    let media_box = page_a.key(&crate::name::Name::literal(b"/MediaBox\0"));
    let contents = page_a.key(&crate::name::Name::literal(b"/Contents\0"));
    let before = media_box.unparse();

    let foreign = page_b.integer_beside(7);
    assert!(
        matches!(
            media_box.set_array_item(0, &foreign),
            Err(burrow_types::Error::Internal(_))
        ),
        "an array item from another document was accepted"
    );
    let foreign_null = page_b.null_beside();
    assert!(
        matches!(
            contents.replace_stream_data(b"0 0 1 rg", &foreign_null, &foreign_null),
            Err(burrow_types::Error::Internal(_))
        ),
        "a stream filter from another document was accepted"
    );
    assert_eq!(media_box.unparse(), before, "and nothing was written");

    // THE NEAR-MISS: the same document's values are written.
    media_box
        .set_array_item(0, &page_a.integer_beside(7))
        .expect("same document");
    media_box.drained().expect("nothing latched");
    assert_ne!(media_box.unparse(), before, "the write landed");
}

fn web_options(limits: Limits) -> OpenOptions<'static> {
    OpenOptions::new(limits, Arc::new(ManualClock::new(0)) as Arc<dyn Clock>)
}

/// The web open applies the page ceiling, as the native open does.
///
/// Deletable with every other test green until the security review of #191 asked: the corpus has
/// no document over the default ceiling, and no test lowered it.
#[test]
fn the_web_open_applies_the_page_ceiling() {
    use crate::redact::graph::OpensForRedaction;

    let bytes = crate::minimal_pdf::pdf_with_pages(3);
    let web = crate::web::WebQpdf::new(Arc::new(NativeBridge::new()));
    let redactor = crate::web::redact_testing::redactor(&web);
    let mut tight = Limits::default();
    tight.max_pages = 2;
    match redactor.open_for_redaction(&bytes, &web_options(tight)) {
        Err(burrow_types::Error::LimitExceeded { limit, stage, .. }) => {
            assert_eq!(
                (limit, stage),
                ("max_pages", burrow_types::Stage::PageCount)
            );
        }
        other => panic!(
            "three pages under a ceiling of two must refuse: {:?}",
            other.map(|_| ())
        ),
    }
    // AND THE SAME REFUSAL AS NATIVE, which is the point of mirroring the open.
    let native = super::open_document(bytes.clone().into_boxed_slice(), &web_options(tight))
        .map(|_| ())
        .unwrap_err();
    let web_error = redactor
        .open_for_redaction(&bytes, &web_options(tight))
        .map(|_| ())
        .unwrap_err();
    assert_eq!(format!("{web_error:?}"), format!("{native:?}"));

    // THE NEAR-MISS: at the ceiling, not past it.
    tight.max_pages = 3;
    assert!(
        redactor
            .open_for_redaction(&bytes, &web_options(tight))
            .is_ok()
    );
}

/// The web open measures the engine heap across the open, and refuses past the ceiling.
///
/// Unobservable over [`NativeBridge`], whose heap reads as zero; this bridge's grows on demand.
/// Detected, not bounded -- `core/CLAUDE.md` -- which is exactly what is asserted: the open happened
/// and the reading afterwards refused it.
#[test]
fn the_web_open_measures_the_engine_heap() {
    use crate::redact::graph::OpensForRedaction;

    let bytes =
        std::fs::read(repository().join("tests/redaction/generated/01-plain-tj.pdf")).unwrap();
    let limits = Limits::default();
    // TWICE THE CEILING, not one byte over it: the check tolerates a noise margin above the ceiling
    // (`estimate::MEASURED_NOISE_MARGIN_BYTES`), and a step of ceiling + 1 is inside it -- which is
    // what this test first planted, and what the check correctly let through.
    let over = RefusingHeap::new(limits.max_memory_bytes.saturating_mul(2));
    let web = crate::web::WebQpdf::new(Arc::new(over));
    match crate::web::redact_testing::redactor(&web)
        .open_for_redaction(&bytes, &web_options(limits))
    {
        Err(burrow_types::Error::LimitExceeded { limit, stage, .. }) => {
            assert_eq!(
                (limit, stage),
                ("max_memory_bytes", burrow_types::Stage::Measured)
            );
        }
        other => panic!(
            "a heap grown past the ceiling must refuse: {:?}",
            other.map(|_| ())
        ),
    }
    // THE NEAR-MISS: a heap that grows by nothing.
    let web = crate::web::WebQpdf::new(Arc::new(RefusingHeap::new(0)));
    assert!(
        crate::web::redact_testing::redactor(&web)
            .open_for_redaction(&bytes, &web_options(limits))
            .is_ok()
    );
}

/// The four web accessors that drain do drain: `page`, `page_content`, `stream_data`, `object`.
///
/// The native twins are `qpdf::redact_graph::tests`. The security review of #191 deleted each of
/// these four drains in `web::redact` and the whole suite stayed green, because no corpus document
/// latches an error at those points. The latch is a real one, made the way the native tests make
/// it: a key of a free-standing null, which qpdf raises for because nothing owns it.
#[test]
fn the_web_accessors_that_drain_report_what_the_document_latched() {
    use crate::name::Name;
    use crate::redact::graph::{OpensForRedaction, PdfDocument, PdfObject};

    let bytes =
        std::fs::read(repository().join("tests/redaction/generated/01-plain-tj.pdf")).unwrap();
    let web = crate::web::WebQpdf::new(Arc::new(NativeBridge::new()));
    let (document, _) = crate::web::redact_testing::redactor(&web)
        .open_for_redaction(&bytes, &web_options(Limits::default()))
        .expect("opens");
    let page = document.page(0).expect("page 0");
    let contents = page.key(&Name::literal(b"/Contents\0"));
    assert_eq!(
        contents.type_code(),
        crate::codes::qpdf::object_type::STREAM,
        "the fixture's /Contents must be one stream for `stream_data` to be asked of it"
    );
    let latch = || {
        let _ = page.null_beside().key(&Name::literal(b"/Anything\0"));
    };

    latch();
    assert!(
        document.page(0).is_err(),
        "`page` returned over a latched error"
    );
    latch();
    assert!(
        page.page_content().is_err(),
        "`page_content` returned over a latched error"
    );
    latch();
    assert!(
        contents.stream_data().is_err(),
        "`stream_data` returned over a latched error"
    );
    latch();
    assert!(
        page.object().is_err(),
        "`object` returned over a latched error"
    );

    // THE NEAR-MISS: with nothing latched, all four answer.
    assert!(document.page(0).is_ok());
    assert!(page.page_content().is_ok());
    assert!(matches!(contents.stream_data(), Ok(Some(_))));
    assert!(page.object().is_ok());
}

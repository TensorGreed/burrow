//! qpdf: read a page's content, write it back, and emit the document.
//!
//! This is the half of finding 4 that decides whether a content-stream redaction is buildable
//! at all through the entry points ADR 0013 permits.

use std::ffi::{CStr, CString};
use std::ptr;

use crate::ffi::*;

pub struct Doc {
    data: qpdf_data,
    /// Kept alive: `qpdf_read_memory` does not copy its buffer.
    _bytes: Vec<u8>,
}

/// What happened while rewriting a page's content. Every field is a measurement the spike
/// reports; nothing here is inferred.
#[derive(Default, Debug, Clone)]
pub struct WriteReport {
    pub content_streams: usize,
    pub original_len: usize,
    pub rewritten_len: usize,
    pub bytes_removed: usize,
    /// Streams beyond the first that had to be emptied because `get_page_content_data`
    /// concatenates and the identity of the individual streams is gone by then.
    pub streams_emptied: usize,
    pub used_untrapped_constructor: bool,
    pub notes: Vec<String>,
}

impl Doc {
    pub fn open(bytes: Vec<u8>) -> Result<Doc, String> {
        // SAFETY: qpdf_init allocates and returns an opaque handle or null.
        let data = unsafe { qpdf_init() };
        if data.is_null() {
            return Err("qpdf_init returned null".into());
        }
        // SAFETY: `data` is a live handle from qpdf_init.
        unsafe {
            qpdf_silence_errors(data);
            qpdf_set_suppress_warnings(data, 1);
            qpdf_set_attempt_recovery(data, 1);
        }
        let description = CString::new("spike").map_err(|e| e.to_string())?;
        // SAFETY: the buffer outlives the handle -- it is moved into the returned Doc, and
        // Drop runs qpdf_cleanup before the Vec is dropped.
        let code = unsafe {
            qpdf_read_memory(
                data,
                description.as_ptr(),
                bytes.as_ptr().cast(),
                bytes.len() as _,
                ptr::null(),
            )
        };
        if code >= 2 {
            let mut handle = data;
            // SAFETY: cleaning up the handle we just created.
            unsafe { qpdf_cleanup(&mut handle) };
            return Err(format!("qpdf_read_memory returned {code}"));
        }
        Ok(Doc {
            data,
            _bytes: bytes,
        })
    }

    pub fn pages(&self) -> i32 {
        // SAFETY: live handle.
        unsafe { qpdf_get_num_pages(self.data) }
    }

    pub fn page(&self, index: usize) -> qpdf_oh {
        // SAFETY: live handle; qpdf_get_page_n is trapped and returns an uninitialised handle
        // rather than throwing when the index is out of range.
        unsafe { qpdf_get_page_n(self.data, index) }
    }

    /// The page's content streams, DECODED and CONCATENATED. There is no per-stream variant
    /// in the C API, which is the fact channel 22 exists to make visible.
    pub fn page_content(&self, page: qpdf_oh) -> Result<Vec<u8>, String> {
        let mut buf: *mut u8 = ptr::null_mut();
        let mut len: usize = 0;
        // SAFETY: both out-parameters are initialised before the call -- qpdf does not write
        // them on its error path, and reading an uninitialised pointer here is how
        // `bridge-qpdf.js` reproduced a free() of 0xc0ffee.
        let code = unsafe { qpdf_oh_get_page_content_data(self.data, page, &mut buf, &mut len) };
        if code >= 2 || buf.is_null() {
            return Err(format!("qpdf_oh_get_page_content_data returned {code}"));
        }
        // SAFETY: qpdf malloc'd `len` bytes at `buf` and hands ownership to us.
        let out = unsafe { std::slice::from_raw_parts(buf, len) }.to_vec();
        // SAFETY: freeing the buffer qpdf allocated, exactly once.
        unsafe { qpdf_oh_free_buffer(&mut buf) };
        Ok(out)
    }

    fn key(&self, oh: qpdf_oh, name: &str) -> qpdf_oh {
        let c = CString::new(name).unwrap_or_default();
        // SAFETY: live handle; qpdf_oh_get_key is trapped and returns a null handle for a key
        // that is not there.
        unsafe { qpdf_oh_get_key(self.data, oh, c.as_ptr()) }
    }

    fn type_code(&self, oh: qpdf_oh) -> i32 {
        // SAFETY: live handle, trapped accessor.
        unsafe { qpdf_oh_get_type_code(self.data, oh) }
    }

    /// The page's content streams as individual object handles.
    pub fn content_streams(&self, page: qpdf_oh) -> Vec<qpdf_oh> {
        let contents = self.key(page, "/Contents");
        // 9 == ot_array, 10 == ot_stream in qpdf's enum; asked by TYPE rather than assumed,
        // because `ffi.rs:402-409` in the production crate records that trapping answers
        // crashes and not wrong answers.
        let name = self.type_name(contents);
        if name == "array" {
            // SAFETY: live handle, trapped accessors.
            let n = unsafe { qpdf_oh_get_array_n_items(self.data, contents) };
            (0..n)
                // SAFETY: index is in range by construction.
                .map(|i| unsafe { qpdf_oh_get_array_item(self.data, contents, i) })
                .collect()
        } else if name == "stream" {
            vec![contents]
        } else {
            vec![]
        }
    }

    pub fn type_name(&self, oh: qpdf_oh) -> String {
        // The C API's `qpdf_oh_get_type_name` is trapped, but taking the code and mapping it
        // here keeps the spike's dependency surface to what it already declares.
        match self.type_code(oh) {
            0 => "uninitialized",
            1 => "reserved",
            2 => "null",
            3 => "boolean",
            4 => "integer",
            5 => "real",
            6 => "string",
            7 => "name",
            8 => "array",
            9 => "dictionary",
            10 => "stream",
            11 => "operator",
            12 => "inlineimage",
            _ => "unknown",
        }
        .to_string()
    }

    /// A null object handle, obtained WITHOUT calling `qpdf_oh_new_null`.
    ///
    /// `qpdf_oh_replace_stream_data` needs two object handles for `/Filter` and
    /// `/DecodeParms`, and the obvious way to make a null one -- `qpdf_oh_new_null` -- is
    /// neither on `engines/qpdf-trapped-functions.txt` nor in
    /// `engines/qpdf-untrapped-accepted.toml`. Asking a dictionary for a key it does not have
    /// goes through `qpdf_oh_get_key`, which IS trapped, and yields a null handle. Whether
    /// the two are interchangeable is measured rather than assumed: see
    /// `null_handles_agree` in main.
    pub fn null_via_missing_key(&self, page: qpdf_oh) -> qpdf_oh {
        self.key(page, "/BurrowSpikeKeyThatIsNotThere")
    }

    pub fn null_via_constructor(&self) -> qpdf_oh {
        // SAFETY: live handle. UNTRAPPED -- called here only to compare with the route above.
        unsafe { qpdf_oh_new_null(self.data) }
    }

    /// Replace a stream's data with `bytes`, unfiltered.
    pub fn replace_stream(&self, stream: qpdf_oh, bytes: &[u8], null: qpdf_oh) {
        // SAFETY: live handle; `bytes` is read, not retained -- qpdf copies into its own
        // buffer through a Buffer provider before this returns.
        unsafe {
            qpdf_oh_replace_stream_data(self.data, stream, bytes.as_ptr(), bytes.len(), null, null);
        }
    }

    /// The type of a key on the page dictionary, or `null` if the page does not have it.
    ///
    /// This is the whole of what redaction can reach through the permitted API: the page and
    /// what hangs off it. `qpdf_get_root` and `qpdf_get_trailer` are both refused by ADR
    /// 0013's caller rule, so anything on the CATALOGUE is not addressable at all.
    fn int_value(&self, oh: qpdf_oh) -> usize {
        if self.type_name(oh) != "integer" {
            return 0;
        }
        // SAFETY: live handle, trapped accessor, type checked first.
        let v = unsafe { qpdf_oh_get_int_value(self.data, oh) };
        usize::try_from(v).unwrap_or(0)
    }

    pub fn page_key_type(&self, page: qpdf_oh, key: &str) -> String {
        self.type_name(self.key(page, key))
    }

    pub fn unparse(&self, oh: qpdf_oh) -> String {
        // SAFETY: live handle; the returned pointer is owned by qpdf and valid until the next
        // call that reuses its scratch buffer, so it is copied immediately.
        unsafe {
            let p = qpdf_oh_unparse_resolved(self.data, oh);
            if p.is_null() {
                String::new()
            } else {
                CStr::from_ptr(p).to_string_lossy().into_owned()
            }
        }
    }

    /// Write the document out. Deterministic ID, so two runs of the harness are comparable.
    pub fn write(&self) -> Result<Vec<u8>, String> {
        // SAFETY: live handle.
        unsafe {
            let code = qpdf_init_write_memory(self.data);
            if code >= 2 {
                return Err(format!("qpdf_init_write_memory returned {code}"));
            }
            qpdf_set_deterministic_ID(self.data, 1);
            let code = qpdf_write(self.data);
            if code >= 2 {
                return Err(format!("qpdf_write returned {code}"));
            }
            let len = qpdf_get_buffer_length(self.data);
            let buf = qpdf_get_buffer(self.data);
            if buf.is_null() {
                return Err("qpdf_get_buffer returned null".into());
            }
            Ok(std::slice::from_raw_parts(buf, len).to_vec())
        }
    }
}

impl Drop for Doc {
    fn drop(&mut self) {
        // SAFETY: the handle is live and this runs exactly once.
        unsafe { qpdf_cleanup(&mut self.data) };
    }
}

/// Open and write with no edit at all. Every instrument runs against THIS, not the fixture, so
/// the measured document and the control differ by the redaction and nothing else.
pub fn roundtrip(bytes: &[u8]) -> Result<Vec<u8>, String> {
    Doc::open(bytes.to_vec())?.write()
}

/// Is the page's `/Thumb` still there, and does it still carry ink?
///
/// Channel 14's payload is pixels on an object nothing draws, so no scan reaches it and no
/// page render shows it. Without this probe the harness reports it as "gone", which is the
/// failure mode this whole spike is about: a check that examines nothing reads as coverage.
#[derive(Default)]
pub struct Thumb {
    pub present: bool,
    pub bytes: usize,
    pub ink: f64,
    /// The decoded pixels, and the `/Width` and `/Height` the dictionary declares, so the
    /// harness can write a file a person can look at. The report cited such a file before the
    /// harness produced one, which review caught: an artifact named in a document and absent
    /// from the tree is a claim, not evidence.
    pub pixels: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

impl Doc {
    pub fn thumbnail(&self, page: qpdf_oh) -> Thumb {
        let thumb = self.key(page, "/Thumb");
        if self.type_name(thumb) != "stream" {
            return Thumb::default();
        }
        // A STREAM IS NOT A DICTIONARY to `getKey`, and reading `/Width` straight off the
        // stream handle silently returned 0 -- so the thumbnail was decoded and never written
        // out, and the report's cited artifact did not exist. `qpdf_oh_get_dict` is the seam.
        let dict = unsafe { qpdf_oh_get_dict(self.data, thumb) };
        let width = self.int_value(self.key(dict, "/Width"));
        let height = self.int_value(self.key(dict, "/Height"));
        let mut buf: *mut u8 = ptr::null_mut();
        let mut len: usize = 0;
        let mut filtered: i32 = 0;
        // SAFETY: three out-parameters, all initialised first.
        let code = unsafe {
            qpdf_oh_get_stream_data(
                self.data,
                thumb,
                QPDF_DL_SPECIALIZED,
                &mut filtered,
                &mut buf,
                &mut len,
            )
        };
        if code >= 2 || buf.is_null() {
            return Thumb {
                present: true,
                ..Default::default()
            };
        }
        // SAFETY: qpdf malloc'd `len` bytes at `buf`.
        let data = unsafe { std::slice::from_raw_parts(buf, len) }.to_vec();
        // SAFETY: freeing qpdf's buffer exactly once.
        unsafe { qpdf_oh_free_buffer(&mut buf) };
        // `filtered == 0` means the bytes are STILL COMPRESSED -- reading them as pixels then
        // would measure entropy, not ink. `prune.rs` treats that case as a refusal; here it is
        // reported as unknown rather than as zero.
        if filtered == 0 {
            return Thumb {
                present: true,
                bytes: data.len(),
                ink: f64::NAN,
                ..Default::default()
            };
        }
        let dark = data.iter().filter(|b| **b < 128).count();
        let ink = if data.is_empty() {
            0.0
        } else {
            dark as f64 / data.len() as f64
        };
        Thumb {
            present: true,
            bytes: data.len(),
            ink,
            pixels: data,
            width,
            height,
        }
    }
}

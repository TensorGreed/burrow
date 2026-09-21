//! Print what PDFium's text page reads from a document, for fixture verification.
//!
//! ```text
//! cargo run -p burrow-ops --features native-engines --release \
//!   --example dump-page-text -- <input.pdf>
//! ```
//!
//! # Why this is an example and not a crate feature
//!
//! `fpdf_text.h` has no references in `core/` or `apps/`, and ADR 0029 §6 keeps it that way on
//! purpose: text extraction is **not admissible evidence** that a redaction worked, because a
//! document can write a `/ToUnicode` that lies or an `/ActualText` that substitutes. What it IS
//! good for is the opposite direction — confirming a canary is present in a fixture BEFORE
//! anything is removed, which is the half of ADR 0029 §8's rule a corpus can check today.
//!
//! So the declarations live in an EXAMPLE, not in the crate's `pdfium/ffi.rs`. It sits under
//! `burrow-engines` because that is the only crate permitted `unsafe` at all (`core/CLAUDE.md`);
//! an attempt to put it in `burrow-ops` fails to compile against the workspace's
//! `unsafe_code = "forbid"`, which is the rule working.
//!
//! The crate's API surface gains nothing, `tools/check-pdfium-is-render-only.sh` is untouched —
//! it is a claim about what a visitor downloads, not about what a native example links — and
//! #129's oracle remains free to decide for itself where the test-time bindings belong. It needs
//! `FPDFText_GetCharBox`, which this does not declare.
//!
//! **Not production code.**

#![allow(
    clippy::print_stderr,
    clippy::print_stdout,
    clippy::unwrap_used,
    reason = "an example that reports to a person and exits"
)]

#[cfg(all(feature = "native-engines", target_os = "linux"))]
mod text {
    use std::ffi::c_void;
    use std::os::raw::{c_char, c_int, c_uint, c_ulong};

    // Declared HERE rather than in the crate's `pdfium/ffi.rs`. See the module docs.
    //
    // `#[link]` is explicit because build.rs's `cargo:rustc-link-lib=dylib=pdfium` applies to
    // the crate, and an example that declares its own symbols does not inherit it — measured:
    // "undefined reference to `FPDF_InitLibrary'" at link time. The search path and rpath DO
    // come from build.rs, so only the library name is needed here.
    #[link(name = "pdfium")]
    unsafe extern "C" {
        pub fn FPDF_InitLibrary();
        pub fn FPDF_LoadMemDocument64(
            data: *const c_void,
            size: usize,
            password: *const c_char,
        ) -> *mut c_void;
        pub fn FPDF_GetPageCount(document: *mut c_void) -> c_int;
        pub fn FPDF_CloseDocument(document: *mut c_void);
        pub fn FPDF_LoadPage(document: *mut c_void, index: c_int) -> *mut c_void;
        pub fn FPDF_ClosePage(page: *mut c_void);
        pub fn FPDF_GetLastError() -> c_ulong;
        pub fn FPDFText_LoadPage(page: *mut c_void) -> *mut c_void;
        pub fn FPDFText_ClosePage(text_page: *mut c_void);
        pub fn FPDFText_CountChars(text_page: *mut c_void) -> c_int;
        pub fn FPDFText_GetUnicode(text_page: *mut c_void, index: c_int) -> c_uint;
    }

    /// Every page's text, one page per line.
    pub fn dump(bytes: &[u8]) -> Result<String, String> {
        // SAFETY: init is idempotent and this is a single-threaded example. core/CLAUDE.md
        // records that concurrent FPDF_InitLibrary aborts the process; there is one thread here.
        unsafe { FPDF_InitLibrary() };
        // SAFETY: PDFium does not copy the buffer; `bytes` outlives every call below.
        let doc = unsafe {
            FPDF_LoadMemDocument64(bytes.as_ptr().cast(), bytes.len(), std::ptr::null())
        };
        if doc.is_null() {
            // SAFETY: reading the error code PDFium just set.
            return Err(format!("FPDF_LoadMemDocument64 failed: {}", unsafe {
                FPDF_GetLastError()
            }));
        }
        let mut out = String::new();
        // SAFETY: live document handle.
        let pages = unsafe { FPDF_GetPageCount(doc) };
        for i in 0..pages {
            // SAFETY: index in range.
            let page = unsafe { FPDF_LoadPage(doc, i) };
            if page.is_null() {
                continue;
            }
            // SAFETY: live page handle.
            let tp = unsafe { FPDFText_LoadPage(page) };
            if !tp.is_null() {
                // SAFETY: live text page.
                let count = unsafe { FPDFText_CountChars(tp) };
                for c in 0..count {
                    // SAFETY: index in range.
                    let raw = unsafe { FPDFText_GetUnicode(tp, c) };
                    out.push(char::from_u32(raw).unwrap_or('\u{fffd}'));
                }
                // SAFETY: closed once, before its page.
                unsafe { FPDFText_ClosePage(tp) };
            }
            out.push('\n');
            // SAFETY: closed once.
            unsafe { FPDF_ClosePage(page) };
        }
        // SAFETY: closed once, after every page.
        unsafe { FPDF_CloseDocument(doc) };
        Ok(out)
    }
}

#[cfg(all(feature = "native-engines", target_os = "linux"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        eprintln!("usage: {} <input.pdf>", args[0]);
        std::process::exit(2);
    }
    let bytes = std::fs::read(&args[1])?;
    print!("{}", text::dump(&bytes)?);
    Ok(())
}

#[cfg(not(all(feature = "native-engines", target_os = "linux")))]
fn main() {
    eprintln!("dump-page-text needs the native engines on Linux.");
    std::process::exit(2);
}

//! Measure what each engine's merge actually preserves, so ADR 0017 can choose one for a
//! reason rather than by intuition.
//!
//! **Not production code and not a test.** It declares the engine entry points a merge
//! needs directly, rather than going through `burrow-engines`' trait seams, precisely
//! because the point is to decide which set of entry points the crate should grow. Nothing
//! here is the shape the operation will take: no `Limits`, no injected clock, no typed
//! errors, no prescan. Those arrive with the operation itself.
//!
//! Run it, from the repository root:
//!
//! ```text
//! python3 tools/make-merge-fidelity-fixtures.py /tmp/merge-fixtures
//! cargo run -p burrow-engines --features native-engines \
//!   --example measure-merge -- /tmp/merge-fixtures /tmp/merge-out
//! ```
//!
//! The damaged and hostile cases come from `tests/conformance/fixtures/`, which is
//! committed, so the only thing to generate is the fidelity set.
//!
//! Each fixture carries a literal `BURROWMARK` inside the feature under test — an outline
//! title, an annotation's `/Contents`, a form field's `/T`, an embedded file's bytes — so
//! "did it survive" is answered by looking for that marker in the merged output, after
//! `qpdf --qdf` has decompressed it. A structural walk would be a second parser to trust;
//! a marker is a fact.

// This whole file is a measurement harness, and the workspace lints that keep library code
// panic-free are the wrong tool for it: a failed measurement should stop loudly and say
// what it was doing, not thread a typed error through a throwaway.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::missing_docs_in_private_items
)]
#![cfg_attr(
    not(all(feature = "native-engines", burrow_native_engines)),
    allow(unused)
)]

#[cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
mod measure {
    use std::ffi::{CString, c_char, c_int, c_uchar, c_ulong, c_void};
    use std::path::Path;

    // ---------------------------------------------------------------- PDFium

    #[repr(C)]
    pub struct FpdfFileWrite {
        pub version: c_int,
        pub write_block: unsafe extern "C" fn(*mut FpdfFileWrite, *const c_void, c_ulong) -> c_int,
    }

    /// Collects everything PDFium writes. The struct PDFium is handed is the first field of
    /// this one, so the callback can recover the collector by casting back — the standard
    /// way this C API is extended, and the reason `#[repr(C)]` is load-bearing here.
    #[repr(C)]
    struct Collector {
        write: FpdfFileWrite,
        out: *mut Vec<u8>,
    }

    unsafe extern "C" fn write_block(
        this: *mut FpdfFileWrite,
        data: *const c_void,
        size: c_ulong,
    ) -> c_int {
        // SAFETY: PDFium hands back exactly the pointer we gave it, which is the address of
        // a live `Collector`'s first field, and calls this only from inside
        // `FPDF_SaveAsCopy` while that `Collector` is on our stack.
        unsafe {
            let collector = this.cast::<Collector>();
            let out = &mut *(*collector).out;
            out.extend_from_slice(std::slice::from_raw_parts(data.cast::<u8>(), size as usize));
        }
        1
    }

    // LINKING AN EXAMPLE IS NOT LINKING THE LIBRARY, and both obvious spellings fail.
    // `build.rs`'s `cargo:rustc-link-lib` lines are not enough on their own here — the
    // symbols below are ones the library crate never references, so nothing pulls them in
    // and every one comes back undefined. Adding `#[link(name = "qpdf")]` fixes that and
    // breaks the other end: it appends a second `-lqpdf` AFTER the C++ runtime, so
    // `__cxa_throw` and friends go unresolved instead. Hence this shape — name each
    // library on the block that needs it, and name the C++ runtime again afterwards, so
    // it is last on the command line where a static archive needs it to be.
    #[link(name = "pdfium")]
    unsafe extern "C" {
        fn FPDF_InitLibrary();
        fn FPDF_LoadMemDocument64(buf: *const c_void, len: usize, pw: *const c_char)
        -> *mut c_void;
        fn FPDF_CloseDocument(doc: *mut c_void);
        fn FPDF_GetPageCount(doc: *mut c_void) -> c_int;
        fn FPDF_CreateNewDocument() -> *mut c_void;
        fn FPDF_ImportPagesByIndex(
            dest: *mut c_void,
            src: *mut c_void,
            indices: *const c_int,
            length: c_ulong,
            index: c_int,
        ) -> c_int;
        fn FPDF_SaveAsCopy(doc: *mut c_void, out: *mut FpdfFileWrite, flags: c_ulong) -> c_int;
        fn FPDF_GetLastError() -> c_ulong;
    }

    pub fn pdfium_merge(inputs: &[Vec<u8>]) -> Result<Vec<u8>, String> {
        // SAFETY: single-threaded harness; init is called once per process by the caller.
        unsafe {
            let dest = FPDF_CreateNewDocument();
            if dest.is_null() {
                return Err(format!(
                    "FPDF_CreateNewDocument failed ({})",
                    FPDF_GetLastError()
                ));
            }
            let mut at = 0;
            let mut open = Vec::new();
            for (n, bytes) in inputs.iter().enumerate() {
                let src =
                    FPDF_LoadMemDocument64(bytes.as_ptr().cast(), bytes.len(), std::ptr::null());
                if src.is_null() {
                    FPDF_CloseDocument(dest);
                    return Err(format!("input {n}: load failed ({})", FPDF_GetLastError()));
                }
                let pages = FPDF_GetPageCount(src);
                // A null index list means "every page", which is what merge wants.
                if FPDF_ImportPagesByIndex(dest, src, std::ptr::null(), 0, at) == 0 {
                    FPDF_CloseDocument(src);
                    FPDF_CloseDocument(dest);
                    return Err(format!(
                        "input {n}: import failed ({})",
                        FPDF_GetLastError()
                    ));
                }
                at += pages;
                open.push(src);
            }

            let mut out = Vec::new();
            let mut collector = Collector {
                write: FpdfFileWrite {
                    version: 1,
                    write_block,
                },
                out: &raw mut out,
            };
            let ok = FPDF_SaveAsCopy(dest, &raw mut collector.write, 0);

            for src in open {
                FPDF_CloseDocument(src);
            }
            FPDF_CloseDocument(dest);

            if ok == 0 {
                return Err(format!("FPDF_SaveAsCopy failed ({})", FPDF_GetLastError()));
            }
            Ok(out)
        }
    }

    pub fn pdfium_init() {
        // SAFETY: called once, before any other PDFium entry point, on one thread.
        unsafe { FPDF_InitLibrary() };
    }

    // ------------------------------------------------------------------ qpdf

    type QpdfData = *mut c_void;
    type QpdfOh = c_uint;
    use std::ffi::c_uint;

    // ONE `#[link]` PER BLOCK, AND THE ORDER OF THE BLOCKS IS THE LINK ORDER.
    //
    // clippy reads three `#[link]` on one block as a duplicated attribute, so they are
    // split -- and splitting them is not cosmetic: a static archive only resolves symbols
    // against archives that come AFTER it. Putting the empty `z` and `jpeg` blocks above
    // this one moved them earlier on the command line and every `inflate`/`deflate` in
    // libqpdf went undefined. They are below, with the C++ runtime last.
    #[link(name = "qpdf", kind = "static")]
    unsafe extern "C" {
        fn qpdf_init() -> QpdfData;
        fn qpdf_cleanup(q: *mut QpdfData);
        fn qpdf_silence_errors(q: QpdfData);
        fn qpdf_set_suppress_warnings(q: QpdfData, v: c_int);
        fn qpdf_read_memory(
            q: QpdfData,
            desc: *const c_char,
            buf: *const c_char,
            len: c_ulong,
            pw: *const c_char,
        ) -> c_int;
        fn qpdf_get_num_pages(q: QpdfData) -> c_int;
        fn qpdf_get_page_n(q: QpdfData, n: usize) -> QpdfOh;
        fn qpdf_add_page(q: QpdfData, newpage_q: QpdfData, newpage: QpdfOh, first: c_int) -> c_int;
        fn qpdf_init_write_memory(q: QpdfData) -> c_int;
        fn qpdf_set_static_ID(q: QpdfData, v: c_int);
        fn qpdf_write(q: QpdfData) -> c_int;
        fn qpdf_get_buffer_length(q: QpdfData) -> usize;
        fn qpdf_get_buffer(q: QpdfData) -> *const c_uchar;
        fn qpdf_has_error(q: QpdfData) -> c_int;
        fn qpdf_get_error(q: QpdfData) -> *mut c_void;
        fn qpdf_set_logger(q: QpdfData, logger: *mut c_void);
        fn qpdflogger_create() -> *mut c_void;
        fn qpdflogger_set_info(logger: *mut c_void, dest: c_int, path: *const c_char);
        fn qpdflogger_set_warn(logger: *mut c_void, dest: c_int, path: *const c_char);
        fn qpdflogger_set_error(logger: *mut c_void, dest: c_int, path: *const c_char);
    }

    /// qpdf's status is a BITMASK, never a plain success/failure code.
    ///
    /// `QPDF_WARNINGS` is bit 0 and `QPDF_ERRORS` is bit 1 (`qpdf-c.h:137-139`). Testing
    /// `!= 0` counts a warnings-only read as a failure — and every damaged file this
    /// project cares about reads with warnings. The first version of this harness did
    /// exactly that and reported qpdf REFUSING `truncated.pdf` and `trailer-removed.pdf`,
    /// contradicting ADR 0013's measured finding that qpdf reads both where PDFium will
    /// not. The contradiction was the harness, not the ADR. `qpdf/mod.rs` gets this right
    /// and says why; this is the same rule, relearned the expensive way.
    const QPDF_ERRORS: c_int = 1 << 1;

    fn has_errors(status: c_int) -> bool {
        status & QPDF_ERRORS != 0
    }

    /// Take the error out of qpdf's slot before destroying the document.
    ///
    /// qpdf's C API prints `WARNING: application did not handle error: ...` to stderr from
    /// `qpdf_cleanup` when an error was recorded and never retrieved — and it does that
    /// OUTSIDE the logger, so silencing and a discarding logger do not stop it. Measured
    /// here: all three were installed and the line still appeared. `qpdf/mod.rs`'s `Drop`
    /// drains the slot first for exactly this reason; the harness had to learn it again.
    unsafe fn drain(q: QpdfData) {
        // SAFETY: `q` is live; `qpdf_get_error` transfers ownership of the error object,
        // which the C API then forgets about. The harness never reads it — the point is
        // only that qpdf no longer considers it unhandled.
        unsafe {
            while qpdf_has_error(q) != 0 {
                let _ = qpdf_get_error(q);
            }
        }
    }

    /// Discard everything qpdf would otherwise write to stderr.
    ///
    /// Without this the harness printed `WARNING: application did not handle error: input:
    /// unable to find any pages while recovering damaged file` — a byte offset away from
    /// being file content in a terminal. `qpdf_silence_errors` and
    /// `qpdf_set_suppress_warnings` are NOT sufficient on their own, which is exactly why
    /// `qpdf/limits.rs` installs a discarding logger as well. Same two layers, same reason.
    unsafe fn silence(q: QpdfData) {
        // SAFETY: `q` is a live qpdf_data and none of these read the document.
        unsafe {
            qpdf_silence_errors(q);
            qpdf_set_suppress_warnings(q, 1);
            let logger = qpdflogger_create();
            // 0 is `qpdf_log_dest_discard` — qpdf's own Pl_Discard, reachable by name from
            // C, so no Rust code ever runs on a C++ stack.
            qpdflogger_set_info(logger, 0, std::ptr::null());
            qpdflogger_set_warn(logger, 0, std::ptr::null());
            qpdflogger_set_error(logger, 0, std::ptr::null());
            qpdf_set_logger(q, logger);
        }
    }

    #[link(name = "z", kind = "static")]
    unsafe extern "C" {}

    #[link(name = "jpeg", kind = "static")]
    unsafe extern "C" {}

    // Last on the command line, which is where a static C++ archive needs its runtime.
    #[link(name = "stdc++")]
    unsafe extern "C" {}

    pub fn qpdf_merge(inputs: &[Vec<u8>]) -> Result<Vec<u8>, String> {
        // SAFETY: single-threaded harness. Every handle is created, used and destroyed
        // within this function, and the source documents deliberately outlive the write —
        // qpdf resolves the foreign pages lazily, so closing them early yields a truncated
        // output rather than an error, which is itself worth knowing.
        unsafe {
            let mut dest = qpdf_init();
            silence(dest);

            let desc = CString::new("input").unwrap();
            let Some(first) = inputs.first() else {
                return Err("merge needs at least one input".to_owned());
            };
            let read = qpdf_read_memory(
                dest,
                desc.as_ptr(),
                first.as_ptr().cast(),
                first.len() as c_ulong,
                std::ptr::null(),
            );
            if has_errors(read) {
                drain(dest);
                qpdf_cleanup(&raw mut dest);
                return Err("input 0: read failed".into());
            }

            let mut sources = Vec::new();
            for (n, bytes) in inputs.iter().enumerate().skip(1) {
                let mut src = qpdf_init();
                silence(src);
                let read = qpdf_read_memory(
                    src,
                    desc.as_ptr(),
                    bytes.as_ptr().cast(),
                    bytes.len() as c_ulong,
                    std::ptr::null(),
                );
                if has_errors(read) {
                    drain(src);
                    qpdf_cleanup(&raw mut src);
                    return Err(format!("input {n}: read failed"));
                }
                let pages = qpdf_get_num_pages(src);
                for i in 0..pages.max(0) {
                    let page = qpdf_get_page_n(src, usize::try_from(i).unwrap_or(0));
                    let rc = qpdf_add_page(dest, src, page, 0);
                    if rc != 0 {
                        return Err(format!("input {n}: add_page {i} failed"));
                    }
                }
                sources.push(src);
            }

            if qpdf_init_write_memory(dest) != 0 {
                return Err("init_write_memory failed".into());
            }
            // AFTER init_write_memory, not before. qpdf-c.h says the write parameters are
            // "called after qpdf_init_write (or qpdf_init_write_memory) and before
            // qpdf_write"; calling this first dereferences a writer that does not exist
            // yet and takes the process down. Measured, not read: it cost a core dump.
            // Deterministic output, so two runs of this harness are comparable.
            qpdf_set_static_ID(dest, 1);
            if qpdf_write(dest) != 0 && qpdf_has_error(dest) != 0 {
                return Err("write failed".into());
            }
            let len = qpdf_get_buffer_length(dest);
            let ptr = qpdf_get_buffer(dest);
            let out = std::slice::from_raw_parts(ptr, len).to_vec();

            for mut src in sources {
                drain(src);
                qpdf_cleanup(&raw mut src);
            }
            drain(dest);
            qpdf_cleanup(&raw mut dest);
            Ok(out)
        }
    }

    /// What `qpdf_get_num_pages` alone says about a file — the capability ADR 0013's
    /// repair finding actually measured. A merge needs `qpdf_get_page_n` and
    /// `qpdf_add_page` on top of it, and those are a strictly higher bar.
    pub fn qpdf_page_count(bytes: &[u8]) -> Option<i32> {
        // SAFETY: single-threaded harness; the handle is created and destroyed here.
        unsafe {
            let mut q = qpdf_init();
            silence(q);
            let desc = CString::new("input").unwrap();
            let read = qpdf_read_memory(
                q,
                desc.as_ptr(),
                bytes.as_ptr().cast(),
                bytes.len() as c_ulong,
                std::ptr::null(),
            );
            let answer = if has_errors(read) {
                None
            } else {
                Some(qpdf_get_num_pages(q))
            };
            drain(q);
            qpdf_cleanup(&raw mut q);
            answer
        }
    }

    // ------------------------------------------------------------------ run

    pub fn run(fixtures: &Path, outdir: &Path) {
        pdfium_init();
        std::fs::create_dir_all(outdir).unwrap();

        // Every case merges a marker-carrying fixture with a plain one, so the output has
        // pages from two documents and the marker has somewhere to be lost.
        let cases: &[(&str, &[&str])] = &[
            ("outline", &["outline.pdf", "plain2.pdf"]),
            ("annotation", &["annotation.pdf", "plain2.pdf"]),
            ("formfield", &["formfield.pdf", "plain2.pdf"]),
            ("attachment", &["attachment.pdf", "plain2.pdf"]),
            ("cropbox", &["cropbox.pdf", "plain2.pdf"]),
            ("plain", &["plain3.pdf", "plain2.pdf"]),
            ("single", &["plain3.pdf"]),
            // The two files ADR 0013 records qpdf reading and PDFium refusing. A merge
            // that can repair one input is a different product from one that cannot.
            ("damaged-truncated", &["corpus:truncated.pdf", "plain2.pdf"]),
            (
                "damaged-trailer",
                &["corpus:trailer-removed.pdf", "plain2.pdf"],
            ),
            // An encrypted input with no password. Both should refuse; what matters is
            // that neither aborts and neither silently produces a short document.
            ("encrypted", &["corpus:encrypted.pdf", "plain2.pdf"]),
            // A hostile file. Neither engine gets a Limits here, so this measures raw
            // engine behaviour, which is exactly what the pre-scan exists to front.
            ("xref-bomb", &["corpus:xref-bomb.pdf", "plain2.pdf"]),
            // The 356-byte file whose object number aborted the process through an
            // untrapped qpdf call in M1 PR 3.
            (
                "object-number",
                &["corpus:object-number-above-int-max.pdf", "plain2.pdf"],
            ),
        ];

        println!("case         engine     pages    bytes  result");
        for (name, files) in cases {
            let inputs: Vec<Vec<u8>> = files
                .iter()
                .map(|f| {
                    // `corpus:` names a committed conformance fixture, resolved against the
                    // repository rather than against the generated set -- so the damaged and
                    // hostile cases need nothing generated at all, and the harness cannot
                    // quietly measure a file someone made up locally.
                    let path = match f.strip_prefix("corpus:") {
                        Some(name) => corpus().join(name),
                        None => fixtures.join(f),
                    };
                    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
                })
                .collect();

            for (engine, merged) in [
                ("pdfium", pdfium_merge(&inputs)),
                ("qpdf", qpdf_merge(&inputs)),
            ] {
                match merged {
                    Ok(bytes) => {
                        let pages = page_count(&bytes);
                        let path = outdir.join(format!("{name}.{engine}.pdf"));
                        std::fs::write(&path, &bytes).unwrap();
                        println!("{name:<12} {engine:<8} {pages:>6} {:>8}  ok", bytes.len());
                    }
                    Err(why) => {
                        let note = if engine == "qpdf" {
                            match inputs.first().and_then(|b| qpdf_page_count(b)) {
                                Some(n) => format!("{why} (qpdf counts {n} pages in input 0)"),
                                None => format!("{why} (qpdf cannot count input 0 either)"),
                            }
                        } else {
                            why
                        };
                        println!("{name:<12} {engine:<8} {:>6} {:>8}  {note}", '-', '-');
                    }
                }
            }
        }
    }

    /// `tests/conformance/fixtures/`, resolved from this crate rather than from the
    /// working directory, so the harness runs the same way from anywhere.
    fn corpus() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/conformance/fixtures")
            .to_path_buf()
    }

    /// Page count of a merged output, via PDFium, purely so the table has a number in it.
    fn page_count(bytes: &[u8]) -> i32 {
        // SAFETY: single-threaded harness, library already initialised.
        unsafe {
            let doc = FPDF_LoadMemDocument64(bytes.as_ptr().cast(), bytes.len(), std::ptr::null());
            if doc.is_null() {
                return -1;
            }
            let n = FPDF_GetPageCount(doc);
            FPDF_CloseDocument(doc);
            n
        }
    }
}

fn main() {
    #[cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
    {
        let args: Vec<String> = std::env::args().collect();
        let fixtures = args
            .get(1)
            .expect("usage: measure-merge <fixtures> <outdir>");
        let outdir = args
            .get(2)
            .expect("usage: measure-merge <fixtures> <outdir>");
        measure::run(std::path::Path::new(fixtures), std::path::Path::new(outdir));
    }

    #[cfg(not(all(feature = "native-engines", burrow_native_engines, target_os = "linux")))]
    {
        eprintln!(
            "measure-merge needs --features native-engines on linux, with engines/fetch.sh \
             and engines/build-native.sh already run."
        );
        std::process::exit(1);
    }
}

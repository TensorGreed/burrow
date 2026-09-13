//! Measure what each way of splitting actually preserves, so the ADR can choose one for a
//! reason rather than by intuition.
//!
//! **Not production code and not a test**, for the same reasons `measure-split`'s sibling
//! `measure-merge.rs` gives: it declares the entry points directly rather than going through
//! the trait seams, because the point is to decide which entry points the crate should grow.
//! No `Limits`, no injected clock, no typed errors, no prescan.
//!
//! Run it, from the repository root:
//!
//! ```text
//! python3 tools/make-split-fidelity-fixtures.py /tmp/split-fixtures
//! cargo run -p burrow-engines --features native-engines \
//!   --example measure-split -- /tmp/split-fixtures /tmp/split-out
//! ```
//!
//! # The two routes, and why there are only two
//!
//! A split needs a DESTINATION document to put pages into, and qpdf offers exactly one way
//! to make an empty one -- `qpdf_empty_pdf` -- which **burrow may not call**. It is not on
//! `engines/qpdf-trapped-functions.txt`, and it does not meet the bar in
//! `engines/qpdf-untrapped-accepted.toml` ("non-parsing ... never touches the PDF's bytes"):
//! its implementation is `QPDF::emptyPDF()`, which is
//!
//! ```text
//! processMemoryFile("empty PDF", EMPTY_PDF, strlen(EMPTY_PDF));
//! ```
//!
//! -- the full parser, on a constant. A `QPDFExc` out of that crosses an `extern "C"` frame
//! and aborts the process (ADR 0013 §1). So the two routes below are the trapped ones:
//!
//!   * **CARVE** -- read the source once per output and `qpdf_remove_page` everything outside
//!     the range. Every output inherits the source's catalog. Costs one full parse per output.
//!   * **BUILD** -- read a near-empty PDF of our own as the destination (through the trapped
//!     `qpdf_read_memory`, so the "empty" document is bytes we ship), then `qpdf_add_page`
//!     the wanted pages from a single open source. One parse of the source; the destination
//!     starts with nothing but a catalog and an empty page tree.
//!
//! The question this harness answers is what each keeps. Both are correct about PAGES; they
//! differ in what comes with them, and that difference is the decision.
//!
//! Each fixture carries a literal `BURROWMARK` inside the feature under test, exactly as the
//! merge harness does, so "did it survive" is answered by looking for that marker in the
//! output after `qpdf --qdf` has decompressed it. A structural walk would be a second parser
//! to trust; a marker is a fact.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
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
    use std::process::{Command, Stdio};
    use std::time::Instant;

    pub type QpdfData = *mut c_void;
    pub type QpdfOh = c_uint;
    use std::ffi::c_uint;

    // One `#[link]` per block, in dependency order, with the C++ runtime last: a static
    // archive has to appear before the libraries it needs and after nothing. The merge
    // harness records what the two obvious spellings do instead.
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
        fn qpdf_remove_page(q: QpdfData, page: QpdfOh) -> c_int;
        fn qpdf_init_write_memory(q: QpdfData) -> c_int;
        fn qpdf_set_static_ID(q: QpdfData, v: c_int);
        fn qpdf_write(q: QpdfData) -> c_int;
        fn qpdf_get_buffer_length(q: QpdfData) -> usize;
        fn qpdf_get_buffer(q: QpdfData) -> *const c_uchar;
        fn qpdf_has_error(q: QpdfData) -> c_int;
        fn qpdf_get_error(q: QpdfData) -> *mut c_void;
    }

    #[link(name = "z", kind = "static")]
    unsafe extern "C" {}

    #[link(name = "jpeg", kind = "static")]
    unsafe extern "C" {}

    #[link(name = "stdc++")]
    unsafe extern "C" {}

    /// A valid PDF with a catalog and **no pages**, used as the BUILD route's destination.
    ///
    /// Ours, not qpdf's: it goes in through the trapped `qpdf_read_memory`, which is the
    /// whole point -- the untrapped `qpdf_empty_pdf` is what this avoids. Written out longhand
    /// with a real xref so qpdf accepts it without reconstructing anything.
    pub fn zero_page_pdf() -> Vec<u8> {
        let objects: [&str; 2] = [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [] /Count 0 >>",
        ];
        let mut out = String::from("%PDF-1.7\n");
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.push_str(&format!("{} 0 obj\n{body}\nendobj\n", i + 1));
        }
        let startxref = out.len();
        out.push_str(&format!(
            "xref\n0 {}\n0000000000 65535 f \n",
            objects.len() + 1
        ));
        for offset in &offsets {
            out.push_str(&format!("{offset:010} 00000 n \n"));
        }
        out.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{startxref}\n%%EOF\n",
            objects.len() + 1
        ));
        out.into_bytes()
    }

    unsafe fn drain(q: QpdfData) {
        // Leaving an error undrained makes qpdf print "application did not handle error" to
        // stderr at cleanup -- measured during ADR 0017's comparison.
        unsafe {
            while qpdf_has_error(q) != 0 {
                let _ = qpdf_get_error(q);
            }
        }
    }

    unsafe fn open(bytes: &[u8]) -> Result<QpdfData, String> {
        unsafe {
            let q = qpdf_init();
            qpdf_silence_errors(q);
            qpdf_set_suppress_warnings(q, 1);
            let desc = CString::new("input").unwrap();
            let rc = qpdf_read_memory(
                q,
                desc.as_ptr(),
                bytes.as_ptr().cast::<c_char>(),
                bytes.len() as c_ulong,
                std::ptr::null(),
            );
            // The ERROR bit, never `!= 0`: a warning is an ordinary outcome.
            if rc & 2 != 0 {
                drain(q);
                let mut q = q;
                qpdf_cleanup(&raw mut q);
                return Err("read failed".to_owned());
            }
            drain(q);
            Ok(q)
        }
    }

    unsafe fn write_out(q: QpdfData) -> Result<Vec<u8>, String> {
        unsafe {
            if qpdf_init_write_memory(q) & 2 != 0 {
                drain(q);
                return Err("init_write_memory failed".to_owned());
            }
            // AFTER `init_write_memory`: the writer does not exist until that call succeeds,
            // and setting the ID first takes the process down by core dump.
            qpdf_set_static_ID(q, 1);
            if qpdf_write(q) & 2 != 0 {
                drain(q);
                return Err("write failed".to_owned());
            }
            drain(q);
            let len = qpdf_get_buffer_length(q);
            let ptr = qpdf_get_buffer(q);
            if ptr.is_null() || len == 0 {
                return Err("no output".to_owned());
            }
            Ok(std::slice::from_raw_parts(ptr, len).to_vec())
        }
    }

    /// CARVE: read the source afresh and remove every page outside `[first, first + count)`.
    pub fn carve(source: &[u8], first: usize, count: usize) -> Result<Vec<u8>, String> {
        unsafe {
            let q = open(source)?;
            let total = qpdf_get_num_pages(q);
            if total < 0 {
                return Err("page count failed".to_owned());
            }
            // BACKWARDS, so an index is never invalidated by an earlier removal.
            for n in (0..total as usize).rev() {
                if n >= first && n < first + count {
                    continue;
                }
                let page = qpdf_get_page_n(q, n);
                if qpdf_remove_page(q, page) & 2 != 0 {
                    drain(q);
                    return Err(format!("remove_page({n}) failed"));
                }
            }
            drain(q);
            let out = write_out(q);
            let mut q = q;
            qpdf_cleanup(&raw mut q);
            out
        }
    }

    /// BUILD: a near-empty destination of our own, plus the wanted pages from one open source.
    ///
    /// **This harness uses a genuinely zero-page constant and the SHIPPED route cannot.** qpdf
    /// refuses to open a zero-page document through `Document::open`, so `extract.rs` uses one
    /// blank page and removes it afterwards. The difference does not change what is kept --
    /// the blank contributes nothing either way -- but this ADR's own theme is measurements
    /// that described something other than the thing, so it is said rather than assumed.
    pub fn build(source_q: QpdfData, first: usize, count: usize) -> Result<Vec<u8>, String> {
        unsafe {
            let blank = zero_page_pdf();
            let dest = open(&blank)?;
            for n in first..first + count {
                let page = qpdf_get_page_n(source_q, n);
                if qpdf_add_page(dest, source_q, page, 0) & 2 != 0 {
                    drain(dest);
                    return Err(format!("add_page({n}) failed"));
                }
            }
            drain(dest);
            let out = write_out(dest);
            let mut dest = dest;
            qpdf_cleanup(&raw mut dest);
            out
        }
    }

    pub fn open_source(bytes: &[u8]) -> Result<QpdfData, String> {
        unsafe { open(bytes) }
    }

    pub fn close(q: QpdfData) {
        let mut q = q;
        // SAFETY: `q` came from `qpdf_init` in `open` and is closed once.
        unsafe { qpdf_cleanup(&raw mut q) };
    }

    /// Whether the decompressed output still contains the fixture's marker.
    fn kept(bytes: &[u8]) -> bool {
        let expanded = expand(bytes);
        expanded.windows(10).any(|w| w == b"BURROWMARK")
    }

    /// Markers naming pages that are NOT in this output.
    ///
    /// **The measurement the "kept" column cannot make, and the one that decides this.** A
    /// route that keeps the source's catalog keeps it whole, so a two-page output carries
    /// outline titles for all five source pages -- and on a privacy tool that is not fidelity,
    /// it is a leak: someone splitting off pages 2-3 to send to another person would be
    /// sending the titles of pages 4 and 5 with them.
    ///
    /// The fixtures put the page number in the marker precisely so this is answerable by
    /// searching rather than by walking a structure.
    fn leaked(bytes: &[u8], kept_pages: &[usize]) -> Vec<usize> {
        let expanded = expand(bytes);
        (1..=PAGES)
            .filter(|n| !kept_pages.contains(n))
            .filter(|n| {
                let needle = format!("BURROWMARK page {n}");
                expanded
                    .windows(needle.len())
                    .any(|w| w == needle.as_bytes())
            })
            .collect()
    }

    const PAGES: usize = 5;

    /// `qpdf --qdf` the bytes, so a marker inside a compressed stream is still findable.
    ///
    /// **Via temp files, and with no fallback.** The first version piped through stdin
    /// (`qpdf --qdf - -`), which this qpdf rejects with `open -: No such file or directory`,
    /// and then fell back to the raw bytes when it got nothing. So it scanned COMPRESSED
    /// output while printing results as though it had expanded them — and its own
    /// missing-CLI warning never fired, because the CLI was present and the invocation was
    /// wrong. A measurement that degrades quietly is the thing this repository keeps finding.
    fn expand(bytes: &[u8]) -> Vec<u8> {
        let dir = std::env::temp_dir();
        let tag = format!("burrow-measure-split-{}", std::process::id());
        let input = dir.join(format!("{tag}-in.pdf"));
        let output = dir.join(format!("{tag}-out.pdf"));
        std::fs::write(&input, bytes).unwrap();

        let status = Command::new("qpdf")
            .args(["--qdf", "--object-streams=disable"])
            .arg(&input)
            .arg(&output)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("the `qpdf` CLI is required: this harness cannot see inside a flated stream");
        assert!(
            matches!(status.code(), Some(0 | 3)),
            "qpdf could not expand the document ({status:?}); the table below would be a \
             measurement of compressed bytes rather than of content"
        );
        let expanded = std::fs::read(&output).unwrap();
        assert!(!expanded.is_empty(), "qpdf produced an empty expansion");
        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&output);
        expanded
    }

    pub fn run(fixtures: &Path, out_dir: &Path) {
        std::fs::create_dir_all(out_dir).unwrap();
        let has_cli = std::process::Command::new("qpdf")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok();
        assert!(
            has_cli,
            "the `qpdf` CLI must be on PATH: without it a marker inside a compressed stream \
             is invisible, and this harness would understate every route equally while \
             printing a table that looks like a decision"
        );

        // MULTI-PAGE, from `tools/make-split-fidelity-fixtures.py`, and that is the whole
        // point. The first version of this harness used the MERGE fixtures, which are one
        // page each -- so a one-page split of them was the identity, every feature trivially
        // survived, and the table reported both routes as perfect. It was convincing and it
        // measured nothing. A split asks what happens to a feature when pages it refers to
        // are taken AWAY, and that needs pages to take away.
        let cases = [
            ("outline-per-page.pdf", "outline, 1 per page"),
            ("fields-across-pages.pdf", "/AcroForm, pp 2 & 4"),
            ("attachment-5page.pdf", "attachment on catalog"),
        ];

        println!(
            "{:<24} {:<22} {:>6} {:>6} {:>14} {:>14}",
            "fixture", "feature", "carve", "build", "carve leaks", "build leaks"
        );
        println!("{}", "-".repeat(90));

        for (name, feature) in cases {
            let path = fixtures.join(name);
            let Ok(bytes) = std::fs::read(&path) else {
                println!("{name:<16} {feature:<18} (missing)");
                continue;
            };

            // PAGES 2-3 OF 5, deliberately not page 1. A range starting at the first page
            // hides the difference that matters: an outline's first entry points at page 1
            // either way, so a split that keeps it looks identical to one that rebuilt it.
            let carved = carve(&bytes, 1, 2);
            let source = open_source(&bytes).unwrap();
            let built = build(source, 1, 2);
            close(source);

            let mark = |r: &Result<Vec<u8>, String>| match r {
                Ok(out) => {
                    if kept(out) {
                        "kept"
                    } else {
                        "LOST"
                    }
                }
                Err(_) => "error",
            };
            let size = |r: &Result<Vec<u8>, String>| match r {
                Ok(out) => out.len().to_string(),
                Err(e) => e.clone(),
            };

            if let Ok(out) = &carved {
                std::fs::write(out_dir.join(format!("carve-{name}")), out).unwrap();
            }
            if let Ok(out) = &built {
                std::fs::write(out_dir.join(format!("build-{name}")), out).unwrap();
            }

            // Pages 2 and 3 of 5 are what was asked for; 1, 4 and 5 are what must not come
            // along.
            let kept_pages = [2usize, 3];
            let leaks = |r: &Result<Vec<u8>, String>| match r {
                Ok(out) => {
                    let pages = leaked(out, &kept_pages);
                    if pages.is_empty() {
                        "none".to_owned()
                    } else {
                        format!("pages {pages:?}")
                    }
                }
                Err(_) => "-".to_owned(),
            };

            println!(
                "{:<24} {:<22} {:>6} {:>6} {:>14} {:>14}",
                name,
                feature,
                mark(&carved),
                mark(&built),
                leaks(&carved),
                leaks(&built)
            );
            let _ = (size(&carved), size(&built));
        }

        // COST, on the fixture that has enough pages for the difference to show. CARVE parses
        // the source once per output; BUILD parses it once in total.
        let many = fixtures.join("boxes-differ.pdf");
        if let Ok(bytes) = std::fs::read(&many) {
            let outputs = 5;

            let started = Instant::now();
            for n in 0..outputs {
                let _ = carve(&bytes, n, 1).unwrap();
            }
            let carve_ms = started.elapsed().as_micros();

            let started = Instant::now();
            let source = open_source(&bytes).unwrap();
            for n in 0..outputs {
                let _ = build(source, n, 1).unwrap();
            }
            close(source);
            let build_ms = started.elapsed().as_micros();

            println!(
                "\ncost, {outputs}-way split of boxes-differ.pdf: carve {carve_ms} us, build {build_ms} us"
            );
        }
    }
}

fn main() {
    #[cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
    {
        let mut args = std::env::args().skip(1);
        let fixtures = args.next().expect("usage: measure-split <fixtures> <out>");
        let out = args.next().expect("usage: measure-split <fixtures> <out>");
        measure::run(std::path::Path::new(&fixtures), std::path::Path::new(&out));
    }
    #[cfg(not(all(feature = "native-engines", burrow_native_engines, target_os = "linux")))]
    {
        eprintln!("measure-split needs --features native-engines on linux");
    }
}

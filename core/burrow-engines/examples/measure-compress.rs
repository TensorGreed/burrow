//! Measure what qpdf alone compresses, so spike 0005 can decide whether `compress` is worth
//! shipping for a reason rather than by intuition.
//!
//! **Not production code and not a test.** Like `measure-merge`, it declares the engine entry
//! points directly rather than going through `burrow-engines`' trait seams, precisely because
//! the point is to decide which set of entry points the crate should grow. No `Limits`, no
//! injected clock, no typed errors, no prescan.
//!
//! Run it, from the repository root:
//!
//! ```text
//! python3 tools/make-compress-fixtures.py /tmp/compress-fixtures
//! cargo run -p burrow-engines --features native-engines --release \
//!   --example measure-compress -- /tmp/compress-fixtures
//! ```
//!
//! A second positional argument runs the same arms over a directory of arbitrary PDFs and
//! reports a DISTRIBUTION rather than a table — used for qpdf's own test suite.
//!
//! # Three arms, because one number cannot separate the causes
//!
//! | arm | what it is | the question |
//! |---|---|---|
//! | **C** | `qpdf_write` with qpdf's own defaults | how much of any "saving" is just the rewrite every burrow operation ALREADY does |
//! | **A** | C plus every compression lever the C API exposes | what a qpdf-only `compress` can actually ship |
//! | **B** | the pinned qpdf CLI with `--recompress-flate --compression-level=9` | the size of the gap ADR 0013's no-C++-shim rule costs |
//!
//! Arm C is the one that makes the table worth reading. `rotate` already emits arm C output
//! for every file it touches, so a `compress` that only reached arm C would be a tool whose
//! entire contribution a person could get by rotating a page and rotating it back.
//!
//! # The inertness control, and why a saving figure needs one
//!
//! `CLAUDE.md`: a check that silently examines nothing reads as coverage. The hazard here is
//! precise — `qpdf_write` rewrites and re-compresses a file all by itself, so a harness that
//! compared arm A against the INPUT would report qpdf's ordinary rewrite as compression and be
//! unable to tell the difference. Every run therefore asserts:
//!
//! - arm A with every lever set back to its default is **byte-identical** to arm C, so the
//!   A-over-C column is attributable to the levers and to nothing else; and
//! - at least one fixture has a non-zero A-over-C saving, so a build in which the levers had
//!   quietly stopped being applied fails instead of reporting zeroes as a finding.
//!
//! Both are checked, both are printed, and the run exits non-zero if either fails.
//!
//! # Per-lever attribution
//!
//! A percentage with no mechanism beside it is not a measurement, so each lever is also
//! measured by DISABLING it from full arm A: the delta is what that lever was worth on that
//! file. That is the direction that reports a lever which has stopped working, which measuring
//! each lever alone against the baseline would not.

// This whole file is a measurement harness, and the workspace lints that keep library code
// panic-free are the wrong tool for it: a failed measurement should stop loudly and say what it
// was doing, not thread a typed error through a throwaway.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::missing_docs_in_private_items,
    clippy::too_many_lines,
    // A measurement harness indexes into vectors it just built and takes medians by halving a
    // length. Both lints exist for attacker-controlled sizes in library code, which is where
    // they stay denied -- see the crate-root `cfg_attr` that does the same for tests.
    clippy::indexing_slicing,
    clippy::integer_division,
    clippy::cast_sign_loss
)]
#![cfg_attr(
    not(all(feature = "native-engines", burrow_native_engines)),
    allow(unused)
)]

#[cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
mod measure {
    use std::ffi::{CString, c_char, c_int, c_uchar, c_uint, c_ulonglong, c_void};
    use std::path::Path;
    use std::process::Command;

    type QpdfData = *mut c_void;

    // ONE `#[link]` PER BLOCK, AND THE ORDER OF THE BLOCKS IS THE LINK ORDER. A static archive
    // only resolves symbols against archives that come AFTER it, so `z` and `jpeg` are below
    // and the C++ runtime is last. Copied deliberately from `measure-merge`, where getting it
    // wrong left every `inflate`/`deflate` in libqpdf undefined.
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
            // `unsigned long long` (`qpdf-c.h:270`), which is what `qpdf/ffi.rs` declares.
            // Identical to `c_ulong` on LP64 Linux and this module is Linux-gated, so the
            // previous spelling was unreachable rather than wrong -- corrected because the
            // crate already has the right form to copy and a silent length truncation is the
            // bug class `core/CLAUDE.md` denies casts over.
            len: c_ulonglong,
            pw: *const c_char,
        ) -> c_int;
        fn qpdf_get_num_pages(q: QpdfData) -> c_int;
        fn qpdf_init_write_memory(q: QpdfData) -> c_int;
        fn qpdf_set_static_ID(q: QpdfData, v: c_int);
        fn qpdf_write(q: QpdfData) -> c_int;
        fn qpdf_get_buffer_length(q: QpdfData) -> usize;
        fn qpdf_get_buffer(q: QpdfData) -> *const c_uchar;
        fn qpdf_has_error(q: QpdfData) -> c_int;
        fn qpdf_get_error(q: QpdfData) -> *mut c_void;
        fn qpdf_set_logger(q: QpdfData, logger: *mut c_void);

        // THE LEVERS UNDER MEASUREMENT. None of these is declared in `qpdf/ffi.rs` today;
        // deciding which of them the crate should grow is the whole point of this harness.
        fn qpdf_set_object_stream_mode(q: QpdfData, mode: c_uint);
        fn qpdf_set_compress_streams(q: QpdfData, v: c_int);
        fn qpdf_set_decode_level(q: QpdfData, level: c_uint);
        fn qpdf_set_preserve_unreferenced_objects(q: QpdfData, v: c_int);
        fn qpdf_set_linearization(q: QpdfData, v: c_int);

        fn qpdflogger_create() -> *mut c_void;
        // FOUR PARAMETERS, not three: `(handle, dest, qpdf_log_fn_t fn, void* udata)`
        // (`qpdflogger-c.h:70-77`). The three-argument spelling this file used to carry --
        // inherited from `measure-merge` -- left the callee reading `udata` from an
        // uninitialised register. It never manifested, because `udata` is touched only in the
        // `qpdf_log_dest_custom` branch, and that is exactly why nothing caught it.
        // `core/burrow-engines/src/qpdf/ffi.rs` has the correct form; this now matches.
        fn qpdflogger_set_info(
            logger: *mut c_void,
            dest: c_int,
            fun: *const c_void,
            udata: *mut c_void,
        );
        fn qpdflogger_set_warn(
            logger: *mut c_void,
            dest: c_int,
            fun: *const c_void,
            udata: *mut c_void,
        );
        fn qpdflogger_set_error(
            logger: *mut c_void,
            dest: c_int,
            fun: *const c_void,
            udata: *mut c_void,
        );
    }

    #[link(name = "z", kind = "static")]
    unsafe extern "C" {}

    #[link(name = "jpeg", kind = "static")]
    unsafe extern "C" {}

    #[link(name = "stdc++")]
    unsafe extern "C" {}

    /// qpdf's status is a BITMASK, never a plain success/failure code.
    ///
    /// `QPDF_WARNINGS` is bit 0 and `QPDF_ERRORS` is bit 1 (`qpdf-c.h:137-139`). Testing
    /// `!= 0` counts a warnings-only read as a failure, and `measure-merge` learned that the
    /// expensive way — it reported qpdf refusing two files it does not refuse.
    const QPDF_ERRORS: c_int = 1 << 1;

    /// `qpdf_log_dest_discard`, which is **3** (`qpdflogger-c.h:58-63`).
    ///
    /// It was `0` here, with a comment calling 0 "qpdf's own Pl_Discard". 0 is
    /// `qpdf_log_dest_default`, which `set_log_dest` maps to `method(nullptr)` -- and
    /// `QPDFLogger::setInfo(nullptr)` selects **stdout**, `setError(nullptr)` selects
    /// **stderr**. So the third suppression layer was installing a logger behaviourally
    /// identical to the default one, i.e. no layer at all.
    ///
    /// `core/burrow-engines/src/codes/qpdf.rs` has always had `LOG_DEST_DISCARD = 3`, so
    /// production was correct and only the harnesses were wrong. Found by security review.
    const LOG_DEST_DISCARD: c_int = 3;

    // qpdf_object_stream_e / qpdf_stream_decode_level_e, from Constants.h.
    const QPDF_O_PRESERVE: c_uint = 1;
    const QPDF_O_GENERATE: c_uint = 2;
    const QPDF_DL_GENERALIZED: c_uint = 1;

    /// The qpdf WRITER's own defaults, read out of `QPDFWriter_private.hh:296-311`.
    ///
    /// This is the finding arm C exists to surface and it is worth stating in the code rather
    /// than only in the report: of the five levers the C API exposes, **four are already the
    /// default**, and `qpdf_write` with no configuration at all — which is what every burrow
    /// operation already calls — has them on.
    ///
    /// | lever | qpdf's default | is it a lever? |
    /// |---|---|---|
    /// | `compress_streams_` | `true` | no — already on |
    /// | `preserve_unreferenced_` | `false` | no — garbage is already collected |
    /// | `decode_level_` | `qpdf_dl_generalized` | no — already the useful level |
    /// | `linearize_` | `false` | no — hint streams are already dropped |
    /// | `object_streams_` | `qpdf_o_preserve` | **yes — the only one** |
    ///
    /// `recompress_flate_` is `false` and has no C API setter at all.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct Levers {
        pub object_streams: c_uint,
        pub compress_streams: bool,
        pub decode_level: c_uint,
        pub drop_unreferenced: bool,
        pub linearize: bool,
    }

    impl Levers {
        /// Arm C: qpdf's defaults, spelled out. What `rotate` already emits.
        pub const BASELINE: Self = Self {
            object_streams: QPDF_O_PRESERVE,
            compress_streams: true,
            decode_level: QPDF_DL_GENERALIZED,
            drop_unreferenced: true,
            linearize: false,
        };

        /// Arm A: every lever the C API exposes, set for smallest output.
        pub const FULL: Self = Self {
            object_streams: QPDF_O_GENERATE,
            ..Self::BASELINE
        };
    }

    unsafe fn drain(q: QpdfData) {
        // SAFETY: `q` is live; `qpdf_get_error` transfers ownership of the error object, which
        // the C API then forgets about. qpdf prints `WARNING: application did not handle
        // error` from `qpdf_cleanup` OUTSIDE the logger if the slot is left full.
        unsafe {
            while qpdf_has_error(q) != 0 {
                let _ = qpdf_get_error(q);
            }
        }
    }

    /// The one discarding logger, created once for the process.
    ///
    /// Created per `qpdf_data` until security review counted the cost: `rewrite` runs about
    /// eight times per file and the distribution arm walks 700, so a per-call logger leaked
    /// roughly 5,600 handles. `qpdf/limits.rs` shares one logger across every document for
    /// the same reason, and qpdf documents the object as reference-counted and shareable
    /// (`qpdflogger-c.h:38-52`).
    static LOGGER: std::sync::OnceLock<usize> = std::sync::OnceLock::new();

    unsafe fn silence(q: QpdfData) {
        // SAFETY: `q` is a live qpdf_data and none of these read the document. All three
        // layers are needed and all three are now actually installed -- see
        // `LOG_DEST_DISCARD`, which was wrong until security review. The logger pointer is
        // created once and only ever read afterwards, so sharing it is sound; it is stored as
        // a `usize` because a raw pointer is not `Sync`.
        unsafe {
            qpdf_silence_errors(q);
            qpdf_set_suppress_warnings(q, 1);
            let logger = *LOGGER.get_or_init(|| {
                let l = qpdflogger_create();
                let none = std::ptr::null();
                let no_data = std::ptr::null_mut();
                qpdflogger_set_info(l, LOG_DEST_DISCARD, none, no_data);
                qpdflogger_set_warn(l, LOG_DEST_DISCARD, none, no_data);
                qpdflogger_set_error(l, LOG_DEST_DISCARD, none, no_data);
                l as usize
            }) as *mut c_void;
            qpdf_set_logger(q, logger);
        }
    }

    /// Read `bytes`, write them back out under `levers`, and return the output.
    ///
    /// `levers: None` calls **no write-parameter setter at all** (bar `qpdf_set_static_ID`,
    /// which only makes two runs comparable). That is arm C as the report describes it --
    /// "`qpdf_write` with qpdf's own defaults", which is what every burrow operation emits --
    /// and it is what makes the inertness control mean something.
    ///
    /// Until security review, arm C was `Some(BASELINE)`: the setters called with qpdf's
    /// default values. The control then compared `rewrite(BASELINE)` against
    /// `rewrite(BASELINE)` -- the same call with the same argument -- so it proved
    /// `qpdf_write` is deterministic and could not have detected the one regression it exists
    /// to catch, a `qpdf_set_object_stream_mode` that had become a no-op. The two happen to be
    /// equivalent (`compress_streams_set_` and `decode_level_set_` are read only by `qdf()`
    /// and `pclm()`, neither of which is enabled), so the numbers were right -- but a control
    /// that cannot fail is not evidence for them.
    pub fn rewrite(bytes: &[u8], levers: Option<Levers>) -> Result<Vec<u8>, String> {
        // SAFETY: single-threaded harness. The handle is created, used and destroyed within
        // this function, and the output buffer is copied out before cleanup.
        unsafe {
            let mut q = qpdf_init();
            silence(q);

            let desc = CString::new("input").unwrap();
            let status = qpdf_read_memory(
                q,
                desc.as_ptr(),
                bytes.as_ptr().cast(),
                bytes.len() as c_ulonglong,
                std::ptr::null(),
            );
            if status & QPDF_ERRORS != 0 {
                drain(q);
                qpdf_cleanup(&raw mut q);
                return Err("read failed".to_owned());
            }

            if qpdf_init_write_memory(q) != 0 {
                drain(q);
                qpdf_cleanup(&raw mut q);
                return Err("init_write_memory failed".to_owned());
            }

            // AFTER `init_write_memory`, NEVER BEFORE. ADR 0017 records that calling a write
            // parameter first dereferences a writer that does not exist yet and takes the
            // process down — found by core dump, not by reading the header.
            qpdf_set_static_ID(q, 1);
            if let Some(levers) = levers {
                qpdf_set_object_stream_mode(q, levers.object_streams);
                qpdf_set_compress_streams(q, c_int::from(levers.compress_streams));
                qpdf_set_decode_level(q, levers.decode_level);
                qpdf_set_preserve_unreferenced_objects(q, c_int::from(!levers.drop_unreferenced));
                qpdf_set_linearization(q, c_int::from(levers.linearize));
            }

            // THE SAME BITMASK AS THE READ. `qpdf_write` returns warnings in bit 0 and errors
            // in bit 1, exactly as `qpdf_read_memory` does, and this line used to test `!= 0`
            // -- the mistake `QPDF_ERRORS`' own doc comment records `measure-merge` making.
            let wrote = qpdf_write(q);
            if wrote & QPDF_ERRORS != 0 {
                drain(q);
                qpdf_cleanup(&raw mut q);
                return Err("write failed".to_owned());
            }

            let len = qpdf_get_buffer_length(q);
            let buf = qpdf_get_buffer(q);
            let out = if buf.is_null() || len == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(buf, len).to_vec()
            };
            drain(q);
            qpdf_cleanup(&raw mut q);
            if out.is_empty() {
                return Err("empty output".to_owned());
            }
            Ok(out)
        }
    }

    pub fn page_count(bytes: &[u8]) -> Result<i32, String> {
        // SAFETY: as `rewrite`; nothing escapes the call.
        unsafe {
            let mut q = qpdf_init();
            silence(q);
            let desc = CString::new("input").unwrap();
            let status = qpdf_read_memory(
                q,
                desc.as_ptr(),
                bytes.as_ptr().cast(),
                bytes.len() as c_ulonglong,
                std::ptr::null(),
            );
            if status & QPDF_ERRORS != 0 {
                drain(q);
                qpdf_cleanup(&raw mut q);
                return Err("read failed".to_owned());
            }
            let n = qpdf_get_num_pages(q);
            drain(q);
            qpdf_cleanup(&raw mut q);
            Ok(n)
        }
    }

    /// Arm B: the pinned qpdf CLI, which reaches what the C API cannot.
    ///
    /// `--recompress-flate` and `--compression-level` exist only as
    /// `QPDFWriter::setRecompressFlate` and the static `Pl_Flate::setCompressionLevel`. Neither
    /// has a C API setter — `grep -c recompress qpdf-c.h` is 0 — so this arm measures a ceiling
    /// burrow cannot reach through the route ADR 0013 chose, not a candidate implementation.
    pub fn cli_recompress(qpdf_bin: &Path, input: &Path, out: &Path) -> Result<u64, String> {
        cli(qpdf_bin, input, out, true)
    }

    /// The same CLI invocation WITHOUT the two recompression flags.
    ///
    /// This is what makes the A-to-B gap reproducible from the committed tool rather than a
    /// number somebody took by hand. It also removes the ID confound the report claims to have
    /// removed: both sides pass `--deterministic-id`, so the only difference between them is
    /// the recompression.
    pub fn cli_plain(qpdf_bin: &Path, input: &Path, out: &Path) -> Result<u64, String> {
        cli(qpdf_bin, input, out, false)
    }

    fn cli(qpdf_bin: &Path, input: &Path, out: &Path, recompress: bool) -> Result<u64, String> {
        let mut args = vec!["--object-streams=generate", "--deterministic-id"];
        if recompress {
            args.push("--recompress-flate");
            args.push("--compression-level=9");
        }
        let status = Command::new(qpdf_bin)
            .args(args)
            .arg(input)
            .arg(out)
            .output()
            .map_err(|e| format!("cannot run {}: {e}", qpdf_bin.display()))?;
        // qpdf exits 3 on warnings, which every damaged file produces and which is not a
        // failure. The same bitmask lesson as `QPDF_ERRORS`, in the CLI's spelling.
        if !matches!(status.status.code(), Some(0 | 3)) {
            return Err(format!("qpdf exited {:?}", status.status.code()));
        }
        Ok(std::fs::metadata(out).map_err(|e| e.to_string())?.len())
    }
}

#[cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
fn main() -> std::process::ExitCode {
    use measure::{Levers, cli_plain, cli_recompress, page_count, rewrite};
    use std::path::{Path, PathBuf};
    use std::process::ExitCode;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(dir) = args.first() else {
        eprintln!(
            "usage: measure-compress <fixture-dir> [--distribution <pdf-dir>]\n\
             \n\
             Generate the fixture directory with:\n\
             \x20 python3 tools/make-compress-fixtures.py <fixture-dir>"
        );
        return ExitCode::from(2);
    };
    let dir = PathBuf::from(dir);

    // The PINNED CLI, built from the pinned source, not whatever `qpdf` is on PATH. A
    // measurement whose provenance is "some qpdf" is not a measurement of our qpdf.
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    // DERIVED, not hard-coded. `engines/build-native.sh` names this directory from `uname -m`,
    // so a hard-coded `aarch64` made arm B silently report `-` on an x86_64 machine -- a whole
    // arm quietly examining nothing, which is the class `CLAUDE.md` calls out.
    let arch = std::env::consts::ARCH;
    let qpdf_bin = repo.join(format!(
        "engines/vendor/src/build-qpdf-plain-{arch}/qpdf/qpdf"
    ));
    let have_cli = qpdf_bin.exists();
    if !have_cli {
        eprintln!(
            "note: {} is absent, so arm B is not measured and is reported as such.",
            qpdf_bin.display()
        );
    }

    let scratch = std::env::temp_dir().join("burrow-measure-compress");
    std::fs::create_dir_all(&scratch).unwrap();

    // ------------------------------------------------------------------ the fixture table

    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "pdf"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no .pdf files in {}", dir.display());

    println!("# Arms, per fixture");
    println!();
    println!(
        "{:<22} {:>10} {:>10} {:>8} {:>10} {:>8} {:>10} {:>10} {:>9}  pages",
        "fixture", "input", "C", "C%", "A", "A%", "B-plain", "B-recomp", "recomp%"
    );

    let mut any_lever_saving = false;
    let mut inertness_compared = 0usize;
    let mut inertness_failures = Vec::new();
    let mut rows = Vec::new();

    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let bytes = std::fs::read(path).unwrap();
        let input = bytes.len() as u64;

        // ARM C: no write-parameter setter is called at all. What `rotate` already emits.
        let baseline = match rewrite(&bytes, None) {
            Ok(v) => v,
            Err(e) => {
                println!("{name:<22}  SKIPPED: {e}");
                continue;
            }
        };
        let full = rewrite(&bytes, Some(Levers::FULL)).expect("arm A failed on a readable file");

        // THE INERTNESS CONTROL. Calling all five setters with qpdf's OWN DEFAULT values must
        // produce byte-identical output to calling none of them. If it does not, then setting
        // a lever to its default is not the same as leaving it alone, and every A-over-C
        // number below is measuring something other than the levers.
        let inert = rewrite(&bytes, Some(Levers::BASELINE)).expect("control run failed");
        inertness_compared += 1;
        if inert != baseline {
            inertness_failures.push(name.clone());
        }
        if full.len() < baseline.len() {
            any_lever_saving = true;
        }

        let pages = page_count(&bytes).unwrap_or(-1);
        let (b_plain, b_len) = if have_cli {
            (
                cli_plain(&qpdf_bin, path, &scratch.join(format!("plain-{name}"))).ok(),
                cli_recompress(&qpdf_bin, path, &scratch.join(&name)).ok(),
            )
        } else {
            (None, None)
        };

        let pct = |out: u64| 100.0 - (out as f64 * 100.0 / input as f64);
        // WHAT RECOMPRESSION IS WORTH, against the arm-A output rather than against the input.
        // That is the denominator the question uses -- "how much smaller again" -- and the
        // report quoted it with a lost decimal place until code review caught it.
        let recomp = match (b_plain, b_len) {
            (Some(plain), Some(rec)) if plain > 0 => {
                format!(
                    "{:+.2}%",
                    (plain as f64 - rec as f64) * 100.0 / plain as f64
                )
            }
            _ => "-".to_owned(),
        };
        println!(
            "{:<22} {:>10} {:>10} {:>7.1}% {:>10} {:>7.1}% {:>10} {:>10} {:>9}  {}",
            name,
            input,
            baseline.len(),
            pct(baseline.len() as u64),
            full.len(),
            pct(full.len() as u64),
            b_plain.map_or_else(|| "-".to_owned(), |n| n.to_string()),
            b_len.map_or_else(|| "-".to_owned(), |n| n.to_string()),
            recomp,
            pages
        );
        rows.push((name, input, baseline.len() as u64, full.len() as u64, b_len));
    }

    // ------------------------------------------------------------------ per-lever attribution

    println!();
    println!("# Per-lever attribution: bytes ADDED when the lever is turned off, from full arm A");
    println!();
    println!(
        "{:<22} {:>14} {:>14} {:>14} {:>14} {:>14}",
        "fixture", "objstreams", "compress", "decode<gen", "keep unref", "linearize ON"
    );
    println!(
        "  (the first four are bytes added when the lever is turned OFF. `linearize` is \
         already off in arm A,\n   so its column is the opposite direction: bytes added when \
         linearization is turned ON.)"
    );

    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let bytes = std::fs::read(path).unwrap();
        let Ok(full) = rewrite(&bytes, Some(Levers::FULL)) else {
            continue;
        };
        let base = full.len() as i64;

        // MEASURED BY DISABLING, not by enabling alone. Turning one lever on against the
        // baseline says what it is worth in isolation; turning one off from the full set says
        // what it is worth in the configuration that would ship — and it is the direction that
        // reports a lever which has silently stopped being applied.
        let off = [
            Levers {
                object_streams: 1,
                ..Levers::FULL
            },
            Levers {
                compress_streams: false,
                ..Levers::FULL
            },
            Levers {
                decode_level: 0,
                ..Levers::FULL
            },
            Levers {
                drop_unreferenced: false,
                ..Levers::FULL
            },
            Levers {
                linearize: true,
                ..Levers::FULL
            },
        ];
        let deltas: Vec<String> = off
            .iter()
            .map(|l| {
                rewrite(&bytes, Some(*l))
                    .map(|v| format!("{:+}", v.len() as i64 - base))
                    .unwrap_or_else(|_| "err".to_owned())
            })
            .collect();
        println!(
            "{:<22} {:>14} {:>14} {:>14} {:>14} {:>14}",
            name, deltas[0], deltas[1], deltas[2], deltas[3], deltas[4]
        );
    }

    // ------------------------------------------------------------------ the distribution arm

    if let Some(pos) = args.iter().position(|a| a == "--distribution") {
        let corpus = PathBuf::from(args.get(pos + 1).expect("--distribution needs a directory"));
        let mut savings: Vec<f64> = Vec::new();
        let mut buckets: [Vec<f64>; 4] = [vec![], vec![], vec![], vec![]];
        let mut worse_sizes: Vec<u64> = Vec::new();
        let mut worse = 0usize;
        let mut identical = 0usize;
        let mut under_one = 0usize;
        let mut under_one_negative = 0usize;
        // THREE CAUSES, THREE COUNTERS. One counter called `unreadable` let the report state
        // that the rejected files were "deliberately damaged", which is a claim about cause
        // that a single counter cannot support -- and it would have hidden the one finding
        // that would actually matter here: a file arm C writes and arm A cannot.
        let mut could_not_read = 0usize;
        let mut arm_c_failed = 0usize;
        let mut arm_a_failed = 0usize;
        let mut examined = 0usize;

        let mut walk = vec![corpus.clone()];
        let mut pdfs = Vec::new();
        while let Some(d) = walk.pop() {
            let Ok(entries) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk.push(p);
                } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pdf")) {
                    pdfs.push(p);
                }
            }
        }
        pdfs.sort();
        let listed = pdfs.len();

        for p in &pdfs {
            let Ok(bytes) = std::fs::read(p) else {
                could_not_read += 1;
                continue;
            };
            if bytes.is_empty() {
                could_not_read += 1;
                continue;
            }
            let Ok(baseline) = rewrite(&bytes, None) else {
                arm_c_failed += 1;
                continue;
            };
            let Ok(full) = rewrite(&bytes, Some(Levers::FULL)) else {
                // LOUD, not folded into the input's fault. A file arm C writes and arm A does
                // not is a finding about object-stream generation, not about a damaged input.
                arm_a_failed += 1;
                continue;
            };
            examined += 1;
            // AGAINST ARM C, not against the input. The question this arm answers is what
            // `compress` adds over what every other operation already emits.
            let saved = 100.0 - (full.len() as f64 * 100.0 / baseline.len() as f64);
            // STRICTLY LARGER, and ties counted separately. `>=` here made a file arm A left
            // at EXACTLY arm C's size read as one it had grown, and two thirds of the reported
            // "made larger" population turned out to be ties -- including the 2.4 MB file the
            // report named as its largest regression, which both arms write identically.
            // `>=` remains the right predicate for the never-worse GUARD; it was never the
            // right one for this label.
            if full.len() > baseline.len() {
                worse += 1;
                worse_sizes.push(bytes.len() as u64);
            } else if full.len() == baseline.len() {
                identical += 1;
            }
            if saved < 1.0 {
                under_one += 1;
                if saved < 0.0 {
                    under_one_negative += 1;
                }
            }
            // BUCKETED BY INPUT SIZE, because "9% of files got larger" and "9% of files got
            // larger and every one of them was under 2 kB" are different findings and only the
            // second one tells a person whether it will happen to them. An object stream has a
            // fixed overhead -- its own dictionary, its offset table, its xref stream -- so the
            // prior is that it loses on small files, and a prior is not a measurement.
            let bucket = match bytes.len() {
                0..=2_047 => 0,
                2_048..=16_383 => 1,
                16_384..=131_071 => 2,
                _ => 3,
            };
            buckets[bucket].push(saved);
            savings.push(saved);
        }

        savings.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let at = |q: f64| {
            savings
                .get(((savings.len() as f64 - 1.0) * q) as usize)
                .copied()
                .unwrap_or(f64::NAN)
        };
        println!();
        println!("# Distribution over {}", corpus.display());
        println!();
        // REPORTS WHAT IT EXAMINED, and against what it expected to examine. A bare "OK" over
        // a count nobody can check is the `4 of 15` failure; `listed` is derivable, so it is
        // printed beside `examined` rather than left to be inferred.
        println!("  .pdf files found        {listed}");
        println!("  examined                {examined}");
        println!("  could not read          {could_not_read}");
        println!("  arm C (defaults) failed {arm_c_failed}");
        println!("  arm A failed where C succeeded  {arm_a_failed}   <-- non-zero is a FINDING");
        println!();
        println!(
            "  arm A saving over arm C, percent (p50 is the upper element, not interpolated):"
        );
        println!("    min                   {:.1}", at(0.0));
        println!("    p25                   {:.1}", at(0.25));
        println!("    median                {:.1}", at(0.50));
        println!("    p75                   {:.1}", at(0.75));
        println!("    max                   {:.1}", at(1.0));
        println!("  files arm A made strictly LARGER      {worse}");
        println!("  files arm A left EXACTLY unchanged    {identical}");
        println!(
            "  files saving under 1%                {under_one}   (of which negative: \
             {under_one_negative})"
        );

        worse_sizes.sort_unstable();
        if !worse_sizes.is_empty() {
            println!();
            println!("  the files arm A made STRICTLY larger, by input size:");
            println!("    smallest              {} bytes", worse_sizes[0]);
            println!(
                "    median                {} bytes",
                worse_sizes[worse_sizes.len() / 2]
            );
            println!(
                "    largest               {} bytes",
                worse_sizes[worse_sizes.len() - 1]
            );
            println!(
                "    under 16 kB           {} of {worse}",
                worse_sizes.iter().filter(|n| **n < 16_384).count()
            );
        }

        println!();
        println!("  median saving by input size:");
        for (label, b) in [
            ("under 2 kB", &buckets[0]),
            ("2 kB - 16 kB", &buckets[1]),
            ("16 kB - 128 kB", &buckets[2]),
            ("over 128 kB", &buckets[3]),
        ] {
            let mut v = b.clone();
            v.sort_by(|a, c| a.partial_cmp(c).unwrap());
            if v.is_empty() {
                println!("    {label:<20}  no files");
            } else {
                println!(
                    "    {label:<20} {:>6.1}%   (n = {}, strictly larger: {}, unchanged: {})",
                    v[v.len() / 2],
                    v.len(),
                    v.iter().filter(|x| **x < 0.0).count(),
                    v.iter().filter(|x| **x == 0.0).count()
                );
            }
        }
    }

    // ------------------------------------------------------------------ the controls' verdict

    println!();
    println!("# Controls");
    println!();
    if inertness_failures.is_empty() {
        // THE COUNT IT ACTUALLY COMPARED, not the count it listed. `files.len()` includes any
        // fixture skipped because arm C failed on it, so the message would have claimed
        // coverage over a file it never examined -- the "4 of 15" shape, small.
        println!(
            "  inertness: PASS — calling all five setters with qpdf's own defaults is \
             byte-identical to calling none of them, on {inertness_compared} of {} fixtures \
             compared, so the A-over-C column is the levers and nothing else.",
            files.len()
        );
    } else {
        println!(
            "  inertness: FAIL on {} fixture(s): {}. The A-over-C column is NOT attributable \
             to the levers.",
            inertness_failures.len(),
            inertness_failures.join(", ")
        );
    }
    if any_lever_saving {
        println!("  non-vacuity: PASS — at least one fixture saves bytes under arm A.");
    } else {
        println!(
            "  non-vacuity: FAIL — no fixture saved a byte under arm A. Either the levers are \
             not being applied, or every fixture is already optimal; both need looking at \
             before any number here is quoted."
        );
    }

    if inertness_failures.is_empty() && any_lever_saving {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[cfg(not(all(feature = "native-engines", burrow_native_engines, target_os = "linux")))]
fn main() {
    eprintln!(
        "measure-compress needs the native engines. Build them and run with \
         --features native-engines:\n\
         \x20 engines/fetch.sh && engines/build-native.sh\n\
         \x20 cargo run -p burrow-engines --features native-engines --release \\\n\
         \x20   --example measure-compress -- <fixture-dir>"
    );
}

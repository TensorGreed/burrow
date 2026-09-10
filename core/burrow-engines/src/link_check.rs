//! Proof that the vendored native engines actually link and run.
//!
//! Compiled only under the `native-engines` feature on Linux. This is the whole of the
//! FFI surface in M1 PR 1 — the real `DocumentEngine` implementation is PR 2. It exists
//! to answer three questions that nothing else can:
//!
//! 1. Does `libpdfium.so` link and initialise?
//! 2. Does `libqpdf.a` link, and is it the version we pinned?
//! 3. **Are the bytes the loader actually mapped the pinned bytes?** The rpath is
//!    supposed to guarantee this; an assertion is worth more than an intention. The
//!    check is by **sha256 of the mapped file**, not by path: `.cargo/config.toml` adds
//!    an `$ORIGIN` rpath and `build.rs` places a verified copy beside the test binaries
//!    (because cargo does not propagate rpaths to dependent crates), so the loader may
//!    legitimately map a copy. Hashing proves it is the right library wherever it sits,
//!    which is the guarantee we actually want.

use std::ffi::{CStr, c_char, c_ulong};
use std::sync::Once;
use std::sync::atomic::{AtomicU64, Ordering};

// PDFium's public C API. Declared here rather than bindgen-generated: three functions do
// not justify a code generator, and hand-written declarations are auditable.
//
// Signatures checked by hand against
// `engines/vendor/native-*/include/pdfium/fpdfview.h` at lines 330, 346 and 627. Nothing
// verifies them automatically, and a typo here is undefined behaviour that no test would
// catch -- so re-check these against the header on any PDFium bump.
unsafe extern "C" {
    /// `FPDF_EXPORT void FPDF_CALLCONV FPDF_InitLibrary()`
    fn FPDF_InitLibrary();
    /// `FPDF_EXPORT void FPDF_CALLCONV FPDF_DestroyLibrary()`
    fn FPDF_DestroyLibrary();
    /// `FPDF_EXPORT unsigned long FPDF_CALLCONV FPDF_GetLastError()`
    fn FPDF_GetLastError() -> c_ulong;
}

// qpdf's C API, from `include/qpdf/qpdf-c.h`. Using the C API rather than the C++ one
// keeps this declaration free of name mangling and ABI questions.
unsafe extern "C" {
    /// `char const* qpdf_get_qpdf_version()`
    fn qpdf_get_qpdf_version() -> *const c_char;
}

/// Initialise PDFium exactly once per process, and return its error state.
///
/// # Invariant this function does not discharge on its own
///
/// `Once` guarantees *this* function initialises at most once. It does **not** stop
/// anything else in the process calling `FPDF_InitLibrary`, and PR 2 adds another
/// caller. The soundness obligation is therefore process-wide: **this must remain the
/// only path that calls `FPDF_InitLibrary`.** Treat that as a live requirement, not as
/// something already satisfied.
///
/// # PDFium's library init is global, and calling it concurrently aborts
///
/// This is not a stylistic preference. `FPDF_InitLibrary` mutates global state and is
/// not re-entrant: two threads calling it at once trip an internal `CHECK` and the
/// process dies with `SIGTRAP` — which is exactly what happened when two tests here
/// each called it and the harness ran them on separate threads.
///
/// So it is guarded by a [`Once`]. **This constraint carries into the real engine
/// implementation (M1 PR 2): init belongs at process scope, once, never per-operation,
/// and `FPDF_DestroyLibrary` must not be called while any other thread might still be
/// inside PDFium.**
///
/// Returns `FPDF_GetLastError()` sampled immediately after initialisation, which should
/// be `FPDF_ERR_SUCCESS` (0).
#[must_use]
pub fn pdfium_ensure_init() -> u64 {
    static INIT: Once = Once::new();
    static LAST_ERROR: AtomicU64 = AtomicU64::new(u64::MAX);

    INIT.call_once(|| {
        // SAFETY: `Once` guarantees this body runs exactly once per process and that no
        // other thread proceeds past `call_once` until it has finished, so the
        // non-re-entrant global init is not raced. `FPDF_InitLibrary` takes no arguments
        // and `FPDF_GetLastError` only reads a global the library just set; no pointer
        // crosses the boundary, so there is nothing to alias or free.
        //
        // `FPDF_DestroyLibrary` is deliberately NOT called. Tearing the library down
        // while another thread could still be using it is unsound, and there is no safe
        // point to do it in a multi-threaded test binary. The process exit reclaims
        // everything.
        let err: c_ulong = unsafe {
            FPDF_InitLibrary();
            FPDF_GetLastError()
        };
        LAST_ERROR.store(err as u64, Ordering::SeqCst);
    });

    LAST_ERROR.load(Ordering::SeqCst)
}

/// Resolve `FPDF_DestroyLibrary` without calling it.
///
/// Taking the function pointer forces the linker to bind the symbol, which is what we
/// want to prove — while actually calling it would risk tearing PDFium down under
/// another thread. See [`pdfium_ensure_init`].
pub fn pdfium_destroy_symbol_resolves() {
    let f: unsafe extern "C" fn() = FPDF_DestroyLibrary;
    // `black_box` rather than a comparison: LLVM knows a function pointer is non-null, so
    // `(f as usize) != 0` folds to `true` and the reference becomes elidable -- the test
    // would still pass while proving nothing. This also avoids the pointer-to-integer
    // cast that the workspace lints deny as a class.
    std::hint::black_box(f);
}

/// The version string reported by the linked qpdf.
///
/// # Errors
///
/// Returns [`Error::Internal`](burrow_types::Error::Internal) if qpdf returns a null
/// pointer, or a string that is not valid UTF-8.
pub fn qpdf_version() -> burrow_types::Result<String> {
    // SAFETY: qpdf_get_qpdf_version returns a pointer to a static, NUL-terminated string
    // owned by libqpdf. It is never freed by us, lives for the program's lifetime, and is
    // not mutated, so building a CStr borrow from it is sound. The null check below is
    // belt and braces — the documented contract is that it never returns null.
    let ptr = unsafe { qpdf_get_qpdf_version() };
    if ptr.is_null() {
        return Err(burrow_types::Error::Internal(
            "qpdf_get_qpdf_version returned null".to_owned(),
        ));
    }
    // SAFETY: `ptr` is non-null (checked above) and points at a NUL-terminated static
    // string, so it is a valid argument to `CStr::from_ptr`.
    let cstr = unsafe { CStr::from_ptr(ptr) };
    cstr.to_str().map(str::to_owned).map_err(|e| {
        burrow_types::Error::Internal(format!("qpdf version string is not UTF-8: {e}"))
    })
}

/// Absolute path of the `libpdfium.so` that `build.rs` verified.
///
/// Baked in at compile time. Note the loader may map a byte-identical copy from beside
/// the test binary instead — see [`pinned_pdfium_sha256`].
#[must_use]
pub const fn pinned_pdfium_path() -> &'static str {
    env!("BURROW_PDFIUM_SO")
}

/// The qpdf version pinned in `engines/pins.toml`.
///
/// Read from the pin at build time so the assertion cannot drift from what was actually
/// fetched and built.
#[must_use]
pub const fn pinned_qpdf_version() -> &'static str {
    env!("BURROW_QPDF_VERSION")
}

/// The sha256 `build.rs` verified `libpdfium.so` against, from `engines/pins.toml`.
///
/// This, not a path, is the provenance guarantee: it identifies the library by content.
#[must_use]
pub const fn pinned_pdfium_sha256() -> &'static str {
    env!("BURROW_PDFIUM_SHA256")
}

/// Paths of every mapped file whose name contains `needle`, read from `/proc/self/maps`.
///
/// Linux-only, which is fine: this module only compiles on Linux.
///
/// Entries marked `(deleted)` are skipped: their recorded path resolves to whatever is
/// at that name now, which is not what was mapped, so hashing it would prove nothing.
///
/// Known limit, out of scope per `SECURITY.md` because it needs local write access: this
/// identifies the file by path, so a library swapped before load and restored before the
/// check would pass. Hashing `/proc/self/map_files/<range>` would pin the actual inode.
///
/// # Errors
///
/// Returns [`Error::Io`](burrow_types::Error::Io) if `/proc/self/maps` cannot be read.
pub fn mapped_files_containing(needle: &str) -> burrow_types::Result<Vec<String>> {
    let maps = std::fs::read_to_string("/proc/self/maps")
        .map_err(|e| burrow_types::Error::Io(format!("/proc/self/maps: {e}")))?;
    let mut out: Vec<String> = maps
        .lines()
        .filter_map(|line| {
            // Format: addr perms offset dev inode  [pathname]
            //
            // Take the REMAINDER of the line after the fifth field, not the sixth
            // whitespace-delimited token: a pathname may contain spaces, and splitting
            // on whitespace would truncate `/home/u/my repo/...` to `/home/u/my` and
            // silently drop the entry.
            let mut fields = line.split_whitespace();
            let inode = fields.nth(4)?;
            let idx = line.rfind(inode)? + inode.len();
            let path = line.get(idx..)?.trim();
            if path.is_empty() || !path.contains(needle) {
                return None;
            }
            // A `(deleted)` entry parses back to the original path, so hashing it would
            // hash bytes that are NOT the ones mapped. Refuse rather than mislead.
            if path.ends_with("(deleted)") {
                return None;
            }
            Some(path.to_owned())
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdfium_links_and_initialises_cleanly() {
        // FPDF_ERR_SUCCESS == 0. A non-zero value here would mean the library loaded but
        // its own init reported a problem.
        assert_eq!(
            pdfium_ensure_init(),
            0,
            "FPDF_GetLastError should be FPDF_ERR_SUCCESS after a successful init"
        );
    }

    #[test]
    fn pdfium_teardown_symbol_links_even_though_we_never_call_it() {
        // Compiles and links, or the binary would not have been produced at all.
        pdfium_destroy_symbol_resolves();
    }

    #[test]
    fn qpdf_links_and_reports_the_pinned_version() {
        let v = qpdf_version().expect("qpdf should report a version");
        assert_eq!(
            v,
            pinned_qpdf_version(),
            "linked qpdf is not the version pinned in engines/pins.toml"
        );
    }

    /// The provenance test.
    ///
    /// Linking is not enough: the loader could satisfy `libpdfium.so` from `/usr/lib`
    /// and every other test here would still pass. This asserts the **content** of what
    /// was mapped, because a byte-identical copy beside the binary is a legitimate
    /// source (see the module docs) but a system library is not.
    #[test]
    fn the_loaded_pdfium_is_the_pinned_library() {
        use sha2::{Digest, Sha256};

        // Force the library to be mapped before inspecting the map.
        let _ = pdfium_ensure_init();

        let mapped = mapped_files_containing("libpdfium")
            .expect("/proc/self/maps should be readable on Linux");
        assert!(
            !mapped.is_empty(),
            "libpdfium.so is not mapped at all, yet its functions linked and ran"
        );

        let expected = pinned_pdfium_sha256();
        for path in &mapped {
            let bytes = std::fs::read(path)
                .unwrap_or_else(|e| panic!("cannot read the mapped library {path}: {e}"));
            let got = Sha256::digest(&bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>();
            assert_eq!(
                got,
                expected,
                "the loader mapped a libpdfium that is NOT the pinned library.\n  \
                 mapped: {path}\n  pinned: {}",
                pinned_pdfium_path()
            );
        }
    }
}

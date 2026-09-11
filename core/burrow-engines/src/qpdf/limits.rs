//! qpdf's process-global resource limits, and the discarding logger.
//!
//! # Why these are set at all
//!
//! qpdf 12.3 added `qpdf_global_set_uint32`, and **every decompression memory limit it
//! offers defaults to zero, meaning unlimited** (`global.hh:372-545`). So does the
//! warning cap. A stock qpdf will happily inflate a decompression bomb until the
//! allocator gives up, and an allocator giving up inside C++ is an `abort()` — not a
//! panic, not catchable, and fatal to the whole process.
//!
//! Setting them is the single most valuable thing this module does.
//!
//! # Why they are constants rather than derived from `Limits`
//!
//! This is the awkward part and it is worth stating plainly rather than hiding.
//! [`burrow_types::Limits`] is **per-operation**; these parameters are **process-global**
//! and take no `qpdf_data`. There is no way to give one caller a 16 MiB ceiling and
//! another 1 GiB at the same time in one process.
//!
//! So the two do different jobs: the caller's `Limits` are enforced in Rust, per
//! operation, and these are a fixed floor under everything — chosen to be far above any
//! legitimate document and far below anything that would exhaust a machine. A caller who
//! sets a *tighter* `max_memory_bytes` still gets it, because that check happens in Rust.
//! A caller who sets a looser one does not get to raise these. That asymmetry is
//! deliberate; ADR 0013 records it.
//!
//! # Ordering
//!
//! Set once, before any `qpdf_data` exists, behind a [`OnceLock`]. Some parameters are
//! documented as one-way or as needing to precede the first object, and all of them are
//! shared mutable global state — so a single initialisation point is the only sane
//! discipline.

use core::ffi::{c_int, c_uint};
use std::sync::OnceLock;

use super::ffi;

// The limit VALUES and the parameter ids live in `crate::codes::qpdf::policy`, ungated, so
// the web path applies exactly the same policy rather than a second copy of it -- or, as
// was the case before M1 PR 4a-i, none at all.
use crate::codes::qpdf::policy;

/// The logger every `qpdf_data` is given. Created once; qpdf shares it internally.
static LOGGER: OnceLock<LoggerHandle> = OnceLock::new();

/// A `qpdflogger_handle` that is safe to share.
///
/// The handle is a pointer, so it is not `Send`/`Sync` by default. It is safe to share
/// here because after [`install`] nothing ever mutates it — the three `qpdflogger_set_*`
/// calls happen once, inside `OnceLock::get_or_init`, before any other thread can observe
/// it — and `qpdf_set_logger` only reads it. `qpdflogger-c.h:49-52` documents the
/// underlying object as shared and reference-counted, which is what makes handing the same
/// handle to many documents correct.
struct LoggerHandle(ffi::QpdfLoggerHandle);

// SAFETY: the pointer is written exactly once during `OnceLock` initialisation and is
// read-only afterwards, so there is no data race. The object it refers to is
// reference-counted inside qpdf and documented as shareable across the handles that name
// it (`qpdflogger-c.h:38-52`). This is the one `unsafe impl` in the qpdf module, and it
// exists because a `*mut c_void` cannot express "immutable after construction".
unsafe impl Send for LoggerHandle {}
// SAFETY: as above -- shared references only ever read the pointer.
unsafe impl Sync for LoggerHandle {}

/// Apply the global limits and build the discarding logger. Runs at most once.
///
/// Returns the logger to attach to each `qpdf_data`.
pub(super) fn install() -> ffi::QpdfLoggerHandle {
    LOGGER
        .get_or_init(|| {
            apply_global_limits();
            LoggerHandle(make_discarding_logger())
        })
        .0
}

/// Set every global limit we care about.
///
/// Failures are deliberately ignored. `qpdf_global_set_uint32` rejects a parameter it does
/// not recognise, which is what a future qpdf removing one looks like — and refusing to
/// open any document because a hardening knob moved would be a worse outcome than opening
/// it with qpdf's own defaults. The knobs that matter are verified by a test instead, so a
/// silent regression here is caught at build time rather than at runtime.
fn apply_global_limits() {
    for (param, value) in limit_settings() {
        // SAFETY: `qpdf_global_set_uint32` takes two integers, touches no pointer, and is
        // documented as process-global with no `qpdf_data` (`qpdf-c.h:1048`). It is called
        // here before any `qpdf_data` exists, from inside `OnceLock::get_or_init`, so no
        // other thread is inside qpdf concurrently.
        let _ = unsafe { ffi::qpdf_global_set_uint32(param, value) };
    }
}

/// Every global parameter this crate sets, and what to.
///
/// Delegates to the shared policy, so the native and web paths cannot drift apart.
fn limit_settings() -> [(c_int, c_uint); 7] {
    policy::settings()
}

/// Create a logger whose info, warning and error streams all go nowhere.
///
/// This is item 7. qpdf's default logger writes warnings to the process's stderr —
/// *including for files that parse successfully* — and those warnings quote object
/// numbers and byte offsets from the file (spike 0001, Finding 6). On the web that lands
/// in the devtools console.
///
/// `qpdf_log_dest_discard` is qpdf's own `Pl_Discard` pipeline, reachable by name from C,
/// so this needs no callback and therefore no Rust code running on a C++ stack.
fn make_discarding_logger() -> ffi::QpdfLoggerHandle {
    // SAFETY: `qpdflogger_create` takes no arguments and returns an owned handle. The
    // three setters take that handle, an enum value, and a null function pointer with null
    // user data -- which is the documented way to select a built-in destination
    // (`qpdflogger-c.h:70-77`); no callback is registered, so no Rust code is invoked from
    // C++. The handle is never freed: it lives for the process, is shared by every
    // document, and `qpdflogger_cleanup` would only drop this handle rather than the
    // underlying object anyway.
    unsafe {
        let logger = ffi::qpdflogger_create();
        if logger.is_null() {
            return logger;
        }
        let discard = policy::LOG_DEST_DISCARD;
        let none = core::ptr::null();
        let no_data = core::ptr::null_mut();
        ffi::qpdflogger_set_info(logger, discard, none, no_data);
        ffi::qpdflogger_set_warn(logger, discard, none, no_data);
        ffi::qpdflogger_set_error(logger, discard, none, no_data);
        logger
    }
}

/// Turn on qpdf's fuzzing mode, process-wide. **Fuzz targets only.**
///
/// Behind the `fuzzing` feature, which only `fuzz/Cargo.toml` enables, so a production
/// build has no way to call this.
///
/// `global.hh:98-133`: this "sets various global limits with the aim of avoiding spurious
/// time-outs and out-of-memory errors" — limits that, in upstream's words, "cannot be
/// imposed on qpdf during normal operation since legitimate PDF files can be very large
/// and complex".
///
/// So it makes a fuzzer's findings meaningful and would make production reject real
/// documents. It is one-way: qpdf offers no means of turning it off again, which is
/// exactly why this is a separate public function rather than a flag on a builder. A
/// process that calls it has decided to be a fuzzer for the rest of its life.
#[cfg(feature = "fuzzing")]
pub fn enable_fuzz_mode() {
    // Ensure the ordinary limits and the discarding logger are in place first, so a fuzz
    // process is quiet for the same reasons production is.
    let _ = install();
    // SAFETY: as `apply_global_limits` -- two integers, no pointer, process-global.
    let _ = unsafe { ffi::qpdf_global_set_uint32(policy::FUZZ_MODE, 1) };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The settings are the point of this module, so assert the list rather than trusting
    /// that someone kept it in step with the docs above.
    #[test]
    fn every_filter_family_gets_a_memory_ceiling() {
        let settings = limit_settings();
        for param in [
            policy::FLATE_MAX_MEMORY,
            policy::DCT_MAX_MEMORY,
            policy::PNG_MAX_MEMORY,
            policy::RUN_LENGTH_MAX_MEMORY,
            policy::TIFF_MAX_MEMORY,
        ] {
            let found = settings.iter().find(|(p, _)| *p == param);
            let (_, value) = found.unwrap_or_else(|| {
                panic!("filter family {param:#x} has no ceiling, so it defaults to unlimited")
            });
            assert!(*value > 0, "a ceiling of zero means unlimited, not none");
        }
    }

    #[test]
    fn nesting_and_warnings_are_bounded() {
        let settings = limit_settings();
        for param in [policy::P_PARSER_MAX_NESTING, policy::P_DOC_MAX_WARNINGS] {
            let (_, value) = settings
                .iter()
                .find(|(p, _)| *p == param)
                .unwrap_or_else(|| panic!("{param:#x} is unset, so it defaults to unlimited"));
            assert!(*value > 0);
        }
    }

    #[test]
    fn installing_is_idempotent_and_yields_a_usable_logger() {
        let first = install();
        let second = install();
        assert!(!first.is_null(), "the discarding logger should be created");
        assert_eq!(first, second, "install must not build a second logger");
    }

    /// The read-only parameter must stay out of the setter list.
    ///
    /// Setting it always fails (`global.cc` has no case for it), so a future edit adding
    /// it back would be a silent no-op with a comment claiming otherwise -- which is
    /// exactly what happened once.
    #[test]
    fn the_read_only_limit_counter_is_never_set() {
        assert!(
            !limit_settings()
                .iter()
                .any(|(p, _)| *p == policy::LIMIT_ERRORS),
            "qpdf_p_limit_errors is read-only; setting it cannot work"
        );
    }
}

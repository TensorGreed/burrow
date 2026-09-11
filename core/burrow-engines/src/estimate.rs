//! The two memory checks, and what each one is actually worth.
//!
//! # `max_memory_bytes` cannot be measured from Rust
//!
//! The allocations that dominate are made by C++ inside PDFium through its own allocator.
//! Rust's allocator never sees them, so no tracking allocator or RAII guard here can
//! observe — let alone cap — them. A tracking allocator would report a small number while
//! the process used gigabytes, which is worse than reporting nothing.
//!
//! So [ADR 0007] specifies an **estimate-based pre-check** on native, and this module
//! implements it in two halves, because one half on its own is not honest.
//!
//! ## Half one: the size-based pre-check, before the engine sees anything
//!
//! [`check_open_memory`] predicts cost from the input's **length**. It runs before the
//! buffer is handed over, so it is the only check that can prevent an allocation rather
//! than react to one.
//!
//! **Its limit is severe and must not be overstated.** It is a function of the byte count
//! and nothing else, so it is blind to everything a PDF *declares*. A 330 KB file whose
//! cross-reference stream declares twenty million entries drives PDFium to allocate about
//! 1.2 GB — roughly 3,900 times this estimate — and no tuning of the constants below would
//! catch it, because the declared size is simply not present in the input's length.
//!
//! An earlier version of this file claimed the opposite: that the estimate "catches the
//! amplification attacks that matter (a small file declaring an enormous raster)". It does
//! not, and it cannot. That claim is removed rather than softened.
//!
//! ## Half two: the measured check, after the load
//!
//! [`check_measured_memory`] compares the process's resident set before and after the open
//! (see [`super::rss`]) and refuses to hand back a document that has already cost more
//! than the caller allowed. It is **after the fact**: the memory is allocated by the time
//! it fires. What it buys is that the bomb above returns `LimitExceeded` instead of `Ok`,
//! and that the allocation is released immediately rather than held for the lifetime of a
//! handle the caller believes is within budget.
//!
//! ## What neither half does
//!
//! Neither prevents the engine from being killed. On a device with a hard ceiling PDFium
//! can hit its own out-of-memory path and `abort()` before any Rust code runs — and an
//! abort is not a panic, so neither the engine thread's `catch_unwind` nor
//! `burrow-ffi::guard` sees it. The real fix is a **structural pre-scan** of the declared
//! sizes before the load, which needs a parser that is not PDFium: qpdf, in M1 PR 3.
//! ADR 0011's consequences record this.
//!
//! On the **web** the same `Limits` field is a genuine hard ceiling, because it becomes
//! the WASM instance's maximum memory and growth past it fails inside the sandbox. The
//! guarantees are unequal, and visibly so.
//!
//! [ADR 0007]: ../../../../docs/adr/0007-limit-enforcement-per-platform.md

use burrow_types::{Limits, Result};

/// Fixed cost of having a document open at all, independent of its size.
///
/// PDFium's allocator arenas, font caches, and per-document structures. Deliberately
/// generous: this term only matters for small inputs, where the limit is not the binding
/// constraint anyway.
const BASE_OVERHEAD_BYTES: u64 = 16 * 1024 * 1024;

/// Measured expansion of an input once PDFium has it open.
///
/// [Spike 0001](../../../../docs/spikes/0001-wasm-engines.md) Finding 4: a 52,429,484-byte
/// input left the engine heap at 65.4 MB, consistent with the buffer being copied in plus
/// roughly a quarter again of structures. `4` means "a quarter more", as a divisor rather
/// than a ratio so the arithmetic stays in integers.
///
/// This describes a *well-behaved* file. It is not, and cannot be, an upper bound.
const EXPANSION_DIVISOR: u64 = 4;

/// Slack added to `max_memory_bytes` before a measured open is rejected.
///
/// Resident set size is process-wide, so another thread allocating during the measurement
/// window is attributed to the open. Every PDFium call is serialised onto one thread,
/// which keeps the window short and the attribution usually right — but "usually" is the
/// honest word, and rejecting a legitimate document is a bug users experience directly.
///
/// **Absolute, not proportional.** Measurement noise is a fixed quantity — whatever other
/// threads happened to allocate — and does not scale with the caller's limit. A
/// proportional tolerance gets this exactly backwards: a 2x margin was tried first and let
/// the 1.2 GB bomb through against the default 1 GiB ceiling, because 1.2 GB is only 1.2x
/// over. 64 MiB is far more than any plausible concurrent allocation here and far less
/// than any overshoot worth reporting.
const MEASURED_NOISE_MARGIN_BYTES: u64 = 64 * 1024 * 1024;

/// Predicted peak engine memory for opening an input of `input_len` bytes.
///
/// Saturating throughout: an absurd length must produce an absurd estimate, which is then
/// rejected, rather than wrapping to a small one that passes.
pub(crate) fn estimated_open_bytes(input_len: u64) -> u64 {
    // `div_euclid` rather than `/`: the workspace denies `integer_division` as a class
    // because silent truncation on an attacker-controlled size is a real bug, and this is
    // the deliberate exception rather than an accident. Truncating downward here is also
    // the conservative direction for a value we then add.
    input_len
        .saturating_add(input_len.div_euclid(EXPANSION_DIVISOR))
        .saturating_add(BASE_OVERHEAD_BYTES)
}

/// Reject an input whose predicted cost exceeds `limits.max_memory_bytes`.
///
/// Called before the buffer reaches the engine. See the module docs for what this does
/// not catch.
///
/// # Errors
///
/// [`Error::LimitExceeded`](burrow_types::Error::LimitExceeded) with
/// `limit: "max_memory_bytes"`, naming the estimate and the ceiling.
pub(crate) fn check_open_memory(input_len: u64, limits: &Limits) -> Result<()> {
    Limits::check(
        "max_memory_bytes",
        estimated_open_bytes(input_len),
        limits.max_memory_bytes,
    )
}

/// Reject a document whose open actually cost more than the caller allowed.
///
/// `before` and `after` are two readings of the same memory counter, taken around the
/// open. **What that counter is differs per platform, and this function deliberately
/// does not know**: on native it is the process resident set (`pdfium::rss`), and on the
/// web it is the engine module's `HEAPU8.byteLength`, which is better attributed —
/// it measures that engine rather than the whole process. Only the *delta* is used, so
/// a WASM heap that never shrinks still reports each operation's own cost honestly.
///
/// Either being `None` — the counter unreadable — degrades to "no measurement", which is
/// not an error: an operation must not fail because a diagnostic was unavailable.
///
/// # Errors
///
/// [`Error::LimitExceeded`](burrow_types::Error::LimitExceeded) with
/// `limit: "max_memory_bytes"` when the growth exceeds the ceiling by more than the
/// tolerance above.
pub(crate) fn check_measured_memory(
    before: Option<u64>,
    after: Option<u64>,
    limits: &Limits,
) -> Result<()> {
    let (Some(before), Some(after)) = (before, after) else {
        return Ok(());
    };
    // Saturating: the resident set can legitimately *fall* during an open, if something
    // else in the process released memory. That is not growth to report.
    let grew_by = after.saturating_sub(before);

    let tolerated = limits
        .max_memory_bytes
        .saturating_add(MEASURED_NOISE_MARGIN_BYTES);
    if grew_by <= tolerated {
        return Ok(());
    }

    // Report the real measurement against the real limit, not against the tolerance: the
    // caller set `max_memory_bytes` and that is the number they need to see.
    Limits::check("max_memory_bytes", grew_by, limits.max_memory_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_types::Error;

    #[test]
    fn the_estimate_is_never_below_the_input_itself() {
        for len in [0, 1, 1024, 52_429_484, u64::MAX / 2] {
            assert!(estimated_open_bytes(len) >= len, "underestimated {len}");
        }
    }

    #[test]
    fn an_absurd_length_saturates_instead_of_wrapping_to_something_small() {
        assert_eq!(estimated_open_bytes(u64::MAX), u64::MAX);
        let limits = Limits::with(|l| l.max_memory_bytes = u64::MAX - 1);
        assert!(check_open_memory(u64::MAX, &limits).is_err());
    }

    #[test]
    fn the_estimate_matches_the_measurement_it_was_derived_from() {
        // Spike 0001 Finding 4: 52,429,484 bytes in, 65.4 MB of engine heap. The estimate
        // must cover that, and must not be wildly above it or it would reject files that
        // are fine.
        let measured = 65_400_000;
        let estimate = estimated_open_bytes(52_429_484);
        assert!(estimate >= measured, "{estimate} < {measured}");
        assert!(
            estimate < measured * 2,
            "{estimate} is more than twice {measured}"
        );
    }

    #[test]
    fn the_check_names_the_limit_and_both_numbers() {
        let limits = Limits::with(|l| l.max_memory_bytes = 1024);
        match check_open_memory(4096, &limits) {
            Err(Error::LimitExceeded {
                limit,
                requested,
                allowed,
            }) => {
                assert_eq!(limit, "max_memory_bytes");
                assert_eq!(requested, estimated_open_bytes(4096));
                assert_eq!(allowed, 1024);
            }
            other => panic!("expected LimitExceeded, got {other:?}"),
        }
    }

    #[test]
    fn a_missing_measurement_is_not_a_failure() {
        let limits = Limits::with(|l| l.max_memory_bytes = 1);
        assert!(check_measured_memory(None, Some(1 << 40), &limits).is_ok());
        assert!(check_measured_memory(Some(0), None, &limits).is_ok());
        assert!(check_measured_memory(None, None, &limits).is_ok());
    }

    #[test]
    fn growth_within_the_noise_margin_is_allowed() {
        let limits = Limits::with(|l| l.max_memory_bytes = 1_000);
        // Exactly at the margin, which is deliberately generous against measurement noise
        // from other threads.
        assert!(
            check_measured_memory(Some(0), Some(1_000 + MEASURED_NOISE_MARGIN_BYTES), &limits)
                .is_ok()
        );
    }

    /// The margin is absolute, so it cannot be outgrown by raising the limit — which is
    /// exactly how a proportional margin let a 1.2 GB open pass a 1 GiB ceiling.
    #[test]
    fn the_margin_does_not_scale_with_the_limit() {
        let one_gib = 1024 * 1024 * 1024;
        let limits = Limits::with(|l| l.max_memory_bytes = one_gib);
        // 1.2 GB against a 1 GiB ceiling: only 1.2x over, and it must still be rejected.
        assert!(check_measured_memory(Some(0), Some(1_250_000_000), &limits).is_err());
    }

    #[test]
    fn growth_far_past_the_limit_is_rejected_and_reports_the_real_limit() {
        let limits = Limits::with(|l| l.max_memory_bytes = 1_000);
        match check_measured_memory(Some(0), Some(MEASURED_NOISE_MARGIN_BYTES + 1_001), &limits) {
            Err(Error::LimitExceeded {
                limit,
                requested,
                allowed,
            }) => {
                assert_eq!(limit, "max_memory_bytes");
                assert_eq!(requested, MEASURED_NOISE_MARGIN_BYTES + 1_001);
                // The caller's number, not the margin.
                assert_eq!(allowed, 1_000);
            }
            other => panic!("expected LimitExceeded, got {other:?}"),
        }
    }

    #[test]
    fn a_shrinking_resident_set_is_not_reported_as_growth() {
        let limits = Limits::with(|l| l.max_memory_bytes = 1);
        assert!(check_measured_memory(Some(1 << 40), Some(0), &limits).is_ok());
    }
}

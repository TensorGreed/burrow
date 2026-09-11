//! When a worker's engine heap has grown far enough that the page should recycle it.
//!
//! # Why this is a decision and not an observation
//!
//! WASM linear memory **only grows**. Neither Emscripten module ever returns a page to the
//! host, so a worker that has opened one large document keeps that footprint for the rest of
//! its life even though every allocation inside it has been freed. The free list is reusable;
//! the memory is not reclaimable.
//!
//! `crate::estimate::check_measured_memory` handles the *within-one-operation* half of this
//! — it compares the delta across a single open against `max_memory_bytes` and refuses a file
//! that costs too much. What it cannot see is the accumulation: a hundred files that each cost
//! 40 MB never trip it, and the worker's heap is 4 GB.
//!
//! So the page terminates a worker whose heap has grown past a ceiling and spawns a fresh one,
//! whose heap starts at the module's initial size. That is a *lifecycle* decision, which
//! [ADR 0009] assigns to the page — but the **threshold** is derived from
//! [`Limits::max_memory_bytes`], and ADR 0009 is equally clear that no binding may enforce any
//! part of [`Limits`]. Hence this function: the verdict is computed here, in the same Rust the
//! native path links, and the page reads a boolean.
//!
//! # Where the numbers come from
//!
//! Both are measured, not chosen. See [ADR 0015] for the readings and the method; the summary
//! is that a worker which has done real work sits at about 18 MiB per engine — two orders of
//! magnitude below the 512 MiB floor — and the only fixture that moves the number at all is
//! `tests/conformance/fixtures/xref-bomb.pdf`, which reaches 1.9 GiB in PDFium when a caller
//! raises `max_memory_bytes` far enough to let it.
//!
//! [ADR 0009]: https://github.com/TensorGreed/burrow/blob/main/docs/adr/0009-web-panic-contract-and-binding-boundary.md
//! [ADR 0015]: https://github.com/TensorGreed/burrow/blob/main/docs/adr/0015-web-worker-lifecycle.md

use burrow_types::Limits;

/// The share of [`Limits::max_memory_bytes`] a single engine heap may reach before the worker
/// is recycled, as a right shift.
///
/// **Half.** The reasoning is not subtle: there are **two** engine heaps in a worker and they
/// are independent address spaces, so a per-heap ceiling of the whole limit would let the pair
/// reach twice it. Recycling at half means the pair cannot exceed `max_memory_bytes` between
/// them, which is the ceiling the caller actually asked for.
///
/// A shift rather than `/ 2` because the workspace denies `clippy::integer_division`: silent
/// truncation on an attacker-controlled size is a real bug class, and the lint does not
/// distinguish a constant divisor from one. For `u64`, `>> 1` is exactly `/ 2`.
const SHARE_SHIFT: u32 = 1;

/// The absolute ceiling, whatever the limit says.
///
/// 512 MiB. `Limits::default()`'s `max_memory_bytes` is generous enough on a desktop that half
/// of it is a heap no phone survives, and a caller is free to set it higher still. A worker at
/// 512 MiB has already cost more than a respawn does (measured at well under a second — ADR
/// 0015), so there is no case for keeping it.
///
/// This is the number that binds in practice. `SHARE_SHIFT` is what makes a *deliberately
/// constrained* caller — a mobile page setting a small `max_memory_bytes` — get the tighter
/// behaviour it asked for rather than this one.
const FLOOR_BYTES: u64 = 512 * 1024 * 1024;

/// The heap size at which a worker should be recycled, for these limits.
///
/// The smaller of half `max_memory_bytes` and an absolute 512 MiB floor, so neither a huge
/// limit nor a tiny one can disable the protection: a huge one is capped by the floor, and a
/// tiny one tightens below it.
///
/// Public so a caller can show it. `bindings/burrow-wasm` reports both engine heap readings on
/// every `Reply`, and a page explaining why a worker was recycled needs the number that decided
/// it as well as the numbers that tripped it.
#[must_use]
pub fn threshold_bytes(limits: &Limits) -> u64 {
    (limits.max_memory_bytes >> SHARE_SHIFT).min(FLOOR_BYTES)
}

/// The smallest `max_memory_bytes` at which recycling still converges.
///
/// **A caller below this thrashes**, and the arithmetic says so plainly: a worker that has done
/// any real work sits at about 18 MiB per engine (ADR 0015 §6), so a threshold under that —
/// i.e. a `max_memory_bytes` under about 36 MiB — is exceeded by the *first* operation on a
/// fresh worker. Every operation then costs a respawn and the heap never gets below the line.
///
/// It is **not clamped**, and that is deliberate: silently ignoring a ceiling the caller set
/// would be worse than honouring one they will notice. This constant exists to be quotable —
/// ADR 0015 §7 anticipates exactly such a caller in a mobile page, and any mobile `Limits`
/// default must sit above it.
pub const MIN_CONVERGING_MEMORY_BYTES: u64 = 64 * 1024 * 1024;

/// Whether a worker holding these two engine heaps should be recycled after replying.
///
/// **After**, not instead of. Recycling is not a failure and the caller must never see it as
/// one: the result is delivered, and only then is the worker discarded.
///
/// Either heap alone is enough. They are separate address spaces with separate ceilings, and a
/// worker is only as healthy as its worse half.
#[must_use]
pub fn should_recycle(pdfium_heap_bytes: u64, qpdf_heap_bytes: u64, limits: &Limits) -> bool {
    let threshold = threshold_bytes(limits);
    pdfium_heap_bytes > threshold || qpdf_heap_bytes > threshold
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A worker doing ordinary work must not be recycled. If it were, every operation would
    /// cost a respawn and the page would be slower than it needs to be for no protection.
    #[test]
    fn an_ordinary_heap_is_not_recycled() {
        let limits = Limits::default();
        assert!(!should_recycle(16 * 1024 * 1024, 8 * 1024 * 1024, &limits));
    }

    /// Either heap alone is enough. Two address spaces, two ceilings; the worker is as
    /// healthy as its worse half.
    #[test]
    fn either_engine_alone_triggers_recycling() {
        let limits = Limits::with(|l| l.max_memory_bytes = 100 * 1024 * 1024);
        let over = threshold_bytes(&limits) + 1;
        assert!(should_recycle(over, 0, &limits), "pdfium alone");
        assert!(should_recycle(0, over, &limits), "qpdf alone");
    }

    /// Exactly at the threshold is not over it. Stated as a test because an off-by-one here
    /// is invisible: it would recycle one operation early, forever, and look like nothing.
    #[test]
    fn the_threshold_itself_is_not_over_it() {
        let limits = Limits::default();
        let at = threshold_bytes(&limits);
        assert!(!should_recycle(at, at, &limits));
        assert!(should_recycle(at + 1, at, &limits));
    }

    /// A caller that sets a small `max_memory_bytes` — a mobile page, say — must get the
    /// tighter behaviour it asked for, not the desktop floor.
    #[test]
    fn a_constrained_caller_gets_a_lower_threshold() {
        let constrained = Limits::with(|l| l.max_memory_bytes = 64 * 1024 * 1024);
        assert_eq!(threshold_bytes(&constrained), 32 * 1024 * 1024);
        assert!(should_recycle(33 * 1024 * 1024, 0, &constrained));
    }

    /// A generous limit must not disable the protection. Without the floor, a caller passing
    /// `u64::MAX` would get a threshold no heap can reach and recycling would silently never
    /// happen — the failure mode being guarded against is a limit that reads as permissive
    /// and is actually "off".
    #[test]
    fn a_huge_limit_is_capped_by_the_floor() {
        let generous = Limits::with(|l| l.max_memory_bytes = u64::MAX);
        assert_eq!(threshold_bytes(&generous), FLOOR_BYTES);
        assert!(should_recycle(FLOOR_BYTES + 1, 0, &generous));
    }

    /// The pathological end, pinned rather than left to be discovered.
    ///
    /// A ceiling below `MIN_CONVERGING_MEMORY_BYTES` recycles on every operation, because the
    /// threshold falls below the ~18 MiB a working engine occupies. Nothing clamps it — a
    /// silently ignored limit would be worse — so this test is the record that the behaviour
    /// is known, and the number a mobile default has to clear.
    #[test]
    fn a_ceiling_below_the_converging_minimum_recycles_every_operation() {
        const WORKING_HEAP: u64 = 18 * 1024 * 1024;

        let too_small = Limits::with(|l| l.max_memory_bytes = 16 * 1024 * 1024);
        assert!(
            should_recycle(WORKING_HEAP, 0, &too_small),
            "a ceiling this low recycles a worker that has merely opened a document"
        );

        let workable = Limits::with(|l| l.max_memory_bytes = MIN_CONVERGING_MEMORY_BYTES);
        assert!(
            !should_recycle(WORKING_HEAP, WORKING_HEAP, &workable),
            "at the documented minimum, ordinary work must NOT recycle"
        );
    }

    /// Two heaps at the threshold must not be able to exceed `max_memory_bytes` between
    /// them. That is the whole reason the share is a half rather than a whole.
    #[test]
    fn the_two_heaps_together_cannot_exceed_the_limit() {
        let limits = Limits::with(|l| l.max_memory_bytes = 200 * 1024 * 1024);
        let at = threshold_bytes(&limits);
        assert!(at.saturating_mul(2) <= limits.max_memory_bytes);
    }
}

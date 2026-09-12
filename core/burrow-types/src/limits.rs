//! Resource ceilings applied to every operation.

/// Memory, time, and size ceilings for a single operation.
///
/// Every operation takes one and enforces it. A missing limit is a denial-of-service
/// bug: adversarial input should hit [`Error::LimitExceeded`](crate::Error::LimitExceeded),
/// never an allocator abort or an unbounded loop.
///
/// # These limits are not equally strong
///
/// Three of these are exact: `max_input_bytes`, `max_pages`, and `max_pixels` are read
/// from the input and compared before anything is allocated or decoded. The other two
/// are best-effort, and the difference matters when you are choosing values.
///
/// `max_duration_ms` is **cooperative**. It is checked at page boundaries and between
/// engine calls, so an operation can overshoot by as long as one call into PDFium or
/// qpdf takes. A file crafted to make a single engine call run for a minute is not
/// stopped by this limit.
///
/// `max_memory_bytes` **bounds nothing, on any platform.** Everything below *detects* an
/// overrun; nothing prevents one. The two words are used deliberately throughout this
/// documentation and are worth keeping straight.
///
/// The reason is not an implementation shortcut. The allocations that dominate are made by
/// C++ inside the engines, through their own allocators. Rust's allocator never sees them,
/// so nothing here can observe — let alone cap — them.
///
/// Three mechanisms consult this value. Every one of them runs *before* the engine is given
/// the file, or *after* it has finished with it:
///
/// 1. A **structural pre-scan**, before either engine sees the bytes. It reads what the file
///    *declares* — cross-reference size, `/Length`s, the `/Prev` chain — and never
///    decompresses or resolves a reference. It refuses a declared-size bomb before anything
///    is allocated, which makes it the only check that prevents rather than reports. Its
///    honest limit is that it sees declarations, not truth: a small file that inflates a
///    compressed object stream passes it (issue #24).
/// 2. A **length-based size estimate**, predicting cost from the input's byte count. Blind to
///    anything declared, so a 330 KB PDF that drives an engine to 1.2 GB sails through it.
///    It also runs on the **PDFium path only**, and on neither qpdf path — its constants are
///    PDFium measurements, and applying them to qpdf would predict the wrong number
///    (issue #26). So a structure check gets two of these three mechanisms, not three.
/// 3. A **measured check**, after the operation, comparing a counter before and after. The
///    memory is allocated by the time it fires. What it buys is that the operation fails
///    instead of returning a handle that is already over budget.
///
/// So `max_memory_bytes` means "you will be told, and the result discarded, if an operation
/// cost more than this" — **not** "an operation cannot cost more than this". A sufficiently
/// adversarial file can still get the process killed before any of them reports, because an
/// engine's own out-of-memory `abort()` is not a panic and is not interceptable.
///
/// # Per-platform differences, and the one that used to be claimed
///
/// An earlier version of these docs said the web was different — that the value became the
/// WASM instance's maximum memory and was therefore "a genuine hard ceiling", making the web
/// the best-protected platform. It is not, and never was: the engine modules declare their
/// own maximum (2 GiB) in their memory sections at build time, and burrow hands Emscripten an
/// already-compiled module rather than creating the memory, so there is no point at which a
/// per-operation maximum could be applied. Measured and corrected in M1 PR 4b; see issue #25.
///
/// What genuinely differs:
///
/// - **The measured check's counter.** On native it is the process resident set, so a peak
///   that occurs *during* the operation and is released before the second reading is
///   invisible. On the web it is the engine module's heap size, and a WASM heap never
///   shrinks — so the web reading *does* include the peak. The real asymmetry runs the
///   opposite way from the one that was claimed, and it is a difference in what is detected,
///   not in what is bounded.
/// - **Worker recycling**, on the web only: a worker whose engine heap has grown past a
///   threshold derived from this value is discarded *after* its result is delivered. That
///   bounds accumulation across operations. It does not bound any single one.
/// - **A 2 GiB per-module ceiling**, on the web only, fixed at build time and not derived
///   from anything a caller sets. It is the one real bound in this whole picture, an
///   allocation past it fails cleanly rather than taking the tab down, and it is **not this
///   limit**.
///
/// Set it conservatively on constrained devices rather than relying on it to save you.
///
/// See `docs/adr/0007-limit-enforcement-per-platform.md` — in particular its 2026-09-12
/// amendment, which is the per-path table this section summarises — for why, and for what
/// would have to change to make any of it a bound.
///
/// The defaults are deliberately conservative; callers on constrained devices should
/// lower them rather than relying on these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// Largest accepted input, in bytes. Exact: checked before the input is parsed.
    pub max_input_bytes: u64,
    /// Peak working memory an operation may use, in bytes.
    ///
    /// **Bounds nothing, on any platform.** A declaration-based pre-scan and a length-based
    /// size estimate before the engine, and a measured check after it: an overrun is
    /// *detected*, not prevented. See the type-level docs — this is the weakest limit here,
    /// and the one most likely to be trusted too far.
    pub max_memory_bytes: u64,
    /// Wall-clock ceiling for one operation, in milliseconds.
    ///
    /// Cooperative: checked at page boundaries and between engine calls, so overshoot
    /// of up to one engine call is possible. Requires a platform clock —
    /// `Instant::now()` panics on `wasm32-unknown-unknown`, so operations take an
    /// injected clock rather than reading time directly.
    pub max_duration_ms: u64,
    /// Largest accepted page count for paged documents. Exact.
    pub max_pages: u64,
    /// Largest accepted decoded raster, in pixels (width x height). Exact: checked
    /// against declared dimensions before any raster is allocated.
    pub max_pixels: u64,
}

impl Limits {
    /// Conservative defaults suitable for a desktop browser tab.
    pub const DEFAULT: Self = Self {
        max_input_bytes: 512 * 1024 * 1024,
        max_memory_bytes: 1024 * 1024 * 1024,
        max_duration_ms: 60_000,
        max_pages: 10_000,
        max_pixels: 256 * 1024 * 1024,
    };

    /// [`Limits::DEFAULT`] with adjustments applied.
    ///
    /// `Limits` is `#[non_exhaustive]`, so callers outside this crate cannot write a
    /// struct literal and cannot use `..Default::default()` either. This is the supported
    /// way to build a customised set, and it keeps working when a field is added.
    ///
    /// ```
    /// use burrow_types::Limits;
    /// let limits = Limits::with(|l| l.max_pages = 100);
    /// assert_eq!(limits.max_pages, 100);
    /// assert_eq!(limits.max_input_bytes, Limits::DEFAULT.max_input_bytes);
    /// ```
    #[must_use]
    pub fn with(edit: impl FnOnce(&mut Self)) -> Self {
        let mut limits = Self::DEFAULT;
        edit(&mut limits);
        limits
    }

    /// Checks `requested` against `allowed`, naming both the limit and the check.
    ///
    /// `stage` comes first because it is the one argument a call site cannot get from context:
    /// the limit name is right there in the field being compared, while *which check this is*
    /// is knowledge only the call site has. It is a [`Stage`](crate::Stage) rather than a
    /// second `&'static str` so the two cannot be transposed.
    ///
    /// # Errors
    ///
    /// Returns [`Error::LimitExceeded`](crate::Error::LimitExceeded) when
    /// `requested > allowed`.
    pub fn check(
        stage: crate::Stage,
        limit: &'static str,
        requested: u64,
        allowed: u64,
    ) -> crate::Result<()> {
        if requested > allowed {
            return Err(crate::Error::LimitExceeded {
                limit,
                stage,
                requested,
                allowed,
            });
        }
        Ok(())
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_permits_the_boundary_and_rejects_one_past_it() {
        assert!(Limits::check(crate::Stage::PageCount, "max_pages", 10, 10).is_ok());
        assert!(Limits::check(crate::Stage::PageCount, "max_pages", 11, 10).is_err());
    }

    #[test]
    fn with_changes_only_what_it_is_asked_to() {
        let limits = Limits::with(|l| l.max_duration_ms = 7);
        assert_eq!(limits.max_duration_ms, 7);
        assert_eq!(limits.max_pages, Limits::DEFAULT.max_pages);
        assert_eq!(limits.max_memory_bytes, Limits::DEFAULT.max_memory_bytes);
    }

    #[test]
    fn defaults_are_nonzero_so_no_operation_is_trivially_unusable() {
        let l = Limits::default();
        for (name, value) in [
            ("max_input_bytes", l.max_input_bytes),
            ("max_memory_bytes", l.max_memory_bytes),
            ("max_duration_ms", l.max_duration_ms),
            ("max_pages", l.max_pages),
            ("max_pixels", l.max_pixels),
        ] {
            assert!(value > 0, "{name} must be positive");
        }
    }
}

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
/// `max_memory_bytes` is enforced differently per platform:
///
/// - **On the web** it is a genuine hard ceiling — it becomes the WASM instance's
///   maximum memory, so exceeding it fails inside the sandbox and is recoverable.
/// - **On native targets** it is enforced in two weaker halves, neither of which is a
///   cap. The allocations that dominate are made by C++ inside the engines, through their
///   own allocators, and Rust cannot observe or bound them.
///   1. A *size-based pre-check* before the input reaches the engine. It predicts cost
///      from the input's **length**, so it is blind to anything the file *declares* — a
///      small file declaring an enormous structure sails through it. That is a real gap,
///      not a rounding error: a 330 KB PDF can drive an engine to allocate 1.2 GB.
///   2. A *measured check* after the operation, comparing the process's resident set
///      before and after. This catches what (1) cannot, but only after the memory has
///      already been allocated. What it buys is that the operation fails instead of
///      returning a handle that is already over budget.
///
/// So on native, `max_memory_bytes` means "you will be told, and the result discarded, if
/// an operation costs more than this" — **not** "an operation cannot cost more than this".
/// A sufficiently adversarial file can still get the process killed by the OS before
/// either check runs, because an engine's own out-of-memory abort is not something Rust
/// can intercept. Set it conservatively on constrained devices rather than relying on it
/// to save you.
///
/// See `docs/adr/0007-limit-enforcement-per-platform.md` for why, and for what would
/// have to change to make these uniform.
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
    /// A hard ceiling on the web; an estimate-based pre-check on native targets. See
    /// the type-level docs — this is the weakest limit here, and the one most likely to
    /// be trusted too far.
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

    /// Checks `requested` against `allowed`, naming the limit in the error.
    ///
    /// # Errors
    ///
    /// Returns [`Error::LimitExceeded`](crate::Error::LimitExceeded) when
    /// `requested > allowed`.
    pub fn check(limit: &'static str, requested: u64, allowed: u64) -> crate::Result<()> {
        if requested > allowed {
            return Err(crate::Error::LimitExceeded {
                limit,
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
        assert!(Limits::check("max_pages", 10, 10).is_ok());
        assert!(Limits::check("max_pages", 11, 10).is_err());
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

//! Resource ceilings applied to every operation.

/// Memory, time, and size ceilings for a single operation.
///
/// Every operation takes one and enforces it. A missing limit is a denial-of-service
/// bug: adversarial input should hit [`Error::LimitExceeded`](crate::Error::LimitExceeded),
/// never an allocator abort or an unbounded loop.
///
/// The defaults are deliberately conservative; callers on constrained devices should
/// lower them rather than relying on these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// Largest accepted input, in bytes.
    pub max_input_bytes: u64,
    /// Peak working memory an operation may allocate, in bytes.
    pub max_memory_bytes: u64,
    /// Wall-clock ceiling for one operation, in milliseconds.
    pub max_duration_ms: u64,
    /// Largest accepted page count for paged documents.
    pub max_pages: u64,
    /// Largest accepted decoded raster, in pixels (width x height).
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

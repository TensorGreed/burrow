//! burrow: privacy-first file tools that run entirely on the user's device.
//!
//! This is the only crate the bindings depend on. Nothing here, or anywhere beneath it,
//! may make a network call: file content never leaves the device.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

pub use burrow_ops as ops;
pub use burrow_types::{Error, Limits, Result};

/// The version of this build of burrow.
#[must_use]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Names of the operations compiled into this build, in stable order.
#[must_use]
pub fn available_operations() -> &'static [&'static str] {
    burrow_ops::AVAILABLE
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_matches_the_crate_manifest() {
        assert_eq!(super::version(), env!("CARGO_PKG_VERSION"));
        assert!(!super::version().is_empty());
    }

    #[test]
    fn limits_and_errors_are_reachable_through_the_public_surface() {
        // Bindings only ever see `burrow_core`, so the re-exports must be usable here.
        let limits = super::Limits::default();
        let err = super::Limits::check("max_pages", limits.max_pages + 1, limits.max_pages)
            .expect_err("one past the ceiling must be rejected");
        assert!(matches!(err, super::Error::LimitExceeded { .. }));
    }
}

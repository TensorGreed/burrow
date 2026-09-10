//! uniffi bindings for burrow: Swift (iOS) and Kotlin (Android).
//!
//! Scaffolding only in M0; uniffi itself is wired up in M3 alongside the Android app.
//!
//! # Panics must not cross this boundary
//!
//! A panic unwinding into Swift or Kotlin is undefined behaviour. Every exported entry
//! point that could conceivably panic must be wrapped in
//! [`std::panic::catch_unwind`] and map the payload to
//! [`Error::Internal`]. See [`guard`].

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

use std::panic::{AssertUnwindSafe, catch_unwind};

use burrow_core::{Error, Result};

/// Runs `f`, converting a panic into [`Error::Internal`] instead of unwinding into the
/// host language.
///
/// Every exported FFI entry point routes through this.
///
/// # Errors
///
/// Returns whatever `f` returns, or [`Error::Internal`] if `f` panicked.
pub fn guard<T, F>(f: F) -> Result<T>
where
    F: FnOnce() -> Result<T>,
{
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(result) => result,
        Err(payload) => {
            // Recover the panic message where possible, but never propagate the unwind.
            let message = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic with non-string payload".to_owned());
            Err(Error::Internal(format!("caught panic: {message}")))
        }
    }
}

/// The version of this build of burrow, for the host app's about screen.
#[must_use]
pub fn version() -> String {
    burrow_core::version().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_passes_success_through_untouched() {
        assert!(matches!(guard(|| Ok(7)), Ok(7)));
    }

    #[test]
    fn guard_converts_a_panic_into_an_internal_error() {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {})); // keep the test output readable
        let result = guard(|| -> Result<()> { panic!("hostile input took an unexpected path") });
        std::panic::set_hook(previous);

        let Err(Error::Internal(message)) = result else {
            panic!("a panic must surface as Error::Internal, got {result:?}");
        };
        assert!(message.contains("hostile input"), "{message}");
    }

    #[test]
    fn version_is_reported_to_the_host() {
        assert!(!version().is_empty());
    }
}

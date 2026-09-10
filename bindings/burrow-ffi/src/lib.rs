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

/// The message every caught panic reports. Deliberately fixed and content-free.
const CAUGHT_PANIC: &str = "caught panic";

/// Runs `f`, converting a panic into [`Error::Internal`] instead of unwinding into the
/// host language.
///
/// Every exported FFI entry point routes through this.
///
/// The panic payload is **discarded**, and the returned error always carries the same
/// fixed message. A panic raised while parsing a hostile file can embed input-derived
/// bytes in its message — an assertion printing an offset, a length, or a slice of the
/// file itself — and an error string crosses into the host app, its logs, and its crash
/// reports. Forwarding the payload would make this function a privacy leak, so the
/// message says only that a panic happened.
///
/// The payload is not lost to a developer: the panic hook still runs before unwinding,
/// so the real message and backtrace reach the platform log. It just does not travel in
/// the error value.
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
        // The payload is dropped here, unexamined, and on purpose. See above.
        Err(_) => Err(Error::Internal(CAUGHT_PANIC.to_owned())),
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

    /// Runs `f` with the panic hook silenced, so a deliberate panic does not clutter
    /// the test output.
    fn without_panic_output<T>(f: impl FnOnce() -> T) -> T {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let out = f();
        std::panic::set_hook(previous);
        out
    }

    #[test]
    fn guard_converts_a_panic_into_an_internal_error() {
        let result = without_panic_output(|| guard(|| -> Result<()> { panic!("boom") }));

        let Err(Error::Internal(message)) = result else {
            panic!("a panic must surface as Error::Internal, got {result:?}");
        };
        assert_eq!(message, CAUGHT_PANIC);
    }

    #[test]
    fn guard_does_not_leak_the_panic_payload_into_the_error() {
        // Stands in for a panic raised deep in engine code whose message has picked up
        // bytes from the file being parsed. That text must not reach the host app.
        const SECRET: &str = "patient-name-Jane-Doe-ssn-123-45-6789";

        let from_str = without_panic_output(|| guard(|| -> Result<()> { panic!("{}", SECRET) }));
        let from_owned =
            without_panic_output(|| guard(|| -> Result<()> { panic!("{}", SECRET.to_owned()) }));

        for result in [from_str, from_owned] {
            let Err(Error::Internal(message)) = result else {
                panic!("expected Error::Internal, got {result:?}");
            };
            assert!(
                !message.contains(SECRET),
                "panic payload leaked into the error: {message}"
            );
            assert!(
                !message.contains("Jane") && !message.contains("6789"),
                "fragment of the panic payload leaked into the error: {message}"
            );
            assert_eq!(message, CAUGHT_PANIC);
        }
    }

    #[test]
    fn guard_handles_a_panic_with_a_non_string_payload() {
        // `panic_any` payloads are not `&str` or `String`; the old implementation had a
        // separate branch for these, and the fixed message must cover them too.
        let result = without_panic_output(|| {
            guard(|| -> Result<()> { std::panic::panic_any(0xdead_beef_u64) })
        });

        let Err(Error::Internal(message)) = result else {
            panic!("expected Error::Internal, got {result:?}");
        };
        assert_eq!(message, CAUGHT_PANIC);
    }

    #[test]
    fn version_is_reported_to_the_host() {
        assert!(!version().is_empty());
    }
}

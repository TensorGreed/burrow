//! wasm-bindgen bindings for burrow, for the web app.
//!
//! Scaffolding only in M0; wasm-bindgen is wired up in M1 with the first operation.
//! For now this crate exists so CI can prove `wasm32-unknown-unknown` still builds.
//!
//! Everything here runs in the browser tab and touches user file content, so nothing
//! in this crate or below it may make a network call.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

/// The version of this build of burrow, for the web app's footer.
#[must_use]
pub fn version() -> &'static str {
    burrow_core::version()
}

/// Names of the operations compiled into this wasm module, in stable order.
#[must_use]
pub fn available_operations() -> &'static [&'static str] {
    burrow_core::available_operations()
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_reported_to_the_web_app() {
        assert!(!super::version().is_empty());
    }

    #[test]
    fn operation_list_matches_the_core_build() {
        assert_eq!(
            super::available_operations(),
            burrow_core::available_operations()
        );
    }
}

//! File operations built on the engine seams in [`burrow_engines`].
//!
//! Empty in M0. Each operation lands as its own module here, added via the
//! `add-operation` skill: core implementation, typed error variants, [`Limits`]
//! enforcement, four kinds of test, a fuzz target, bindings, and a web page.
//!
//! [`Limits`]: burrow_types::Limits

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

/// Operations available in this build, in stable order.
///
/// Empty until M1. Exists so the bindings and the web app have something to enumerate
/// rather than hard-coding a list that drifts.
pub const AVAILABLE: &[&str] = &[];

#[cfg(test)]
mod tests {
    #[test]
    fn available_operations_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for op in super::AVAILABLE {
            assert!(seen.insert(op), "duplicate operation: {op}");
        }
    }
}

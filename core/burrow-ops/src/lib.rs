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

/// The engine trait seams, re-exported so `burrow-core` can pass them on.
///
/// Operations here are written against these traits and never against a C API. The web
/// binding needs the `web` module in particular: it implements
/// [`engines::web::PdfiumBridge`] and [`engines::web::QpdfBridge`] with `#[wasm_bindgen]`,
/// which is the one thing a binding is permitted to do that no other crate can.
///
/// Re-exported rather than depended on directly because `core/CLAUDE.md` makes
/// `burrow-core` the only crate the bindings may depend on, and that rule is worth more
/// than the one hop it costs.
pub use burrow_engines as engines;

pub mod merge;
pub mod rotate;
pub mod split;

pub use merge::{Input, check_total_input_bytes, merge};
pub use rotate::{Pages, rotate};
pub use split::{Cuts, split};

/// Operations available in this build, in stable order.
///
/// Exists so the bindings and the web app have something to enumerate rather than
/// hard-coding a list that drifts.
///
/// **This is the list of operations the CORE implements, not the list any one binding exposes.**
/// `split` and `rotate` are here and `bindings/burrow-wasm` has an entry point for neither —
/// there is no `impl PageExtractor for WebQpdf` and no `impl PageRotator for WebQpdf`, so the
/// web cannot perform either at all. Code review flagged the divergence for `split`; it is
/// recorded rather than hidden, because the alternative is a constant that means something
/// different depending on which crate reads it. A caller that needs to know what a *binding*
/// can do must ask the binding.
pub const AVAILABLE: &[&str] = &["merge", "rotate", "split"];

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

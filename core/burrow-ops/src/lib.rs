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

pub mod compress;
pub mod merge;
pub mod render;
pub mod reorder;
pub mod rotate;
pub mod split;
/// The shared post-operation check every operation runs (ADR 0022).
///
/// `pub` rather than `pub(crate)` because it is the contract the next operation has to meet,
/// and `CLAUDE.md`'s definition of done names it by path. Its `Expected` is **not** re-exported
/// at the crate root beside the operations: no caller outside this crate constructs one, and
/// putting it there would read as part of the calling surface rather than of the seam.
pub mod verify;

pub use compress::{Outcome, compress};
pub use merge::{Input, check_total_input_bytes, merge};
pub use render::{Fit, Render, Rendered, begin as render_begin, render};
pub use reorder::reorder;
pub use rotate::{Pages, rotate};
pub use split::{Cuts, Split, begin as split_begin, split};

/// Operations available in this build, in stable order.
///
/// Exists so the bindings and the web app have something to enumerate rather than
/// hard-coding a list that drifts.
///
/// **This is the list of operations the CORE implements, not the list any one binding exposes.**
/// `split` is here and `bindings/burrow-wasm` has no entry point for it: there is no
/// `impl PageExtractor for WebQpdf`, so the web cannot split at all. **`compress` is now a
/// second such operation** — there is no `impl DocumentCompressor for WebQpdf`, and
/// `engines/qpdf-not-exported.toml` argues the one function it needs out of the wasm export
/// list with the change that removes it named. That is exactly the drift this paragraph was
/// written to stop being an example of, so it is named here rather than left for somebody to
/// find by counting. `rotate` was in the same
/// position until its bridge landed and now has both an `impl PageRotator for WebQpdf` and a
/// `rotate` entry point — this sentence said otherwise for one commit, which is why it names
/// the two operations separately rather than as a pair. Code review flagged the original
/// divergence; it is recorded rather than hidden, because the alternative is a constant that
/// means something different depending on which crate reads it. A caller that needs to know
/// what a *binding* can do must ask the binding.
pub const AVAILABLE: &[&str] = &["compress", "merge", "render", "reorder", "rotate", "split"];

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

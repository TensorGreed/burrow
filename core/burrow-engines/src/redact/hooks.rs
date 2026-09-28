//! Three seams a test needs and no document can reach, shared by every engine the policy runs on.
//!
//! Moved here from `qpdf::tests` with the policy (#191): they are read by the generic witness and
//! the generic operation, so they belong beside them rather than beside one engine's tests.

// The `Cleared` the verification receives is built inside a closure in `redact_page` and
// consumed immediately. Nothing returns it, and a security review measured what that costs:
// forcing `cut_fonts` empty disabled the mapping check for every document and the whole suite
// stayed green, because `redact_verify`'s fakes construct a `Cleared` by hand and `burrow-ops`'
// fake engine never verifies. THE WIRING IS THE SEAM NEITHER FAKE REACHES.
//
// A `thread_local` rather than a parameter because the closure's signature is
// `Fn(&[u8]) -> Result<()>`, and widening it for a test would put the test in the type.
thread_local! {
    pub(crate) static LAST_EXPECTATION: std::cell::RefCell<Option<crate::redact_verify::Cleared>> =
        const { std::cell::RefCell::new(None) };
}

/// Record what the check was handed. Called from `redact_page`.
pub(crate) fn record_expectation(expected: &crate::redact_verify::Cleared) {
    LAST_EXPECTATION.with(|slot| *slot.borrow_mut() = Some(expected.clone()));
}

/// What the last redaction on this thread told its check.
///
/// Read only by `qpdf::tests`, which drive the real engine; the web engine's tests, in
/// `qpdf::web_differential_tests`, assert on the outcome instead.
#[cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
pub(crate) fn last_expectation() -> Option<crate::redact_verify::Cleared> {
    LAST_EXPECTATION.with(|slot| slot.borrow().clone())
}

// A read-back failure a test can force, for the one question no document can ask.
//
// `region_is_cleared` only fails on a document that a correct operation never produces, so
// "does the operation propagate a rejection" has no fixture. Read by the witness's `open_output`
// -- in the witness rather than in the verify closure, because a hook in the closure would be
// bypassed by the very mutation this exists to catch.
thread_local! {
    pub(crate) static FORCED_FAILURE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether the read-back should fail, and the error it fails with.
pub(crate) fn forced_read_back_failure() -> Option<burrow_types::Error> {
    FORCED_FAILURE
        .with(std::cell::Cell::get)
        .then(|| burrow_types::Error::Malformed("planted read-back failure".to_owned()))
}

// THE NARROWING, SWITCHED OFF, for the one question the verification exists to answer: if font
// surgery stops removing the mappings of the codes it removed, does the read-back refuse? No
// document can ask it, because a correct operation always narrows. #218's defect was a check that
// never fired on exactly this, and a review found it by editing the engine; this is that edit as a
// seam. Counted, so a test can assert the narrowing was actually reached and skipped.
thread_local! {
    pub(crate) static SKIP_NARROWING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    pub(crate) static NARROWINGS_SKIPPED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Whether font surgery should leave `/ToUnicode` and `/Differences` alone, counting each skip.
pub(crate) fn narrowing_skipped() -> bool {
    let skip = SKIP_NARROWING.with(std::cell::Cell::get);
    if skip {
        NARROWINGS_SKIPPED.with(|n| n.set(n.get() + 1));
    }
    skip
}

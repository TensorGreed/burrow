//! Engine error codes, and the mapping from a code to a typed [`burrow_types::Error`].
//!
//! # Why this is not inside the engine modules
//!
//! It used to be: `pdfium::errors` and `qpdf::errors`, each `pub(super)` inside a module
//! gated on `all(feature = "native-engines", burrow_native_engines, target_os = "linux")`.
//! That was fine while there was one implementation of each engine. There are now two —
//! the native one, and M1 PR 4's web one driving a *separate* Emscripten module across a
//! JS bridge (ADR 0006 option 1) — and the requirement that both produce **identical typed
//! outcomes** is what ROADMAP M1 item 12's differential conformance harness exists to
//! enforce.
//!
//! Two copies of a mapping table cannot be relied on to stay identical. One copy, compiled
//! on every target, can. So the tables live here, ungated, and both implementations call
//! them.
//!
//! Nothing in this module touches FFI. A code is an integer; classifying one needs no
//! engine, no linkage and no platform. That is why hoisting it out of the gate is possible
//! at all, and it is also why these tests run everywhere rather than only on a Linux host
//! with the engines built — including the two that matter most,
//! `a_zero_error_code_on_a_failed_call_is_never_success` (ADR 0006 requirement 6) and
//! `no_mapped_message_contains_a_digit`.
//!
//! # The three rules both tables follow
//!
//! 1. **Failure is decided by the primary return value, never by the error code.** A null
//!    handle or a negative count is what says "this failed"; the code only *classifies* a
//!    failure that is already established.
//! 2. **Classification is by code, never by prose.** Engine messages are not a stable API
//!    and are not ours.
//! 3. **Messages are fixed constants.** No error code, no engine text, and above all no
//!    input bytes.

pub(crate) mod pdfium;
pub(crate) mod qpdf;

//! Typed errors and resource limits shared across the burrow workspace.
//!
//! This crate is deliberately dependency-light and free of I/O: the bindings
//! (`burrow-ffi` and `burrow-wasm`) depend on it without pulling in engine code, so
//! the "no panics cross the FFI boundary" rule can be audited in one place.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

mod clock;
mod error;
mod limits;
mod password;
mod stage;

#[cfg(not(target_family = "wasm"))]
pub use clock::SystemClock;
pub use clock::{Clock, Deadline, ManualClock};
pub use error::{Error, Result};
pub use limits::Limits;
pub use password::Password;
pub use stage::Stage;

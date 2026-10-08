//! The control for `wipe_on_free.rs`: with nothing wiping, a redaction frees the canary.
//!
//! Without this the other binary's zero is not a measurement. A witness that never sees the
//! canary reports zero over any allocator, wiping or not, and so does a redaction that happens
//! never to copy the decoded content into the Rust heap. This binary shows the copies exist and
//! that the witness finds them. See `support/wipe_witness.rs`.

#![cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;
#[path = "support/wipe_witness.rs"]
mod wipe_witness;

use wipe_witness::Witness;

#[global_allocator]
static ALLOCATOR: Witness<std::alloc::System> = Witness(std::alloc::System);

#[test]
fn without_the_wipe_a_redaction_frees_blocks_that_still_hold_the_canary() {
    let (_output, seen) = wipe_witness::redact_watched();
    assert!(
        seen.held_canary > 0,
        "the witness saw no freed block holding the canary among {} frees; it cannot tell a \
         wiping allocator from a blind one",
        seen.freed
    );
}

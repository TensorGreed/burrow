//! #199: under `WipeOnFree`, nothing a redaction frees from the Rust heap still holds the text it
//! removed.
//!
//! The allocator here is the type both bindings declare, with a witness underneath it that scans
//! every block as it is handed back. `wipe_on_free_control.rs` runs the same redaction with the
//! witness and no wipe, and requires it to find the canary; this requires the same frees to hold
//! none.
//!
//! What this does not cover: qpdf's own heap, which is C++ `malloc` natively and a separate module
//! on the web, and memory still live when the operation returns. The wasm-heap canary is the test
//! for both, in the browser.

#![cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;
#[path = "support/wipe_witness.rs"]
mod wipe_witness;

use burrow_engines::wipe::WipeOnFree;
use wipe_witness::Witness;

#[global_allocator]
static ALLOCATOR: WipeOnFree<Witness<std::alloc::System>> =
    WipeOnFree::new(Witness(std::alloc::System));

/// A floor on what is examined. Both binaries measured **369,541** frees during this redaction
/// when the test was written -- the padding's tokens are most of them -- and the control found the
/// canary in **9**, the largest 82,838 bytes. A count far below that means the witness has stopped
/// seeing the operation, not that the operation got cleaner.
const MIN_FREES: usize = 100_000;

#[test]
fn a_redaction_frees_no_block_that_still_holds_the_canary() {
    let (_output, seen) = wipe_witness::redact_watched();
    assert!(
        seen.freed >= MIN_FREES,
        "only {} frees were examined; the witness is not seeing the redaction",
        seen.freed
    );
    assert_eq!(
        seen.dirty, 0,
        "{} of {} freed blocks held a non-zero byte",
        seen.dirty, seen.freed
    );
    assert_eq!(
        seen.held_canary, 0,
        "{} of {} freed blocks still held the canary",
        seen.held_canary, seen.freed
    );
}

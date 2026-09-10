//! Option 2: everything in one Emscripten module.
//!
//! Rust is compiled for `wasm32-unknown-emscripten` and statically linked against the
//! same `libqpdf.a` that option 1 loads as a separate module. One module, one heap, no
//! JS bridge, and no copy between heaps.
//!
//! Two questions this answers that option 1 cannot:
//!   1. Does the link actually succeed, across an emsdk built independently of Rust?
//!   2. Does `catch_unwind` work here? On `wasm32-unknown-unknown` a panic is a trap
//!      and cannot be caught in Rust; ADR 0002's `guard` rule depends on it working.
//!
//! Spike code.

use std::ffi::c_int;
use std::panic::{AssertUnwindSafe, catch_unwind};

// The same C shim as option 1 -- but linked directly rather than reached through JS.
unsafe extern "C" {
    fn qpdf_probe_pages(data: *const u8, len: c_int) -> c_int;
    fn qpdf_probe_version_ok() -> c_int;
}

/// Typed outcome, exactly as `burrow-ops` would express it.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Pages(u32),
    Malformed,
    PasswordRequired,
    Unsupported,
    Internal,
}

fn open(bytes: &[u8]) -> Outcome {
    // No copy across a heap boundary: qpdf reads directly out of Rust's slice, because
    // it is the same linear memory. This is option 2's central claim.
    let rc = unsafe { qpdf_probe_pages(bytes.as_ptr(), bytes.len() as c_int) };
    match rc {
        n if n >= 0 => Outcome::Pages(n as u32),
        -1 => Outcome::Malformed,
        -2 => Outcome::PasswordRequired,
        -3 => Outcome::Unsupported,
        _ => Outcome::Internal,
    }
}

fn main() {
    let corpus = std::env::args().nth(1).unwrap_or_else(|| "corpus".to_owned());

    println!("shim reachable: {}", unsafe { qpdf_probe_version_ok() } == 1);

    for name in [
        "small-1page.pdf",
        "medium-100page.pdf",
        "large-50mb.pdf",
        "malformed-truncated.pdf",
        "malformed-badxref.pdf",
        "malformed-notpdf.bin",
    ] {
        let path = format!("{corpus}/{name}");
        match std::fs::read(&path) {
            Ok(bytes) => {
                let n = bytes.len();
                let out = open(&bytes);
                println!("  {name:<24} {n:>10} B  {out:?}");
            }
            Err(e) => println!("  {name:<24} (unreadable: {e})"),
        }
    }

    // The catch_unwind question. If this prints "caught", then ADR 0002's guard rule
    // holds on the web under option 2 -- which it cannot under option 1.
    let caught = catch_unwind(AssertUnwindSafe(|| {
        panic!("spike: deliberate panic on wasm32-unknown-emscripten");
    }));
    println!(
        "catch_unwind result: {}",
        if caught.is_err() { "CAUGHT (unwinding works)" } else { "no panic?!" }
    );

    // And is the module still usable afterwards?
    println!("shim still reachable after catch: {}", unsafe { qpdf_probe_version_ok() } == 1);
    let after = open(&std::fs::read(format!("{corpus}/medium-100page.pdf")).unwrap_or_default());
    println!("open after caught panic: {after:?}");
}

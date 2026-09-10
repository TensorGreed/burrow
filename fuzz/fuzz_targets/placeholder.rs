//! Placeholder so `fuzz/` is a valid crate before there is anything to fuzz.
//!
//! Real targets land in M1, one per parser entry point, starting with document open.
//! Each will look like:
//!
//! ```ignore
//! #![no_main]
//! use libfuzzer_sys::fuzz_target;
//!
//! fuzz_target!(|data: &[u8]| {
//!     // Must never panic, hang, or exceed Limits, whatever `data` contains.
//!     let _ = burrow_core::ops::open(data, &burrow_core::Limits::default());
//! });
//! ```
//!
//! See `.claude/skills/add-operation/SKILL.md`.

fn main() {
    eprintln!("no fuzz targets yet; see docs/ROADMAP.md (M1)");
}

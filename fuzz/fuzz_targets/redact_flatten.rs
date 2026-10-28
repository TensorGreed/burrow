//! Fuzz redaction v1's page replacement (ADR 0031 §1 step 7), through the operation.
//!
//! ```bash
//! # from fuzz/, after `python3 ../tools/seed-fuzz-corpus.py`
//! LD_LIBRARY_PATH=../engines/vendor/native-$(uname -m)/lib ASAN_OPTIONS=detect_leaks=0 \
//!   cargo +nightly fuzz run redact_flatten -- -max_total_time=60 -timeout=10 -rss_limit_mb=2048
//! ```
//!
//! The input is a whole document. The first page with a frame is flattened with a picture of
//! exactly the size its frame needs, so the run reaches the write and the read-back rather than
//! stopping at the size check.
//!
//! # The claims
//!
//! 1. **No panic, no hang, no abort**, on any document: the open's forced resolution and the
//!    untrapped constructors it guards are what this exercises hardest.
//! 2. **If it succeeded, the output has the input's page count and every page's frame**, read
//!    here through the operation's own frame reading, independently of the read-back inside it.
//! 3. **A failure is a typed error**, which is any `Err`.

#![no_main]

use std::sync::{Arc, Once};

use burrow_engines::OpenOptions;
use burrow_engines::qpdf::Qpdf;
use burrow_engines::redact::flatten::{Picture, pixels_for};
use burrow_engines::redact::frames::PageFrameReading;
use burrow_types::{Limits, ManualClock};
use libfuzzer_sys::fuzz_target;

/// Tight limits, so a limit escape shows up as a wrong answer rather than as an out-of-memory.
/// `max_pixels` keeps each picture under 3 MB.
fn limits() -> Limits {
    Limits::with(|l| {
        l.max_input_bytes = 8 * 1024 * 1024;
        l.max_memory_bytes = 512 * 1024 * 1024;
        l.max_pages = 512;
        l.max_duration_ms = 5_000;
        l.max_pixels = 1_000_000;
    })
}

fn make_panics_fatal() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            previous(info);
            std::process::abort();
        }));
    });
}

fn frames(bytes: &[u8], options: &OpenOptions<'_>) -> Option<Vec<(f64, f64)>> {
    let readings = burrow_ops::redact::page_frames(&Qpdf::new(), bytes, options).ok()?;
    Some(
        readings
            .iter()
            .map(|reading| match reading {
                PageFrameReading::Frame { width, height } => (*width, *height),
                _ => (f64::NAN, f64::NAN),
            })
            .collect(),
    )
}

fuzz_target!(|data: &[u8]| {
    make_panics_fatal();
    let options = OpenOptions::new(limits(), Arc::new(ManualClock::new(0)));
    let Some(before) = frames(data, &options) else {
        return;
    };
    let Some((page, (width, height))) = before
        .iter()
        .copied()
        .enumerate()
        .find(|(_, (width, _))| width.is_finite())
    else {
        return;
    };
    let (Ok(wide), Ok(high)) = (pixels_for(width), pixels_for(height)) else {
        return;
    };
    let pixels = u64::from(wide) * u64::from(high);
    if pixels > limits().max_pixels {
        return;
    }
    let Ok(length) = usize::try_from(pixels * 3) else {
        return;
    };
    let rgb: Vec<u8> = (0..length).map(|i| (i % 253) as u8).collect();
    let picture = Picture::new(wide, high, &rgb);
    let Ok(output) = burrow_ops::redact::flatten::page(&Qpdf::new(), data, page, &picture, &options)
    else {
        return;
    };
    let after = frames(&output, &options).expect("a flattened output reads back");
    assert_eq!(after.len(), before.len(), "the page count changed");
    for (index, (was, is)) in before.iter().zip(&after).enumerate() {
        let same = (was.0.is_nan() && is.0.is_nan())
            || (was.0.to_bits() == is.0.to_bits() && was.1.to_bits() == is.1.to_bits());
        assert!(same, "page {index}'s frame changed");
    }
});

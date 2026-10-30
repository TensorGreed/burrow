//! The flatten's `max_memory_bytes`, end to end, at redaction's own pixel ceiling.
//!
//! **Detected, not bounded** (ADR 0007's 2026-09-12 amendment): the resident set is read before
//! the flatten opens the document and compared after the write and after the read-back. A page at
//! [`MAX_FLATTEN_PIXELS`] with a picture that does not compress is the costliest input the slice
//! accepts, so it is the one that shows the comparison firing.
//!
//! PROCESS-ISOLATED, BECAUSE THE RESIDENT SET IS PROCESS-WIDE: this file is its own test binary,
//! with one test in it, so nothing else allocates between the two readings.

#![cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    reason = "tests"
)]

use std::sync::Arc;

use burrow_engines::qpdf::Qpdf;
use burrow_engines::redact::flatten::{MAX_FLATTEN_PIXELS, Picture, pixels_for};
use burrow_engines::{OpenOptions, PageFlattener};
use burrow_types::{Clock, Deadline, Error, Limits, ManualClock, Result, Stage};

/// One page of `media_box`, drawing one line, in a minimal document.
fn one_page(media_box: &str) -> Vec<u8> {
    let line = "0 0 m 10 10 l S";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox {media_box} /Resources << >> /Contents 4 0 R >>"
        ),
        format!(
            "<< /Length {} >>\nstream\n{line}\nendstream",
            line.len() + 1
        ),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

fn flatten(bytes: &[u8], picture: &Picture<'_>, limits: Limits) -> Result<Vec<u8>> {
    let options = OpenOptions::new(limits, Arc::new(ManualClock::new(0)) as Arc<dyn Clock>);
    let deadline = Deadline::start(options.clock.as_ref(), &options.limits);
    Qpdf.flatten_page(bytes, 0, picture, &options, &deadline)
}

/// A page exactly at the ceiling, with pixels a generator makes incompressible: it flattens under
/// the default limits, and the same flatten under a lowered `max_memory_bytes` is refused with
/// that limit, at the measured stage, after the work has been done. KILLS: deleting the
/// measurement after the write.
#[test]
fn the_ceiling_page_flattens_and_a_lowered_memory_limit_detects_its_cost() {
    let bytes = one_page("[0 0 2992.5 6888.25]");
    let (wide, high) = (pixels_for(2992.5).unwrap(), pixels_for(6888.25).unwrap());
    assert_eq!(u64::from(wide) * u64::from(high), MAX_FLATTEN_PIXELS);
    let mut rgb = vec![0_u8; (u64::from(wide) * u64::from(high) * 3) as usize];
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    for byte in &mut rgb {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = (state >> 24) as u8;
    }
    let picture = Picture::new(wide, high, &rgb);

    let lowered = Limits::with(|limits| limits.max_memory_bytes = 128 * 1024 * 1024);
    match flatten(&bytes, &picture, lowered) {
        Err(Error::LimitExceeded {
            stage: Stage::Measured,
            limit: "max_memory_bytes",
            requested,
            ..
        }) => eprintln!("measured growth under the lowered limit: {requested} bytes"),
        other => panic!(
            "expected the measured memory refusal, got {:?}",
            other.map(|o| o.len())
        ),
    }

    let output = flatten(&bytes, &picture, Limits::DEFAULT).expect("the ceiling page flattens");
    assert!(
        output.len() > rgb.len(),
        "the picture did not stay incompressible"
    );
}

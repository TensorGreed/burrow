//! Redaction v1's page replacement end to end, on the real qpdf, through the operation
//! (ADR 0031 §1 step 7; `burrow_ops::redact::flatten`).
//!
//! The engine's own read-back (V1's frames, V2, V3) and its lying-write tests are in
//! `burrow_engines::redact::flatten`. What this adds is the operation's half -- the page count and
//! the rotation vector with page N's entry 0 -- on real documents, and a census of the committed
//! conformance fixtures: how many flatten, so the fuzz target's seeds are known to reach `Ok`.

#![cfg(all(feature = "native-engines", target_os = "linux"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use std::sync::Arc;

use burrow_engines::qpdf::Qpdf;
use burrow_engines::redact::flatten::{Picture, pixels_for};
use burrow_engines::redact::frames::PageFrameReading;
use burrow_engines::{OpenOptions, PageRotator};
use burrow_types::{Clock, Limits, ManualClock, Result};

fn options() -> OpenOptions<'static> {
    OpenOptions::new(
        Limits::DEFAULT,
        Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
    )
}

fn frames(bytes: &[u8]) -> Result<Vec<Option<(f64, f64)>>> {
    Ok(
        burrow_ops::redact::page_frames(&Qpdf::new(), bytes, &options())?
            .into_iter()
            .map(|reading| match reading {
                PageFrameReading::Frame { width, height } => Some((width, height)),
                _ => None,
            })
            .collect(),
    )
}

fn flatten(bytes: &[u8], page: usize) -> Result<Vec<u8>> {
    let (width, height) = frames(bytes)?[page].expect("the page has a frame");
    let (wide, high) = (pixels_for(width)?, pixels_for(height)?);
    let rgb: Vec<u8> = (0..usize::try_from(u64::from(wide) * u64::from(high) * 3).unwrap())
        .map(|i| (i % 249) as u8)
        .collect();
    burrow_ops::redact::flatten::page(
        &Qpdf::new(),
        bytes,
        page,
        &Picture::new(wide, high, &rgb),
        &options(),
    )
}

fn rotations(bytes: &[u8]) -> Vec<i64> {
    let engine = Qpdf::new();
    let source = PageRotator::open(&engine, bytes.to_vec().into_boxed_slice(), &options()).unwrap();
    (0..engine.pages(&source).unwrap())
        .map(|page| engine.effective_rotation(&source, page).unwrap().degrees())
        .collect()
}

/// A three-page document whose middle page is turned: after flattening it, that page reads
/// `/Rotate 0` and the others keep theirs, and every frame is the input's.
#[test]
fn the_flattened_page_reads_back_unturned_and_the_others_keep_their_turn() {
    let text = |rotate: [i64; 3]| {
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>".to_owned(),
        ];
        for turn in rotate {
            objects.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 500] /Rotate {turn} \
                 /Resources << >> >>"
            ));
        }
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
        for offset in offsets {
            out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
        );
        out
    };
    let input = text([90, 270, 180]);
    let output = flatten(&input, 1).unwrap();
    assert_eq!(rotations(&output), vec![90, 0, 180]);
    assert_eq!(frames(&output).unwrap(), frames(&input).unwrap());
    assert_eq!(
        frames(&output).unwrap()[1],
        Some((500.0, 300.0)),
        "the turned frame, written"
    );
}

/// Every committed conformance fixture whose first page has a frame is flattened. Counted, and
/// held to at least one `Ok`, so the fuzz target seeded from these fixtures is known to reach
/// the write and the read-back rather than only the refusals.
#[test]
fn the_conformance_fixtures_flatten_or_refuse_with_a_typed_error() {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/conformance/fixtures");
    let mut paths: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "pdf"))
        .collect();
    paths.sort();
    let (mut examined, mut flattened, mut unframed) = (0, 0, 0);
    for path in &paths {
        let bytes = std::fs::read(path).unwrap();
        examined += 1;
        let Ok(readings) = frames(&bytes) else {
            unframed += 1;
            continue;
        };
        let Some(page) = readings.iter().position(Option::is_some) else {
            unframed += 1;
            continue;
        };
        if flatten(&bytes, page).is_ok() {
            flattened += 1;
        }
    }
    eprintln!(
        "flatten census: {examined} fixtures examined, {flattened} flattened, {unframed} with no \
         framed page, {} refused",
        examined - flattened - unframed
    );
    assert_eq!(examined, paths.len());
    assert!(
        examined >= 20,
        "only {examined} fixtures found under {}",
        root.display()
    );
    assert!(
        flattened >= 1,
        "no committed fixture flattens, so nothing seeds the write"
    );
}

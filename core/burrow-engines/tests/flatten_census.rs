//! The flatten census HARNESS (ADR 0031 "Census"; PLAN.md S13). Tooling, not a test of the engine.
//!
//! **It measures nothing about acceptance.** It walks every page of every listed document and
//! records, per (set, document, page), whatever the flatten pieces on `main` can produce today:
//! the page's display frame as redaction reads it, the pixel size at 150 dpi, the verdict of each
//! pixel ceiling, and slice 1's page replacement (`PageFlattener::flatten_page`) under each ceiling.
//! Every figure the ADR's census list names whose slice is not built is a column of its own, left
//! empty and named in `COLUMNS`, so an absent figure is visible rather than silently missing.
//!
//! Slice 9a's paint and cross-check (`redact::flatten::paint`, `cross_check`, `pdfium::flatten_render`)
//! are `pub(crate)`: an external test binary cannot call them, so the `s9a_*` columns stay empty and
//! the harness says why in each row's legend (`columns.tsv` beside the output). Reaching them needs
//! a visibility change in product code, which this harness does not make.
//!
//! Ignored, and driven by `~/burrow-census/flatten-census/run.sh` (the script that lists the sets and gates the
//! counts); see `~/burrow-census/flatten-census/DESIGN.md`.
//!
//! Inputs: `BURROW_FLATTEN_CENSUS_LIST`, one `<set>\t<path>` per line. Output:
//! `BURROW_FLATTEN_CENSUS_OUT`, one row per page (or one row per document the frame reader refuses
//! whole, page column `-`), beside `<out>.docs` with one line per document.
#![cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use burrow_engines::qpdf::Qpdf;
use burrow_engines::redact::flatten::{MAX_FLATTEN_PIXELS, Picture, pixels_for};
use burrow_engines::redact::frames::PageFrameReading;
use burrow_engines::{OpenOptions, PageFlattener, PageFrameReader};
use burrow_types::{Clock, Deadline, Limits, SystemClock};

/// The web's pixel ceiling (`max_pixels` on the web build; ADR 0031 decision 7).
const WEB_PIXELS: u64 = 4_194_304;

/// `(column, slice that fills it, what it is)`. The first block is filled today; the rest is empty
/// by design. Written to `<out>.columns.tsv` so the output file names what it lacks.
const COLUMNS: &[(&str, &str, &str)] = &[
    ("set", "harness", "which document set the row came from"),
    ("doc", "harness", "path as listed"),
    (
        "pages",
        "harness",
        "page count the frame reader opened the document with",
    ),
    (
        "page",
        "harness",
        "0-based page, or - for a document refused whole",
    ),
    (
        "frame",
        "S1",
        "frame:<w>x<h> in points, refused:<rule>, or doc:<outcome> for a whole-document refusal",
    ),
    ("px_w", "S1", "pixel width at 150 dpi (pixels_for)"),
    ("px_h", "S1", "pixel height at 150 dpi (pixels_for)"),
    ("pixels", "S1", "px_w * px_h"),
    ("ceil_web", "S1", "ok or over: pixels against 4,194,304"),
    ("ceil_native", "S1", "ok or over: pixels against 89,478,485"),
    (
        "s1_native",
        "S1",
        "flatten_page under the native ceiling: ok or the [tag] / err:<kind>",
    ),
    (
        "s1_web",
        "S1",
        "flatten_page under the web ceiling: =native when pixels fit the web ceiling (same code path), else its own call",
    ),
    (
        "out_bytes",
        "S1",
        "length of the flattened output under the native ceiling",
    ),
    ("out_fnv", "S1", "FNV-1a 64 of that output"),
    (
        "s1_ms",
        "timing",
        "flatten_page wall time under the native ceiling (excluded from the determinism compare)",
    ),
    (
        "s9a_paint",
        "S9a",
        "EMPTY: paint_rectangle is pub(crate), unreachable from an external test",
    ),
    (
        "s9a_cross_check",
        "S9a",
        "EMPTY: cross_check is pub(crate), unreachable from an external test",
    ),
    ("render_ms", "S9", "EMPTY: render time per page"),
    (
        "write_ms",
        "S9",
        "EMPTY: write time per page (s1_ms is the S1 write alone)",
    ),
    ("v6_ms", "S8", "EMPTY: V6 time per page"),
    ("form_field_on_page", "S5", "EMPTY"),
    ("xfa_form", "S5", "EMPTY"),
    ("annotation_dependent_kept", "S5", "EMPTY"),
    ("structure_tree_kept", "S5", "EMPTY"),
    ("metadata_thumbnail", "S5", "EMPTY"),
    (
        "drawing_survives",
        "S4",
        "EMPTY: by producer and role, incl. decision 69's wider refusal",
    ),
    (
        "streams_multi_role",
        "S4",
        "EMPTY: decision 71 item 1, both readings",
    ),
    ("cidset_cidtogid", "S4", "EMPTY: decision 71 item 2"),
    ("survivor_roles", "S4", "EMPTY: disclosed survivors by role"),
    ("object_walk_count", "S6", "EMPTY: objects the walks meet"),
    ("page_tree_ambiguous", "S3", "EMPTY"),
    ("page_identity_mismatch", "S7", "EMPTY"),
    (
        "pdfium_noload_size_rotate",
        "S7",
        "EMPTY: whether PDFium's no-load size reflects /Rotate",
    ),
    (
        "frames_32_vs_64",
        "S3",
        "EMPTY: PDFium 32-bit frames against qpdf's",
    ),
    ("render_determinism", "S9", "EMPTY: three ways"),
    ("five_reader_twin", "S10", "EMPTY"),
    (
        "paint_vs_pdfium_render",
        "S9",
        "EMPTY: page N render against the painted bitmap",
    ),
    (
        "dest_into_page_n",
        "S5",
        "EMPTY: every destination into page N",
    ),
];

fn outcome<T>(result: &burrow_types::Result<T>) -> String {
    match result {
        Ok(_) => "ok".to_owned(),
        Err(error) => {
            let text = error.to_string();
            match (text.find('['), text.find(']')) {
                (Some(a), Some(b)) if b > a => text[a..=b].to_owned(),
                _ => format!("err:{}", text.split(':').next().unwrap_or("?").trim()),
            }
        }
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn options(max_pixels: u64) -> OpenOptions<'static> {
    OpenOptions::new(
        Limits::with(|limits| limits.max_pixels = max_pixels),
        Arc::new(SystemClock::new()) as Arc<dyn Clock>,
    )
}

fn flatten(
    bytes: &[u8],
    page: usize,
    picture: &Picture<'_>,
    max_pixels: u64,
) -> burrow_types::Result<Vec<u8>> {
    let options = options(max_pixels);
    let deadline = Deadline::start(options.clock.as_ref(), &options.limits);
    Qpdf.flatten_page(bytes, page, picture, &options, &deadline)
}

/// Cells a row fills; `row_of` pads the rest with empty cells, one per unbuilt figure.
const FILLED: usize = 15;

fn row_of(mut cells: Vec<String>) -> String {
    assert!(cells.len() <= FILLED);
    cells.resize(COLUMNS.len(), String::new());
    cells.join("\t")
}

#[test]
#[ignore = "local census harness"]
fn census() {
    let list = std::env::var("BURROW_FLATTEN_CENSUS_LIST").unwrap();
    let out_path = std::env::var("BURROW_FLATTEN_CENSUS_OUT").unwrap();
    let mut inputs: Vec<(String, PathBuf)> = Vec::new();
    for line in std::fs::read_to_string(list).unwrap().lines() {
        if let Some((set, path)) = line.split_once('\t') {
            inputs.push((set.to_owned(), PathBuf::from(path)));
        }
    }
    let mut out = std::fs::File::create(&out_path).unwrap();
    let mut docs = std::fs::File::create(format!("{out_path}.docs")).unwrap();
    let mut columns = std::fs::File::create(format!("{out_path}.columns.tsv")).unwrap();
    for (name, slice, what) in COLUMNS {
        writeln!(columns, "{name}\t{slice}\t{what}").unwrap();
    }
    let header: Vec<&str> = COLUMNS.iter().map(|(name, _, _)| *name).collect();
    writeln!(out, "{}", header.join("\t")).unwrap();

    let mut seen = BTreeSet::new();
    let (mut examined, mut rows) = (0_u64, 0_u64);
    for (set, path) in &inputs {
        if !seen.insert((set.clone(), path.clone())) {
            continue;
        }
        examined += 1;
        let bytes = std::fs::read(path).unwrap();
        let name = path.display().to_string();
        let frames = match Qpdf.page_frames(&bytes, &options(Limits::DEFAULT.max_pixels)) {
            Ok(frames) => frames,
            Err(error) => {
                let verdict = outcome::<()>(&Err(error));
                writeln!(docs, "{set}\t{name}\t0\topen:{verdict}").unwrap();
                let cells = vec![
                    set.clone(),
                    name,
                    "0".into(),
                    "-".into(),
                    format!("doc:{verdict}"),
                ];
                writeln!(out, "{}", row_of(cells)).unwrap();
                rows += 1;
                continue;
            }
        };
        writeln!(docs, "{set}\t{name}\t{}\topen:ok", frames.pages).unwrap();
        for (page, reading) in frames.readings.iter().enumerate() {
            let mut cells = vec![
                set.clone(),
                name.clone(),
                frames.pages.to_string(),
                page.to_string(),
            ];
            match reading {
                PageFrameReading::Refused { rule, .. } => {
                    cells.push(format!("refused:{}", rule.rule()));
                }
                PageFrameReading::Frame { width, height } => {
                    cells.push(format!("frame:{width}x{height}"));
                    if let (Ok(w), Ok(h)) = (pixels_for(*width), pixels_for(*height)) {
                        let pixels = u64::from(w) * u64::from(h);
                        cells.extend([
                            w.to_string(),
                            h.to_string(),
                            pixels.to_string(),
                            if pixels <= WEB_PIXELS { "ok" } else { "over" }.to_owned(),
                            if pixels <= MAX_FLATTEN_PIXELS {
                                "ok"
                            } else {
                                "over"
                            }
                            .to_owned(),
                        ]);
                        // A zeroed buffer is calloc-lazy, so a ceiling-sized refusal costs no touch.
                        let rgb = vec![0_u8; usize::try_from(pixels * 3).unwrap()];
                        let picture = Picture::new(w, h, &rgb);
                        let start = Instant::now();
                        let native = flatten(&bytes, page, &picture, MAX_FLATTEN_PIXELS);
                        let ms = start.elapsed().as_millis();
                        cells.push(outcome(&native));
                        cells.push(if pixels <= WEB_PIXELS {
                            "=native".to_owned()
                        } else {
                            outcome(&flatten(&bytes, page, &picture, WEB_PIXELS))
                        });
                        match &native {
                            Ok(output) => {
                                cells.push(output.len().to_string());
                                cells.push(format!("{:016x}", fnv1a(output)));
                            }
                            Err(_) => cells.extend([String::new(), String::new()]),
                        }
                        cells.push(ms.to_string());
                    }
                }
                _ => cells.push("reading:unknown".to_owned()),
            }
            writeln!(out, "{}", row_of(cells)).unwrap();
            rows += 1;
        }
    }
    eprintln!("flatten census: {examined} documents examined, {rows} rows written");
}

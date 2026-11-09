//! The flatten census HARNESS (ADR 0031 "Census"; PLAN.md S13). Tooling, not a test of the engine.
//!
//! **It measures nothing about acceptance.** It walks every page of every listed document and
//! records, per (set, document, page), whatever the flatten pieces on `main` can produce today:
//! the page's display frame as redaction reads it, the pixel size at 150 dpi, the verdict of each
//! pixel ceiling, and slice 1's page replacement (`PageFlattener::flatten_page`) under each ceiling.
//! Every figure the ADR's census list names whose slice is not built is a column of its own, left
//! empty and named in `COLUMNS`, so an absent figure is visible rather than silently missing.
//!
//! Slice 9a's columns come from the crate's own `pub(crate)` items, which is why this harness is
//! an in-crate `#[cfg(test)]` module rather than a `tests/` binary (owner (103), 2026-11-09): page
//! N's frame as qpdf reads it (`redact::frame::of`), the paint in PDFium's heap
//! (`pdfium::flatten_render::render_painted`), and the cross-check on the painted picture
//! (`redact::flatten::cross_check::check_paint`). The region painted on every page is fixed and
//! named in `columns.tsv`: [`census_region`]. No product item's visibility changes for this.
//!
//! Ignored, and driven by `~/burrow-census/flatten-census/run.sh` (the script that lists the sets
//! and gates the counts) with `cargo test -p burrow-engines --lib --features native-engines --
//! --ignored census`; see `~/burrow-census/rounds/flatten-census-harness/DESIGN.md`.
//!
//! Inputs: `BURROW_FLATTEN_CENSUS_LIST`, one `<set>\t<path>` per line. Output:
//! `BURROW_FLATTEN_CENSUS_OUT`, one row per page (or one row per document the frame reader refuses
//! whole, page column `-`), beside `<out>.docs` with one line per document.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test-only census harness"
)]

use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use burrow_types::{Clock, Deadline, Limits, SystemClock};

use super::render_painted;
use crate::pdfium::{Pdfium, PdfiumDocument};
use crate::pdfsyntax::region::Region;
use crate::qpdf::Qpdf;
use crate::redact::flatten::cross_check::check_paint;
use crate::redact::flatten::{MAX_FLATTEN_PIXELS, Picture, pixels_for};
use crate::redact::frames::PageFrameReading;
use crate::redact::graph::{OpensForRedaction, PdfDocument};
use crate::{DocumentEngine, OpenOptions, PageFlattener, PageFrameReader};

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
        "render_painted under the native ceiling, region census_region (left 0.31w+0.137, top 0.23h+0.211, 0.2w x 0.1h display points): ok:<l>,<t>,<r>,<b> (the painted pixel rectangle), the [tag] / err:<kind>, frame:<outcome> when redact::frame::of refuses page N, or open:<outcome> when PDFium refuses the document",
    ),
    (
        "s9a_cross_check",
        "S9a",
        "check_paint on that picture under the native ceiling: ok or the [tag] / err:<kind>; - when there was no picture",
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
        Err(error) => refusal(error),
    }
}

/// A refusal's `[tag]`, or `err:<kind>` when it carries none.
fn refusal(error: &burrow_types::Error) -> String {
    let text = error.to_string();
    match (text.find('['), text.find(']')) {
        (Some(a), Some(b)) if b > a => text[a..=b].to_owned(),
        _ => format!("err:{}", text.split(':').next().unwrap_or("?").trim()),
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
const FILLED: usize = 17;

/// The region the census paints on every page: inside the display frame `dw` x `dh`, off every
/// centre line, edges between pixels (the shape `flatten_render_tests::region_in` uses).
fn census_region(dw: f64, dh: f64) -> Region {
    Region {
        left: dw * 0.31 + 0.137,
        top: dh * 0.23 + 0.211,
        width: dw * 0.2,
        height: dh * 0.1,
    }
}

/// Slice 9a's two cells for page `page`: the paint, then the cross-check.
fn s9a<D: PdfDocument>(
    page: usize,
    redaction: &burrow_types::Result<D>,
    pdfium: &burrow_types::Result<PdfiumDocument>,
) -> [String; 2] {
    let no_picture = "-".to_owned();
    let document = match redaction {
        Ok(document) => document,
        Err(error) => return [format!("frame:{}", refusal(error)), no_picture],
    };
    let frame = match document
        .page(page)
        .and_then(|object| crate::redact::frame::of(&object))
    {
        Ok(frame) => frame,
        Err(error) => return [format!("frame:{}", refusal(&error)), no_picture],
    };
    let source = match pdfium {
        Ok(source) => source,
        Err(error) => return [format!("open:{}", refusal(error)), no_picture],
    };
    let (dw, dh) = frame.displayed_size();
    let region = census_region(dw, dh);
    let options = options(MAX_FLATTEN_PIXELS);
    let deadline = Deadline::start(options.clock.as_ref(), &options.limits);
    let painted = match render_painted(source, page as u64, &frame, &region, &options, &deadline) {
        Ok(painted) => painted,
        Err(error) => return [refusal(&error), no_picture],
    };
    let rect = painted.painted;
    let paint = format!(
        "ok:{},{},{},{}",
        rect.left, rect.top, rect.right, rect.bottom
    );
    let check = check_paint((dw, dh), &painted.picture(), &region, &options.limits);
    [paint, outcome(&check)]
}

fn row_of(mut cells: Vec<String>) -> String {
    assert!(cells.len() <= FILLED);
    cells.resize(COLUMNS.len(), String::new());
    cells.join("\t")
}

#[test]
#[ignore = "local census harness: run.sh drives it with `-- --ignored census`"]
fn flatten_census() {
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
        // Opened once per document for slice 9a: qpdf's graph (page N's frame as the write and
        // the cross-check read it) and PDFium's document (the paint).
        let redaction = Qpdf
            .open_for_redaction(&bytes, &options(MAX_FLATTEN_PIXELS))
            .map(|(document, _)| document);
        let pdfium = Pdfium::new().open(
            bytes.clone().into_boxed_slice(),
            &options(MAX_FLATTEN_PIXELS),
        );
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
                        cells.extend(s9a(page, &redaction, &pdfium));
                    }
                }
            }
            writeln!(out, "{}", row_of(cells)).unwrap();
            rows += 1;
        }
    }
    eprintln!("flatten census: {examined} documents examined, {rows} rows written");
}

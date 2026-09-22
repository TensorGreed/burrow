//! The real resolver, driven over the committed producer corpus.
//!
//! Every test on #131 ran against a fake `Resources` that returned the same width for every
//! code and never refused. This is the first time the walk has met a real document, and the
//! point of these tests is to record **what the fake got wrong** rather than to assert that
//! everything works.

use std::sync::Arc;

use burrow_types::{Clock, Limits, ManualClock};

use super::handle::ObjectHandle;
use super::resources::PageResources;
use super::{Document, open_document};
use crate::OpenOptions;
use crate::pdfsyntax::geometry::glyphs_in;

fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/redaction/fixtures")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn open(bytes: Vec<u8>) -> Document {
    open_document(
        bytes.into_boxed_slice(),
        &OpenOptions::new(
            Limits::default(),
            Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
        ),
    )
    .expect("the fixture opens")
    .0
}

/// Walk page 0 of a fixture with the real resolver, returning the glyph count or the refusal.
fn walk(name: &str) -> Result<usize, String> {
    let document = open(fixture(name));
    // SAFETY: page 0 exists in every fixture here, which the assertion below would catch.
    let page = unsafe { ObjectHandle::page(&document, 0) };
    let content = page.page_content().map_err(|error| format!("{error:?}"))?;
    let resources = PageResources::of(&page).map_err(|error| format!("{error:?}"))?;
    glyphs_in(&content, &resources)
        .map(|glyphs| glyphs.len())
        .map_err(|error| format!("{error:?}"))
}

#[test]
fn what_the_real_corpus_does() {
    // NOT AN ASSERTION THAT IT ALL WORKS. This prints the outcome per fixture so the report
    // is a measurement; the assertions below are about specific findings.
    for name in [
        "producer-writer.pdf",
        "producer-latex.pdf",
        "producer-ocr-scan.pdf",
        "producer-vertical-writing.pdf",
    ] {
        match walk(name) {
            Ok(count) => eprintln!("  {name:<34} {count} glyph(s)"),
            Err(error) => eprintln!("  {name:<34} REFUSED {error}"),
        }
    }
}

/// How often a document carries no metrics of its own, across every committed fixture.
///
/// # A refusal this common is a decision, not just a correct answer
///
/// A standard-14 font with no `/Widths` has its advances in the viewer, not the file. Refusing
/// is right — a guess misplaces every glyph by the difference — but if it fires on most real
/// documents then redaction refuses most real documents, and that is a product decision rather
/// than an implementation detail. So it is counted rather than assumed rare.
#[test]
fn how_often_the_missing_metrics_refusal_fires() {
    let mut walked = Vec::new();
    let mut no_widths = Vec::new();
    let mut other = Vec::new();

    for directory in [
        "../../tests/redaction/fixtures",
        "../../tests/conformance/fixtures",
    ] {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(directory);
        let Ok(entries) = std::fs::read_dir(&base) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("pdf") {
                continue;
            }
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            match walk_bytes(&bytes) {
                Ok(count) => walked.push((name, count)),
                Err(error) if error.contains("[no-widths]") => no_widths.push(name),
                Err(error) => other.push((name, error)),
            }
        }
    }

    eprintln!("\n  MISSING-METRICS SURVEY over every committed fixture");
    eprintln!("  walked cleanly          : {}", walked.len());
    for (name, count) in &walked {
        eprintln!("      {name:<38} {count} glyph(s)");
    }
    eprintln!("  refused for no `/Widths`: {}", no_widths.len());
    for name in &no_widths {
        eprintln!("      {name}");
    }
    eprintln!("  refused for other reasons: {}", other.len());
    for (name, error) in &other {
        let rule = error
            .split_once('[')
            .and_then(|(_, rest)| rest.split_once(']'))
            .map_or("(unnamed)", |(rule, _)| rule);
        eprintln!("      {name:<38} {rule}");
    }

    // THE NON-VACUITY CONTROL. A survey that examined nothing would report zero of everything
    // and read as "the refusal never fires".
    let total = walked.len() + no_widths.len() + other.len();
    assert!(
        total >= 20,
        "the survey examined {total} fixture(s), which is too few to be both corpora"
    );
}

/// Walk a document's first page, reporting the refusal as text.
///
/// Opens fallibly: the conformance corpus carries deliberately damaged fixtures — truncated
/// files, an xref bomb, a thing that is not a PDF at all — and a survey that panicked on the
/// first of them would report the refusal rate over the fixtures before it.
fn walk_bytes(bytes: &[u8]) -> Result<usize, String> {
    let (document, _, _, _) = open_document(
        bytes.to_vec().into_boxed_slice(),
        &OpenOptions::new(
            Limits::default(),
            Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
        ),
    )
    .map_err(|error| format!("[unopenable] {error:?}"))?;
    // SAFETY: guarded by the page count below.
    let pages = document.page_count().map_err(|e| format!("{e:?}"))?;
    if pages == 0 {
        return Err("[no-pages]".to_owned());
    }
    let page = unsafe { ObjectHandle::page(&document, 0) };
    let content = page.page_content().map_err(|e| format!("{e:?}"))?;
    let resources = PageResources::of(&page).map_err(|e| format!("{e:?}"))?;
    glyphs_in(&content, &resources)
        .map(|glyphs| glyphs.len())
        .map_err(|e| format!("{e:?}"))
}

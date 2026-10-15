//! Every page's frame, natively and on the web over real qpdf (#136, slice S2).
//!
//! Two halves:
//!
//! - **One refusal per rule, each beside its twin**, on both engines: a page that differs from the
//!   refused one by exactly the key the rule is about, and reads as a frame. A rule whose twin also
//!   refused would be a refusal of everything; one whose fixture read as a frame would be a rule
//!   that matches nothing.
//! - **The redaction corpus, natively against the web**: every document `tests/redaction/
//!   outcomes.tsv` records, its frames equal and its refusals by the same rule on both engines --
//!   with one divergence the engines are meant to have, counted and named: a `/MediaBox` from the
//!   page tree, which PDFium vouches for natively and nothing can on the web yet (#206). And each
//!   reading is held to the redaction outcome the golden pinned for the same page, so a frame here
//!   is the frame the operation used, not one read beside it.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout,
    reason = "test support over committed fixtures; the workspace lints are for library code"
)]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use burrow_types::{Clock, Error, Limits, ManualClock, Result};

use super::web_differential_tests::NativeBridge;
use crate::redact::frames::{FrameRule, PageFrameReading};
use crate::{OpenOptions, PageFrameReader};

fn options(limits: Limits) -> OpenOptions<'static> {
    OpenOptions::new(limits, Arc::new(ManualClock::new(0)) as Arc<dyn Clock>)
}

fn repository() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

/// A document of `objects`, numbered from 1, with a correct cross-reference table.
fn pdf_of(objects: &[String]) -> Vec<u8> {
    let mut out = String::from("%PDF-1.7\n");
    let mut offsets = Vec::new();
    for (index, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.push_str(&format!("{} 0 obj\n{body}\nendobj\n", index + 1));
    }
    let xref_at = out.len();
    out.push_str(&format!(
        "xref\n0 {}\n0000000000 65535 f \n",
        objects.len() + 1
    ));
    for offset in &offsets {
        out.push_str(&format!("{offset:010} 00000 n \n"));
    }
    out.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
        objects.len() + 1
    ));
    out.into_bytes()
}

/// One page per entry of `pages`, each that entry spliced into its dictionary, under one `/Pages`
/// node carrying `parent`. Every page draws the same text, which nothing here reads.
fn document(parent: &str, pages: &[&str]) -> Vec<u8> {
    let content = "BT /F1 12 Tf 20 350 Td (SECRET) Tj ET\n";
    let first_page = 5;
    let kids: Vec<String> = (0..pages.len())
        .map(|at| format!("{} 0 R", first_page + at))
        .collect();
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        format!(
            "<< /Type /Pages /Count {} /Kids [{}] {parent} >>",
            pages.len(),
            kids.join(" ")
        ),
        format!(
            "<< /Length {} >>\nstream\n{content}endstream",
            content.len()
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_owned(),
    ];
    for extra in pages {
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /Contents 3 0 R \
             /Resources << /Font << /F1 4 0 R >> >> {extra} >>"
        ));
    }
    pdf_of(&objects)
}

/// One reading, as a line two engines can be compared by.
fn line(reading: &PageFrameReading) -> String {
    match reading {
        PageFrameReading::Frame { width, height } => format!("FRAME {width} x {height}"),
        PageFrameReading::Refused { rule, error } => {
            format!("REFUSED {} {:?}", rule.rule(), error)
        }
    }
}

/// A whole answer, as one line per page or the error.
fn answer(result: &Result<Vec<PageFrameReading>>) -> Vec<String> {
    match result {
        Ok(readings) => readings.iter().map(line).collect(),
        Err(error) => vec![format!(
            "ERR {}",
            format!("{error:?}").replace(['\n', '\t'], " ")
        )],
    }
}

/// Both engines' answers for `bytes`, natively first.
fn on_both(bytes: &[u8]) -> (Vec<String>, Vec<String>) {
    let options = options(Limits::default());
    let natively = answer(&super::Qpdf.page_frames(bytes, &options));
    let web = crate::web::WebQpdf::new(Arc::new(NativeBridge::new()));
    let on_the_web = answer(&web.page_frames(bytes, &options));
    (natively, on_the_web)
}

/// A page refused by `rule` on both engines, and its twin read as `frame` on both.
fn refused_beside_its_twin(rule: FrameRule, parent: &str, refused: &str, twin: &str, frame: &str) {
    let (natively, on_the_web) = on_both(&document(parent, &[refused]));
    let expected = format!("REFUSED {} ", rule.rule());
    assert!(
        natively.len() == 1 && natively[0].starts_with(&expected),
        "{}: natively {natively:?}",
        rule.rule()
    );
    assert_eq!(
        on_the_web,
        natively,
        "{}: the engines disagree",
        rule.rule()
    );
    let (natively, on_the_web) = on_both(&document(parent, &[twin]));
    assert_eq!(natively, vec![frame.to_owned()], "{}'s twin", rule.rule());
    assert_eq!(on_the_web, natively, "{}'s twin on the web", rule.rule());
}

#[test]
fn a_box_rotate_or_unit_in_a_shape_the_renderer_reads_otherwise_is_unreadable() {
    refused_beside_its_twin(
        FrameRule::PageFrameUnreadable,
        "",
        "/MediaBox [0 0 300 400] /Rotate [90]",
        "/MediaBox [0 0 300 400] /Rotate 90",
        "FRAME 400 x 300",
    );
    // A CROP BOX THAT SHARES NO AREA, and its twin that overlaps: clipped to the media box.
    refused_beside_its_twin(
        FrameRule::PageFrameUnreadable,
        "",
        "/MediaBox [0 0 300 400] /CropBox [400 0 500 100]",
        "/MediaBox [0 0 300 400] /CropBox [250 0 500 100]",
        "FRAME 50 x 100",
    );
}

#[test]
fn a_user_unit_other_than_one_is_refused() {
    refused_beside_its_twin(
        FrameRule::UserUnitNotOne,
        "",
        "/MediaBox [0 0 300 400] /UserUnit 2",
        "/MediaBox [0 0 300 400] /UserUnit 1",
        "FRAME 300 x 400",
    );
}

#[test]
fn a_rotate_or_crop_box_from_the_tree_is_refused() {
    // THE PARENT CARRIES IT, AND THE TWIN CARRIES IT ITSELF.
    refused_beside_its_twin(
        FrameRule::PageAttributeInherited,
        "/Rotate 90",
        "/MediaBox [0 0 300 400]",
        "/MediaBox [0 0 300 400] /Rotate 90",
        "FRAME 400 x 300",
    );
    refused_beside_its_twin(
        FrameRule::PageAttributeInherited,
        "/CropBox [0 0 100 100]",
        "/MediaBox [0 0 300 400]",
        "/MediaBox [0 0 300 400] /CropBox [0 0 100 100]",
        "FRAME 100 x 100",
    );
}

/// `/Resources` from the tree refuses every region on the page, and its twin declares them.
#[test]
fn resources_from_the_tree_are_refused() {
    let with = |parent: &str, page: &str| {
        let content = "BT /F1 12 Tf 20 350 Td (SECRET) Tj ET\n";
        pdf_of(&[
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            format!("<< /Type /Pages /Count 1 /Kids [5 0 R] {parent} >>"),
            format!(
                "<< /Length {} >>\nstream\n{content}endstream",
                content.len()
            ),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
                .to_owned(),
            format!(
                "<< /Type /Page /Parent 2 0 R /Contents 3 0 R /MediaBox [0 0 300 400] {page} >>"
            ),
        ])
    };
    let resources = "/Resources << /Font << /F1 4 0 R >> >>";
    let (natively, on_the_web) = on_both(&with(resources, ""));
    assert!(
        natively[0].starts_with("REFUSED page-attribute-inherited ")
            && natively[0].contains("/Resources"),
        "{natively:?}"
    );
    assert_eq!(on_the_web, natively);
    let (natively, on_the_web) = on_both(&with("", resources));
    assert_eq!(natively, vec!["FRAME 300 x 400".to_owned()]);
    assert_eq!(on_the_web, natively);
}

/// A page with no `/MediaBox` anywhere is the whole document's refusal, not the page's: qpdf
/// repairs it while counting the pages -- it warns, and supplies one -- so `[engine-repaired-input]`
/// refuses at the open on both engines, as it refuses the redaction. `[no-display-box]` stays
/// `frame.rs`'s for an engine that does not repair; it is unreachable through qpdf, measured here.
#[test]
fn a_page_with_no_media_box_anywhere_is_refused_as_the_repair_it_is() {
    let (natively, on_the_web) = on_both(&document("", &[""]));
    assert_eq!(natively.len(), 1);
    assert!(
        natively[0].starts_with("ERR ") && natively[0].contains("[engine-repaired-input]"),
        "{natively:?}"
    );
    assert_eq!(on_the_web, natively);
    let (natively, on_the_web) = on_both(&document("", &["/MediaBox [0 0 300 400]"]));
    assert_eq!(natively, vec!["FRAME 300 x 400".to_owned()]);
    assert_eq!(on_the_web, natively);
}

#[test]
fn a_rotate_that_is_not_whole_is_refused() {
    refused_beside_its_twin(
        FrameRule::RotateNotWhole,
        "",
        "/MediaBox [0 0 300 400] /Rotate 90.5",
        "/MediaBox [0 0 300 400] /Rotate 90.0",
        "FRAME 400 x 300",
    );
}

#[test]
fn a_display_box_with_no_extent_is_refused() {
    refused_beside_its_twin(
        FrameRule::DisplayBox,
        "",
        "/MediaBox [0 0 0 400]",
        "/MediaBox [0 0 1 400]",
        "FRAME 1 x 400",
    );
}

/// A `/MediaBox` from the page tree: natively PDFium vouches for it and it is a frame; on the web
/// nothing can yet, and it is `[media-box-unverified]` -- the divergence the two engines are meant
/// to have (#206), and the one the corpus differential below admits. The twin declares the box on
/// the page and reads alike on both.
#[test]
fn a_media_box_from_the_tree_is_a_frame_natively_and_unverified_on_the_web() {
    let inherited = document("/MediaBox [0 0 300 400]", &[""]);
    let (natively, on_the_web) = on_both(&inherited);
    assert_eq!(natively, vec!["FRAME 300 x 400".to_owned()]);
    assert_eq!(on_the_web.len(), 1);
    assert!(
        on_the_web[0].starts_with("REFUSED media-box-unverified "),
        "the web has no renderer to vouch for it: {on_the_web:?}"
    );
    // NATIVELY, A RENDERER THAT DISAGREES is the same refusal: a Letter-sized box away from the
    // origin cannot be told from the renderer's null default by size (`frame.rs`).
    let undecidable = document("/MediaBox [100 0 712 792]", &[""]);
    let (natively, _) = on_both(&undecidable);
    assert!(
        natively[0].starts_with("REFUSED media-box-unverified "),
        "a size cannot decide this box: {natively:?}"
    );
    let declared = document("", &["/MediaBox [0 0 300 400]"]);
    let (natively, on_the_web) = on_both(&declared);
    assert_eq!(natively, vec!["FRAME 300 x 400".to_owned()]);
    assert_eq!(on_the_web, natively);
}

/// Many pages, in order, each answered for itself: one bad page refuses itself and no other, and
/// the renderer -- opened once natively -- answers every inherited page.
#[test]
fn every_page_is_answered_for_itself_and_in_order() {
    let bytes = document(
        "/MediaBox [0 0 612 792]",
        &[
            "/MediaBox [0 0 300 400] /Rotate 270",
            "/MediaBox [0 0 300 400] /UserUnit 3",
            "",
            "/MediaBox [0 0 300 400] /CropBox [10 20 110 220] /Rotate -90",
            "",
        ],
    );
    let (natively, on_the_web) = on_both(&bytes);
    assert_eq!(natively.len(), 5, "{natively:?}");
    assert_eq!(natively[0], "FRAME 400 x 300");
    assert!(natively[1].starts_with("REFUSED user-unit-not-one "));
    assert_eq!(natively[2], "FRAME 612 x 792");
    assert_eq!(natively[3], "FRAME 200 x 100");
    assert_eq!(natively[4], "FRAME 612 x 792");
    for at in [0, 1, 3] {
        assert_eq!(on_the_web[at], natively[at], "page {at}");
    }
    for at in [2, 4] {
        assert!(on_the_web[at].starts_with("REFUSED media-box-unverified "));
    }
}

/// A document the redaction refuses at its open is refused here with the same error, on both
/// engines: a ceiling before the engine, the page ceiling, and the deadline per page.
#[test]
fn the_open_s_refusals_and_every_ceiling_are_the_whole_document_s() {
    let bytes = document("", &["/MediaBox [0 0 300 400]"; 3]);
    let web = crate::web::WebQpdf::new(Arc::new(NativeBridge::new()));
    let both = |limits: Limits| {
        let options = options(limits);
        (
            answer(&super::Qpdf.page_frames(&bytes, &options)),
            answer(&web.page_frames(&bytes, &options)),
        )
    };
    // PAGES: at the ceiling, and one past it.
    let mut limits = Limits::default();
    limits.max_pages = 3;
    let (natively, on_the_web) = both(limits);
    assert_eq!(natively.len(), 3);
    assert_eq!(on_the_web, natively);
    limits.max_pages = 2;
    let (natively, on_the_web) = both(limits);
    assert!(natively[0].contains("max_pages"), "{natively:?}");
    assert_eq!(on_the_web, natively);
    // INPUT BYTES, before the engine.
    let mut limits = Limits::default();
    limits.max_input_bytes = 64;
    let (natively, on_the_web) = both(limits);
    assert!(natively[0].contains("max_input_bytes"), "{natively:?}");
    assert_eq!(on_the_web, natively);
    // THE DEADLINE, on a fresh clock per run that reads a second later each time: one page fits
    // the budget and forty do not, so what refuses is the per-page checkpoint, not the open.
    let mut limits = Limits::default();
    limits.max_duration_ms = 10_000;
    let fresh = || {
        OpenOptions::new(
            limits,
            Arc::new(super::rotate_tests::SteppingClock::new(1_000)) as Arc<dyn Clock>,
        )
    };
    let one = document("", &["/MediaBox [0 0 300 400]"]);
    let forty = document("", &["/MediaBox [0 0 300 400]"; 40]);
    for result in [
        super::Qpdf.page_frames(&one, &fresh()),
        web.page_frames(&one, &fresh()),
    ] {
        assert_eq!(answer(&result), vec!["FRAME 300 x 400".to_owned()]);
    }
    for result in [
        super::Qpdf.page_frames(&forty, &fresh()),
        web.page_frames(&forty, &fresh()),
    ] {
        match result {
            Err(Error::LimitExceeded { limit, stage, .. }) => assert_eq!(
                (limit, stage),
                ("max_duration_ms", burrow_types::Stage::Deadline)
            ),
            other => panic!("the deadline must refuse: {:?}", answer(&other)),
        }
    }
    // A PASSWORD-PROTECTED OR UNREADABLE DOCUMENT is the open's: not a PDF at all.
    let (natively, on_the_web) = on_both(b"not a pdf");
    assert!(natively[0].starts_with("ERR "), "{natively:?}");
    assert_eq!(on_the_web, natively);
}

/// A repair qpdf makes while the pages are read -- after the open's check passed -- refuses the
/// whole document, on both engines. The fixture's page tree points into an object stream qpdf
/// cannot load: the open is clean (the redaction's own open lets it through, and the reference
/// pass refuses it next), and reading the page is what makes qpdf recover.
#[test]
fn a_repair_while_the_pages_are_read_refuses_the_document() {
    use crate::redact::graph::OpensForRedaction;

    let bytes = std::fs::read(
        repository().join("tests/redaction/generated/evade-object-stream-qpdf-cannot-load.pdf"),
    )
    .unwrap();
    // THE OPEN IS CLEAN, or this would be measuring the open's check instead.
    let options = options(Limits::default());
    assert!(super::Qpdf.open_for_redaction(&bytes, &options).is_ok());
    let (natively, on_the_web) = on_both(&bytes);
    assert_eq!(natively.len(), 1, "{natively:?}");
    assert!(
        natively[0].starts_with("ERR ") && natively[0].contains("[engine-repaired-input]"),
        "{natively:?}"
    );
    assert_eq!(on_the_web, natively);
}

/// The redaction corpus: native against web, and each against the pinned redaction outcomes.
#[test]
fn the_web_reads_the_native_frames_over_the_redaction_corpus() {
    let golden =
        std::fs::read_to_string(repository().join("tests/redaction/outcomes.tsv")).unwrap();
    let options = options(Limits::default());
    let web = crate::web::WebQpdf::new(Arc::new(NativeBridge::new()));

    // WHAT THE GOLDEN SAYS EACH PAGE'S REDACTION DID, per document: (page, recorded outcome).
    let mut cases: BTreeMap<String, Vec<(usize, String)>> = BTreeMap::new();
    let mut documents = Vec::new();
    for line in golden.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        match fields[..] {
            ["INPUT", name, _] => documents.push(name.to_owned()),
            ["CASE", name, page, _, _, recorded] => cases
                .entry(name.to_owned())
                .or_default()
                .push((page.parse().unwrap(), recorded.to_owned())),
            _ => {}
        }
    }

    let (mut pages, mut frames, mut refusals, mut whole) = (0usize, 0usize, 0usize, 0usize);
    let mut unverified_on_the_web = 0usize;
    let mut held_to_the_golden = 0usize;
    let mut whole_exact = 0usize;
    let mut divergences = Vec::new();
    let mut against_the_golden = Vec::new();
    for name in &documents {
        let bytes = std::fs::read(repository().join(name)).unwrap();
        let native = super::Qpdf.page_frames(&bytes, &options);
        let on_the_web = web.page_frames(&bytes, &options);
        let (left, right) = (answer(&native), answer(&on_the_web));
        if left.len() != right.len() {
            divergences.push(format!("{name}: native {left:?}\n    web {right:?}"));
            continue;
        }
        for (at, (l, r)) in left.iter().zip(&right).enumerate() {
            // THE ONE DIVERGENCE MEANT: a frame natively that the web cannot vouch for.
            if l != r {
                if l.starts_with("FRAME ") && r.starts_with("REFUSED media-box-unverified ") {
                    unverified_on_the_web += 1;
                } else {
                    divergences.push(format!("{name} page {at}\n    native {l}\n    web    {r}"));
                }
            }
        }
        // AND THE PINNED REDACTION OUTCOME, page by page.
        for (page, recorded) in cases.get(name).map_or(&[][..], Vec::as_slice) {
            let held = match &native {
                // A DOCUMENT REFUSED WHOLE is refused by the redaction too -- by the same error
                // when the refusal is the open's, and possibly by an earlier one when it is the
                // repair qpdf makes lazily while the pages are read, because the redaction runs
                // document-level checks this does not (the reference pass refuses first).
                Err(error) => {
                    let ours = format!("ERR {}", format!("{error:?}").replace(['\n', '\t'], " "));
                    whole_exact += usize::from(ours == *recorded);
                    recorded.starts_with("ERR ").then_some(()).ok_or(ours)
                }
                Ok(readings) => match readings.get(*page) {
                    // A PAGE PAST THE END has no reading; the redaction names its own rule.
                    None => Ok(()),
                    // A REDACTION THAT RAN had a frame for its page.
                    Some(PageFrameReading::Frame { .. }) if recorded.starts_with("OK ") => Ok(()),
                    // A FRAME RULE IN THE OUTCOME is this page's refusal, word for word.
                    Some(PageFrameReading::Refused { error, .. }) => {
                        let ours =
                            format!("ERR {}", format!("{error:?}").replace(['\n', '\t'], " "));
                        // The redaction may refuse earlier, by a document-level rule this reading
                        // does not run; then it names no frame rule at all.
                        let frame_rule = FrameRule::ALL
                            .iter()
                            .any(|rule| recorded.contains(&format!("[{}]", rule.rule())));
                        if ours == *recorded || !frame_rule && recorded.starts_with("ERR ") {
                            Ok(())
                        } else {
                            Err(ours)
                        }
                    }
                    // A FRAME, AND A REDACTION REFUSED: by anything but a frame rule.
                    Some(PageFrameReading::Frame { .. }) => {
                        let frame_rule = FrameRule::ALL
                            .iter()
                            .any(|rule| recorded.contains(&format!("[{}]", rule.rule())));
                        if frame_rule {
                            Err("a frame".to_owned())
                        } else {
                            Ok(())
                        }
                    }
                },
            };
            match held {
                Ok(()) => held_to_the_golden += 1,
                Err(ours) => against_the_golden.push(format!(
                    "{name} page {page}\n    pinned {recorded}\n    frames {ours}"
                )),
            }
        }
        match &native {
            Ok(readings) => {
                pages += readings.len();
                for reading in readings {
                    match reading {
                        PageFrameReading::Frame { .. } => frames += 1,
                        _ => refusals += 1,
                    }
                }
            }
            Err(_) => whole += 1,
        }
    }

    println!(
        "page frames: {} documents, {pages} pages; natively {frames} frames and {refusals} page \
         refusals, {whole} documents refused whole; {unverified_on_the_web} frames the web cannot \
         vouch for (#206); {held_to_the_golden} pinned redaction cases agree, {whole_exact} of \
         them a whole-document refusal word for word",
        documents.len()
    );
    assert!(
        divergences.is_empty(),
        "{} readings diverge between the native and the web engine:\n{}",
        divergences.len(),
        divergences.join("\n")
    );
    assert!(
        against_the_golden.is_empty(),
        "{} readings disagree with the pinned redaction outcome:\n{}",
        against_the_golden.len(),
        against_the_golden.join("\n")
    );
    // THE COUNTS THE GOLDEN SAYS, so a narrower run cannot read as the whole one.
    let recorded_cases: usize = cases.values().map(Vec::len).sum();
    assert_eq!(documents.len(), 183, "the golden's documents");
    assert_eq!(held_to_the_golden, recorded_cases);
    // NON-VACUITY: the corpus reaches frames, page refusals and whole-document refusals alike.
    assert!(
        frames > 0 && refusals > 0 && whole > 0,
        "{frames} {refusals} {whole}"
    );
    drop(web);
}

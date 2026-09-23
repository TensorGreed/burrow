//! The retained-font disclosure count, against an independent tally.
//!
//! # Why this is its own file
//!
//! `FontOutcome::also_used_by` is the only number this operation puts in front of a person:
//! *"four other pages keep using this font"*. Everything else the redaction reports is about
//! the document it produced; this is a claim about the pages it did **not** touch, derived from
//! the sharing walk and checkable against nothing else in the output.
//!
//! An over-count is a person told their redaction is messier than it is. An **under-count tells
//! them it is cleaner**, and that is the direction §7's disclosure exists to prevent. So the
//! count is checked against a tally that does not go through `sharing.rs`: for the corpus, the
//! page count itself, which bounds it absolutely; for the multi-page case, a document built so
//! the answer is known before it is asked.
//!
//! # The corpus is single-page, and that is why the built fixture is here
//!
//! Every document in `tests/redaction/generated` has one page, so the retain path never fires
//! on it — `fonts_wholly_within` is trivially satisfied when the redacted set is every page.
//! A multi-page document redacted on one page is the **ordinary** shape, and it appears nowhere
//! in the corpus, so it is built here.

#![cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "a test asserting against a fixture it built; the workspace lints are for \
              attacker-controlled input in library code, where they stay denied"
)]

mod support;

use std::collections::BTreeSet;

use burrow_engines::pdfsyntax::region::Region;
use support::char_box_oracle::chars_on_page;

/// `/Helvetica`'s widths for the codes these fixtures draw, declared in the file.
///
/// **Declared rather than inherited.** A standard-14 font with no `/Widths` has no advances in
/// the document at all, and burrow refuses it rather than guessing — which would make every
/// assertion below a refusal. Stating them is what a producer does, and what makes this fixture
/// about font sharing rather than about the standard-14 gap.
const WIDTHS: &str = "[556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 \
                      556 556 556 556 556 556 556 556 556 556]";

/// A document of `pages` pages. Every page draws with the font at object 5; page 0 also draws
/// with the font at object 6, which no other page names.
///
/// Two fonts, because one of each outcome is what makes the report's two arms distinguishable:
/// object 5 is used outside the redacted set and must be **retained**, object 6 is not and must
/// be **cut**. A fixture with one font can only ever show one arm.
fn shared_font_document(pages: usize) -> Vec<u8> {
    let kids: Vec<String> = (0..pages).map(|at| format!("{} 0 R", 7 + at * 2)).collect();
    let mut objects: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        format!(
            "<< /Type /Pages /Count {pages} /Kids [{}] >>",
            kids.join(" ")
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding \
         /FirstChar 65 /LastChar 90 /Widths PLACEHOLDER >>"
            .to_owned(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding \
         /FirstChar 65 /LastChar 90 /Widths PLACEHOLDER >>"
            .to_owned(),
        "<< /Type /Null >>".to_owned(),
        "<< /Type /Null >>".to_owned(),
    ];
    // Objects 3 and 4 are the fonts; 5 and 6 are their numbers once the list is one-based.
    objects[2] = objects[2].replace("PLACEHOLDER", WIDTHS);
    objects[3] = objects[3].replace("PLACEHOLDER", WIDTHS);

    for at in 0..pages {
        let mut content = format!("BT /Shared 24 Tf 72 {} Td (AAAA) Tj ET\n", 700 - at * 10);
        if at == 0 {
            content.push_str("BT /Alone 24 Tf 72 640 Td (BBBB) Tj ET\n");
        }
        let resources = if at == 0 {
            "<< /Font << /Shared 3 0 R /Alone 4 0 R >> >>"
        } else {
            "<< /Font << /Shared 3 0 R >> >>"
        };
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources {resources} \
             /Contents {} 0 R >>",
            8 + at * 2
        ));
        objects.push(format!(
            "<< /Length {} >>\nstream\n{content}endstream",
            content.len()
        ));
    }

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

#[test]
fn the_fixture_draws_on_every_page_before_anything_is_asserted_about_it() {
    // NON-VACUITY, FIRST. A fixture whose later pages draw nothing would satisfy every
    // sharing assertion below without sharing anything.
    let pdf = shared_font_document(4);
    for page in 0..4 {
        let chars = chars_on_page(&pdf, page);
        let drawn = chars.iter().filter(|char| !char.generated).count();
        let expected = if page == 0 { 8 } else { 4 };
        assert_eq!(
            drawn, expected,
            "page {page} draws {drawn} glyphs, not {expected}"
        );
    }
}

#[test]
fn a_font_other_pages_use_is_retained_and_the_count_names_how_many() {
    let pdf = shared_font_document(4);
    let region = Region {
        left: 60.0,
        top: 792.0 - 730.0,
        width: 200.0,
        height: 120.0,
    };
    let redacted: BTreeSet<usize> = [0].into_iter().collect();
    let (out, report) = burrow_engines::redact_probe::redact_page(&pdf, 0, redacted, region)
        .expect("a four-page document redacted on its first page");

    assert!(out.starts_with(b"%PDF"));
    assert_eq!(report.fonts.len(), 2, "two fonts: {:?}", report.fonts);

    let retained: Vec<_> = report.retained().collect();
    assert_eq!(
        retained.len(),
        1,
        "exactly one font is used outside the redacted set: {:?}",
        report.fonts
    );
    // THE INDEPENDENT TALLY. Three other pages name `/Shared`, by construction of the fixture
    // above -- a number this test knows without asking the sharing walk, which is the whole
    // point of checking it.
    assert_eq!(
        retained[0].also_used_by, 3,
        "the disclosure must say three other pages, and says {}",
        retained[0].also_used_by
    );
    assert!(report.discloses_a_retained_font());

    let cut: Vec<_> = report.fonts.iter().filter(|font| font.cut).collect();
    assert_eq!(cut.len(), 1, "the page-local font is cuttable");
    assert_eq!(cut[0].also_used_by, 0);
}

#[test]
fn redacting_every_page_leaves_nothing_to_disclose() {
    // The control on the assertion above: the same document, the same region, every page in the
    // redacted set. If `also_used_by` were counting pages rather than pages OUTSIDE the set,
    // this would still report three.
    let pdf = shared_font_document(4);
    let region = Region {
        left: 60.0,
        top: 792.0 - 730.0,
        width: 200.0,
        height: 120.0,
    };
    let redacted: BTreeSet<usize> = (0..4).collect();
    let (_, report) = burrow_engines::redact_probe::redact_page(&pdf, 0, redacted, region)
        .expect("every page redacted");
    assert!(
        !report.discloses_a_retained_font(),
        "every font is cuttable when every page using it is redacted: {:?}",
        report.fonts
    );
    for font in &report.fonts {
        assert_eq!(font.also_used_by, 0, "{font:?}");
    }
}

#[test]
fn the_count_can_never_exceed_the_pages_that_exist() {
    // THE BOUND THAT HOLDS FOR EVERY DOCUMENT, checked against a number that does not come
    // from the sharing walk. An under-count is the hazard and this does not catch one; it
    // catches the failure that would make the number meaningless in the other direction, and it
    // is the only claim available over a corpus of single-page documents.
    for pages in 1..=6 {
        let pdf = shared_font_document(pages);
        let region = Region {
            left: 60.0,
            top: 792.0 - 730.0,
            width: 200.0,
            height: 120.0,
        };
        let redacted: BTreeSet<usize> = [0].into_iter().collect();
        let (_, report) = burrow_engines::redact_probe::redact_page(&pdf, 0, redacted, region)
            .expect("a document of this many pages");
        for font in &report.fonts {
            assert!(
                font.also_used_by < pages,
                "{pages} pages, one redacted, and a font reports {} others: {font:?}",
                font.also_used_by
            );
        }
        // And the shared font's count is exactly the pages that are not redacted.
        let retained: Vec<_> = report.retained().collect();
        if pages == 1 {
            assert!(retained.is_empty(), "one page, all of it redacted");
        } else {
            assert_eq!(retained.len(), 1);
            assert_eq!(retained[0].also_used_by, pages - 1);
        }
    }
}

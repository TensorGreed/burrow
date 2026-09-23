//! One fixture per defence in the qpdf redaction steps, so deleting the defence fails.
//!
//! # Why this file exists
//!
//! A security review planted twenty mutations against commit `22ceffa`. Nine survived, and
//! **all nine were in `qpdf/redact_steps.rs`** — the module that drives every other one. The
//! units it drives were well covered: deleting `MAX_ENTRIES`, `MAX_TOTAL_OPERANDS`, the
//! `bfrange` bounds or the `keep` filter all turned something red. Deleting the calls to them
//! did not.
//!
//! That is the shape of a seam nobody tested. `CLAUDE.md`'s rule is the whole argument: *a
//! defence nothing fails for is not a defence.* Each test here is named for the mutation it
//! kills, and each is built so that removing the check it names makes it fail rather than
//! merely making it different.
//!
//! # These are documents, not unit fixtures
//!
//! Every one goes in through `redact_probe::redact_page`, which is what the nine surviving
//! mutations had in common: they were all reachable from the operation and none of them were
//! reached. A unit test of `narrow_differences` would have passed with the call to it deleted.

#![cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "a test asserting against fixtures it built; the workspace lints are for \
              attacker-controlled input in library code, where they stay denied"
)]

mod support;

use std::collections::BTreeSet;

use burrow_engines::pdfsyntax::region::Region;
use support::pdf_builder::{Builder, helvetica_with_widths};

/// The region every fixture here is redacted with: the upper band of the page.
fn band() -> Region {
    Region {
        left: 40.0,
        top: 40.0,
        width: 500.0,
        height: 120.0,
    }
}

/// Redact page 0 of `pdf`, naming only page 0.
fn redact(pdf: &[u8]) -> burrow_types::Result<(Vec<u8>, burrow_engines::redact::Report)> {
    let redacted: BTreeSet<usize> = [0].into_iter().collect();
    burrow_engines::redact_probe::redact_page(pdf, 0, redacted, band())
}

/// Whether the emitted document contains `needle` once decompressed by qpdf.
///
/// A raw scan of the output finds nothing, because qpdf re-compresses every stream it writes.
/// A check that scanned the raw bytes would report silence for the wrong reason — the exact
/// shape ADR 0019's third correction was about.
fn decompressed(pdf: &[u8]) -> Vec<u8> {
    let dir = std::env::temp_dir().join(format!("burrow-defences-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let input = dir.join(format!("in-{:?}.pdf", std::thread::current().id()));
    let output = dir.join(format!("out-{:?}.pdf", std::thread::current().id()));
    std::fs::write(&input, pdf).expect("write the input");
    let qpdf = which_qpdf();
    let status = std::process::Command::new(&qpdf)
        .args(["--qdf", "--object-streams=disable", "--decode-level=all"])
        .arg(&input)
        .arg(&output)
        .status()
        .expect("the qpdf CLI must be available; without it a scan reports silence");
    assert!(
        status.success() || status.code() == Some(3),
        "qpdf refused to normalise the output: {status:?}"
    );
    std::fs::read(&output).expect("read the normalised output")
}

/// The `qpdf` CLI, from PATH or from the vendored build.
fn which_qpdf() -> std::path::PathBuf {
    let vendored = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../engines/vendor/src")
        .join(format!("build-qpdf-plain-{}", std::env::consts::ARCH))
        .join("qpdf/qpdf");
    if vendored.exists() {
        return vendored;
    }
    // `std::env::consts::ARCH` and the directory the build script names do not always agree;
    // fall back to whatever `qpdf` is on PATH rather than reporting a leak scan that never ran.
    for entry in std::fs::read_dir(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../engines/vendor/src"),
    )
    .into_iter()
    .flatten()
    .flatten()
    {
        let candidate = entry.path().join("qpdf/qpdf");
        if candidate.exists() {
            return candidate;
        }
    }
    std::path::PathBuf::from("qpdf")
}

/// Assert `needle` is absent from the normalised output, naming what it is.
fn assert_absent(pdf: &[u8], needle: &[u8], what: &str) {
    let normalised = decompressed(pdf);
    assert!(
        !normalised
            .windows(needle.len())
            .any(|window| window == needle),
        "{what}: {} survived the redaction",
        String::from_utf8_lossy(needle)
    );
}

/// Assert `needle` is present, which is the non-vacuity control for [`assert_absent`].
fn assert_present(pdf: &[u8], needle: &[u8], what: &str) {
    let normalised = decompressed(pdf);
    assert!(
        normalised
            .windows(needle.len())
            .any(|window| window == needle),
        "{what}: {} is not in the document, so nothing below asks anything",
        String::from_utf8_lossy(needle)
    );
}

/// A one-page document drawing `(SECRET)` in the band and `(KIN)` below it, with a font
/// carrying `extra` keys.
///
/// **The two strings share no character**, which is load-bearing rather than tidy. The first
/// version drew `(KEEP)` below the band, and `E` is in both — so `E` was still drawn, its
/// `/Differences` name was correctly kept, and the test read that as the narrowing having
/// failed. A fixture whose kept and removed text overlap cannot ask this question.
fn page_with_font(extra: &str) -> Vec<u8> {
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let page = pdf.reserve();
    let font = pdf.add(&format!(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 32 /LastChar 94 \
         /Widths {} {extra} >>",
        support::pdf_builder::HELVETICA_WIDTHS
    ));
    let content = pdf.stream(
        "",
        "BT /F1 24 Tf 72 700 Td (SECRET) Tj ET\nBT /F1 24 Tf 72 300 Td (KIN) Tj ET\n",
    );
    pdf.put(
        page,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 {font} 0 R >> >> /Contents {content} 0 R >>"
        ),
    );
    pdf.put(
        pages,
        &format!("<< /Type /Pages /Count 1 /Kids [{page} 0 R] >>"),
    );
    pdf.put(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    pdf.build(catalog)
}

// ---------------------------------------------------------------------------------------
// The narrowings: each is a call that a mutation could delete without any test noticing.
// ---------------------------------------------------------------------------------------

#[test]
fn the_to_unicode_entries_for_removed_codes_do_not_reach_the_output() {
    // KILLS: `narrow_to_unicode` keeping every code, and `narrow_font` not calling it.
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let page = pdf.reserve();
    // `S`, `E`, `C`, `R`, `T` map to themselves; the mapping IS the secret's spelling.
    let to_unicode = pdf.stream(
        "",
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CMapType 2 def\n\
         1 begincodespacerange\n<00> <FF>\nendcodespacerange\n\
         6 beginbfchar\n<53> <0053>\n<45> <0045>\n<43> <0043>\n<52> <0052>\n<54> <0054>\n\
         <4B> <004B>\nendbfchar\nendcmap\nend\nend\n",
    );
    let font = pdf.add(&format!(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 32 /LastChar 94 \
         /Widths {} /ToUnicode {to_unicode} 0 R >>",
        support::pdf_builder::HELVETICA_WIDTHS
    ));
    let content = pdf.stream(
        "",
        "BT /F1 24 Tf 72 700 Td (SECRET) Tj ET\nBT /F1 24 Tf 72 300 Td (KIN) Tj ET\n",
    );
    pdf.put(
        page,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 {font} 0 R >> >> /Contents {content} 0 R >>"
        ),
    );
    pdf.put(
        pages,
        &format!("<< /Type /Pages /Count 1 /Kids [{page} 0 R] >>"),
    );
    pdf.put(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    let document = pdf.build(catalog);

    // NON-VACUITY FIRST. `<53>` is `S`, drawn only in the band.
    assert_present(&document, b"<53> <0053>", "the fixture's /ToUnicode");

    let (out, _) = redact(&document).expect("a one-page redaction");
    assert_absent(&out, b"<53> <0053>", "the removed code's mapping");
    // AND THE KEPT CODE'S MAPPING SURVIVES. Without this the test passes for a redaction that
    // deleted the whole stream, which is the defect `tounicode` was written to stop.
    assert_present(&out, b"<4B> <004B>", "the kept code's mapping");
}

#[test]
fn the_differences_names_for_removed_codes_do_not_reach_the_output() {
    // KILLS: `narrow_differences` never substituting, and `narrow_font` not calling it.
    let document = page_with_font(
        "/Encoding << /Type /Encoding /BaseEncoding /WinAnsiEncoding \
         /Differences [83 /Sacute 69 /Egrave 75 /Kcommaaccent] >>",
    );
    assert_present(&document, b"/Sacute", "the fixture's /Differences");

    let (out, _) = redact(&document).expect("a one-page redaction");
    // `S` (83) and `E` (69) are drawn only inside the band; `K` (75) only outside it.
    assert_absent(&out, b"/Sacute", "a removed code's glyph name");
    assert_absent(&out, b"/Egrave", "a removed code's glyph name");
    assert_present(&out, b"/Kcommaaccent", "a kept code's glyph name");
}

#[test]
fn a_differences_array_assigning_one_code_twice_keeps_only_the_last_name() {
    // KILLS: the single-pass `/Differences` narrowing. `[75 /Sacute /Egrave]` puts the removed
    // text's spelling at codes 75 and 76, and `[75 /Sacute 75 /Kcommaaccent]` assigns code 75
    // twice -- of which a reader takes the last. A pass that asked only "is this code kept?"
    // kept both.
    let document = page_with_font(
        "/Encoding << /Type /Encoding /BaseEncoding /WinAnsiEncoding \
         /Differences [75 /Sacute 75 /Kcommaaccent] >>",
    );
    assert_present(&document, b"/Sacute", "the fixture's /Differences");

    let (out, _) = redact(&document).expect("a one-page redaction");
    assert_absent(&out, b"/Sacute", "the shadowed name at a kept code");
    assert_present(&out, b"/Kcommaaccent", "the name a reader actually takes");
}

#[test]
fn a_differences_anchor_outside_a_character_code_is_refused_rather_than_folded_to_zero() {
    // KILLS: `u32::try_from(...).unwrap_or(0)`. An anchor of -1 silently became code 0, so
    // every name after it was tested against the wrong code and a name spelling removed text
    // stayed in the file.
    for anchor in ["-1", "4294967296"] {
        let document = page_with_font(&format!(
            "/Encoding << /Type /Encoding /Differences [{anchor} /Sacute /Egrave] >>"
        ));
        let error = redact(&document).expect_err("an anchor outside a character code");
        let text = format!("{error:?}");
        assert!(
            text.contains("differences-anchor"),
            "an anchor of {anchor} must refuse by name, got: {text}"
        );
    }
}

#[test]
fn the_widths_for_removed_codes_are_zeroed() {
    // KILLS: `narrow_font` skipping the `/Widths` loop. The width array is the one place a
    // removed glyph's metrics survive as data rather than as a name.
    let document = page_with_font("");
    let (out, _) = redact(&document).expect("a one-page redaction");
    let normalised = decompressed(&out);
    let text = String::from_utf8_lossy(&normalised);
    let widths = text
        .split("/Widths")
        .nth(1)
        .and_then(|rest| rest.split(']').next())
        .expect("a /Widths array in the output");
    // `S` is code 83, `/FirstChar` is 32, so index 51. `K` is 75, index 43 -- drawn outside
    // the band and therefore kept.
    let entries: Vec<&str> = widths.trim_start_matches(" [").split_whitespace().collect();
    assert_eq!(entries.get(51), Some(&"0"), "the removed code's width");
    assert_ne!(entries.get(43), Some(&"0"), "the kept code's width");
}

// ---------------------------------------------------------------------------------------
// The refusals: each is a check that a mutation could stop raising.
// ---------------------------------------------------------------------------------------

#[test]
fn a_type_three_procedure_that_shows_text_refuses_by_name() {
    // KILLS: `check_type_three` never refusing, and `affected_streams` not calling it.
    // The glyph is invoked at the top of the page and its procedure draws text of its own --
    // text the walk cannot box, which is why the whole page is refused rather than edited.
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let page = pdf.reserve();
    let procedure = pdf.stream("", "10 0 0 0 0 0 d0\nBT /F9 30 Tf 0 0 Td (SECRET) Tj ET\n");
    let inner_font = pdf.add(&helvetica_with_widths());
    let font = pdf.add(&format!(
        "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 10 10] \
         /FontMatrix [0.1 0 0 0.1 0 0] /CharProcs << /mark {procedure} 0 R >> \
         /Encoding << /Type /Encoding /Differences [65 /mark] >> \
         /FirstChar 65 /LastChar 65 /Widths [10] \
         /Resources << /Font << /F9 {inner_font} 0 R >> >> >>"
    ));
    let content = pdf.stream("", "BT /T3 24 Tf 72 700 Td (A) Tj ET\n");
    pdf.put(
        page,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /T3 {font} 0 R >> >> /Contents {content} 0 R >>"
        ),
    );
    pdf.put(
        pages,
        &format!("<< /Type /Pages /Count 1 /Kids [{page} 0 R] >>"),
    );
    pdf.put(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    let document = pdf.build(catalog);

    let error = redact(&document).expect_err("a Type 3 procedure that shows text");
    let text = format!("{error:?}");
    assert!(
        text.contains("type-three-procedure-shows-text"),
        "got: {text}"
    );
}

#[test]
fn a_type_three_procedure_drawing_outside_its_own_box_is_still_refused() {
    // KILLS: scoping `check_type_three` to the glyphs the region reached. The glyph is invoked
    // at y=700, its `/FontBBox` is ten units, and the procedure draws a hundred units BELOW
    // the origin -- so the region over that ink reaches no glyph the walk boxed, and the
    // narrower scope refused nothing while the secret rendered.
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let page = pdf.reserve();
    let procedure = pdf.stream(
        "",
        "10 0 0 0 0 0 d0\nBT /F9 30 Tf 30 -100 Td (SECRET) Tj ET\n",
    );
    let inner_font = pdf.add(&helvetica_with_widths());
    let font = pdf.add(&format!(
        "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 10 10] \
         /FontMatrix [0.1 0 0 0.1 0 0] /CharProcs << /mark {procedure} 0 R >> \
         /Encoding << /Type /Encoding /Differences [65 /mark] >> \
         /FirstChar 65 /LastChar 65 /Widths [10] \
         /Resources << /Font << /F9 {inner_font} 0 R >> >> >>"
    ));
    // Drawn at y=200, far below the band the region covers.
    let content = pdf.stream("", "BT /T3 24 Tf 72 200 Td (A) Tj ET\n");
    pdf.put(
        page,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /T3 {font} 0 R >> >> /Contents {content} 0 R >>"
        ),
    );
    pdf.put(
        pages,
        &format!("<< /Type /Pages /Count 1 /Kids [{page} 0 R] >>"),
    );
    pdf.put(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    let document = pdf.build(catalog);

    let error = redact(&document).expect_err("a Type 3 procedure the region cannot reach");
    assert!(
        format!("{error:?}").contains("type-three-procedure-shows-text"),
        "got: {error:?}"
    );
}

#[test]
fn a_differences_naming_one_code_twice_does_not_hide_a_type_three_procedure() {
    // KILLS: `char_proc_name` taking the FIRST assignment. It resolved `[65 /g 65 /mark]` to
    // the harmless procedure while a reader draws the other one. The resolution is gone
    // entirely -- every procedure is scanned -- and this is the fixture that says so.
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let page = pdf.reserve();
    let harmless = pdf.stream("", "10 0 0 0 0 0 d0\n0 0 1 1 re f\n");
    let procedure = pdf.stream("", "10 0 0 0 0 0 d0\nBT /F9 30 Tf 0 0 Td (SECRET) Tj ET\n");
    let inner_font = pdf.add(&helvetica_with_widths());
    let font = pdf.add(&format!(
        "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 10 10] \
         /FontMatrix [0.1 0 0 0.1 0 0] \
         /CharProcs << /g {harmless} 0 R /mark {procedure} 0 R >> \
         /Encoding << /Type /Encoding /Differences [65 /g 65 /mark] >> \
         /FirstChar 65 /LastChar 65 /Widths [10] \
         /Resources << /Font << /F9 {inner_font} 0 R >> >> >>"
    ));
    let content = pdf.stream("", "BT /T3 24 Tf 72 700 Td (A) Tj ET\n");
    pdf.put(
        page,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /T3 {font} 0 R >> >> /Contents {content} 0 R >>"
        ),
    );
    pdf.put(
        pages,
        &format!("<< /Type /Pages /Count 1 /Kids [{page} 0 R] >>"),
    );
    pdf.put(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    let document = pdf.build(catalog);

    let error = redact(&document).expect_err("the shadowed procedure shows text");
    assert!(
        format!("{error:?}").contains("type-three-procedure-shows-text"),
        "got: {error:?}"
    );
}

#[test]
fn a_page_whose_contents_is_not_a_single_stream_refuses_by_name() {
    // KILLS: dropping the "/Contents must be a stream" check in `rewrite`.
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let page = pdf.reserve();
    let font = pdf.add(&helvetica_with_widths());
    let first = pdf.stream("", "BT /F1 24 Tf 72 700 Td (SEC) Tj ET\n");
    let second = pdf.stream("", "BT /F1 24 Tf 72 300 Td (KIN) Tj ET\n");
    pdf.put(
        page,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 {font} 0 R >> >> \
             /Contents [{first} 0 R {second} 0 R] >>"
        ),
    );
    pdf.put(
        pages,
        &format!("<< /Type /Pages /Count 1 /Kids [{page} 0 R] >>"),
    );
    pdf.put(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    let document = pdf.build(catalog);

    let error = redact(&document).expect_err("an array /Contents");
    assert!(
        format!("{error:?}").contains("contents-not-a-stream"),
        "got: {error:?}"
    );
}

#[test]
fn a_page_key_outside_the_allowlist_is_stripped() {
    // KILLS: `strip_page_keys` stripping nothing.
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let page = pdf.reserve();
    let font = pdf.add(&helvetica_with_widths());
    let metadata = pdf.stream("/Type /Metadata /Subtype /XML", "<x>BURROW-PAGE-META</x>\n");
    let content = pdf.stream(
        "",
        "BT /F1 24 Tf 72 700 Td (SECRET) Tj ET\nBT /F1 24 Tf 72 300 Td (KIN) Tj ET\n",
    );
    pdf.put(
        page,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 {font} 0 R >> >> /Contents {content} 0 R \
             /Metadata {metadata} 0 R >>"
        ),
    );
    pdf.put(
        pages,
        &format!("<< /Type /Pages /Count 1 /Kids [{page} 0 R] >>"),
    );
    pdf.put(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    let document = pdf.build(catalog);

    assert_present(
        &document,
        b"BURROW-PAGE-META",
        "the fixture's page metadata",
    );
    let (out, _) = redact(&document).expect("a one-page redaction");
    assert_absent(&out, b"BURROW-PAGE-META", "the page's /Metadata");
}

#[test]
fn an_annotation_whose_rect_reaches_the_region_is_removed() {
    // KILLS: not removing annotations at all. `/Annots` is on the page-key allowlist and
    // `affected_streams` never walks `/AP /N`, so before this the appearance stream drawing
    // over the region survived untouched and the output still rendered the secret.
    let document = annotated_document("[72 690 400 740]");
    assert_present(
        &document,
        b"BURROW-ANNOT-SECRET",
        "the fixture's appearance",
    );

    let (out, _) = redact(&document).expect("a one-page redaction");
    assert_absent(&out, b"BURROW-ANNOT-SECRET", "the annotation's appearance");
}

#[test]
fn an_annotation_clear_of_the_region_survives() {
    // THE NEAR-MISS TWIN. Without it the test above passes for an implementation that removed
    // every annotation on every redacted page, which is a different rule and a worse one.
    let document = annotated_document("[72 100 400 150]");
    let (out, _) = redact(&document).expect("a one-page redaction");
    assert_present(
        &out,
        b"BURROW-ANNOT-SECRET",
        "an annotation outside the region",
    );
}

/// A one-page document with a `/FreeText` annotation at `rect` whose appearance draws a canary.
fn annotated_document(rect: &str) -> Vec<u8> {
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let page = pdf.reserve();
    let font = pdf.add(&helvetica_with_widths());
    let appearance = pdf.stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 328 50]",
        "BT /F1 18 Tf 2 20 Td (BURROW-ANNOT-SECRET) Tj ET\n",
    );
    let annotation = pdf.add(&format!(
        "<< /Type /Annot /Subtype /FreeText /Rect {rect} /F 4 \
         /AP << /N {appearance} 0 R >> >>"
    ));
    let content = pdf.stream("", "BT /F1 24 Tf 72 300 Td (KIN) Tj ET\n");
    pdf.put(
        page,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 {font} 0 R >> >> /Contents {content} 0 R \
             /Annots [{annotation} 0 R] >>"
        ),
    );
    pdf.put(
        pages,
        &format!("<< /Type /Pages /Count 1 /Kids [{page} 0 R] >>"),
    );
    pdf.put(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    pdf.build(catalog)
}

#[test]
fn an_annotation_whose_rect_is_not_four_numbers_refuses_by_name() {
    // KILLS: treating an unreadable `/Rect` as "outside the region". Where it sits is then
    // unknown, and unknown is not outside.
    let document = annotated_document("[72 690 400]");
    let error = redact(&document).expect_err("a /Rect that is not four numbers");
    assert!(
        format!("{error:?}").contains("annotation-rect"),
        "got: {error:?}"
    );
}

#[test]
fn a_page_index_past_the_end_refuses_rather_than_reaching_for_the_page() {
    // KILLS: the page-count check `page_handle`'s SAFETY comment relies on. Before this the
    // check lived in one caller and the comment claimed the constructor made it.
    let document = page_with_font("");
    let redacted: BTreeSet<usize> = [0].into_iter().collect();
    let error = burrow_engines::redact_probe::redact_page(&document, 7, redacted, band())
        .expect_err("a page the document does not have");
    assert!(
        format!("{error:?}").contains("page"),
        "the refusal must name the page, got: {error:?}"
    );
}

#[test]
fn a_form_xobject_drawn_twice_refuses_when_the_region_reaches_inside_it() {
    // KILLS: `check_form_sharing` never refusing. The form is drawn on page 0 inside the band
    // and again on page 1, so editing it would remove text from a page nobody selected --
    // corruption on a page §6's read-back never looks at.
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let first = pdf.reserve();
    let second = pdf.reserve();
    let font = pdf.add(&helvetica_with_widths());
    let form = pdf.stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 400 40] \
         /Resources << /Font << /F1 3 0 R >> >>",
        "BT /F1 24 Tf 2 8 Td (SECRET) Tj ET\n",
    );
    let first_content = pdf.stream("", "q 1 0 0 1 72 700 cm /Fx Do Q\n");
    let second_content = pdf.stream("", "q 1 0 0 1 72 400 cm /Fx Do Q\n");
    for (page, content) in [(first, first_content), (second, second_content)] {
        pdf.put(
            page,
            &format!(
                "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
                 /Resources << /Font << /F1 {font} 0 R >> /XObject << /Fx {form} 0 R >> >> \
                 /Contents {content} 0 R >>"
            ),
        );
    }
    pdf.put(
        pages,
        &format!("<< /Type /Pages /Count 2 /Kids [{first} 0 R {second} 0 R] >>"),
    );
    pdf.put(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    let document = pdf.build(catalog);

    let error = redact(&document).expect_err("a shared form the region reaches into");
    let text = format!("{error:?}");
    assert!(
        text.contains("shared-form"),
        "the refusal must name the sharing rule, got: {text}"
    );
}

#[test]
fn a_form_xobject_drawn_twice_is_not_refused_when_the_region_misses_it() {
    // THE LETTERHEAD NEAR-MISS. Without it the test above passes for an implementation that
    // refused every page carrying a shared form, which would refuse a large share of real
    // documents -- the scoping ADR 0029's amendment settled.
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let first = pdf.reserve();
    let second = pdf.reserve();
    let font = pdf.add(&helvetica_with_widths());
    let form = pdf.stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 400 40] \
         /Resources << /Font << /F1 3 0 R >> >>",
        "BT /F1 24 Tf 2 8 Td (LETTERHEAD) Tj ET\n",
    );
    // The shared form is drawn at the FOOT of page 0, well clear of the band; the text the
    // region reaches is the page's own.
    let first_content = pdf.stream(
        "",
        "q 1 0 0 1 72 60 cm /Fx Do Q\nBT /F1 24 Tf 72 700 Td (SECRET) Tj ET\n",
    );
    let second_content = pdf.stream("", "q 1 0 0 1 72 60 cm /Fx Do Q\n");
    for (page, content) in [(first, first_content), (second, second_content)] {
        pdf.put(
            page,
            &format!(
                "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
                 /Resources << /Font << /F1 {font} 0 R >> /XObject << /Fx {form} 0 R >> >> \
                 /Contents {content} 0 R >>"
            ),
        );
    }
    pdf.put(
        pages,
        &format!("<< /Type /Pages /Count 2 /Kids [{first} 0 R {second} 0 R] >>"),
    );
    pdf.put(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    let document = pdf.build(catalog);

    let (out, _) = redact(&document).expect("the region never reaches the shared form");
    assert_present(&out, b"LETTERHEAD", "the shared form's own text");
}

#[test]
fn a_font_whose_encoding_is_shared_with_another_page_is_retained_rather_than_cut() {
    // KILLS: cuttability keyed on the font dictionary alone. Two fonts, one indirect
    // `/Encoding` and one indirect `/ToUnicode` between them; page 0 is redacted and page 1 is
    // not. Cutting page 0's font writes THROUGH the shared objects, so page 1 loses its
    // mapping -- and the report said `cut: true, also_used_by: 0`, that nothing outside the
    // operation had been affected.
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let first = pdf.reserve();
    let second = pdf.reserve();
    let encoding = pdf.add(
        "<< /Type /Encoding /BaseEncoding /WinAnsiEncoding \
         /Differences [83 /Sacute 75 /Kcommaaccent] >>",
    );
    let to_unicode = pdf.stream(
        "",
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CMapType 2 def\n\
         1 begincodespacerange\n<00> <FF>\nendcodespacerange\n\
         2 beginbfchar\n<53> <0053>\n<4B> <004B>\nendbfchar\nendcmap\nend\nend\n",
    );
    let shared = format!(
        "/FirstChar 32 /LastChar 94 /Widths {} /Encoding {encoding} 0 R \
         /ToUnicode {to_unicode} 0 R",
        support::pdf_builder::HELVETICA_WIDTHS
    );
    let font_a = pdf.add(&format!(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica {shared} >>"
    ));
    let font_b = pdf.add(&format!(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica {shared} >>"
    ));
    let first_content = pdf.stream("", "BT /F1 24 Tf 72 700 Td (S) Tj ET\n");
    let second_content = pdf.stream("", "BT /F1 24 Tf 72 400 Td (SK) Tj ET\n");
    for (page, content, font) in [
        (first, first_content, font_a),
        (second, second_content, font_b),
    ] {
        pdf.put(
            page,
            &format!(
                "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
                 /Resources << /Font << /F1 {font} 0 R >> >> /Contents {content} 0 R >>"
            ),
        );
    }
    pdf.put(
        pages,
        &format!("<< /Type /Pages /Count 2 /Kids [{first} 0 R {second} 0 R] >>"),
    );
    pdf.put(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    let document = pdf.build(catalog);

    let (out, report) = redact(&document).expect("a two-page document redacted on page 0");

    // PAGE 1 STILL HAS ITS MAPPING. The assertion the report cannot make for itself.
    assert_present(&out, b"<53> <0053>", "the shared /ToUnicode");
    assert_present(&out, b"/Sacute", "the shared /Differences");
    // AND THE OPERATION SAYS SO. A fix that stopped cutting but still reported `cut: true`
    // would pass the two assertions above and tell the person the opposite of the truth.
    assert!(
        report.discloses_a_retained_font(),
        "the font is retained because the objects it is edited through are shared: {:?}",
        report.fonts
    );
    assert!(
        report.retained().all(|font| font.also_used_by > 0),
        "a retained font names how many pages keep using it: {:?}",
        report.fonts
    );
}

#[test]
fn a_parent_chain_that_never_terminates_refuses_by_name() {
    // KILLS: dropping `redact_frame`'s `/Parent` depth ceiling. Without it the climb for an
    // inherited `/MediaBox` or `/Rotate` runs forever on a cyclic page tree -- which is a
    // hang rather than a wrong answer, and a hang has no error message to read.
    //
    // The cycle is two `/Pages` nodes naming each other as `/Parent`, and the page declares
    // neither box, so the climb is forced.
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let loop_a = pdf.reserve();
    let loop_b = pdf.reserve();
    let page = pdf.reserve();
    let font = pdf.add(&helvetica_with_widths());
    let content = pdf.stream("", "BT /F1 24 Tf 72 700 Td (SECRET) Tj ET\n");
    pdf.put(
        page,
        &format!(
            "<< /Type /Page /Parent {loop_a} 0 R \
             /Resources << /Font << /F1 {font} 0 R >> >> /Contents {content} 0 R >>"
        ),
    );
    pdf.put(loop_a, &format!("<< /Type /Pages /Parent {loop_b} 0 R >>"));
    pdf.put(loop_b, &format!("<< /Type /Pages /Parent {loop_a} 0 R >>"));
    pdf.put(
        pages,
        &format!("<< /Type /Pages /Count 1 /Kids [{page} 0 R] >>"),
    );
    pdf.put(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    let document = pdf.build(catalog);

    let error = redact(&document).expect_err("a /Parent chain that does not terminate");
    let text = format!("{error:?}");
    assert!(
        text.contains("page-tree-depth") || text.contains("no-display-box"),
        "the refusal must name the climb or the missing box, got: {text}"
    );
}

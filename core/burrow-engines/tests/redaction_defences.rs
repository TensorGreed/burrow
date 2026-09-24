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
//! Every one goes in through `burrow_ops::redact::page`, which is what the nine surviving
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
use support::char_box_oracle::{TOLERANCE_PT, chars_on_page, ink_overlaps, page_size};
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
    support::redact_page(pdf, 0, redacted, band())
}

/// The refusal `pdf` produces, or a panic naming `what` — **without printing the document**.
///
/// `expect_err` on a `Result<(Vec<u8>, _), _>` formats the `Ok` side, which is the whole
/// emitted PDF as a list of byte literals. Two of these turned a one-line assertion failure
/// into several thousand lines of decimal, which is a test that is harder to read when it fails
/// than when it passes.
fn refusal(pdf: &[u8], what: &str) -> String {
    match redact(pdf) {
        Ok((out, report)) => panic!(
            "{what}: redacted {} bytes instead of refusing, report {report:?}",
            out.len()
        ),
        Err(error) => format!("{error:?}"),
    }
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

/// The object numbers of the page's `/Contents` elements, **in the output**.
///
/// qpdf renumbers objects on write, so the numbers the fixture built with do not survive. The
/// correspondence that does survive is positional: element *i* of the output's `/Contents`
/// array is element *i* of the input's. Reading them out of the emitted array is what lets an
/// assertion name an element at all.
///
/// # Panics
///
/// If the output has no `/Contents` array — which for these fixtures means the array was
/// collapsed to a single reference, and the caller should be told that rather than left to
/// interpret an empty list.
fn contents_elements(pdf: &[u8]) -> Vec<usize> {
    let normalised = decompressed(pdf);
    let text = String::from_utf8_lossy(&normalised).into_owned();
    let at = text
        .find("/Contents [")
        .expect("the output page must still have a /Contents ARRAY");
    let rest = &text[at + "/Contents [".len()..];
    let end = rest.find(']').expect("an unterminated /Contents array");
    // `5 0 R 6 0 R` is SIX tokens, not two. Taking every number would give the generations
    // too, which is how the first version reported four elements for a two-element array.
    rest[..end]
        .split_whitespace()
        .collect::<Vec<&str>>()
        .chunks(3)
        .filter_map(|chunk| match chunk {
            [number, _generation, r] if *r == "R" => number.parse::<usize>().ok(),
            _ => None,
        })
        .collect()
}

/// The decoded body of object `number` in the normalised output.
///
/// The output is normalised with `--decode-level=all`, so stream bodies are plain text. Reading
/// one **by object number** is what lets an assertion be about a single `/Contents` element
/// rather than about the page's concatenation, which is the distinction a collapsed array
/// destroys while leaving every page-level assertion true.
fn stream_body(pdf: &[u8], number: usize) -> String {
    let normalised = decompressed(pdf);
    let text = String::from_utf8_lossy(&normalised).into_owned();
    let marker = format!("\n{number} 0 obj");
    let at = text
        .find(&marker)
        .unwrap_or_else(|| panic!("object {number} is not in the output"));
    let rest = &text[at..];
    let start = rest
        .find("stream\n")
        .unwrap_or_else(|| panic!("object {number} is not a stream in the output"));
    let end = rest
        .find("endstream")
        .unwrap_or_else(|| panic!("object {number} has no endstream"));
    rest[start + "stream\n".len()..end].to_owned()
}

/// Assert `needle` is absent from the normalised output, naming what it is.
fn assert_absent(pdf: &[u8], needle: &[u8], what: &str) {
    let normalised = decompressed(pdf);
    // AND AS ASCII HEX, in either case: the rewriter re-emits every KEPT code as a hex string
    // (`<4B454550>`), so a canary a redaction left standing is spelled that way in its output and
    // a literal-only scan reads it as gone. Found by #176's review.
    let hex: String = needle.iter().map(|byte| format!("{byte:02X}")).collect();
    for spelling in [
        needle.to_vec(),
        hex.clone().into_bytes(),
        hex.to_ascii_lowercase().into_bytes(),
    ] {
        assert!(
            !normalised
                .windows(spelling.len())
                .any(|window| window == spelling.as_slice()),
            "{what}: {} survived the redaction, spelled {}",
            String::from_utf8_lossy(needle),
            String::from_utf8_lossy(&spelling)
        );
    }
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
        let text = refusal(&document, "an anchor outside a character code");
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

    let text = refusal(&document, "a Type 3 procedure that shows text");
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

    let text = refusal(&document, "a Type 3 procedure the region cannot reach");
    assert!(
        text.contains("type-three-procedure-shows-text"),
        "got: {text}"
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

    let text = refusal(&document, "the shadowed procedure shows text");
    assert!(
        text.contains("type-three-procedure-shows-text"),
        "got: {text}"
    );
}

#[test]
fn a_contents_array_is_rewritten_element_by_element_and_stays_an_array() {
    // PDF 32000-1 §7.8.2 makes an array `/Contents` ONE lexical stream, so the cut is planned
    // on the concatenation and written back along the element boundaries. A rewriter that put
    // the whole concatenation into the first element would collapse the array -- which is what
    // spike 0006 measured the naive route doing.
    //
    // The text object straddles the two elements on purpose: `BT` is in the first and the `Tj`
    // in the second. Element-wise tokenising sees two parses, neither holding a complete text
    // object, so a fixture whose elements each parse alone would not ask this question.
    //
    // # This test asserted the wrong thing first, and a review measured it
    //
    // It checked only that `/Contents` still contained a `[`. Collapsing the **distribution of
    // bytes across the elements** leaves the array of references untouched -- the mutation that
    // writes the whole concatenation into element 0 and empties the rest passed it, which is
    // exactly the defect the comment in `rewrite` names. The array token was never the thing
    // at risk.
    //
    // So the assertions are now per element: element 0 keeps its own bytes and element 1 keeps
    // its own, and neither is the concatenation of both.
    let (document, _, _) = array_contents_document(false);
    let (out, _) = redact(&document).expect("an array /Contents redacts");

    assert_absent(&out, b"SECRET", "the removed text");
    assert_present(&out, b"KIN", "the text outside the region");

    // RESOLVED THROUGH THE OUTPUT'S OWN ARRAY, because qpdf renumbers on write. The first
    // version of this read the fixture's object numbers and could not find them.
    let elements = contents_elements(&out);
    assert_eq!(elements.len(), 2, "two elements in, two elements out");
    let head_bytes = stream_body(&out, elements[0]);
    let tail_bytes = stream_body(&out, elements[1]);
    // Element 0 held the `BT … Td` prefix and must still hold it, and only it.
    assert!(
        head_bytes.contains("Td"),
        "element 0 lost its own content: {head_bytes:?}"
    );
    assert!(
        !head_bytes.contains("KIN"),
        "element 1's text was written into element 0 -- the array was collapsed: {head_bytes:?}"
    );
    // Element 1 held the showing operators and must still hold them, and must not be empty.
    assert!(
        tail_bytes.contains("KIN"),
        "element 1 lost its own content: {tail_bytes:?}"
    );
    assert!(
        !tail_bytes.trim().is_empty(),
        "element 1 was emptied -- the array was collapsed into element 0"
    );
}

#[test]
fn an_array_whose_reached_element_is_shared_refuses_by_name() {
    // THE CASE THE SEAM LEAKS AT. Element 1 -- the one holding the text the region reaches --
    // is also page 1's whole `/Contents`. Editing it in place removes that text from a page
    // nobody selected, and §6's read-back would be clean on the page it was given.
    //
    // Under-detecting this asks only whether the ARRAY object is shared, which it is not.
    let (document, _, _) = array_contents_document(true);
    let text = refusal(&document, "the reached element is shared");
    assert!(
        text.contains("shared-contents"),
        "the refusal must name the sharing rule, got: {text}"
    );
}

#[test]
fn an_array_whose_shared_element_the_region_misses_is_redacted() {
    // THE NEAR-MISS TWIN, and the over-detection half. Element 0 is the shared one and it holds
    // the letterhead, which sits at the foot of the page; the region reaches only element 1.
    // A page-level rule -- "any element shared, refuse" -- would refuse this, and a shared
    // letterhead across every page of a document is an ordinary shape rather than an exotic
    // one. Without this test the rule above could tighten to a page-level one and stay green.
    let (document, _) = shared_letterhead_document();
    let (out, _) = redact(&document).expect("the region never reaches the shared element");
    assert_absent(&out, b"SECRET", "the removed text");
    assert_present(&out, b"LETTERHEAD", "the shared element, untouched");
}

#[test]
fn one_page_referencing_one_stream_twice_refuses_by_name() {
    // `/Contents [5 0 R 5 0 R]` is legal and is ONE page sharing with ITSELF. The concatenation
    // holds that text twice, and an edit written back to the object applies at both positions.
    // A rule that asked "does another PAGE use this?" answers no and is wrong, which is why the
    // count is of references rather than of pages.
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let page = pdf.reserve();
    let font = pdf.add(&helvetica_with_widths());
    let body = pdf.stream("", "BT /F1 24 Tf 72 700 Td (SECRET) Tj ET\n");
    pdf.put(
        page,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 {font} 0 R >> >> \
             /Contents [{body} 0 R {body} 0 R] >>"
        ),
    );
    pdf.put(
        pages,
        &format!("<< /Type /Pages /Count 1 /Kids [{page} 0 R] >>"),
    );
    pdf.put(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    let document = pdf.build(catalog);

    let text = refusal(&document, "one object referenced twice by one page");
    assert!(text.contains("shared-contents"), "got: {text}");
}

#[test]
fn two_pages_sharing_one_whole_contents_stream_refuses_by_name() {
    // The simplest form of the hazard, and the one ADR 0029 named: two pages, one `/Contents`
    // object, no array anywhere.
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let first = pdf.reserve();
    let second = pdf.reserve();
    let font = pdf.add(&helvetica_with_widths());
    let body = pdf.stream("", "BT /F1 24 Tf 72 700 Td (SECRET) Tj ET\n");
    for page in [first, second] {
        pdf.put(
            page,
            &format!(
                "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
                 /Resources << /Font << /F1 {font} 0 R >> >> /Contents {body} 0 R >>"
            ),
        );
    }
    pdf.put(
        pages,
        &format!("<< /Type /Pages /Count 2 /Kids [{first} 0 R {second} 0 R] >>"),
    );
    pdf.put(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    let document = pdf.build(catalog);

    let text = refusal(&document, "two pages sharing one content stream");
    assert!(text.contains("shared-contents"), "got: {text}");
}

#[test]
fn a_contents_array_holding_something_that_is_not_a_stream_refuses_by_name() {
    // Skipping a non-stream element would shift every later element's index, so the map from an
    // offset back to an object would name the wrong one -- a cut written to the wrong stream,
    // reported as success.
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let page = pdf.reserve();
    let font = pdf.add(&helvetica_with_widths());
    let null = pdf.add("<< /Type /Null >>");
    let body = pdf.stream("", "BT /F1 24 Tf 72 700 Td (SECRET) Tj ET\n");
    pdf.put(
        page,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 {font} 0 R >> >> \
             /Contents [{null} 0 R {body} 0 R] >>"
        ),
    );
    pdf.put(
        pages,
        &format!("<< /Type /Pages /Count 1 /Kids [{page} 0 R] >>"),
    );
    pdf.put(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    let document = pdf.build(catalog);

    let text = refusal(&document, "a non-stream element");
    assert!(text.contains("contents-not-a-stream"), "got: {text}");
}

/// A one- or two-page document whose page 0 `/Contents` is an array of two elements, with the
/// text object **straddling** them: `BT` ends element 0, the `Tj` is in element 1.
///
/// With `share`, page 1 exists and its whole `/Contents` is element 1 — the element the region
/// reaches. Without it, page 0 is the only page and nothing is shared.
fn array_contents_document(share: bool) -> (Vec<u8>, usize, usize) {
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let first = pdf.reserve();
    let second = pdf.reserve();
    let font = pdf.add(&helvetica_with_widths());
    // ELEMENT 0 ENDS MID-TEXT-OBJECT. Tokenising it alone yields a `BT` with no `ET`.
    let head = pdf.stream("", "BT /F1 24 Tf 72 700 Td\n");
    let tail = pdf.stream("", "(SECRET) Tj ET\nBT /F1 24 Tf 72 300 Td (KIN) Tj ET\n");
    pdf.put(
        first,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 {font} 0 R >> >> \
             /Contents [{head} 0 R {tail} 0 R] >>"
        ),
    );
    if share {
        pdf.put(
            second,
            &format!(
                "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
                 /Resources << /Font << /F1 {font} 0 R >> >> /Contents {tail} 0 R >>"
            ),
        );
        pdf.put(
            pages,
            &format!("<< /Type /Pages /Count 2 /Kids [{first} 0 R {second} 0 R] >>"),
        );
    } else {
        pdf.put(second, "<< /Type /Null >>");
        pdf.put(
            pages,
            &format!("<< /Type /Pages /Count 1 /Kids [{first} 0 R] >>"),
        );
    }
    pdf.put(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    (pdf.build(catalog), head, tail)
}

/// Two pages sharing element 0 — a letterhead at the foot of the page — with each page's own
/// body in element 1. The region reaches the body and never the letterhead.
fn shared_letterhead_document() -> (Vec<u8>, usize) {
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let first = pdf.reserve();
    let second = pdf.reserve();
    let font = pdf.add(&helvetica_with_widths());
    let letterhead = pdf.stream("", "BT /F1 12 Tf 72 40 Td (LETTERHEAD) Tj ET\n");
    let body_one = pdf.stream("", "BT /F1 24 Tf 72 700 Td (SECRET) Tj ET\n");
    let body_two = pdf.stream("", "BT /F1 24 Tf 72 700 Td (OTHER) Tj ET\n");
    for (page, body) in [(first, body_one), (second, body_two)] {
        pdf.put(
            page,
            &format!(
                "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
                 /Resources << /Font << /F1 {font} 0 R >> >> \
                 /Contents [{letterhead} 0 R {body} 0 R] >>"
            ),
        );
    }
    pdf.put(
        pages,
        &format!("<< /Type /Pages /Count 2 /Kids [{first} 0 R {second} 0 R] >>"),
    );
    pdf.put(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    (pdf.build(catalog), letterhead)
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
    let text = refusal(&document, "a /Rect that is not four numbers");
    assert!(text.contains("annotation-rect"), "got: {text}");
}

#[test]
fn a_page_index_past_the_end_refuses_rather_than_reaching_for_the_page() {
    // KILLS: the page-count check `page_handle`'s SAFETY comment relies on.
    //
    // **`redacted` NAMES PAGE 7, and that is the point.** With `{0}` the `burrow-ops` guard
    // fires first -- "the page being redacted must be one the operation covers" -- and the
    // constructor's bound is never reached. A review disabled that bound and the whole suite
    // stayed green, because this test was asserting on a rule it had been silently re-pointed
    // at when #134 added the guard. `a_page_outside_the_redacted_set_is_refused` covers the
    // guard separately, so naming page 7 here loses nothing.
    let document = page_with_font("");
    let redacted: BTreeSet<usize> = [7].into_iter().collect();
    let text = match support::redact_page(&document, 7, redacted, band()) {
        Ok((out, _)) => panic!(
            "redacted {} bytes for a page that does not exist",
            out.len()
        ),
        Err(error) => format!("{error:?}"),
    };
    assert!(
        text.contains("page-out-of-range"),
        "the CONSTRUCTOR's bound must be what refuses, got: {text}"
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
    // THE FORM'S RESOURCES NAME THE REAL FONT. They said `/F1 3 0 R` -- a hardcoded object
    // number that is not the font -- and the fixture passed anyway, because the walk ignored a
    // form's own `/Resources` entirely. Fixing that leak turned this fixture red, which is the
    // fixture telling the truth for the first time.
    let form = pdf.stream(
        &format!(
            "/Type /XObject /Subtype /Form /BBox [0 0 400 40] \
             /Resources << /Font << /F1 {font} 0 R >> >>"
        ),
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

    let text = refusal(&document, "a shared form the region reaches into");
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
        &format!(
            "/Type /XObject /Subtype /Form /BBox [0 0 400 40] \
             /Resources << /Font << /F1 {font} 0 R >> >>"
        ),
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

    let text = refusal(&document, "a /Parent chain that does not terminate");
    // THE RULE THAT FIRES MOVED WHEN #134 LANDED, and the new one is earlier. The operation now
    // reads the input's `/Rotate` vector before anything is edited -- that is the promise
    // `Expected::RegionCleared` carries -- and the sweep walks the page tree, so a cycle is
    // refused there rather than in `redact_frame`'s climb. Both are bounded walks of the same
    // tree naming the same shape; the earlier one is the better place to refuse from, because
    // nothing has been touched yet.
    assert!(
        text.contains("page tree is deeper")
            || text.contains("page-tree-depth")
            || text.contains("no-display-box"),
        "the refusal must name the page-tree walk or the missing box, got: {text}"
    );
}

#[test]
fn a_contents_array_past_the_element_ceiling_is_refused_by_the_walk() {
    // THE CEILING, THROUGH THE OPERATION. What it measures is that the refusal reaches a
    // caller of `redact_page` at all -- the ceiling itself is the walk's, and
    // `sharing_tests::a_contents_array_past_the_element_ceiling_is_refused_by_the_walk` is
    // what makes it non-inert, because only a direct call can tell which ceiling fired.
    //
    // Kept rather than deleted as a duplicate: it is the only thing asserting that the walk's
    // refusal is not swallowed somewhere between `new` and the caller.
    let elements = burrow_engines::pdfsyntax::contents::MAX_ELEMENTS + 1;
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let page = pdf.reserve();
    let font = pdf.add(&helvetica_with_widths());
    let mut refs = Vec::with_capacity(elements);
    refs.push(format!(
        "{} 0 R",
        pdf.stream("", "BT /F1 24 Tf 72 700 Td (SECRET) Tj ET\n")
    ));
    for _ in 1..elements {
        refs.push(format!("{} 0 R", pdf.stream("", " ")));
    }
    pdf.put(
        page,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 {font} 0 R >> >> /Contents [{}] >>",
            refs.join(" ")
        ),
    );
    pdf.put(
        pages,
        &format!("<< /Type /Pages /Count 1 /Kids [{page} 0 R] >>"),
    );
    pdf.put(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    let document = pdf.build(catalog);

    let text = refusal(&document, "past the element ceiling");
    assert!(
        text.contains("contents-too-many"),
        "the WALK must refuse it, not the splice one step later: {text}"
    );
}

#[test]
fn a_contents_array_at_the_element_ceiling_is_read() {
    // THE NEAR-MISS. Without it the test above passes for a ceiling of one, and a ceiling of
    // one would refuse every two-element document in existence.
    let elements = burrow_engines::pdfsyntax::contents::MAX_ELEMENTS;
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let page = pdf.reserve();
    let font = pdf.add(&helvetica_with_widths());
    let mut refs = Vec::with_capacity(elements);
    refs.push(format!(
        "{} 0 R",
        pdf.stream("", "BT /F1 24 Tf 72 700 Td (SECRET) Tj ET\n")
    ));
    refs.push(format!(
        "{} 0 R",
        pdf.stream("", "BT /F1 24 Tf 72 300 Td (KIN) Tj ET\n")
    ));
    for _ in 2..elements {
        refs.push(format!("{} 0 R", pdf.stream("", " ")));
    }
    pdf.put(
        page,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 {font} 0 R >> >> /Contents [{}] >>",
            refs.join(" ")
        ),
    );
    pdf.put(
        pages,
        &format!("<< /Type /Pages /Count 1 /Kids [{page} 0 R] >>"),
    );
    pdf.put(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    let document = pdf.build(catalog);

    let (out, _) = redact(&document).expect("exactly at the ceiling");
    assert_absent(&out, b"SECRET", "the removed text");
    assert_present(&out, b"KIN", "the text outside the region");
}

#[test]
fn a_stream_that_is_both_a_page_content_and_a_form_is_counted_once_in_total() {
    // THE TWO-COUNTER LEAK. `check_form_sharing` consulted the form counts and
    // `check_contents_sharing` the content counts, and neither was the TOTAL. An object reached
    // once by each route has one reference in each map, so both checks passed and the object
    // was edited -- removing the text from the page that draws it as a form, with §6's
    // read-back clean on the page that was asked for.
    //
    // A content stream may legally carry extra dictionary keys, so one object can be a valid
    // page content stream AND a valid Form XObject at once. Nothing exotic is needed for the
    // other two routes the review measured; this is the one that needs saying out loud.
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let first = pdf.reserve();
    let second = pdf.reserve();
    let font = pdf.add(&helvetica_with_widths());
    // Page 0's whole /Contents, and also a Form XObject page 1 draws.
    let dual = pdf.stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 612 792]",
        "BT /F1 24 Tf 72 700 Td (SECRET) Tj ET\n",
    );
    let other = pdf.stream("", "q 1 0 0 1 0 0 cm /Fm0 Do Q\n");
    pdf.put(
        first,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 {font} 0 R >> >> /Contents {dual} 0 R >>"
        ),
    );
    pdf.put(
        second,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 {font} 0 R >> /XObject << /Fm0 {dual} 0 R >> >> \
             /Contents {other} 0 R >>"
        ),
    );
    pdf.put(
        pages,
        &format!("<< /Type /Pages /Count 2 /Kids [{first} 0 R {second} 0 R] >>"),
    );
    pdf.put(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    let document = pdf.build(catalog);

    let text = refusal(&document, "one object, two routes, two references");
    assert!(
        text.contains("shared-contents") || text.contains("shared-form"),
        "the refusal must name a sharing rule, got: {text}"
    );
}

#[test]
fn a_form_the_region_reaches_that_is_also_another_pages_contents_refuses() {
    // The mirror of the case above, and the reason the fix is one total rather than each check
    // learning about the other's map: here the region reaches a FORM, and what makes it shared
    // is a `/Contents` reference.
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let first = pdf.reserve();
    let second = pdf.reserve();
    let font = pdf.add(&helvetica_with_widths());
    let dual = pdf.stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 612 792]",
        "BT /F1 24 Tf 72 700 Td (SECRET) Tj ET\n",
    );
    let body = pdf.stream("", "q 1 0 0 1 0 0 cm /Fm0 Do Q\n");
    pdf.put(
        first,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 {font} 0 R >> /XObject << /Fm0 {dual} 0 R >> >> \
             /Contents {body} 0 R >>"
        ),
    );
    pdf.put(
        second,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 {font} 0 R >> >> /Contents {dual} 0 R >>"
        ),
    );
    pdf.put(
        pages,
        &format!("<< /Type /Pages /Count 2 /Kids [{first} 0 R {second} 0 R] >>"),
    );
    pdf.put(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    let document = pdf.build(catalog);

    let text = refusal(&document, "a form that is also another page's content");
    assert!(
        text.contains("shared-form") || text.contains("shared-contents"),
        "the refusal must name a sharing rule, got: {text}"
    );
}

/// A three-element `/Contents` where the region reaches the **middle** element, with `shared`
/// naming which element is also another page's whole `/Contents`.
fn three_element_document(shared: usize) -> Vec<u8> {
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let first = pdf.reserve();
    let second = pdf.reserve();
    let font = pdf.add(&helvetica_with_widths());
    // Element 0 draws at the foot, element 1 in the band, element 2 at the foot.
    let parts = [
        pdf.stream("", "BT /F1 12 Tf 72 40 Td (HEAD) Tj ET\n"),
        pdf.stream("", "BT /F1 24 Tf 72 700 Td (SECRET) Tj ET\n"),
        pdf.stream("", "BT /F1 12 Tf 72 20 Td (FOOT) Tj ET\n"),
    ];
    pdf.put(
        first,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 {font} 0 R >> >> \
             /Contents [{} 0 R {} 0 R {} 0 R] >>",
            parts[0], parts[1], parts[2]
        ),
    );
    pdf.put(
        second,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 {font} 0 R >> >> /Contents {} 0 R >>",
            parts[shared]
        ),
    );
    pdf.put(
        pages,
        &format!("<< /Type /Pages /Count 2 /Kids [{first} 0 R {second} 0 R] >>"),
    );
    pdf.put(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    pdf.build(catalog)
}

#[test]
fn the_sharing_question_is_asked_of_the_element_the_cut_lands_in_not_a_fixed_one() {
    // KILLS: any constant index. A review planted "always ask about the LAST element" and the
    // whole suite stayed green, because every array fixture had exactly two elements and the
    // reached one was the last -- so `locate`'s answer and `len() - 1` were the same number
    // everywhere. `Contents::locate` is well tested; the OPERATION's use of it was not.
    //
    // Three elements, and the region reaches the MIDDLE one. Now "first", "last" and "the one
    // the cut lands in" are three different answers.

    // The middle element is shared: it is the one being edited, so refuse.
    let text = refusal(&three_element_document(1), "the middle element is shared");
    assert!(text.contains("shared-contents"), "got: {text}");

    // The FIRST element is shared and the region never reaches it: redact.
    let (out, _) = redact(&three_element_document(0)).expect("the shared element is element 0");
    assert_absent(&out, b"SECRET", "the removed text");
    assert_present(&out, b"HEAD", "the shared element, untouched");

    // The LAST element is shared and the region never reaches it: redact. This is the one a
    // `len() - 1` mutation gets wrong in the refusing direction.
    let (out, _) = redact(&three_element_document(2)).expect("the shared element is element 2");
    assert_absent(&out, b"SECRET", "the removed text");
    assert_present(&out, b"FOOT", "the shared element, untouched");
}

#[test]
fn a_page_with_no_contents_is_redacted_to_a_no_op_rather_than_refused() {
    // `/Contents` is optional (PDF 32000-1 §7.7.3.3) and a page without one is blank. Refusing
    // it was an undisclosed regression this commit introduced and a review measured against
    // the parent: the parent returned Ok and this returned `contents-missing`.
    //
    // A blank page has nothing to remove, so the redaction is a no-op that still produces a
    // document -- which is what a caller asking to redact a blank page should get.
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let page = pdf.reserve();
    let font = pdf.add(&helvetica_with_widths());
    pdf.put(
        page,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 {font} 0 R >> >> >>"
        ),
    );
    pdf.put(
        pages,
        &format!("<< /Type /Pages /Count 1 /Kids [{page} 0 R] >>"),
    );
    pdf.put(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    let document = pdf.build(catalog);

    let (out, _) = redact(&document).expect("a blank page is legal and redacts to a no-op");
    assert!(out.starts_with(b"%PDF"));
}

#[test]
fn a_contents_element_whose_data_will_not_decode_refuses_by_name() {
    // What a stream draws is unknown if its data will not decode, and unknown is not "nothing".
    // Declared `/FlateDecode` over bytes that are not flate.
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let page = pdf.reserve();
    let font = pdf.add(&helvetica_with_widths());
    let broken = pdf.stream("/Filter /DCTDecode", "not a jpeg at all\n");
    pdf.put(
        page,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 {font} 0 R >> >> /Contents {broken} 0 R >>"
        ),
    );
    pdf.put(
        pages,
        &format!("<< /Type /Pages /Count 1 /Kids [{page} 0 R] >>"),
    );
    pdf.put(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    let document = pdf.build(catalog);

    let text = refusal(&document, "a /Contents that will not decode");
    assert!(
        text.contains("contents-unreadable"),
        "the refusal must be the /Contents one, not some other unreadable: {text}"
    );
}

#[test]
fn a_contents_array_that_decodes_past_the_memory_ceiling_is_refused_before_it_is_held() {
    // KILLS: the missing running total in `page_contents`.
    //
    // `MAX_ELEMENTS` is 4096 and the same object may be referenced by every element, so the
    // decoded total is 4096 x the element size however small the file is -- no compression
    // needed. A security review measured 4096 references to one 1 MB stream: a **1.02 MB
    // input** reaching **8.2 GB** resident in 2.9 s, with the operation failing on a ceiling
    // inside `glyphs_in` only after both allocations had happened.
    //
    // THE CEILING IS STATED RATHER THAN THE FIXTURE SIZED TO THE DEFAULT. Building a document
    // that passes a one-gigabyte default is a memory experiment, and a fixture sized to a
    // default stops testing anything the day the default moves. 64 elements of 64 kB is 4 MB,
    // against a stated 1 MB.
    //
    // ONE OBJECT REFERENCED 4096 TIMES, which is what makes the file small and the decode
    // large -- the amplification the bound exists for. The sharing rule would also refuse this
    // document, but `page_contents` runs first inside `affected_streams`, so the bound is what
    // fires and the assertion below says so by name.
    //
    // The ceiling has to clear the OPEN's own size estimate, which consults `max_memory_bytes`
    // at the `SizeEstimate` stage: a 1 MB ceiling was refused before this code ran at all, on
    // the first version of this test.
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let page = pdf.reserve();
    let font = pdf.add(&helvetica_with_widths());
    let padding = "% ".to_owned() + &"x".repeat(64 * 1024) + "\n";
    let body = pdf.stream(
        "",
        &format!("BT /F1 24 Tf 72 700 Td (SECRET) Tj ET\n{padding}"),
    );
    let refs: Vec<String> = std::iter::repeat_n(format!("{body} 0 R"), 4096).collect();
    pdf.put(
        page,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 {font} 0 R >> >> /Contents [{}] >>",
            refs.join(" ")
        ),
    );
    pdf.put(
        pages,
        &format!("<< /Type /Pages /Count 1 /Kids [{page} 0 R] >>"),
    );
    pdf.put(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    let document = pdf.build(catalog);

    let mut limits = burrow_types::Limits::default();
    limits.max_memory_bytes = 32 * 1024 * 1024;
    let redacted: BTreeSet<usize> = [0].into_iter().collect();
    let text = match support::redact_page_with(&document, 0, redacted, band(), limits) {
        Ok((out, _)) => panic!("redacted {} bytes past the memory ceiling", out.len()),
        Err(error) => format!("{error:?}"),
    };
    assert!(
        text.contains("contents-too-large"),
        "the refusal must come from the decode bound, not from a ceiling downstream of the \
         allocation: {text}"
    );
}

#[test]
fn a_contents_array_inside_the_memory_ceiling_is_read() {
    // THE NEAR-MISS. Without it the test above passes for a ceiling of zero, which would refuse
    // every document there is.
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let page = pdf.reserve();
    let font = pdf.add(&helvetica_with_widths());
    let head = pdf.stream("", "BT /F1 24 Tf 72 700 Td\n");
    let tail = pdf.stream("", "(SECRET) Tj ET\nBT /F1 24 Tf 72 300 Td (KIN) Tj ET\n");
    pdf.put(
        page,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 {font} 0 R >> >> \
             /Contents [{head} 0 R {tail} 0 R] >>"
        ),
    );
    pdf.put(
        pages,
        &format!("<< /Type /Pages /Count 1 /Kids [{page} 0 R] >>"),
    );
    pdf.put(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    let document = pdf.build(catalog);

    let mut limits = burrow_types::Limits::default();
    limits.max_memory_bytes = 32 * 1024 * 1024;
    let redacted: BTreeSet<usize> = [0].into_iter().collect();
    let (out, _) = support::redact_page_with(&document, 0, redacted, band(), limits)
        .expect("well inside the ceiling");
    assert_absent(&out, b"SECRET", "the removed text");
}

#[test]
fn a_page_outside_the_redacted_set_is_refused() {
    // KILLS: dropping the page-in-set check. `cut_fonts` decides cuttability across `redacted`,
    // so redacting a page the set does not contain judges that page's fonts against a set that
    // excludes it -- not a wrong answer so much as an unanswerable question.
    let document = page_with_font("");
    let elsewhere: BTreeSet<usize> = [1].into_iter().collect();
    let text = match support::redact_page(&document, 0, elsewhere, band()) {
        Ok((out, _)) => panic!("redacted {} bytes for a page it does not cover", out.len()),
        Err(error) => format!("{error:?}"),
    };
    assert!(
        text.contains("must be one the operation covers"),
        "got: {text}"
    );
}

#[test]
fn the_read_back_reports_what_it_examined_rather_than_nothing() {
    // NON-VACUITY FOR THE INSTRUMENT. Every assertion the verification makes is of the form
    // "this set is empty"; a witness that reported empty sets for everything would satisfy all
    // three checks over any document at all. A mutation sweep planted exactly that for the
    // mapped codes and for the page keys and nothing failed.
    //
    // So: a document whose font HAS mappings and whose page HAS keys, redacted successfully,
    // with the emitted document's own numbers read back and asserted non-zero.
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let page = pdf.reserve();
    let to_unicode = pdf.stream(
        "",
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CMapType 2 def\n\
         1 begincodespacerange\n<00> <FF>\nendcodespacerange\n\
         2 beginbfchar\n<53> <0053>\n<4B> <004B>\nendbfchar\nendcmap\nend\nend\n",
    );
    let font = pdf.add(&format!(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 32 /LastChar 94 \
         /Widths {} /ToUnicode {to_unicode} 0 R \
         /Encoding << /Type /Encoding /Differences [83 /Sacute 75 /Kcommaaccent] >> >>",
        support::pdf_builder::HELVETICA_WIDTHS
    ));
    let content = pdf.stream(
        "",
        "BT /F1 24 Tf 72 700 Td (S) Tj ET\nBT /F1 24 Tf 72 300 Td (K) Tj ET\n",
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

    let (out, report) = support::redact_page(&document, 0, [0].into_iter().collect(), band())
        .expect("a document with mappings and page keys redacts");

    // THE FONT WAS CUT, so the mapping check applied to it rather than exempting it.
    assert!(
        report.fonts.iter().any(|font| font.cut),
        "the mapping check only applies to cut fonts, so one must have been cut: {:?}",
        report.fonts
    );

    // AND THE OUTPUT STILL HAS BOTH THINGS THE CHECK LOOKS AT. If the emitted document had no
    // mappings and no page keys at all, every check would pass over nothing.
    let normalised = decompressed(&out);
    let text = String::from_utf8_lossy(&normalised);
    assert!(
        text.contains("beginbfchar"),
        "the output must still carry a /ToUnicode for the check to have examined one"
    );
    assert!(
        text.contains("/Differences"),
        "the output must still carry a /Differences for the check to have examined one"
    );
    assert!(
        text.contains("/Contents"),
        "the output must still carry page keys for the check to have examined them"
    );
    // The removed code's mapping is gone and the kept one's survives -- which is what makes the
    // two sets the check compares genuinely different.
    assert!(!text.contains("<53> <0053>"), "the removed code's mapping");
    assert!(text.contains("<4B> <004B>"), "the kept code's mapping");
}

/// A page whose `/F1` has zero widths and a form whose own `/F1` has real ones, drawing
/// `SECRETSECRET` inside the form.
///
/// **The fixture that would have caught the form-resources leak**, and the reason it is built
/// with two fonts under one name: a form resolving `/F1` against the page places every glyph at
/// the same point (zero advance), while a form resolving it against its own `/Resources` spreads
/// them across 160 points. PDFium does the second. Any fixture whose page font and form font
/// agree cannot tell the two apart, which is why none of the existing ones did.
fn form_with_its_own_font() -> Vec<u8> {
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let page = pdf.reserve();
    let zeros = "[".to_owned() + &"0 ".repeat(63) + "]";
    let page_font = pdf.add(&format!(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding \
         /FirstChar 32 /LastChar 94 /Widths {zeros} >>"
    ));
    let form_font = pdf.add(&helvetica_with_widths());
    let form = pdf.stream(
        &format!(
            "/Type /XObject /Subtype /Form /BBox [0 0 612 792] \
             /Resources << /Font << /F1 {form_font} 0 R >> >>"
        ),
        "BT /F1 24 Tf 72 700 Td (SECRETSECRET) Tj ET\n",
    );
    let content = pdf.stream("", "q 1 0 0 1 0 0 cm /Fm0 Do Q\n");
    pdf.put(
        page,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 {page_font} 0 R >> /XObject << /Fm0 {form} 0 R >> >> \
             /Contents {content} 0 R >>"
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
fn a_form_resolves_its_font_against_its_own_resources_and_not_the_pages() {
    // THE LEAK, AS A FIXTURE. Before the fix: PDFium rendered twelve characters from x=73 to
    // x=233; burrow placed all twelve in a zero-width box at x=72, a region over the rendered
    // text reached nothing, the redaction returned `Ok`, both verification passes agreed, and
    // SEVEN characters were still drawn inside the rectangle the user selected.
    //
    // The assertion is against PDFium rather than against a remembered number: this is exactly
    // the disagreement ADR 0029 §6's oracle exists to catch, and it had never been asked
    // because no fixture gave a form resources of its own.
    let document = form_with_its_own_font();
    let oracle: Vec<_> = chars_on_page(&document, 0)
        .into_iter()
        .filter(|char| !char.generated)
        .collect();
    assert_eq!(oracle.len(), 12, "the fixture draws twelve characters");

    let walked = burrow_engines::glyphs_on_first_page(&document, &support::walk_options())
        .expect("the walk succeeds");
    assert_eq!(walked.len(), 12, "and burrow places twelve");

    // EVERY GLYPH AGREES WITH PDFIUM'S ORIGIN. The zero-width failure put them all at x=72.
    for (at, (glyph, char)) in walked.iter().zip(&oracle).enumerate() {
        assert!(
            (glyph.origin.0 - char.origin.0).abs() < TOLERANCE_PT,
            "glyph {at}: burrow places x={:.1}, PDFium reads x={:.1}",
            glyph.origin.0,
            char.origin.0
        );
    }
}

#[test]
fn a_region_over_a_forms_rendered_text_removes_it() {
    // The consequence, end to end. The region covers the middle of what PDFium renders; every
    // character whose ink is inside it must be gone from the output.
    let document = form_with_its_own_font();
    let (_, height) = page_size(&document, 0);
    let region = Region {
        left: 150.0,
        top: height - 724.0,
        width: 300.0,
        height: 40.0,
    };
    let before: Vec<_> = chars_on_page(&document, 0)
        .into_iter()
        .filter(|char| !char.generated)
        .collect();
    let reached = before
        .iter()
        .filter(|char| {
            ink_overlaps(
                &char.ink,
                region.left,
                region.top,
                region.width,
                region.height,
                height,
            )
        })
        .count();
    assert!(
        reached >= 5,
        "the region must reach several characters, reached {reached}"
    );

    let (out, _) = support::redact_page(&document, 0, [0].into_iter().collect(), region)
        .expect("a form's text is redactable");
    let after: Vec<_> = chars_on_page(&out, 0)
        .into_iter()
        .filter(|char| !char.generated)
        .collect();
    let left_inside = after
        .iter()
        .filter(|char| {
            ink_overlaps(
                &char.ink,
                region.left,
                region.top,
                region.width,
                region.height,
                height,
            )
        })
        .count();
    assert_eq!(
        left_inside, 0,
        "{left_inside} character(s) are still rendered inside the region the user selected"
    );
}

/// Nested carrying spans over many removals stay linear, measured rather than argued.
///
/// # Why a wall clock, which this suite otherwise avoids
///
/// This was quadratic twice, in two different places, and neither was visible to any assertion
/// about output — both produced *correct* output, slowly. The first was the covering-span walk,
/// found by a security review at 250,000 spans over one removal: 18.34 s. A `BTreeSet` took it
/// to 0.59 s and looked like the fix.
///
/// It was not. The set deduplicated the push and not the scan, so the cost stayed `spans x
/// removals` — and the measurement that "confirmed" the fix had varied only one of those two
/// factors. A second review varied both: 60,000 x 60,000, an 11,842-byte file, **109 s** and
/// 417 MB. A cursor into the stack fixed that one, and measuring the result found 13.8 s still
/// there, in `glyph_edits`, which rescanned every removal for every operation — 3.6e9
/// comparisons. Grouping once gives 0.72 s.
///
/// So: 8k, 16k and 60k, and the bound is wall clock because that is what was wrong. The margin
/// is wide (roughly 14x the measured 0.72 s) so an ordinary slow machine does not fail it, and a
/// return to either quadratic is 15x to 150x over it. A tighter bound would be a flaky test; a
/// looser one would not have caught the 13.8 s intermediate state, which is the one that was
/// believed fixed.
#[test]
fn nested_carriers_over_many_removals_do_not_go_quadratic() {
    fn page(n: usize) -> Vec<u8> {
        let mut pdf = Builder::new();
        let catalog = pdf.reserve();
        let pages = pdf.reserve();
        let page = pdf.reserve();
        let font = pdf.add(&format!(
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 32 /LastChar 94 \
             /Widths {} >>",
            support::pdf_builder::HELVETICA_WIDTHS
        ));
        let mut body = String::new();
        for _ in 0..n {
            body.push_str("/S << /MCID 0 /ActualText (x) >> BDC\n");
        }
        body.push_str("BT /F1 24 Tf\n");
        for _ in 0..n {
            body.push_str("1 0 0 1 72 700 Tm (S) Tj\n");
        }
        body.push_str("ET\n");
        for _ in 0..n {
            body.push_str("EMC\n");
        }
        let content = pdf.stream("", &body);
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

    const CEILING: std::time::Duration = std::time::Duration::from_secs(10);
    for n in [8_000usize, 16_000, 60_000] {
        let pdf = page(n);
        let started = std::time::Instant::now();
        let (_, report) = redact(&pdf).unwrap_or_else(|error| {
            panic!("{n} nested carriers over {n} removals should redact: {error:?}")
        });
        let took = started.elapsed();
        // AND THE WORK WAS ACTUALLY DONE. A walk that found nothing would be very fast and
        // would pass a timing bound while leaking every carrier it skipped.
        assert_eq!(
            report.dropped_carried_text, n,
            "{n} nested carriers: stripped {} of them",
            report.dropped_carried_text
        );
        assert!(
            took < CEILING,
            "{n} nested carriers over {n} removals took {took:?}, over the {CEILING:?} ceiling \
             -- the covering-span walk or the glyph grouping has gone quadratic again"
        );
    }
}

/// One form drawn 4,000 times is refused by its deadline while the walk runs, not after (#175).
///
/// # Why a wall clock, again
///
/// Before the geometry walk took a deadline, this document was refused by `max_duration_ms` too
/// -- at the next checkpoint between engine calls, **17.9 s** into a 100 ms budget. The rule was
/// right and the time was not, so asserting the rule alone would pass the defect. Each `Do`
/// re-walks the form's 100,000 operations; the cost is draws x form size, from a 233 KB file.
///
/// Measured after: 104 ms. The ceiling is 2 s, ~9x under the defect and ~19x over the fix,
/// so a slow machine does not fail it and a regression cannot pass it. Which checkpoint fires is
/// pinned deterministically by `geometry`'s unit tests; this pins that the operation reaches them.
#[test]
fn a_form_drawn_thousands_of_times_is_stopped_by_its_deadline_inside_the_walk() {
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let page = pdf.reserve();
    let font = pdf.add(&format!(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 32 /LastChar 94 \
         /Widths {} >>",
        support::pdf_builder::HELVETICA_WIDTHS
    ));
    let form = pdf.stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 612 792]",
        &"q Q\n".repeat(50_000),
    );
    let mut body = String::from("BT /F1 24 Tf 1 0 0 1 72 700 Tm (S) Tj ET\n");
    body.push_str(&"/Fm0 Do\n".repeat(4_000));
    let content = pdf.stream("", &body);
    pdf.put(
        page,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] /Resources << /Font \
             << /F1 {font} 0 R >> /XObject << /Fm0 {form} 0 R >> >> /Contents {content} 0 R >>"
        ),
    );
    pdf.put(
        pages,
        &format!("<< /Type /Pages /Count 1 /Kids [{page} 0 R] >>"),
    );
    pdf.put(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    let pdf = pdf.build(catalog);

    const CEILING: std::time::Duration = std::time::Duration::from_secs(2);
    let limits = burrow_types::Limits::with(|limits| limits.max_duration_ms = 100);
    let started = std::time::Instant::now();
    let outcome = support::redact_page_with(&pdf, 0, [0].into_iter().collect(), band(), limits);
    let took = started.elapsed();
    match outcome {
        Err(burrow_types::Error::LimitExceeded { limit, .. }) => {
            assert_eq!(limit, "max_duration_ms", "refused by the wrong limit");
        }
        Err(error) => panic!("refused, but not by its deadline: {error:?}"),
        Ok((out, _)) => panic!("redacted {} bytes past a 100 ms deadline", out.len()),
    }
    assert!(
        took < CEILING,
        "4,000 draws of a 100,000-operation form took {took:?} to refuse, over the {CEILING:?} \
         ceiling -- the geometry walk is no longer reading the deadline while it works"
    );
}

/// The disclosure's **count** is the number of property lists stripped, not merely non-zero.
///
/// Only `discloses_dropped_alternative_text()` — a bool — drove §7, and the corpus's only
/// assertion about the field compared that accessor against its own body. Measured: mutating
/// `self.dropped_carried_text += dropped_here` to `+= dropped_here.min(1)` survived
/// `redaction_corpus`, `redaction_defences` and `redaction_disclosure`. Silencing it entirely was
/// caught; undercounting was not, and `dropped_carried_text` is a `pub` field a caller may show.
///
/// Three covering spans inside the band and one outside it, so the number also pins that the
/// walk does not strip a span the region never reached.
#[test]
fn the_number_of_stripped_property_lists_is_reported_not_just_its_sign() {
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let page = pdf.reserve();
    let font = pdf.add(&format!(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 32 /LastChar 94 \
         /Widths {} >>",
        support::pdf_builder::HELVETICA_WIDTHS
    ));
    let mut body = String::new();
    for (index, y) in [740, 700, 660].into_iter().enumerate() {
        body.push_str(&format!(
            "/Span << /MCID {index} /ActualText (CARRIED-{index}) >> BDC\n\
             BT /F1 24 Tf 72 {y} Td (SECRET) Tj ET\nEMC\n"
        ));
    }
    // OUTSIDE THE BAND. Its glyphs are never removed, so its `/ActualText` must survive and must
    // not be counted -- a walk that stripped every span on the page would still report 4.
    body.push_str(
        "/Span << /MCID 3 /ActualText (KEPT) >> BDC\n\
         BT /F1 24 Tf 72 300 Td (KIN) Tj ET\nEMC\n",
    );
    let content = pdf.stream("", &body);
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
    let (out, report) = redact(&pdf.build(catalog)).expect("three carried spans are rewritable");
    assert_eq!(
        report.dropped_carried_text, 3,
        "expected exactly the three spans the band covers, got {}",
        report.dropped_carried_text
    );
    assert!(report.discloses_dropped_alternative_text());
    for index in 0..3 {
        assert_absent(
            &out,
            format!("CARRIED-{index}").as_bytes(),
            "a stripped span",
        );
    }
    assert_present(&out, b"KEPT", "the span the region never reached");
}

/// The carrier fixtures whose canary must never reach the output.
///
/// # The membership is hand-written; the canaries are not
///
/// **Which fixtures are carrier shapes** is a judgement, and nothing in the manifest states it:
/// the list mixes `/ActualText` evasions, a Type 3 procedure that draws, and a `/ToUnicode` in a
/// form-local font, and their `probes_refusal` groups do not line up with "burrow looked at the
/// carrier". So the names stay here.
///
/// **Each canary is the manifest's** (#176). It was a second column copying
/// `tests/redaction/manifest.toml`'s `placement.canary` -- 15 of 15 matching when a review
/// cross-checked them, and `09-actualtext.pdf`, the headline fixture, missing until someone
/// noticed. A copy beside the thing it mirrors is the shape that rotted three times on this
/// milestone. Now a fixture removed from the manifest fails by name, and a canary changed there
/// fails the presence check on the generated file.
const CARRIER_EVASIONS: [&str; 22] = [
    "evade-actualtext-around-a-form.pdf",
    "evade-actualtext-inside-a-form.pdf",
    "evade-actualtext-around-a-nested-form.pdf",
    "evade-actualtext-in-the-middle-form.pdf",
    "evade-actualtext-over-a-form-without-resources.pdf",
    // NOT AN `/ActualText` SHAPE, and here for exactly that reason. A Type 3 glyph procedure
    // that draws a form keeps the secret's drawing operators in the output while rendering
    // nothing -- covered, not gone. The corpus reports it refusing; without this, restoring the
    // defect only moved it from the refused column to the redacted one, and the floor is a
    // floor, so the sweep stayed green. Measured: that mutation SURVIVED until this line.
    "evade-text-in-type3-via-form.pdf",
    // THE MEMO CASE. Six fixtures above and none of them reached it: a code review measured
    // that emptying the memo entirely is caught by `evade-actualtext-in-the-middle-form`, while
    // **path-dependent truncation** of it was caught by nothing.
    "evade-actualtext-under-a-form-with-two-parents.pdf",
    "evade-type3-font-named-only-inside-a-form.pdf",
    // THE CANARY IS A CMap ENTRY, not a drawn string: `/ToUnicode` is where the removed
    // character survives when the font is never narrowed. Its twin is here for the same reason
    // it exists at all -- if narrowing broke outright, only the twin would tell them apart.
    "evade-tounicode-in-a-form-local-font.pdf",
    "nearmiss-tounicode-on-a-page-font.pdf",
    // THE PAGE-WIDENING CASE. Its keep line is below the band these tests redact, so the page
    // contributes no removed glyph and is in the stream list only because it carries the span.
    "evade-actualtext-on-a-page-that-draws-nothing-itself.pdf",
    // THE DETECTOR/REWRITER GAP. `/ActualText` named as an array item is not a key, so the
    // rewriter removes nothing; before the rule that refuses this, the string reached the output.
    "evade-actualtext-named-outside-key-position.pdf",
    // ITS NEAR-MISS, here rather than only in the corpus because the claim is the same one: an
    // ordinary `/Lang (en-US)` beside the glyphs must be redacted, not refused, and either way
    // the canary must not come out.
    "nearmiss-ordinary-string-in-a-property-list.pdf",
    // THE HEADLINE FIXTURE FOR THIS FEATURE, which was not in this list. Spike 0006's channel 9
    // is `/ActualText` on a marked-content span — the plain shape the whole rewriter is about —
    // and the byte-level absence assertion ran on every evasion of it and not on it.
    "09-actualtext.pdf",
    // Its near-miss: the span is around a form the region never reaches, and the canary is drawn
    // by the page outside the span. It must be redacted rather than refused, and either way the
    // canary must not come out.
    "nearmiss-actualtext-around-an-untouched-form.pdf",
    // #166: A PROPERTY LIST NAMED THROUGH `/Properties`. The four evasions put the text where a
    // page-level or own-resources-only resolver misses it; the three twins are the shapes a
    // resolver that refused too much would take offline.
    "evade-actualtext-named-through-properties.pdf",
    "evade-actualtext-named-in-a-form-scope.pdf",
    "evade-actualtext-named-in-a-form-that-inherits.pdf",
    "evade-actualtext-behind-a-reference-in-named-properties.pdf",
    "nearmiss-named-properties-without-text.pdf",
    "nearmiss-named-actualtext-outside-the-region.pdf",
    "nearmiss-named-properties-decoy-on-the-page.pdf",
];

/// The rules this suite will accept a refusal *by*.
///
/// The `Err` arm below asserted only that the refusal **named** a rule. That passes for any named
/// refusal at all, including one about input size or page count — so a fixture that stopped
/// reaching the carrier logic entirely, because a generator change made it malformed or oversized,
/// would still have read as "the defence held". The canary would not be in the output, which is
/// true and says nothing.
///
/// These are the rules that mean burrow looked at the carrier and declined. Adding one is a
/// deliberate act; a refusal outside the list fails and names itself in the message.
const CARRIER_REFUSALS: [&str; 7] = [
    "marked-content-properties-unresolved",
    "marked-content-named-properties-carry-text",
    "marked-content-split-across-elements",
    "marked-content-carries-opaque-string",
    "form-vanished",
    "shared-form-would-change-elsewhere",
    "type-three-procedure-shows-text",
];

#[test]
fn a_carrier_never_reaches_the_output_however_deeply_its_glyphs_are_nested() {
    // WHY THIS IS NOT LEFT TO `redaction_corpus.rs`, which already runs these documents.
    //
    // That harness asks whether a glyph the region reached is still drawn, by comparing
    // PDFium's unicode **and origin**. For a span carrying `/ActualText` the origin does not
    // discriminate: PDFium reports every character of the replacement string at the span's
    // starting point, so all of them share one origin and the comparison turns on the unicode
    // alone. Measured (#164): with the nested descent in `form_names_for` disabled, this
    // document redacted, the carrier survived verbatim in the output, and the corpus sweep
    // stayed green and reported `reached 1, removed 2 more`.
    //
    // So the assertion that matters is made here, on the bytes: the carrier is not in the
    // output. That is the claim a user cares about, and it is the one the corpus cannot make.
    //
    // WRITTEN TO SURVIVE #165. A refusal satisfies it and so does a rewriter that narrows or
    // drops the entry — the test pins "the secret does not come out", not "burrow refuses". If
    // it pinned the refusal, landing the rewriter would turn an improvement into a failure,
    // which is what `redaction_corpus.rs`'s header warns against.
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/redaction/generated");
    let mut examined = 0usize;
    let declared = declared();
    for name in CARRIER_EVASIONS {
        let canaries = canaries_of(&declared, name);
        let path = directory.join(name);
        let pdf = std::fs::read(&path).unwrap_or_else(|error| {
            panic!("{name}: {error} -- run tools/check-redaction-corpus.sh to generate it")
        });
        // NON-VACUITY FIRST. A fixture whose canary is not in it measures nothing, and these
        // are generated, so a generator change could quietly empty one.
        for canary in &canaries {
            assert_present(&pdf, canary.as_bytes(), name);
        }
        examined += 1;
        match redact(&pdf) {
            Err(error) => {
                let text = format!("{error:?}");
                // A `nearmiss-` FIXTURE MUST NOT REFUSE, AND NOTHING HERE SAID SO. A refusal
                // keeps the canary out of the output, so a rule that grew until it fired on the
                // ordinary shape passed this suite unchanged: measured by reinstating the
                // rejected "refuse any property list holding a string" rule, which flipped
                // `nearmiss-ordinary-string-in-a-property-list` from redacted to refused, moved
                // the census 51/8 to 50/9, and failed nothing. `CARRIER_REFUSALS` below closes
                // the other half -- a fixture refusing for an unrelated reason -- and could not
                // close this one, because the rule it refuses by is the right rule fired on the
                // wrong document.
                // NOT THE AUTHORITY ANY MORE, and kept as a fast local guard. What a fixture must do
                // is its manifest `expect_after`, which `tools/check-redaction-corpus.py --after`
                // judges against a real run (#176); every `nearmiss-` fixture there says `gone`.
                // This prefix check predates that and catches the same mistake sooner, in the
                // suite that runs the carriers.
                assert!(
                    !name.starts_with("nearmiss-"),
                    "{name}: a near-miss must be redacted, not refused -- the rule fired on the \
                     ordinary shape it exists to stay off: {text}"
                );
                assert!(
                    text.contains('[') && text.contains(']'),
                    "{name}: refused without naming a rule: {text}"
                );
                assert!(
                    CARRIER_REFUSALS.iter().any(|rule| text.contains(rule)),
                    "{name}: refused by a rule that is not about the carrier, so this fixture \
                     stopped measuring what it was written for: {text}"
                );
            }
            Ok((out, _)) => {
                for canary in &canaries {
                    assert_absent(&out, canary.as_bytes(), name);
                }
            }
        }
    }
    assert_eq!(
        examined,
        CARRIER_EVASIONS.len(),
        "every carrier evasion fixture must be examined"
    );
}

/// #166's fixtures, each with the **one** rule its design says it must refuse by, or `None` for a
/// twin that must redact.
///
/// `CARRIER_REFUSALS` accepts any of seven rules, which is right for its question -- did burrow
/// look at the carrier and decline -- and too loose for this one. A named list carrying text
/// refused as *unresolved* would pass it, and that is the pre-#166 outcome: the resolver could be
/// deleted and the carrier test would stay green. So the rule is pinned per fixture here.
const RESOLVED_OUTCOMES: [(&str, Option<&str>); 27] = [
    (
        "evade-actualtext-named-through-properties.pdf",
        Some("marked-content-named-properties-carry-text"),
    ),
    (
        "evade-actualtext-named-in-a-form-scope.pdf",
        Some("marked-content-named-properties-carry-text"),
    ),
    (
        "evade-actualtext-named-in-a-form-that-inherits.pdf",
        Some("marked-content-named-properties-carry-text"),
    ),
    (
        "evade-actualtext-behind-a-reference-in-named-properties.pdf",
        Some("marked-content-properties-unresolved"),
    ),
    ("nearmiss-named-properties-without-text.pdf", None),
    ("nearmiss-named-actualtext-outside-the-region.pdf", None),
    ("nearmiss-named-properties-decoy-on-the-page.pdf", None),
    // OPTIONAL CONTENT, WHICH REDACTION NEVER REFUSED. The first two refused before #166 as
    // `marked-content-properties-unresolved` -- by the accident of being named, not by the rule
    // ADR 0029 §3 wrote for them.
    ("13-optional-content.pdf", Some("optional-content")),
    ("evade-oc-two-levels-down.pdf", Some("optional-content")),
    ("evade-oc-outside-the-region.pdf", Some("optional-content")),
    ("evade-oc-on-an-annotation.pdf", Some("optional-content")),
    ("evade-oc-on-an-image.pdf", Some("optional-content")),
    // THE REACHES A RESOURCES-ONLY WALK MISSES. Each is unused on the page, so the pattern and
    // Type 3 rules do not answer first and leave the reach unmeasured.
    (
        "evade-oc-inside-an-unused-pattern.pdf",
        Some("optional-content"),
    ),
    (
        "evade-oc-inside-an-unused-type3-font.pdf",
        Some("optional-content"),
    ),
    (
        "evade-oc-on-an-appearance-stream.pdf",
        Some("optional-content"),
    ),
    // FOUND BY THE #166 SECURITY REVIEW. An untyped membership dictionary is caught by the mark,
    // not by the type; a named appearance state is the branch no fixture reached.
    (
        "evade-oc-untyped-membership-dictionary.pdf",
        Some("optional-content-marked"),
    ),
    (
        "evade-oc-untyped-group.pdf",
        Some("optional-content-marked"),
    ),
    (
        "evade-oc-on-an-appearance-state.pdf",
        Some("optional-content"),
    ),
    // The twin `evade-oc-two-levels-down` has had since #164: nested forms with no layer at all.
    ("nearmiss-nested-forms-no-oc.pdf", None),
    ("nearmiss-oc-on-another-page.pdf", None),
    // A STREAM WHERE A DICTIONARY BELONGS. PDFium reads the stream's dictionary; burrow read it
    // as absent. Three of these were measured leaks returning `Ok`.
    (
        "evade-resources-stream-on-the-page.pdf",
        Some("not-a-dictionary-where-one-belongs"),
    ),
    (
        "evade-resources-stream-on-a-form.pdf",
        Some("not-a-dictionary-where-one-belongs"),
    ),
    (
        "evade-font-decoy-behind-a-resources-stream.pdf",
        Some("not-a-dictionary-where-one-belongs"),
    ),
    (
        "evade-xobject-category-as-a-stream.pdf",
        Some("not-a-dictionary-where-one-belongs"),
    ),
    (
        "evade-properties-as-a-stream-holding-a-layer.pdf",
        Some("not-a-dictionary-where-one-belongs"),
    ),
    (
        "evade-property-list-as-a-stream.pdf",
        Some("not-a-dictionary-where-one-belongs"),
    ),
    ("nearmiss-resources-inherited-from-pages.pdf", None),
];

/// The `probes_refusal` groups whose fixtures [`RESOLVED_OUTCOMES`] must cover, every one.
const RESOLVED_GROUPS: [&str; 3] = ["named /Properties", "optional content", "not a dictionary"];

/// The manifest, as `tools/check-redaction-corpus.sh` writes it beside the generated corpus.
fn manifest() -> serde_json::Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/redaction/generated/manifest.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "{}: {error} -- run tools/check-redaction-corpus.sh to write it",
            path.display()
        )
    });
    serde_json::from_str(&text).expect("the manifest export is JSON")
}

/// Each declared fixture's file name, `probes_refusal` group, and EVERY placement's canary --
/// the first placement's alone let a second canary in the same fixture go unasked (#176's review).
fn declared() -> Vec<(String, Option<String>, Vec<String>)> {
    let manifest = manifest();
    manifest["fixture"]
        .as_array()
        .expect("the manifest declares fixtures")
        .iter()
        .map(|fixture| {
            let file = fixture["file"]
                .as_str()
                .expect("every fixture names a file");
            let name = file.rsplit('/').next().unwrap_or(file).to_owned();
            let group = fixture["probes_refusal"].as_str().map(str::to_owned);
            let canaries = fixture["placement"]
                .as_array()
                .map(|placements| {
                    placements
                        .iter()
                        .filter_map(|placement| placement["canary"].as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            (name, group, canaries)
        })
        .collect()
}

/// The canaries `name` declares, refusing a fixture the manifest does not list or gives none.
fn canaries_of(declared: &[(String, Option<String>, Vec<String>)], name: &str) -> Vec<String> {
    let canaries = declared
        .iter()
        .find(|(file, _, _)| file == name)
        .map(|(_, _, canaries)| canaries.clone())
        .unwrap_or_default();
    assert!(
        !canaries.is_empty(),
        "{name}: a fixture the manifest does not declare, or declares no canary for"
    );
    canaries
}

#[test]
fn a_named_property_list_and_a_layer_refuse_for_their_own_reason() {
    // THE SET IS THE MANIFEST'S, not this file's. `examined == RESOLVED_OUTCOMES.len()` held by
    // construction -- a code review pointed out it could not fail -- while a fixture added to a
    // group with no entry here would go unpinned in silence. So the expectation comes from the
    // groups the fixtures declare, and `13-optional-content`, which predates the groups and
    // declares none, is the one name added by hand.
    let declared = declared();
    let mut expected: BTreeSet<String> = declared
        .iter()
        .filter(|(_, group, _)| {
            group
                .as_deref()
                .is_some_and(|group| RESOLVED_GROUPS.contains(&group))
        })
        .map(|(name, _, _)| name.clone())
        .collect();
    expected.insert("13-optional-content.pdf".to_owned());
    let listed: BTreeSet<String> = RESOLVED_OUTCOMES
        .iter()
        .map(|(name, _)| (*name).to_owned())
        .collect();
    assert_eq!(
        listed,
        expected,
        "RESOLVED_OUTCOMES pins {} fixtures; the manifest's groups declare {}",
        listed.len(),
        expected.len()
    );

    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/redaction/generated");
    let mut redacted = 0usize;
    for (name, wanted) in RESOLVED_OUTCOMES {
        let pdf = std::fs::read(directory.join(name)).unwrap_or_else(|error| {
            panic!("{name}: {error} -- run tools/check-redaction-corpus.sh to generate it")
        });
        match (redact(&pdf), wanted) {
            (Err(error), Some(rule)) => {
                // THE RULE, BRACKETED, not a substring of the message: `optional-content`
                // appears in prose elsewhere and a bare `contains` would be satisfied by it.
                let text = format!("{error:?}");
                assert!(
                    text.contains(&format!("[{rule}]")),
                    "{name}: refused, but not by `{rule}`: {text}"
                );
            }
            (Err(error), None) => panic!(
                "{name}: a near-miss must be redacted, not refused -- the rule fired on the \
                 ordinary shape it exists to stay off: {error:?}"
            ),
            (Ok((out, report)), Some(rule)) => panic!(
                "{name}: redacted {} bytes where `{rule}` should have refused, report {report:?}",
                out.len()
            ),
            // AND THE CANARY IS GONE. Redacting is not the claim; the secret leaving is.
            (Ok((out, _)), None) => {
                for canary in canaries_of(&declared, name) {
                    assert_present(&pdf, canary.as_bytes(), name);
                    assert_absent(&out, canary.as_bytes(), name);
                }
                redacted += 1;
            }
        }
    }
    let twins = RESOLVED_OUTCOMES
        .iter()
        .filter(|(_, rule)| rule.is_none())
        .count();
    eprintln!(
        "  #166 outcomes: {} of {} declared fixtures pinned, {redacted} of {twins} near-misses \
         redacted with their canary gone",
        listed.len(),
        expected.len()
    );
    assert_eq!(
        redacted, twins,
        "every near-miss must be checked for its canary"
    );
}

#[test]
fn an_annotation_layer_on_a_page_that_draws_nothing_is_refused() {
    // KILLS: moving the optional-content walk after the blank-page return. The comment at the
    // call site says it runs first because an annotation can carry `/OC` on a page with no
    // `/Contents`; a code review moved it and nothing failed. A document test cannot live in the
    // corpus, whose sweep requires every page to draw its keep line.
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let page = pdf.reserve();
    let ocg = pdf.add("<< /Type /OCG /Name (a layered note) >>");
    let annot = pdf.add(&format!(
        "<< /Type /Annot /Subtype /Text /Rect [72 700 92 720] /Contents (note) /OC {ocg} 0 R >>"
    ));
    pdf.put(
        page,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] /Resources << >> \
             /Annots [{annot} 0 R] >>"
        ),
    );
    pdf.put(
        pages,
        &format!("<< /Type /Pages /Count 1 /Kids [{page} 0 R] >>"),
    );
    pdf.put(
        catalog,
        &format!(
            "<< /Type /Catalog /Pages {pages} 0 R /OCProperties << /OCGs [{ocg} 0 R] \
             /D << /OFF [{ocg} 0 R] >> >> >>"
        ),
    );
    let refused = refusal(
        &pdf.build(catalog),
        "a blank page with a layered annotation",
    );
    assert!(
        refused.contains("[optional-content]"),
        "a blank page's layered annotation must refuse by name: {refused}"
    );
}

/// A page whose `/Properties` holds `on_page` ordinary lists, drawing the secret inside a form
/// whose own `/Properties` holds `in_form` more, one of them covering the secret.
///
/// **Two scopes, because one cannot reach the ceiling.** `pdfsyntax::dict::MAX_KEYS` refuses a
/// single dictionary past 4,096 keys before this ceiling sees it, so what `MAX_PROPERTY_LISTS`
/// actually bounds is the total across scopes -- a form per scope, each at the per-dictionary
/// cap. A one-scope fixture measured the other ceiling and reported this one as tested.
fn page_with_property_lists(on_page: usize, in_form: usize) -> Vec<u8> {
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let page = pdf.reserve();
    let font = pdf.add(&format!(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 32 /LastChar 94 \
         /Widths {} >>",
        support::pdf_builder::HELVETICA_WIDTHS
    ));
    let lists = |count: usize| -> String {
        (0..count)
            .map(|at| format!("/M{at} << /MCID {at} >> "))
            .collect()
    };
    let form = pdf.stream(
        &format!(
            "/Type /XObject /Subtype /Form /BBox [0 0 612 792] \
             /Resources << /Font << /F1 {font} 0 R >> /Properties << {}>> >>",
            lists(in_form)
        ),
        "/P /M0 BDC BT /F1 24 Tf 72 700 Td (SECRET) Tj ET EMC\n",
    );
    let content = pdf.stream("", "/X1 Do\n");
    pdf.put(
        page,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 {font} 0 R >> /XObject << /X1 {form} 0 R >> \
             /Properties << {}>> >> /Contents {content} 0 R >>",
            lists(on_page)
        ),
    );
    pdf.put(
        pages,
        &format!("<< /Type /Pages /Count 1 /Kids [{page} 0 R] >>"),
    );
    pdf.put(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    pdf.build(catalog)
}

/// One page drawing `(SECRET)` in the band with `/F1`, whose resources `shape` writes.
///
/// `shape` gets the builder and the font's object number and returns what follows `/Resources`
/// on the page (empty for none), what the `/Pages` node carries, and any other page keys.
fn page_shaped(shape: impl FnOnce(&mut Builder, usize) -> (String, String, String)) -> Vec<u8> {
    page_shaped_drawing("BT /F1 24 Tf 72 700 Td (SECRET) Tj ET\n", shape)
}

/// As [`page_shaped`], with the page's content stream written by the caller.
fn page_shaped_drawing(
    drawing: &str,
    shape: impl FnOnce(&mut Builder, usize) -> (String, String, String),
) -> Vec<u8> {
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let page = pdf.reserve();
    let font = pdf.add(&format!(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 32 /LastChar 94 \
         /Widths {} >>",
        support::pdf_builder::HELVETICA_WIDTHS
    ));
    let (resources, on_pages, page_extra) = shape(&mut pdf, font);
    let content = pdf.stream("", drawing);
    let resources = if resources.is_empty() {
        String::new()
    } else {
        format!(" /Resources {resources}")
    };
    pdf.put(
        page,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792]{resources}{page_extra} \
             /Contents {content} 0 R >>"
        ),
    );
    pdf.put(
        pages,
        &format!("<< /Type /Pages /Count 1 /Kids [{page} 0 R]{on_pages} >>"),
    );
    pdf.put(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    pdf.build(catalog)
}

/// A Type 3 font dictionary, written out, with `extra` keys.
fn type_three(extra: &str) -> String {
    format!(
        "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] \
         /FontMatrix [0.001 0 0 0.001 0 0] /FirstChar 97 /LastChar 97 /Widths [1000] \
         /Encoding << /Differences [97 /a] >> {extra} >>"
    )
}

#[test]
fn anything_but_a_dictionary_where_one_belongs_is_refused_on_every_route() {
    // KILLS: the type check skipped for any one key, and the first fix's version of it, which
    // refused a STREAM and read an array or an integer as absent -- a security review measured a
    // page `/Resources [ ]` inheriting a decoy font from `/Pages` and leaving the secret drawn.
    // One case per route the gate reads, because a mutation dropping the check for one key
    // survived while every other key had a fixture.
    type Shape = Box<dyn FnOnce(&mut Builder, usize) -> (String, String, String)>;
    let font = |f: usize| format!("/Font << /F1 {f} 0 R >>");
    let cases: Vec<(&str, Shape)> = vec![
        (
            "page /Resources an array, a font on /Pages",
            Box::new(move |_, f| {
                (
                    "[ ]".into(),
                    format!(" /Resources << {} >>", font(f)),
                    String::new(),
                )
            }),
        ),
        (
            "page /Resources an integer",
            Box::new(move |_, f| {
                (
                    "0".into(),
                    format!(" /Resources << {} >>", font(f)),
                    String::new(),
                )
            }),
        ),
        // NOT HERE: a stream `/Resources` on `/Pages` with none on the page. qpdf repairs that
        // before this walk runs -- "Resources is missing or invalid; repairing" -- by giving the
        // page an empty dictionary, so the gate never sees it. What follows from the repair is
        // in `a_do_naming_nothing_the_resources_hold_is_refused`.
        (
            "/Font a stream",
            Box::new(move |pdf, f| {
                let s = pdf.stream(&format!(" /F1 {f} 0 R"), "");
                (format!("<< /Font {s} 0 R >>"), String::new(), String::new())
            }),
        ),
        (
            "/XObject an array",
            Box::new(move |_, f| {
                (
                    format!("<< {} /XObject [ ] >>", font(f)),
                    String::new(),
                    String::new(),
                )
            }),
        ),
        (
            "/Pattern a stream",
            Box::new(move |pdf, f| {
                let s = pdf.stream("", "");
                (
                    format!("<< {} /Pattern {s} 0 R >>", font(f)),
                    String::new(),
                    String::new(),
                )
            }),
        ),
        (
            "/Properties an integer",
            Box::new(move |_, f| {
                (
                    format!("<< {} /Properties 7 >>", font(f)),
                    String::new(),
                    String::new(),
                )
            }),
        ),
        (
            "a /Properties entry an array",
            Box::new(move |_, f| {
                (
                    format!("<< {} /Properties << /M0 [ ] >> >>", font(f)),
                    String::new(),
                    String::new(),
                )
            }),
        ),
        (
            "a Type 3 font's /CharProcs a stream",
            Box::new(move |pdf, f| {
                let s = pdf.stream("", "");
                let t3 = pdf.add(&type_three(&format!("/CharProcs {s} 0 R")));
                (
                    format!("<< /Font << /F1 {f} 0 R /T3 {t3} 0 R >> >>"),
                    String::new(),
                    String::new(),
                )
            }),
        ),
        (
            "a Type 3 font's /Resources an array",
            Box::new(move |pdf, f| {
                let t3 = pdf.add(&type_three("/CharProcs << >> /Resources [ ]"));
                (
                    format!("<< /Font << /F1 {f} 0 R /T3 {t3} 0 R >> >>"),
                    String::new(),
                    String::new(),
                )
            }),
        ),
        (
            // TWO DIRECT FONTS SHARE THE IDENTITY `(0, 0)`, and a memo keyed on it read only
            // the first one's resources. The second one's is the wrong type.
            "the second of two direct Type 3 fonts, /Resources a stream",
            Box::new(move |pdf, f| {
                let s = pdf.stream("", "");
                let first = type_three("/CharProcs << >> /Resources << >>");
                let second = type_three(&format!("/CharProcs << >> /Resources {s} 0 R"));
                (
                    format!("<< /Font << /F1 {f} 0 R /A {first} /B {second} >> >>"),
                    String::new(),
                    String::new(),
                )
            }),
        ),
        (
            "an annotation's /AP a stream",
            Box::new(move |pdf, f| {
                let s = pdf.stream("", "");
                (
                    format!("<< {} >>", font(f)),
                    String::new(),
                    format!(
                        " /Annots [ << /Type /Annot /Subtype /Square /Rect [300 10 320 30] /AP {s} 0 R >> ]"
                    ),
                )
            }),
        ),
        (
            // NOT ONLY A STREAM: a mutation refusing only streams here survived until this case.
            "an /Annots entry an integer",
            Box::new(move |_, f| {
                (
                    format!("<< {} >>", font(f)),
                    String::new(),
                    " /Annots [ 0 ]".into(),
                )
            }),
        ),
        (
            // TWO DIRECT `/Properties` SHARE THE IDENTITY `(0, 0)`, and a memo keyed on it
            // checked only the first form's. The second one's entry is the wrong type.
            "the second of two forms' direct /Properties, an entry an array",
            Box::new(move |pdf, f| {
                let first = pdf.stream(
                    " /Type /XObject /Subtype /Form /BBox [0 0 1 1] \
                     /Resources << /Properties << /M0 << /MCID 0 >> >> >>",
                    "",
                );
                let second = pdf.stream(
                    " /Type /XObject /Subtype /Form /BBox [0 0 1 1] \
                     /Resources << /Properties << /M0 [ ] >> >>",
                    "",
                );
                (
                    format!(
                        "<< {} /XObject << /A {first} 0 R /B {second} 0 R >> >>",
                        font(f)
                    ),
                    String::new(),
                    String::new(),
                )
            }),
        ),
        (
            // THE REMOVAL STEP SKIPPED THIS ENTRY, so an annotation over the region survived.
            "an /Annots entry a stream",
            Box::new(move |pdf, f| {
                let s = pdf.stream(" /Type /Annot /Subtype /Square /Rect [72 690 300 730]", "");
                (
                    format!("<< {} >>", font(f)),
                    String::new(),
                    format!(" /Annots [ {s} 0 R ]"),
                )
            }),
        ),
    ];
    let total = cases.len();
    let mut refused = 0usize;
    for (what, shape) in cases {
        let text = refusal(&page_shaped(shape), what);
        assert!(
            text.contains("[not-a-dictionary-where-one-belongs]"),
            "{what}: refused, but not for the type: {text}"
        );
        refused += 1;
    }
    // THE NEAR-MISS: the same page, every key the type it should be, including an inherited
    // `/Resources` and a `null` where a dictionary is optional.
    let control = page_shaped(move |_, f| {
        (
            String::new(),
            format!(" /Resources << /Font << /F1 {f} 0 R >> /XObject null >>"),
            String::new(),
        )
    });
    let (out, _) = redact(&control).expect("every key its proper type must redact");
    assert_absent(&out, b"SECRET", "the well-typed control");
    eprintln!(
        "  wrong-type routes: {refused} of {total} refused by name, and the control redacted"
    );
}

#[test]
fn a_do_naming_nothing_the_resources_hold_is_refused() {
    // KILLS: `PageResources::form` reading an absent name as "draws nothing". Two routes to one
    // `Do` that PDFium draws and burrow's walk stepped over, each measured `Ok` with the secret
    // still drawn.
    let form = |pdf: &mut Builder, f: usize| {
        pdf.stream(
            &format!(
                " /Type /XObject /Subtype /Form /BBox [0 0 612 792] \
                 /Resources << /Font << /F1 {f} 0 R >> >>"
            ),
            "BT /F1 24 Tf 72 700 Td (SECRET) Tj ET\n",
        )
    };
    // 1. The form is named only by a stream-valued `/Resources` on `/Pages`. qpdf repairs that
    //    by giving the page an empty dictionary, so `/X1` names nothing by the time burrow
    //    reads it; PDFium resolves it through the original.
    let repaired = page_shaped_drawing("/X1 Do\n", |pdf, f| {
        let x1 = form(pdf, f);
        let s = pdf.stream(&format!(" /XObject << /X1 {x1} 0 R >>"), "");
        (String::new(), format!(" /Resources {s} 0 R"), String::new())
    });
    // 2. A form whose own `/Resources` has no `/XObject` draws `/X2 Do`. PDFium falls back to
    //    the page's `/XObject`, where `/X2` draws the secret. Found by the #166 security review.
    let fallback = page_shaped_drawing("/X1 Do\n", |pdf, f| {
        let x2 = form(pdf, f);
        let x1 = pdf.stream(
            &format!(
                " /Type /XObject /Subtype /Form /BBox [0 0 612 792] \
                 /Resources << /Font << /F1 {f} 0 R >> >>"
            ),
            "/X2 Do\n",
        );
        (
            format!("<< /Font << /F1 {f} 0 R >> /XObject << /X1 {x1} 0 R /X2 {x2} 0 R >> >>"),
            String::new(),
            String::new(),
        )
    });
    for (what, pdf) in [
        ("a Do the qpdf repair left naming nothing", repaired),
        ("a Do PDFium resolves by falling back to the page", fallback),
    ] {
        let refused = refusal(&pdf, what);
        assert!(
            refused.contains("[xobject-missing]"),
            "{what}: refused, but not because the name resolves to nothing: {refused}"
        );
    }
}

#[test]
fn a_font_object_reused_as_an_appearance_resources_is_read_as_resources_too() {
    // KILLS: one memo for "queued as a font" and "read as resources". The dictionary below is the
    // page's `/F9` and an appearance stream's `/Resources`; queued as the font first, it was
    // skipped as resources, so the `/OCG` in its `/Properties` was never read. Measured by the
    // #166 security review, `Ok`, with poppler and MuPDF hiding the layer in the output.
    let refused = refusal(
        &page_shaped(|pdf, f| {
            let ocg = pdf.add("<< /Type /OCG /Name (a layer) >>");
            let both = pdf.add(&format!("<< /Properties << /L0 {ocg} 0 R >> >>"));
            let appearance = pdf.stream(
                &format!(" /Type /XObject /Subtype /Form /BBox [0 0 20 20] /Resources {both} 0 R"),
                "0 0 20 20 re f\n",
            );
            (
                format!("<< /Font << /F1 {f} 0 R /F9 {both} 0 R >> >>"),
                String::new(),
                format!(
                    " /Annots [ << /Type /Annot /Subtype /Square /Rect [300 10 320 30] \
                     /AP << /N {appearance} 0 R >> >> ]"
                ),
            )
        }),
        "a font dictionary that is also an appearance's resources",
    );
    assert!(refused.contains("[optional-content]"), "{refused}");
}

/// A page whose one annotation's appearance is `appearance`, built by the caller.
fn page_with_appearance(appearance: impl FnOnce(&mut Builder) -> String) -> Vec<u8> {
    page_shaped(move |pdf, f| {
        let ap = appearance(pdf);
        (
            format!("<< /Font << /F1 {f} 0 R >> >>"),
            String::new(),
            format!(
                " /Annots [ << /Type /Annot /Subtype /Square /Rect [300 10 320 30] /AS /On \
                 /AP << /N {ap} >> >> ]"
            ),
        )
    })
}

/// An appearance form drawing `content`, with `extra` stream-dictionary keys.
fn appearance_form(pdf: &mut Builder, extra: &str, content: &str) -> usize {
    pdf.stream(
        &format!(" /Type /XObject /Subtype /Form /BBox [0 0 20 20]{extra}"),
        content,
    )
}

#[test]
fn an_optional_content_mark_anywhere_an_appearance_reaches_is_refused() {
    // KILLS: the mark scan deleted, narrowed to appearances, reading the FIRST operand, or
    // skipped for a state dictionary. The geometry walk refuses an `/OC` mark in every stream it
    // draws and never draws an appearance or a form an appearance draws; the #166 security
    // reviews measured a layer hidden in the output through each shape below.
    const MARK: &str = "/OC << /Type /OCMD /OCGs [ ] >> BDC 0 0 20 20 re f EMC\n";
    let cases: Vec<(&str, Vec<u8>)> = vec![
        (
            "inline in the appearance",
            page_with_appearance(|pdf| format!("{} 0 R", appearance_form(pdf, "", MARK))),
        ),
        (
            "in a form the appearance draws",
            page_with_appearance(|pdf| {
                let inner = appearance_form(pdf, "", MARK);
                let outer = appearance_form(
                    pdf,
                    &format!(" /Resources << /XObject << /Fm1 {inner} 0 R >> >>"),
                    "/Fm1 Do\n",
                );
                format!("{outer} 0 R")
            }),
        ),
        (
            "in one state of a state dictionary",
            page_with_appearance(|pdf| {
                let on = appearance_form(pdf, "", MARK);
                let off = appearance_form(pdf, "", "");
                format!("<< /On {on} 0 R /Off {off} 0 R >>")
            }),
        ),
        (
            "behind a padding operand",
            page_with_appearance(|pdf| {
                format!(
                    "{} 0 R",
                    appearance_form(
                        pdf,
                        "",
                        "/Pad /OC << /Type /OCMD /OCGs [ ] >> BDC 0 0 20 20 re f EMC\n"
                    )
                )
            }),
        ),
    ];
    for (what, pdf) in cases {
        let refused = refusal(&pdf, what);
        assert!(refused.contains("[optional-content]"), "{what}: {refused}");
    }

    // A MARK THAT CANNOT BE READ IS NOT A MARK THAT IS ABSENT: content holding `BDC` that does
    // not lex refuses rather than passing.
    let unreadable = page_with_appearance(|pdf| {
        format!(
            "{} 0 R",
            appearance_form(pdf, "", "/OC << /Type /OCMD >> BDC (unterminated\n")
        )
    });
    assert!(
        redact(&unreadable).is_err(),
        "an appearance whose marked content does not lex must refuse"
    );

    // THE NEAR-MISSES: an ordinary mark, and content that holds no `BDC` at all -- which is not
    // lexed, so syntax burrow would refuse elsewhere does not take the page offline here.
    for (what, content) in [
        (
            "an ordinary mark",
            "/P << /MCID 0 >> BDC 0 0 20 20 re f EMC\n",
        ),
        (
            "no mark, and content that does not lex",
            "0 0 20 20 re f (unterminated\n",
        ),
    ] {
        let page = page_with_appearance(|pdf| format!("{} 0 R", appearance_form(pdf, "", content)));
        let (out, _) = redact(&page).unwrap_or_else(|error| panic!("{what}: {error:?}"));
        assert_absent(&out, b"SECRET", what);
    }
}

#[test]
fn a_padded_optional_content_mark_on_the_page_is_refused() {
    // The page's own content is the geometry walk's. `BDC` now has an operand count, so the
    // padding that hid the tag from a first-operand read is refused before the tag is read.
    let refused = refusal(
        &page_shaped_drawing(
            "/Pad /OC /OC1 BDC BT /F1 24 Tf 72 700 Td (SECRET) Tj ET EMC\n",
            |_, f| {
                (
                    format!("<< /Font << /F1 {f} 0 R >> >>"),
                    String::new(),
                    String::new(),
                )
            },
        ),
        "a padded /OC mark on the page",
    );
    assert!(refused.contains("[operand-count-mismatch]"), "{refused}");
}

#[test]
fn many_names_on_one_large_property_list_classify_it_once() {
    // KILLS: disabling `PropertyScopes`' identity memo. A security review measured the shape
    // before the memo existed: thousands of names pointing at one large list, each unparsed and
    // lexed again. Correct output, slowly -- so no assertion about bytes could see it, and the
    // bound is on time, as `nested_carriers_over_many_removals_do_not_go_quadratic` is.
    let mut pdf = Builder::new();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let page = pdf.reserve();
    let font = pdf.add(&format!(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 32 /LastChar 94 \
         /Widths {} >>",
        support::pdf_builder::HELVETICA_WIDTHS
    ));
    let padding = "0 ".repeat(50_000);
    let list = pdf.add(&format!("<< /MCID 0 /K [{padding}] >>"));
    let names: String = (0..4000).map(|at| format!("/M{at} {list} 0 R ")).collect();
    let content = pdf.stream("", "/P /M0 BDC BT /F1 24 Tf 72 700 Td (SECRET) Tj ET EMC\n");
    pdf.put(
        page,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 {font} 0 R >> /Properties << {names}>> >> \
             /Contents {content} 0 R >>"
        ),
    );
    pdf.put(
        pages,
        &format!("<< /Type /Pages /Count 1 /Kids [{page} 0 R] >>"),
    );
    pdf.put(catalog, &format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    let started = std::time::Instant::now();
    let (out, _) = redact(&pdf.build(catalog)).expect("an ordinary named list redacts");
    let took = started.elapsed();
    // THE WORK WAS DONE, not skipped: a walk that resolved nothing would be fast too.
    assert_absent(&out, b"SECRET", "a page naming one large list 4,000 times");
    assert!(
        took < std::time::Duration::from_secs(10),
        "4,000 names on one 100 kB list took {took:?}; the list is being re-read per name"
    );
}

#[test]
fn property_lists_past_the_ceiling_are_refused_by_name_and_at_it_are_resolved() {
    // KILLS: deleting `MAX_PROPERTY_LISTS`' check. Each entry is an `unparse` through the
    // engine, and the scopes a redaction reads are as many as the forms on a path to a glyph.
    //
    // THE BOUNDARY, BOTH SIDES. At the ceiling the named span resolves to an ordinary list and
    // the page redacts -- which is also the proof the ceiling is not refusing ordinary pages.
    let (out, _) = redact(&page_with_property_lists(4095, 1))
        .expect("4096 property lists across two scopes is the ceiling, not past it");
    assert_absent(&out, b"SECRET", "a page at the property-list ceiling");
    let refused = refusal(&page_with_property_lists(4096, 1), "one past the ceiling");
    assert!(
        refused.contains("[properties-too-many]"),
        "one past the ceiling must refuse by name: {refused}"
    );
}

/// A page declaring a form the region reaches, plus an **undrawn** chain of `levels` forms each/// A page declaring a form the region reaches, plus an **undrawn** chain of `levels` forms each
/// naming the next `branch` times.
///
/// Undrawn is the whole point: the geometry walk's own `MAX_FORM_DRAWS` counts forms it
/// *draws*, so a graph nothing draws is invisible to it. Only the resource-graph lookup walks
/// this, which is why the ceiling had to be its own.
fn page_with_an_undrawn_form_graph(levels: usize, branch: usize) -> Vec<u8> {
    let mut objects: Vec<String> = Vec::new();
    let mut push = |body: String| -> usize {
        objects.push(body);
        objects.len() + 20
    };
    let form = |extra: &str, data: &str| {
        format!(
            "<< /Type /XObject /Subtype /Form /BBox [0 0 400 200] {extra} /Length {} >>\n\
             stream\n{data}endstream",
            data.len()
        )
    };
    // AN EMPTY `/Resources`, NOT NONE. A form declaring none makes `scope_of` continue with the
    // enclosing dictionary — correct, and it means the descent's depth is no longer the chain's
    // length, so the graph trips `form-graph-too-deep` before the visit budget it is here to
    // test. Declaring an empty one keeps this fixture about the branching.
    let mut current = push(form("/Resources << >>", ""));
    for _ in 0..levels {
        let refs: String = (0..branch)
            .map(|at| format!("/F{at} {current} 0 R "))
            .collect();
        current = push(form(&format!("/Resources << /XObject << {refs}>> >>"), ""));
    }
    let bomb = current;
    let drawn = push(form(
        "/Resources << /Font << /Helv 6 0 R >> >>",
        "BT /Helv 20 Tf 40 100 Td (SECRET) Tj ET\n",
    ));

    let content = "/Drawn Do\n";
    let mut all = vec![
        format!(
            "<< /Type /Page /Parent 9 0 R /MediaBox [0 0 400 200] /Contents 5 0 R \
             /Resources << /Font << /Helv 6 0 R >> \
             /XObject << /Drawn {drawn} 0 R /Bomb {bomb} 0 R >> >> >>"
        ),
        // THREE FREE SLOTS, so the content stream lands on object 5 and the font on 6, which is
        // what the page dictionary above names. An off-by-one here produces `qpdf: the document
        // is damaged` from a test whose subject is not the xref table.
        String::new(),
        String::new(),
        String::new(),
        format!(
            "<< /Length {} >>\nstream\n{content}endstream",
            content.len()
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Name /Helv >>".to_owned(),
        String::new(),
        String::new(),
        "<< /Type /Pages /Kids [1 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Catalog /Pages 9 0 R >>".to_owned(),
    ];
    all.resize(20, String::new());
    all.extend(objects);

    let mut out = String::from("%PDF-1.7\n");
    let mut offsets = Vec::new();
    for (index, body) in all.iter().enumerate() {
        if body.is_empty() {
            offsets.push(None);
            continue;
        }
        offsets.push(Some(out.len()));
        out.push_str(&format!("{} 0 obj\n{body}\nendobj\n", index + 1));
    }
    let xref_at = out.len();
    out.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", all.len() + 1));
    for offset in &offsets {
        match offset {
            Some(at) => out.push_str(&format!("{at:010} 00000 n \n")),
            None => out.push_str("0000000000 65535 f \n"),
        }
    }
    out.push_str(&format!(
        "trailer\n<< /Size {} /Root 10 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
        all.len() + 1
    ));
    out.into_bytes()
}

/// A page whose form graph is a **cycle**: `/Outer` draws `/Inner`, `/Inner` draws `/Outer`.
fn page_with_a_form_cycle() -> Vec<u8> {
    let outer = 22usize;
    let inner = 23usize;
    let content = "/Drawn Do\n";
    let body = |extra: &str, data: &str| {
        format!(
            "<< /Type /XObject /Subtype /Form /BBox [0 0 400 200] {extra} /Length {} >>\n\
             stream\n{data}endstream",
            data.len()
        )
    };
    let mut all = vec![
        format!(
            "<< /Type /Page /Parent 9 0 R /MediaBox [0 0 400 200] /Contents 5 0 R \
             /Resources << /Font << /Helv 6 0 R >> \
             /XObject << /Drawn 21 0 R /Outer {outer} 0 R >> >> >>"
        ),
        String::new(),
        String::new(),
        String::new(),
        format!(
            "<< /Length {} >>\nstream\n{content}endstream",
            content.len()
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Name /Helv >>".to_owned(),
        String::new(),
        String::new(),
        "<< /Type /Pages /Kids [1 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Catalog /Pages 9 0 R >>".to_owned(),
    ];
    all.resize(20, String::new());
    all.push(body(
        "/Resources << /Font << /Helv 6 0 R >> >>",
        "BT /Helv 20 Tf 40 100 Td (SECRET) Tj ET\n",
    ));
    all.push(body(
        &format!("/Resources << /XObject << /Inner {inner} 0 R >> >>"),
        "",
    ));
    all.push(body(
        &format!("/Resources << /XObject << /Outer {outer} 0 R >> >>"),
        "",
    ));

    let mut out = String::from("%PDF-1.7\n");
    let mut offsets = Vec::new();
    for (index, item) in all.iter().enumerate() {
        if item.is_empty() {
            offsets.push(None);
            continue;
        }
        offsets.push(Some(out.len()));
        out.push_str(&format!("{} 0 obj\n{item}\nendobj\n", index + 1));
    }
    let xref_at = out.len();
    out.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", all.len() + 1));
    for offset in &offsets {
        match offset {
            Some(at) => out.push_str(&format!("{at:010} 00000 n \n")),
            None => out.push_str("0000000000 65535 f \n"),
        }
    }
    out.push_str(&format!(
        "trailer\n<< /Size {} /Root 10 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
        all.len() + 1
    ));
    out.into_bytes()
}

#[test]
fn a_form_that_draws_the_form_that_draws_it_terminates() {
    // THE CYCLE GUARD, which nothing exercised. A security review deleted `open.insert`'s
    // refusal, `open.remove`, and the depth cap from both recursions -- eight mutations -- and
    // every one survived the suite. The ceilings the module's rustdoc argues for were a defence
    // nothing failed for.
    //
    // A form that draws the form that draws it is a document that exists; without the guard
    // this does not return. The assertion is therefore that it returns AT ALL, and quickly --
    // there is no output to check, because the cycle is never drawn.
    let pdf = page_with_a_form_cycle();
    let started = std::time::Instant::now();
    let outcome = redact(&pdf);
    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "a form cycle took {elapsed:?}, so the guard is not stopping it"
    );
    // EITHER OUTCOME IS FINE AND THE POINT IS NEITHER. A cycle nothing draws may be walked
    // past or refused; what must not happen is that it runs forever. Asserting a particular
    // verdict here would pin a decision this test was not written to make.
    match outcome {
        Ok((out, _)) => assert!(out.starts_with(b"%PDF")),
        Err(error) => {
            let text = format!("{error:?}");
            assert!(text.contains('['), "refused without naming a rule: {text}");
        }
    }
}

#[test]
fn a_form_graph_that_branches_is_refused_rather_than_walked_exponentially() {
    // THE DEFECT THIS KILLS, measured by a code review on the commit that introduced it.
    //
    // The resource-graph lookup bounded depth (`MAX_FORM_DEPTH`) and guarded cycles with a set
    // of forms currently open. That pair *looks* like a bound and is not: the open set is a
    // PATH set, so a DAG is re-explored once per path and the work is `branch ^ depth`. It is
    // the same mistake `MAX_FORM_DRAWS`' own rustdoc was written to warn about, made in a
    // different function two commits later.
    //
    // Measured on this tree, release, through the public operation:
    //
    //   levels 4,  branch 3 — 1,965 bytes —     3 ms
    //   levels 8,  branch 3 — 2,697 bytes —    45 ms
    //   levels 12, branch 3 — 3,429 bytes — 2,770 ms
    //   levels 16, branch 4 — 4,337 bytes — did not return in 300 s
    //
    // Nine times per two levels, which is 3². With the budget: 3.7 ms and a named refusal.
    //
    // ONE LEVEL SHORT OF THE CEILING IS THE NEAR-MISS, and it must still redact — a rule that
    // refused any nested graph would pass this test and refuse ordinary documents, since every
    // drawing program emits nested forms.
    let shallow = page_with_an_undrawn_form_graph(3, 2);
    let (out, _) = redact(&shallow).expect("a small form graph is ordinary and must be walked");
    assert!(out.starts_with(b"%PDF"));

    // FOURTEEN LEVELS OF THREE. Twelve-by-three is 2.77 s unbudgeted, which sits under any
    // threshold generous enough not to flake — so removing the scope walk's budget still passed
    // while a second budget elsewhere produced the refusal. Fourteen-by-three is 4.8M visits and
    // does not return, so no threshold can be too generous; and it stays one level inside
    // `MAX_FORM_DEPTH`, so the refusal under test is the visit budget rather than the depth cap.
    let deep = page_with_an_undrawn_form_graph(14, 3);
    let started = std::time::Instant::now();
    let error = refusal(&deep, "a branching form graph");
    let elapsed = started.elapsed();
    assert!(
        error.contains("form-graph-too-large"),
        "refused by the wrong rule: {error}"
    );
    // THE TIME IS THE ASSERTION, not decoration: the refusal is only a fix if it arrives
    // before the work does. Generous by three orders of magnitude against the 2.77 s measured
    // without the budget, so a slow machine cannot fail it while a restored exponent cannot
    // pass it.
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "refused, but took {elapsed:?} to do it -- the budget is not bounding the descent"
    );
}

//! #344: the `/FontBBox` a font declares in its `/FontDescriptor`, and the floor envelope.
//!
//! Before #344 the walk read `/FontBBox` from the font dictionary alone. A Type 1, TrueType or CID
//! font declares it in the descriptor -- for a composite font, the descendant's -- so every such
//! glyph was boxed by its advance alone, from the baseline up one em, and a descender from the line
//! above reached a region its box did not. The census on 3b46b21 measured it: 137 of 3,236 `Ok`
//! tiles with a character PDFium inks inside the region, all on embedded Type 1 or CID fonts that
//! carry a descriptor box.
//!
//! The rule (ADR 0029 §3, amended for #344): a glyph's box is its advance box, unioned with every
//! four-number box its font declares, unioned with the floor envelope as `ENVELOPE_RULE` says.
//! Boxes are unioned, never chosen between. A box of more than four items, or one holding a
//! non-number, refuses `[number-unreadable]`.
//!
//! # What each test measures
//!
//! The page is 400 points square and the region is x 100..300, y 250..300 in PDF space. Line 1 draws
//! one `g` at 40 points above the region; an anchor `A` in a second font sits far away at (20, 20),
//! so the output always walks a glyph and the second reading's R2 is never skipped. **The witness
//! for a removal is PDFium**: line 1's character is in PDFium's text of the input and absent from
//! the output, and the anchor is in both. Each removal has a near-miss twin -- the same document
//! with the declared box, or the baseline, moved just short of the region -- where the glyph is
//! kept, so the removal is the box's and not the document's.
//!
//! Most fixtures declare boxes no real font has (0.6 em below the baseline), so that the declared
//! box alone reaches the region and the envelope does not: the test then measures the READ, under
//! either envelope rule. The ink tests (`*_ink_*`) use a real descender and count PDFium's dark
//! pixels instead.
//!
//! A `/FontFile*` here is a stream PDFium does not load, so it draws with a substitute: the file
//! still embeds a program as far as burrow's resolver is concerned, which is the path under test.

#![cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test code: a failure here is a broken fixture or a real finding, and it should be \
              loud; the one index reads an AFM line just asserted to hold four numbers"
)]

mod support;

use std::collections::BTreeSet;
use std::fmt::Write as _;

use burrow_engines::pdfsyntax::geometry::{ENVELOPE_RULE, EnvelopeRule, FLOOR_ENVELOPE};
use burrow_engines::pdfsyntax::region::Region;
use support::char_box_oracle::{chars_on_page, dark_pixels_with_annotations};

/// The region, in display space: x 100..300, and y 250..300 in PDF space on a 400-point page.
const REGION: Region = Region {
    left: 100.0,
    top: 100.0,
    width: 200.0,
    height: 50.0,
};

/// The same rectangle for the pixel count, at one pixel a point.
const PIXELS: (i32, i32, i32, i32) = (100, 100, 200, 50);

/// Line 1's x, inside the region's span.
const X: f64 = 150.0;

/// A baseline 16 points above the region: a box reaching 0.6 em (24 pt) below it reaches the
/// region, one reaching 0.38 em (15.2 pt) stops 0.8 pt short, and the envelope's 0.293 em
/// (11.72 pt) stops 4.28 pt short. So only a DECLARED deep box reaches from here.
const DEEP: f64 = 316.0;

/// The declared box that reaches from [`DEEP`]: 0.6 em below the baseline.
const REACHES: &str = "0 -600 1000 900";

/// Its near-miss twin: 0.38 em below, 0.8 pt short of the region.
const SHORT: &str = "0 -380 1000 900";

/// A baseline the envelope reaches from (0.293 em of 40 pt is 11.72: down to 298.28) ...
const ENVELOPE_REACHES: f64 = 310.0;

/// ... and its twin, where the envelope stops 0.28 pt short.
const ENVELOPE_SHORT: f64 = 312.0;

/// A baseline from which Helvetica's `g` INKS the region: its descender is about 0.21 em, so at 40
/// points it reaches about 297.6. The advance box stops at 306.
const INKS: f64 = 306.0;

/// Where the anchor glyph is, in the second font.
const ANCHOR: (f64, f64) = (20.0, 20.0);

/// The size line 1 is drawn at.
const SIZE: u32 = 40;

// ---- documents ------------------------------------------------------------------------------

/// A document from its objects: 1 catalog, 2 pages, 3 page, 4 content, 5 `/F1`, 6 `/F2` (the
/// anchor's font), then `objects` from 7. `resources` is added to the page's `/Resources`.
fn document(content: &str, f1: &[u8], resources: &str, objects: &[Vec<u8>]) -> Vec<u8> {
    let content = format!(
        "{content}BT /F2 12 Tf {} {} Td (A) Tj ET\n",
        ANCHOR.0, ANCHOR.1
    );
    let mut all: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] \
             /Resources << /Font << /F1 5 0 R /F2 6 0 R >> {resources} >> /Contents 4 0 R >>"
        )
        .into_bytes(),
        stream("", content.as_bytes()),
        f1.to_vec(),
        simple_font("Type1", "").into_bytes(),
    ];
    all.extend(objects.iter().cloned());
    assemble(&all)
}

/// The file, objects numbered from 1, the catalogue first.
fn assemble(objects: &[Vec<u8>]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref_at = out.len();
    let mut tail = format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1);
    for offset in &offsets {
        let _ = writeln!(tail, "{offset:010} 00000 n ");
    }
    let _ = write!(
        tail,
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
        objects.len() + 1
    );
    out.extend_from_slice(tail.as_bytes());
    out
}

fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    [
        format!("<< {dict} /Length {} >>\nstream\n", data.len()).as_bytes(),
        data,
        b"\nendstream",
    ]
    .concat()
}

/// Codes 65 (`A`) to 103 (`g`), each 556 thousandths wide.
fn widths() -> String {
    format!("[{}]", vec!["556"; 39].join(" "))
}

/// A simple font of `subtype`, declaring its widths, with `keys` added.
fn simple_font(subtype: &str, keys: &str) -> String {
    format!(
        "<< /Type /Font /Subtype /{subtype} /BaseFont /Helvetica /Encoding /WinAnsiEncoding \
         /FirstChar 65 /LastChar 103 /Widths {} {keys} >>",
        widths()
    )
}

/// A font descriptor whose `/FontBBox` entry is `bbox` (the whole `/FontBBox …` text, or empty).
fn descriptor(bbox: &str, extra: &str) -> Vec<u8> {
    format!(
        "<< /Type /FontDescriptor /FontName /Helvetica /Flags 32 {bbox} /ItalicAngle 0 \
         /Ascent 931 /Descent -225 /CapHeight 718 /StemV 88 {extra} >>"
    )
    .into_bytes()
}

/// Line 1: one `g` at `(X, y)`.
fn line_one(y: f64) -> String {
    format!("BT /F1 {SIZE} Tf {X} {y} Td (g) Tj ET\n")
}

/// A simple font of `subtype` at baseline `y` whose descriptor (object 7) declares `bbox`, with
/// a program under `program` (object 8) when one is named.
fn simple(subtype: &str, y: f64, bbox: &str, program: Option<&str>) -> Vec<u8> {
    let mut objects = Vec::new();
    let extra = match program {
        Some(key) => format!("/{key} 8 0 R"),
        None => String::new(),
    };
    objects.push(descriptor(&format!("/FontBBox [{bbox}]"), &extra));
    if let Some(key) = program {
        let dict = if key == "FontFile3" {
            "/Subtype /Type1C"
        } else {
            ""
        };
        objects.push(stream(dict, b"not a font program"));
    }
    document(
        &line_one(y),
        simple_font(subtype, "/FontDescriptor 7 0 R").as_bytes(),
        "",
        &objects,
    )
}

/// A Type 0 font over a descendant of `cid_subtype`, at baseline `y`. `type0_keys` go on the
/// Type 0 dictionary, `descendant_keys` on the descendant (object 7), whose descriptor is
/// object 8 and declares `descendant_bbox` (the whole entry, or empty).
fn composite(
    cid_subtype: &str,
    y: f64,
    type0_keys: &str,
    descendant_keys: &str,
    descendant_bbox: &str,
    objects: &[Vec<u8>],
) -> Vec<u8> {
    let gid_map = if cid_subtype == "CIDFontType2" {
        "/CIDToGIDMap /Identity"
    } else {
        ""
    };
    let mut all = vec![
        format!(
            "<< /Type /Font /Subtype /{cid_subtype} /BaseFont /Helvetica \
             /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> \
             /FontDescriptor 8 0 R {gid_map} {descendant_keys} >>"
        )
        .into_bytes(),
        descriptor(descendant_bbox, ""),
    ];
    all.extend(objects.iter().cloned());
    document(
        &format!("BT /F1 {SIZE} Tf {X} {y} Td <0067> Tj ET\n"),
        format!(
            "<< /Type /Font /Subtype /Type0 /BaseFont /Helvetica /Encoding /Identity-H \
             /DescendantFonts [7 0 R] {type0_keys} >>"
        )
        .as_bytes(),
        "",
        &all,
    )
}

/// A Type 3 font at baseline `y`: `/FontMatrix` 0.001, a dictionary `/FontBBox` of `dict_bbox`
/// (the whole entry, or empty), `keys` added, and every glyph drawn by `procedure` (object 7).
fn type_three(
    y: f64,
    dict_bbox: &str,
    keys: &str,
    procedure: &[u8],
    objects: &[Vec<u8>],
) -> Vec<u8> {
    type_three_with(
        &line_one(y),
        dict_bbox,
        "[0.001 0 0 0.001 0 0]",
        "",
        keys,
        procedure,
        objects,
    )
}

fn type_three_with(
    content: &str,
    dict_bbox: &str,
    matrix: &str,
    widths_override: &str,
    keys: &str,
    procedure: &[u8],
    objects: &[Vec<u8>],
) -> Vec<u8> {
    let widths = if widths_override.is_empty() {
        widths()
    } else {
        widths_override.to_owned()
    };
    let mut all = vec![stream("", procedure)];
    all.extend(objects.iter().cloned());
    document(
        content,
        format!(
            "<< /Type /Font /Subtype /Type3 {dict_bbox} /FontMatrix {matrix} \
             /CharProcs << /A 7 0 R /g 7 0 R >> \
             /Encoding << /Type /Encoding /Differences [65 /A 103 /g] >> \
             /FirstChar 65 /LastChar 103 /Widths {widths} /Resources << >> {keys} >>"
        )
        .as_bytes(),
        "",
        &all,
    )
}

// ---- outcomes -------------------------------------------------------------------------------

fn redact(pdf: &[u8]) -> burrow_types::Result<(Vec<u8>, burrow_engines::redact::Report)> {
    let covered: BTreeSet<usize> = [0].into_iter().collect();
    support::redact_page(pdf, 0, covered, REGION)
}

/// Whether PDFium draws a character from the file with its origin at `at`.
fn drawn_at(pdf: &[u8], at: (f64, f64)) -> bool {
    chars_on_page(pdf, 0)
        .iter()
        .any(|c| !c.generated && (c.origin.0 - at.0).abs() < 0.5 && (c.origin.1 - at.1).abs() < 0.5)
}

/// Every origin PDFium reads on page 0, for a failure message.
fn origins(pdf: &[u8]) -> Vec<(f64, f64)> {
    chars_on_page(pdf, 0).iter().map(|c| c.origin).collect()
}

/// Line 1's glyph at `at` is REMOVED: PDFium draws it in the input and not in the output, and the
/// anchor in both.
#[track_caller]
fn removed(what: &str, pdf: &[u8], at: (f64, f64)) -> Vec<u8> {
    assert!(
        drawn_at(pdf, at),
        "{what}: the input must draw line 1's glyph at {at:?}; PDFium's origins: {:?}",
        origins(pdf)
    );
    assert!(
        drawn_at(pdf, ANCHOR),
        "{what}: the input must draw the anchor"
    );
    let (out, _) = redact(pdf).unwrap_or_else(|error| panic!("{what}: {error}"));
    assert!(
        !drawn_at(&out, at),
        "{what}: line 1's glyph is still drawn -- its declared box reaches the region and the \
         redaction did not read it"
    );
    assert!(
        drawn_at(&out, ANCHOR),
        "{what}: the anchor must survive, or the output walks nothing and R2 is skipped"
    );
    out
}

/// Whether burrow's own walk places a glyph with its origin at `at` on page 0.
fn walked_at(pdf: &[u8], at: (f64, f64)) -> bool {
    burrow_engines::glyphs_on_first_page(pdf, &support::walk_options())
        .expect("the document walks")
        .iter()
        .any(|g| (g.origin.0 - at.0).abs() < 0.5 && (g.origin.1 - at.1).abs() < 0.5)
}

/// As [`removed`], for a Type 3 glyph whose procedure draws nothing. PDFium reports no character
/// for one (measured: the anchor alone), so the witness is weaker and says so: the output's
/// content stream no longer shows the glyph, read by burrow's walk, which takes only its codes and
/// positions from it -- not the box under test.
#[track_caller]
fn removed_type3(what: &str, pdf: &[u8], at: (f64, f64)) {
    assert!(
        walked_at(pdf, at),
        "{what}: the input must show line 1's glyph"
    );
    let (out, _) = redact(pdf).unwrap_or_else(|error| panic!("{what}: {error}"));
    assert!(
        !walked_at(&out, at),
        "{what}: line 1's glyph is still shown"
    );
    assert!(drawn_at(&out, ANCHOR), "{what}: the anchor must survive");
}

/// As [`kept`], for a Type 3 glyph whose procedure draws nothing.
#[track_caller]
fn kept_type3(what: &str, pdf: &[u8], at: (f64, f64)) {
    assert!(
        walked_at(pdf, at),
        "{what}: the input must show line 1's glyph"
    );
    let (out, _) = redact(pdf).unwrap_or_else(|error| panic!("{what}: {error}"));
    assert!(walked_at(&out, at), "{what}: line 1's glyph was removed");
}

/// THE TWIN: line 1's glyph at `at` is KEPT, and the redaction succeeds.
#[track_caller]
fn kept(what: &str, pdf: &[u8], at: (f64, f64)) {
    assert!(
        drawn_at(pdf, at),
        "{what}: the input must draw line 1's glyph at {at:?}; PDFium's origins: {:?}",
        origins(pdf)
    );
    let (out, _) = redact(pdf).unwrap_or_else(|error| panic!("{what}: {error}"));
    assert!(
        drawn_at(&out, at),
        "{what}: line 1's glyph was removed, though nothing it declares reaches the region"
    );
}

/// The redaction refuses under `rule`.
#[track_caller]
fn refuses(what: &str, pdf: &[u8], rule: &str) {
    match redact(pdf) {
        Ok((out, _)) => panic!("{what}: Ok ({} bytes), expected [{rule}]", out.len()),
        Err(error) => {
            let message = error.to_string();
            assert!(message.contains(&format!("[{rule}]")), "{what}: {message}");
        }
    }
}

fn dark_in_region(pdf: &[u8]) -> u64 {
    dark_pixels_with_annotations(pdf, 0, PIXELS).0
}

// ---- the envelope itself --------------------------------------------------------------------

#[test]
fn the_envelope_is_the_union_of_the_core_afm_boxes() {
    // DERIVED, NOT TYPED: the constant is checked against the fourteen AFM files ADR 0030 commits,
    // so it cannot drift from them. Counted: fourteen files, or the union is of something else.
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/adobe-core14-afm");
    let mut union = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    let mut read = 0;
    for entry in std::fs::read_dir(&directory).expect("the AFM directory") {
        let path = entry.expect("an entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("afm") {
            continue;
        }
        let text = std::fs::read(&path).expect("an AFM file");
        let text = String::from_utf8_lossy(&text);
        let line = text
            .lines()
            .find(|line| line.starts_with("FontBBox "))
            .unwrap_or_else(|| panic!("{}: no FontBBox line", path.display()));
        let numbers: Vec<f64> = line
            .split_whitespace()
            .skip(1)
            .map(|n| n.parse().expect("a number"))
            .collect();
        assert_eq!(numbers.len(), 4, "{line}");
        union = (
            union.0.min(numbers[0]),
            union.1.min(numbers[1]),
            union.2.max(numbers[2]),
            union.3.max(numbers[3]),
        );
        read += 1;
    }
    assert_eq!(read, 14, "the fourteen Core AFM files, and no other count");
    let ems = (
        union.0 / 1000.0,
        union.1 / 1000.0,
        union.2 / 1000.0,
        union.3 / 1000.0,
    );
    assert_eq!(
        (
            FLOOR_ENVELOPE.left,
            FLOOR_ENVELOPE.bottom,
            FLOOR_ENVELOPE.right,
            FLOOR_ENVELOPE.top
        ),
        ems,
        "FLOOR_ENVELOPE is not the union of the committed AFM boxes"
    );
}

#[test]
fn the_rule_in_force_is_the_owners_choice_and_both_rules_say_what_they_say() {
    // THE SWITCH, pinned both ways: the census measures each rule by flipping `ENVELOPE_RULE`, and
    // a flip that left this green would mean the switch is not the rule's only input.
    assert!(EnvelopeRule::Always.applies(true));
    assert!(EnvelopeRule::Always.applies(false));
    assert!(!EnvelopeRule::WhenNoUsableBox.applies(true));
    assert!(EnvelopeRule::WhenNoUsableBox.applies(false));
    assert_eq!(
        ENVELOPE_RULE,
        EnvelopeRule::Always,
        "owner, 2026-10-18: Always (variant (b)); a census of the other rule flips this \
         deliberately"
    );
}

// ---- the descriptor's box, per font type ----------------------------------------------------

#[test]
fn a_type1_descriptor_box_is_read() {
    // F-1: an embedded Type 1 (`/FontFile`).
    removed(
        "Type1 /FontFile",
        &simple("Type1", DEEP, REACHES, Some("FontFile")),
        (X, DEEP),
    );
    kept(
        "Type1 /FontFile, twin",
        &simple("Type1", DEEP, SHORT, Some("FontFile")),
        (X, DEEP),
    );
    // And a Type1C program (`/FontFile3`), the other way a Type 1 font embeds.
    removed(
        "Type1 /FontFile3",
        &simple("Type1", DEEP, REACHES, Some("FontFile3")),
        (X, DEEP),
    );
}

#[test]
fn a_truetype_descriptor_box_is_read_embedded_or_not() {
    // F-2: an embedded TrueType (`/FontFile2`); F-2b: one with no program, declaring `/Widths`.
    removed(
        "TrueType /FontFile2",
        &simple("TrueType", DEEP, REACHES, Some("FontFile2")),
        (X, DEEP),
    );
    kept(
        "TrueType /FontFile2, twin",
        &simple("TrueType", DEEP, SHORT, Some("FontFile2")),
        (X, DEEP),
    );
    removed(
        "TrueType, not embedded",
        &simple("TrueType", DEEP, REACHES, None),
        (X, DEEP),
    );
    kept(
        "TrueType, not embedded, twin",
        &simple("TrueType", DEEP, SHORT, None),
        (X, DEEP),
    );
}

#[test]
fn a_composite_fonts_box_is_read_from_its_descendants_descriptor() {
    // F-3 and F-3b: the box on the DESCENDANT's descriptor only, under both CID subtypes.
    for subtype in ["CIDFontType2", "CIDFontType0"] {
        let reaches = format!("/FontBBox [{REACHES}]");
        let short = format!("/FontBBox [{SHORT}]");
        removed(
            subtype,
            &composite(subtype, DEEP, "", "/DW 556", &reaches, &[]),
            (X, DEEP),
        );
        kept(
            &format!("{subtype}, twin"),
            &composite(subtype, DEEP, "", "/DW 556", &short, &[]),
            (X, DEEP),
        );
    }
}

#[test]
fn an_undersized_descendant_box_is_covered_by_the_envelope_under_the_always_rule() {
    // F-11 on a composite font: the descendant's descriptor declares a usable box that stops at the
    // baseline, so only the envelope reaches the region from here. Every other composite fixture
    // declares a box that reaches on its own, so none of them can see a composite glyph losing the
    // envelope. The twin sits where the envelope stops short.
    let undersized = "/FontBBox [0 0 1000 700]";
    for subtype in ["CIDFontType2", "CIDFontType0"] {
        match ENVELOPE_RULE {
            EnvelopeRule::Always => {
                removed(
                    &format!("{subtype}, an undersized descendant box"),
                    &composite(subtype, ENVELOPE_REACHES, "", "/DW 556", undersized, &[]),
                    (X, ENVELOPE_REACHES),
                );
            }
            EnvelopeRule::WhenNoUsableBox => {
                kept(
                    &format!("{subtype}, an undersized descendant box, narrower rule"),
                    &composite(subtype, ENVELOPE_REACHES, "", "/DW 556", undersized, &[]),
                    (X, ENVELOPE_REACHES),
                );
            }
        }
        kept(
            &format!("{subtype}, an undersized descendant box, twin"),
            &composite(subtype, ENVELOPE_SHORT, "", "/DW 556", undersized, &[]),
            (X, ENVELOPE_SHORT),
        );
    }
}

#[test]
fn a_composite_fonts_box_counts_whatever_the_glyphs_width() {
    // `/DW` alone, no `/W`; and a `/W` giving this glyph no width at all, whose advance box is then
    // a line: the declared box is all that gives it extent.
    let reaches = format!("/FontBBox [{REACHES}]");
    let short = format!("/FontBBox [{SHORT}]");
    for (what, widths) in [("/DW only", "/DW 1000"), ("/W zero", "/W [103 [0]]")] {
        removed(
            what,
            &composite("CIDFontType2", DEEP, "", widths, &reaches, &[]),
            (X, DEEP),
        );
        kept(
            &format!("{what}, twin"),
            &composite("CIDFontType2", DEEP, "", widths, &short, &[]),
            (X, DEEP),
        );
    }
}

#[test]
fn a_type0_parents_descriptor_box_is_unioned_and_the_descendants_still_counts() {
    // F-4: the box on the Type 0 dictionary's own descriptor, which is non-standard: unioned,
    // harmless. Its twin carries both, and only the DESCENDANT's reaches -- so a reader that took
    // the parent's in place of the descendant's keeps the glyph.
    let objects = [descriptor(&format!("/FontBBox [{REACHES}]"), "")];
    removed(
        "the Type 0 parent's descriptor",
        &composite(
            "CIDFontType2",
            DEEP,
            "/FontDescriptor 9 0 R",
            "/DW 556",
            "",
            &objects,
        ),
        (X, DEEP),
    );
    let parent_short = [descriptor(&format!("/FontBBox [{SHORT}]"), "")];
    removed(
        "both descriptors, only the descendant's reaching",
        &composite(
            "CIDFontType2",
            DEEP,
            "/FontDescriptor 9 0 R",
            "/DW 556",
            &format!("/FontBBox [{REACHES}]"),
            &parent_short,
        ),
        (X, DEEP),
    );
    kept(
        "both descriptors, neither reaching",
        &composite(
            "CIDFontType2",
            DEEP,
            "/FontDescriptor 9 0 R",
            "/DW 556",
            &format!("/FontBBox [{SHORT}]"),
            &parent_short,
        ),
        (X, DEEP),
    );
}

/// A Type 3 glyph procedure that draws nothing.
const NOTHING: &[u8] = b"556 0 d0\n";

#[test]
fn a_type3_descriptor_box_is_unioned_with_its_dictionary_box() {
    // F-5: a Type 3 font's descriptor box reaching below its dictionary box removes the glyph.
    let descriptor_reaches = [descriptor(&format!("/FontBBox [{REACHES}]"), "")];
    let descriptor_short = [descriptor(&format!("/FontBBox [{SHORT}]"), "")];
    removed_type3(
        "Type 3, the descriptor's box deeper",
        &type_three(
            DEEP,
            "/FontBBox [0 0 556 900]",
            "/FontDescriptor 8 0 R",
            NOTHING,
            &descriptor_reaches,
        ),
        (X, DEEP),
    );
    kept_type3(
        "Type 3, neither reaching",
        &type_three(
            DEEP,
            "/FontBBox [0 0 556 900]",
            "/FontDescriptor 8 0 R",
            NOTHING,
            &descriptor_short,
        ),
        (X, DEEP),
    );
    // F-5b: and the other way round. A reader that CHOSE the descriptor's box over the dictionary's
    // keeps this glyph.
    let descriptor_small = [descriptor("/FontBBox [0 0 556 900]", "")];
    removed_type3(
        "Type 3, the dictionary's box deeper",
        &type_three(
            DEEP,
            &format!("/FontBBox [{REACHES}]"),
            "/FontDescriptor 8 0 R",
            NOTHING,
            &descriptor_small,
        ),
        (X, DEEP),
    );
}

// ---- the box's own state --------------------------------------------------------------------

#[test]
fn a_font_with_no_box_gets_the_envelope() {
    // F-6, F-6b: no descriptor, and a descriptor without `/FontBBox`. Under either rule the
    // envelope applies -- there is no usable box. (F-6c, `/FontBBox null`, is refused before the
    // box is read: see `a_null_box_is_refused_by_the_null_value_rule`.)
    let fonts: [(&str, String, Vec<Vec<u8>>); 2] = [
        ("no descriptor", simple_font("Type1", ""), vec![]),
        (
            "a descriptor without /FontBBox",
            simple_font("Type1", "/FontDescriptor 7 0 R"),
            vec![descriptor("", "")],
        ),
    ];
    for (what, font, objects) in &fonts {
        removed(
            what,
            &document(&line_one(ENVELOPE_REACHES), font.as_bytes(), "", objects),
            (X, ENVELOPE_REACHES),
        );
        kept(
            &format!("{what}, twin"),
            &document(&line_one(ENVELOPE_SHORT), font.as_bytes(), "", objects),
            (X, ENVELOPE_SHORT),
        );
    }
}

#[test]
fn a_null_box_is_refused_by_the_null_value_rule() {
    // F-6c: `/FontBBox null` would read as absent, but the document-wide `[null-value]` rule
    // refuses it first (a value the engine drops when it writes). Pinned so that a change to
    // either rule is seen here.
    refuses(
        "/FontBBox null",
        &document(
            &line_one(ENVELOPE_REACHES),
            simple_font("Type1", "/FontDescriptor 7 0 R").as_bytes(),
            "",
            &[descriptor("/FontBBox null", "")],
        ),
        "null-value",
    );
}

#[test]
fn a_descender_with_no_declared_box_is_inked_in_the_region_and_removed() {
    // THE ISSUE'S SHAPE, by ink: Helvetica's `g` at 40 points with its baseline 6 points above the
    // region inks inside it, and its advance box does not reach. On `main` this was `Ok` with the
    // ink left. Under either rule the envelope removes it.
    let pdf = document(
        &line_one(INKS),
        simple_font("Type1", "").as_bytes(),
        "",
        &[],
    );
    assert!(dark_in_region(&pdf) > 0, "the input must ink the region");
    let out = removed("a descender, no declared box", &pdf, (X, INKS));
    assert_eq!(dark_in_region(&out), 0, "ink left in the region");
}

#[test]
fn an_undersized_declared_box_is_covered_by_the_envelope_only_under_the_always_rule() {
    // F-11: a usable box that stops at the baseline, on a font whose `g` inks below it. Under the
    // rule in force (`Always`, owner 2026-10-18) the envelope covers it; under the narrower rule
    // the declared box is trusted and the ink stays -- the residue that rule accepts, measured
    // here rather than described.
    let pdf = simple("Type1", INKS, "0 0 1000 700", None);
    assert!(dark_in_region(&pdf) > 0, "the input must ink the region");
    match ENVELOPE_RULE {
        EnvelopeRule::Always => {
            let out = removed("an undersized box, Always", &pdf, (X, INKS));
            assert_eq!(dark_in_region(&out), 0, "ink left in the region");
        }
        EnvelopeRule::WhenNoUsableBox => {
            let (out, _) = redact(&pdf).expect("the narrower rule redacts");
            assert!(
                dark_in_region(&out) > 0,
                "WhenNoUsableBox: the residue this rule accepts is no longer there -- re-measure"
            );
        }
    }
}

#[test]
fn a_box_of_three_numbers_falls_back_to_the_envelope() {
    // F-7: not four numbers, so not a box: the envelope applies, under either rule. No refusal.
    let objects = [descriptor("/FontBBox [0 -600 1000]", "")];
    let font = simple_font("Type1", "/FontDescriptor 7 0 R");
    removed(
        "three numbers",
        &document(&line_one(ENVELOPE_REACHES), font.as_bytes(), "", &objects),
        (X, ENVELOPE_REACHES),
    );
    kept(
        "three numbers, twin",
        &document(&line_one(ENVELOPE_SHORT), font.as_bytes(), "", &objects),
        (X, ENVELOPE_SHORT),
    );
}

#[test]
fn a_box_of_more_than_four_items_refuses_at_every_site() {
    // F-7b, the owner's refusal of 2026-10-17. Its near-miss twin is EXACTLY FOUR NUMERIC items,
    // admitted and used: `a_type1_descriptor_box_is_read` and its siblings. Five numbers, and five
    // with a name in them. The all-number case is the one that separates the too-many-items
    // refusal from the non-number refusal: a name refuses under either, five numbers only under
    // the count. It says nothing about the order the two checks run in.
    for items in [format!("{REACHES} 0"), format!("{REACHES} /Extra")] {
        refuses(
            "descriptor",
            &simple("Type1", DEEP, &items, None),
            "number-unreadable",
        );
        refuses(
            "descendant's descriptor",
            &composite(
                "CIDFontType2",
                DEEP,
                "",
                "/DW 556",
                &format!("/FontBBox [{items}]"),
                &[],
            ),
            "number-unreadable",
        );
        refuses(
            "Type 3 dictionary",
            &type_three(DEEP, &format!("/FontBBox [{items}]"), "", NOTHING, &[]),
            "number-unreadable",
        );
    }
}

#[test]
fn a_fast_refusal_on_a_shared_huge_box() {
    // THE COST FIXTURE: one 65,536-item `/FontBBox`, the most `numbers_strict` would read, in one
    // descriptor shared by 400 fonts that the page all draws with. Under the old fall-back, which
    // read a box that was not four numbers in full and then used none, that was a 65,536-item read
    // for every font. The refusal removes that cost: the first font to read the box refuses the
    // document. What this pins is the refusal; checking the count before reading any item is
    // cheap, but nothing here tells it apart from one full read followed by the refusal.
    let fonts = 400;
    let mut content = String::new();
    let mut names = String::new();
    let mut objects = Vec::new();
    let big = format!("[{}]", vec!["0"; 65_536].join(" "));
    objects.push(descriptor(&format!("/FontBBox {}", "8 0 R"), "")); // 7
    objects.push(big.into_bytes()); // 8
    for at in 0..fonts {
        let number = 9 + at;
        let _ = write!(names, "/G{at} {number} 0 R ");
        let _ = writeln!(content, "BT /G{at} 10 Tf 350 350 Td (A) Tj ET");
        objects.push(simple_font("Type1", "/FontDescriptor 7 0 R").into_bytes());
    }
    let pdf = {
        let content = format!(
            "{content}BT /F2 12 Tf {} {} Td (A) Tj ET\n",
            ANCHOR.0, ANCHOR.1
        );
        let mut all: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] \
                 /Resources << /Font << /F2 6 0 R {names}>> >> /Contents 4 0 R >>"
            )
            .into_bytes(),
            stream("", content.as_bytes()),
            simple_font("Type1", "").into_bytes(),
            simple_font("Type1", "").into_bytes(),
        ];
        all.extend(objects);
        assemble(&all)
    };
    let started = std::time::Instant::now();
    refuses("a 65,536-item box", &pdf, "number-unreadable");
    let took = started.elapsed();
    // GENEROUS, because a busy machine is not a defect. It is not a measure of the read order.
    assert!(
        took < std::time::Duration::from_secs(10),
        "the refusal took {took:?}"
    );
}

#[test]
fn a_box_holding_a_non_number_refuses_at_the_new_sites() {
    // F-8 and F-8c, ADR 0029 row 133 extended to the descriptor and the descendant's descriptor: a
    // string item, and a nested array (#241's shape). Four items, so the count does not decide.
    for items in ["0 -600 1000 (900)", "0 -600 1000 [900]"] {
        refuses(
            items,
            &simple("Type1", DEEP, items, None),
            "number-unreadable",
        );
        refuses(
            items,
            &composite(
                "CIDFontType2",
                DEEP,
                "",
                "/DW 556",
                &format!("/FontBBox [{items}]"),
                &[],
            ),
            "number-unreadable",
        );
    }
}

#[test]
fn an_indirect_box_and_indirect_items_are_read_through() {
    // F-9: as PDFium reads them, on the descriptor and on the descendant's descriptor.
    let objects = [
        descriptor("/FontBBox 8 0 R", ""),
        format!("[{REACHES}]").into_bytes(),
    ];
    let font = simple_font("Type1", "/FontDescriptor 7 0 R");
    removed(
        "an indirect array",
        &document(&line_one(DEEP), font.as_bytes(), "", &objects),
        (X, DEEP),
    );
    let objects = [
        descriptor("/FontBBox [0 8 0 R 1000 900]", ""),
        b"-600".to_vec(),
    ];
    removed(
        "an indirect item",
        &document(&line_one(DEEP), font.as_bytes(), "", &objects),
        (X, DEEP),
    );
    let objects = [format!("[{REACHES}]").into_bytes()];
    removed(
        "an indirect array on the descendant's descriptor",
        &composite(
            "CIDFontType2",
            DEEP,
            "",
            "/DW 556",
            "/FontBBox 9 0 R",
            &objects,
        ),
        (X, DEEP),
    );
}

#[test]
fn an_inverted_box_is_normalised_and_used() {
    // F-9b: the same set of points, so normalising loses nothing.
    removed(
        "inverted",
        &simple("Type1", DEEP, "1000 900 0 -600", None),
        (X, DEEP),
    );
}

#[test]
fn a_zero_area_box_is_still_unioned() {
    // F-17 (and F-9c, F-9d): a box with no width, and one with no height, each reaching the
    // region. Zero area does not make a box absent: its line reaches where it reaches. On a simple
    // font's descriptor, and on a Type 3 dictionary, where `main` already unioned it.
    removed(
        "zero width",
        &simple("Type1", DEEP, "0 -600 0 900", None),
        (X, DEEP),
    );
    removed(
        "zero height",
        &simple("Type1", DEEP, "0 -600 1000 -600", None),
        (X, DEEP),
    );
    kept(
        "zero width, twin",
        &simple("Type1", DEEP, "0 -380 0 900", None),
        (X, DEEP),
    );
    removed_type3(
        "Type 3 dictionary, zero width",
        &type_three(DEEP, "/FontBBox [0 -1000 0 1000]", "", NOTHING, &[]),
        (X, DEEP),
    );
}

#[test]
fn a_box_at_the_readers_range_is_used_and_one_past_it_refuses() {
    // F-12: 2^24 - 1, the largest a reader holds exactly, is used -- over-removal, the safe
    // direction: the glyph goes. One past, at the new descriptor site, refuses.
    let huge = "-16777215 -16777215 16777215 16777215";
    removed("2^24 - 1", &simple("Type1", DEEP, huge, None), (X, DEEP));
    refuses(
        "2^24 + 1",
        &simple("Type1", DEEP, "0 -16777217 1000 900", None),
        "number-unreadable",
    );
}

#[test]
fn a_huge_box_under_a_huge_matrix_refuses_rather_than_covering_everything() {
    // F-13: a box finite in the file that composes to a non-finite one on the page refuses; it is
    // never an infinite box returning `Ok`. The CTM is 2^24 composed 42 times, about 2.7e303:
    // finite, and finite through the font matrix and size (the glyph transform's own check passes),
    // so it is the descriptor box's 2^24 corners that overflow.
    let huge = "-16777215 -16777215 16777215 16777215";
    let objects = [descriptor(&format!("/FontBBox [{huge}]"), "")];
    let content = format!(
        "q {}BT /F1 {SIZE} Tf 0 0 Td (g) Tj ET Q\n",
        "16777216 0 0 16777216 0 0 cm ".repeat(42)
    );
    let pdf = document(
        &content,
        simple_font("Type1", "/FontDescriptor 7 0 R").as_bytes(),
        "",
        &objects,
    );
    refuses(
        "a box composing to a non-finite one",
        &pdf,
        "non-finite-geometry",
    );
    // The twin: the same matrix over a box of one em is finite, so the refusal is the box's.
    let objects = [descriptor("/FontBBox [0 0 1 1]", "")];
    let pdf = document(
        &content,
        simple_font("Type1", "/FontDescriptor 7 0 R").as_bytes(),
        "",
        &objects,
    );
    if let Err(error) = redact(&pdf) {
        assert!(
            !error.to_string().contains("[non-finite-geometry]"),
            "the twin refused for the box: {error}"
        );
    }
}

// ---- composition ----------------------------------------------------------------------------

#[test]
fn a_rotated_text_matrix_rotates_the_declared_box() {
    // F-14: `0 -1 1 0` turns glyph space a quarter clockwise, so the box's 0.6 em below the
    // baseline lies 24 points to the LEFT of the origin, which sits 16 points right of the region.
    let at = (316.0, 275.0);
    let pdf = |bbox: &str| {
        let objects = [descriptor(&format!("/FontBBox [{bbox}]"), "")];
        document(
            &format!("BT /F1 {SIZE} Tf 0 -1 1 0 {} {} Tm (g) Tj ET\n", at.0, at.1),
            simple_font("Type1", "/FontDescriptor 7 0 R").as_bytes(),
            "",
            &objects,
        )
    };
    removed("rotated", &pdf(REACHES), at);
    kept("rotated, twin", &pdf(SHORT), at);
}

#[test]
fn horizontal_scaling_scales_the_declared_box_once() {
    // F-14b: `Tz 50` halves the box's width. A box two ems wide reaches 40 points right of the
    // origin at 40 points under `Tz 50`; the twin's origin is 41 points left of the region. A box
    // scaled through text space only -- without the font matrix and size -- reaches from both.
    let pdf = |x: f64| {
        let objects = [descriptor("/FontBBox [0 -200 2000 900]", "")];
        document(
            &format!("BT /F1 {SIZE} Tf 50 Tz {x} 260 Td (g) Tj ET\n"),
            simple_font("Type1", "/FontDescriptor 7 0 R").as_bytes(),
            "",
            &objects,
        )
    };
    removed("Tz 50", &pdf(70.0), (70.0, 260.0));
    kept("Tz 50, twin", &pdf(59.0), (59.0, 260.0));
}

#[test]
fn horizontal_scaling_leaves_the_envelopes_height_alone() {
    // L-a: `Tz` scales the envelope's width, never its height. A font with no declared box, so the
    // envelope is all that reaches below the baseline: under `Tz 50` it still reaches 0.293 em
    // down from [`ENVELOPE_REACHES`], and stops short from [`ENVELOPE_SHORT`]. An envelope scaled
    // by `Tz` in both axes reaches only half as far and keeps the first glyph too.
    let font = simple_font("Type1", "");
    let at = |y: f64| {
        document(
            &format!("BT /F1 {SIZE} Tf 50 Tz {X} {y} Td (g) Tj ET\n"),
            font.as_bytes(),
            "",
            &[],
        )
    };
    removed(
        "Tz 50, envelope only",
        &at(ENVELOPE_REACHES),
        (X, ENVELOPE_REACHES),
    );
    kept(
        "Tz 50, envelope only, twin",
        &at(ENVELOPE_SHORT),
        (X, ENVELOPE_SHORT),
    );
}

#[test]
fn a_negative_size_or_scale_flips_the_envelope_and_it_still_reaches() {
    // F-18: `-40 Tf` turns the glyph upside down, so the envelope's 0.293 em below the baseline is
    // 11.72 points ABOVE it, where the advance box -- now below the baseline -- is not. `-100 Tz`
    // flips x alone: the envelope's 0.2 em left of the origin is 8 points right of it. Built as a
    // rectangle in text space and unioned there, the flipped envelope comes out inverted and
    // `Rect::union` keeps the advance box's edges: transformed first, it does not.
    let font = simple_font("Type1", "");
    let flipped = |y: f64| {
        document(
            &format!("BT /F1 -{SIZE} Tf 200 {y} Td (g) Tj ET\n"),
            font.as_bytes(),
            "",
            &[],
        )
    };
    removed("-40 Tf", &flipped(242.0), (200.0, 242.0));
    kept("-40 Tf, twin", &flipped(237.5), (200.0, 237.5));
    // `-100 Tz`: the advance runs left from the origin, 22.24 points; the envelope reaches 8 right.
    let mirrored = |x: f64| {
        document(
            &format!("BT /F1 {SIZE} Tf -100 Tz {x} 260 Td (g) Tj ET\n"),
            font.as_bytes(),
            "",
            &[],
        )
    };
    removed("-100 Tz", &mirrored(95.0), (95.0, 260.0));
    kept("-100 Tz, twin", &mirrored(91.5), (91.5, 260.0));
}

/// A page drawing line 1 from a Form XObject (object 9) whose font is `/F1`, with the descriptor
/// (object 7) declaring `bbox`. `shared` adds a second page drawing the same form.
fn in_a_form(bbox: &str, shared: bool) -> Vec<u8> {
    let form_content = line_one(DEEP);
    let form = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 400 400] /Resources << /Font << /F1 5 0 R >> >>",
        form_content.as_bytes(),
    );
    let content = format!(
        "q /Fm1 Do Q\nBT /F2 12 Tf {} {} Td (A) Tj ET\n",
        ANCHOR.0, ANCHOR.1
    );
    let kids = if shared { "[3 0 R 10 0 R]" } else { "[3 0 R]" };
    let count = if shared { 2 } else { 1 };
    let mut all: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!("<< /Type /Pages /Kids {kids} /Count {count} >>").into_bytes(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] \
           /Resources << /Font << /F2 6 0 R >> /XObject << /Fm1 9 0 R >> >> /Contents 4 0 R >>"
            .to_vec(),
        stream("", content.as_bytes()),
        simple_font("Type1", "/FontDescriptor 7 0 R").into_bytes(),
        simple_font("Type1", "").into_bytes(),
        descriptor(&format!("/FontBBox [{bbox}]"), ""),
        b"null".to_vec(),
        form,
    ];
    if shared {
        all.push(
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] \
               /Resources << /XObject << /Fm1 9 0 R >> >> /Contents 11 0 R >>"
                .to_vec(),
        );
        all.push(stream("", b"q /Fm1 Do Q\n"));
    }
    assemble(&all)
}

#[test]
fn a_descriptor_box_inside_a_form_is_read() {
    // F-15, and F-15b: in a form drawn on two pages, the cut it now makes is one the shared-form
    // rule refuses -- row 116's stricter move. The twin of each keeps the glyph and redacts.
    removed("in a form", &in_a_form(REACHES, false), (X, DEEP));
    kept("in a form, twin", &in_a_form(SHORT, false), (X, DEEP));
    refuses(
        "in a shared form",
        &in_a_form(REACHES, true),
        "shared-form-would-change-elsewhere",
    );
    redact(&in_a_form(SHORT, true)).expect("in a shared form, twin: nothing reaches, so no cut");
}

// ---- the Type 3 image check keeps the dictionary's box -------------------------------------

#[test]
fn the_type3_image_check_reads_the_dictionary_box_alone() {
    // F-10 (P5): an image a Type 3 procedure draws outside the DICTIONARY's box and inside the
    // descriptor's still refuses. Fed the union, the check would move this to `Ok`. The glyph sits
    // far from the region, so the image check is the only thing that can refuse.
    let procedure =
        b"556 0 d0\nq 1000 0 0 1000 2000 0 cm BI /W 1 /H 1 /BPC 8 /CS /G ID \x00 EI Q\n";
    let far = "BT /F1 12 Tf 20 350 Td (g) Tj ET\n";
    let big = [descriptor("/FontBBox [-5000 -5000 5000 5000]", "")];
    refuses(
        "an image inside the descriptor's box only",
        &type_three_with(
            far,
            "/FontBBox [0 0 1000 1000]",
            "[0.001 0 0 0.001 0 0]",
            "",
            "/FontDescriptor 8 0 R",
            procedure,
            &big,
        ),
        "type-three-image-outside-its-box",
    );
    redact(&type_three_with(
        far,
        "/FontBBox [0 0 3000 1000]",
        "[0.001 0 0 0.001 0 0]",
        "",
        "/FontDescriptor 8 0 R",
        procedure,
        &big,
    ))
    .expect("the twin: the dictionary's box contains the image");
}

#[test]
fn the_type3_image_check_does_not_use_the_envelope() {
    // F-10b: a Type 3 font with NO dictionary box under a `/FontMatrix` of 0.0001 draws an image
    // inside the envelope's numbers read as glyph units. The envelope is in text space; taken as a
    // glyph-space box it would "contain" this image, and the refusal on `main` would become `Ok`.
    let procedure =
        b"5560 0 d0\nq 1000 0 0 1000 0 -200 cm BI /W 1 /H 1 /BPC 8 /CS /G ID \x00 EI Q\n";
    let widths = format!("[{}]", vec!["5560"; 39].join(" "));
    refuses(
        "no dictionary box, an image inside the envelope's numbers",
        &type_three_with(
            "BT /F1 12 Tf 20 350 Td (g) Tj ET\n",
            "",
            "[0.0001 0 0 0.0001 0 0]",
            &widths,
            "",
            procedure,
            &[],
        ),
        "type-three-image-outside-its-box",
    );
    // The twin: the same font with a dictionary box that contains the image redacts.
    redact(&type_three_with(
        "BT /F1 12 Tf 20 350 Td (g) Tj ET\n",
        "/FontBBox [0 -200 1000 800]",
        "[0.0001 0 0 0.0001 0 0]",
        &widths,
        "",
        procedure,
        &[],
    ))
    .expect("the twin: the dictionary's box contains the image");
}

#[test]
fn the_type3_image_check_uses_no_envelope_in_either_unit() {
    // F-10b at a `/FontMatrix` above one: the envelope's numbers read as glyph units here, not as
    // thousandths of them. With no dictionary box, an image drawn within those numbers in glyph
    // space is outside any box the font declares, and refuses. Together with
    // `the_type3_image_check_does_not_use_the_envelope`, no reading of the envelope as a
    // glyph-space box can contain the image. The glyph sits far from the region, so the image
    // check is the only thing that can refuse.
    let procedure = b"1 0 d0\nq 1 0 0 1 0 0 cm BI /W 1 /H 1 /BPC 8 /CS /G ID \x00 EI Q\n";
    let widths = format!("[{}]", vec!["1"; 39].join(" "));
    let far = "BT /F1 1 Tf 20 350 Td (g) Tj ET\n";
    refuses(
        "no dictionary box, glyph units above one",
        &type_three_with(far, "", "[10 0 0 10 0 0]", &widths, "", procedure, &[]),
        "type-three-image-outside-its-box",
    );
    // The twin: a dictionary box holding the image redacts.
    redact(&type_three_with(
        far,
        "/FontBBox [0 0 1 1]",
        "[10 0 0 10 0 0]",
        &widths,
        "",
        procedure,
        &[],
    ))
    .expect("the twin: the dictionary's box contains the image");
}

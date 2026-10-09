//! #290 and #297: a standard-14 font with no `/Widths`, placed by glyph NAME from the generated
//! table, and the descriptor flags that change which glyph a code draws.
//!
//! Each leak shape here returned `Ok` on `main` with the secret drawn in the region, measured by
//! #290's issue and its round-0 spec review:
//! - `/Differences [96 /grave]` or `[65 /W]` padding: the padding was advanced by the code's
//!   StandardEncoding glyph, not the named one, so the secret sat elsewhere to burrow;
//! - `/MacRomanEncoding`, read as Standard, so code 96 was `quoteleft` (222) where PDFium draws
//!   `grave` (333);
//! - a symbolic `/Flags`, which changes PDFium's code-to-glyph mapping;
//! - AllCaps (#297), on a Type1, MMType1 or TrueType font unless an embedded program loads:
//!   lowercase codes drawn as capitals at the capitals' widths.
//!
//! The first two now redact, and the redacted page carries no character at all -- ink in the region
//! is not enough, because a glyph placed wrongly is MOVED by the redaction, not removed. The rest
//! refuse by name, each beside a twin that redacts.

#![cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::collections::BTreeSet;

use burrow_engines::pdfsyntax::region::Region;
use support::char_box_oracle::{chars_on_page, dark_pixels_with_annotations};

/// The region, in display space: x 100..300, and y 250..300 in PDF space on a 400-point page.
const REGION: Region = Region {
    left: 100.0,
    top: 100.0,
    width: 200.0,
    height: 50.0,
};
const PIXELS: (i32, i32, i32, i32) = (100, 100, 200, 50);

/// A one-page document drawing `content` with `objects` from object 5, `/F1` being object 5.
fn document(content: &[u8], objects: &[&[u8]]) -> Vec<u8> {
    let stream = [
        format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        content,
        b"\nendstream",
    ]
    .concat();
    let mut all: Vec<&[u8]> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] /Resources << /Font << /F1 5 0 R \
          >> >> /Contents 4 0 R >>",
        &stream,
    ];
    all.extend_from_slice(objects);
    let mut out = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in all.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", all.len() + 1).as_bytes());
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            all.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// `count` copies of `pad` then SECRET, at size 20 from `x`, in `/F1`.
fn padded(pad: u8, count: usize, x: f64) -> Vec<u8> {
    let mut content = format!("BT /F1 20 Tf {x} 260 Td <").into_bytes();
    for _ in 0..count {
        content.extend_from_slice(format!("{pad:02X}").as_bytes());
    }
    content.extend_from_slice(b"534543524554> Tj ET");
    content
}

/// Helvetica with no `/Widths`, `extra` written into the font dictionary.
fn helvetica(extra: &str) -> Vec<u8> {
    format!("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica {extra} >>").into_bytes()
}

fn redact(pdf: &[u8]) -> burrow_types::Result<(Vec<u8>, burrow_engines::redact::Report)> {
    let covered: BTreeSet<usize> = [0].into_iter().collect();
    support::redact_page(pdf, 0, covered, REGION)
}

fn dark_in_region(pdf: &[u8]) -> u64 {
    dark_pixels_with_annotations(pdf, 0, PIXELS).0
}

/// THE SHAPE REDACTS CLEAN: it inks the region on the input, and the output carries no ink in
/// the region and none of SECRET's letters anywhere on the page -- a moved glyph is not a removed
/// one. The padding is outside the region and is rightly kept.
#[track_caller]
fn redacts_clean(what: &str, pdf: &[u8]) {
    assert!(
        dark_in_region(pdf) > 0,
        "{what}: the input must ink the region"
    );
    let (out, _) = redact(pdf).unwrap_or_else(|error| panic!("{what}: {error}"));
    assert_eq!(dark_in_region(&out), 0, "{what}: ink left in the region");
    let left: Vec<_> = chars_on_page(&out, 0)
        .into_iter()
        .filter(|c| !c.generated && "SECRT".contains(char::from_u32(c.unicode).unwrap_or(' ')))
        .collect();
    assert!(
        left.is_empty(),
        "{what}: {} of SECRET's letters survive, moved",
        left.len()
    );
}

#[track_caller]
fn refuses(what: &str, pdf: &[u8], rule: &str) {
    match redact(pdf) {
        Err(error) => assert!(
            error.to_string().contains(&format!("[{rule}]")),
            "{what}: {error}"
        ),
        Ok(_) => panic!("{what}: redacted, where [{rule}] must refuse"),
    }
}

/// `/Differences` REMAPS THE PADDING (#290). Fifty code-96 glyphs named `/grave` (333) pad SECRET
/// from x = -230 into the region; burrow placed them as `quoteleft` (222) and kept SECRET. Twenty
/// `/W` at code 65 (944, where `A` is 667) did the same. By name, both redact clean; the twin
/// draws the same padding with no `/Differences`.
#[test]
fn a_differences_remap_is_placed_by_the_name_it_gives() {
    redacts_clean(
        "/Differences [96 /grave]",
        &document(
            &padded(0x60, 50, -230.0),
            &[&helvetica("/Encoding << /Differences [96 /grave] >>")],
        ),
    );
    redacts_clean(
        "/Differences [65 /W]",
        &document(
            &padded(0x41, 20, -150.0),
            &[&helvetica("/Encoding << /Differences [65 /W] >>")],
        ),
    );
    redacts_clean(
        "WinAnsi, no /Differences",
        &document(
            &padded(0x60, 50, -230.0),
            &[&helvetica("/Encoding /WinAnsiEncoding")],
        ),
    );
}

/// MACROMAN (the owner's condition 5): read as Standard, code 96 was `quoteleft` (222) where PDFium
/// draws `grave` (333), and fifty of them left SECRET in the region -- `Ok`, 385 dark pixels. As
/// MacRoman, it redacts clean. THE NEAR-MISS TWIN is a code whose MacRoman glyph differs from
/// WinAnsi's: 0x8E is `eacute` in MacRoman and `Zcaron` in WinAnsi, past the tabulated base-encoding
/// range, so it refuses rather than being read under either; the same glyph named through
/// `/Differences` is placed by name.
#[test]
fn macroman_is_admitted_at_the_codes_it_shares_and_refuses_where_it_differs() {
    redacts_clean(
        "/MacRomanEncoding, code 96",
        &document(
            &padded(0x60, 50, -230.0),
            &[&helvetica("/Encoding /MacRomanEncoding")],
        ),
    );
    refuses(
        "/MacRomanEncoding, code 0x8E",
        &document(
            b"BT /F1 20 Tf 110 260 Td <8E534543524554> Tj ET",
            &[&helvetica("/Encoding /MacRomanEncoding")],
        ),
        "no-widths",
    );
    redacts_clean(
        "/Differences [142 /eacute] over MacRoman",
        &document(
            b"BT /F1 20 Tf 110 260 Td <8E534543524554> Tj ET",
            &[&helvetica(
                "/Encoding << /BaseEncoding /MacRomanEncoding /Differences [142 /eacute] >>",
            )],
        ),
    );
}

/// A BASE ENCODING THE TABLE IS NOT MEASURED UNDER refuses rather than reading as Standard:
/// PDFium's MacExpert behaves as WinAnsi, and its PDFDoc differs by subtype (measured).
#[test]
fn an_unmeasured_base_encoding_refuses() {
    let content = b"BT /F1 20 Tf 110 260 Td (SECRET) Tj ET";
    for encoding in [
        "/Encoding /MacExpertEncoding",
        "/Encoding /PDFDocEncoding",
        "/Encoding /SomeUnknownEncoding",
        "/Encoding << /BaseEncoding /MacExpertEncoding >>",
    ] {
        refuses(
            encoding,
            &document(content, &[&helvetica(encoding)]),
            "width-source",
        );
    }
    redacts_clean(
        "/Encoding /StandardEncoding",
        &document(content, &[&helvetica("/Encoding /StandardEncoding")]),
    );
}

/// A NAME THE TABLE DOES NOT ACCEPT refuses: one PDFium's bundled face draws at another width than
/// the AFM (`guillemotleft`, 612 against 556), one outside the AFM that PDFium resolves anyway
/// (`uni0057`, `W.alt`), and `.notdef`.
#[test]
fn a_name_the_table_does_not_accept_refuses() {
    for name in ["guillemotleft", "uni0057", "W.alt", ".notdef"] {
        let pdf = document(
            b"BT /F1 20 Tf 110 260 Td <41534543524554> Tj ET",
            &[&helvetica(&format!(
                "/Encoding << /Differences [65 /{name}] >>"
            ))],
        );
        refuses(name, &pdf, "no-widths");
    }
}

/// `/Differences` READ AS PDFIUM READS IT, or refused: a string item, a real anchor and a negative
/// anchor refuse; a code given two names takes the LAST, as PDFium's does, and redacts.
#[test]
fn differences_items_are_read_as_pdfium_reads_them() {
    let content = b"BT /F1 20 Tf 110 260 Td (SECRET) Tj ET";
    for (what, differences, rule) in [
        ("a string item", "65 (x) /W", "differences-item-unreadable"),
        ("a real anchor", "65.5 /W", "differences-item-unreadable"),
        ("a negative anchor", "-1 /W /X", "differences-anchor"),
    ] {
        let pdf = document(
            content,
            &[&helvetica(&format!(
                "/Encoding << /Differences [{differences}] >>"
            ))],
        );
        refuses(what, &pdf, rule);
    }
    // LAST WINS: code 96 named `/quoteleft` then `/grave`; PDFium draws `grave`.
    redacts_clean(
        "a repeated code",
        &document(
            &padded(0x60, 50, -230.0),
            &[&helvetica(
                "/Encoding << /Differences [96 /quoteleft 96 /grave] >>",
            )],
        ),
    );
}

/// A SYMBOLIC `/Flags` on a font with no `/Widths` (#290) changes PDFium's code-to-glyph mapping,
/// and refuses `[font-flags]`, for each subtype; `/Flags 32` (nonsymbolic) redacts.
#[test]
fn a_symbolic_font_with_no_widths_refuses() {
    let content = b"BT /F1 20 Tf 110 260 Td (SECRET) Tj ET";
    let descriptor = |flags: &str| {
        format!("<< /Type /FontDescriptor /FontName /Helvetica /Flags {flags} >>").into_bytes()
    };
    for subtype in ["Type1", "TrueType", "MMType1"] {
        let font = format!(
            "<< /Type /Font /Subtype /{subtype} /BaseFont /Helvetica /FontDescriptor 6 0 R >>"
        );
        refuses(
            subtype,
            &document(content, &[font.as_bytes(), &descriptor("4")]),
            "font-flags",
        );
    }
    let font = b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FontDescriptor 6 0 R >>";
    redacts_clean("/Flags 32", &document(content, &[font, &descriptor("32")]));
    // AN UNMEASURED BIT refuses too: the rule is an allowlist, not a list of known-bad bits. Bit 5
    // (16) has not been measured on the bundled faces.
    refuses(
        "/Flags 48 (an unmeasured bit)",
        &document(content, &[font, &descriptor("48")]),
        "font-flags",
    );
    // EVERY MEASURED-NEUTRAL BIT AT ONCE redacts: FixedPitch, Serif, Script, Nonsymbolic, Italic,
    // SmallCaps and ForceBold -- so an allowlist that shrank would over-refuse here.
    redacts_clean(
        "/Flags 393323 (every neutral bit)",
        &document(content, &[font, &descriptor("393323")]),
    );
}

/// ALLCAPS (#297): on a simple font, with or without `/Widths`, PDFium
/// draws a lowercase code as the capital at the capital's width unless an embedded program loads.
/// Refused `[font-flags]` on every simple font, written as 65568 or 65568.9 (4294967295 refuses as
/// a number first). The twin that redacts is `/Flags 32`.
#[test]
fn allcaps_on_any_simple_font_refuses() {
    let content = b"BT /F1 20 Tf 110 260 Td (aaaaSECRET) Tj ET";
    let descriptor = |flags: &str, extra: &str| {
        format!("<< /Type /FontDescriptor /FontName /Helvetica /Flags {flags} {extra} >>")
            .into_bytes()
    };
    let no_widths = b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FontDescriptor 6 0 R >>";
    let widths: Vec<u8> = format!(
        "<< /Type /Font /Subtype /TrueType /BaseFont /FooSans /FirstChar 32 /LastChar 126 \
         /Widths [{}] /FontDescriptor 6 0 R >>",
        vec!["600"; 95].join(" ")
    )
    .into_bytes();
    for flags in ["65568", "65568.9"] {
        refuses(
            &format!("no /Widths, /Flags {flags}"),
            &document(content, &[no_widths, &descriptor(flags, "")]),
            "font-flags",
        );
    }
    // 4294967295 sets every bit to PDFium; past the range both readers hold, it refuses as a number
    // before the flags are judged.
    refuses(
        "no /Widths, /Flags 4294967295",
        &document(content, &[no_widths, &descriptor("4294967295", "")]),
        "number-unreadable",
    );
    refuses(
        "declared /Widths, AllCaps",
        &document(content, &[&widths, &descriptor("65568", "")]),
        "font-flags",
    );
    redacts_clean(
        "/Flags 32",
        &document(content, &[no_widths, &descriptor("32", "")]),
    );
    // A `/FontFile2` THAT DOES NOT LOAD -- empty here -- is not an embedded program to PDFium, which
    // then draws `a` at `A`'s width. The first fix exempted any font naming a program and returned
    // `Ok` with SECRET kept (#290's round-1 security review); AllCaps now refuses on every simple
    // font, the owner's decision, so a program that does load refuses too (the accepted cost).
    let program: &[u8] = b"<< /Length 0 >>\nstream\n\nendstream";
    refuses(
        "AllCaps over a /FontFile2 that does not load",
        &document(
            content,
            &[&widths, &descriptor("65568", "/FontFile2 7 0 R"), program],
        ),
        "font-flags",
    );
}

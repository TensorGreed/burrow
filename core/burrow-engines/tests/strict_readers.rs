//! #125's third security review: values a renderer reads ITEM BY ITEM, or by their bytes, that
//! burrow read another way.
//!
//! Each shape returned `Ok` on `main` with the secret still drawn in the region:
//!
//! - an array of numbers holding a reference, a string or a name -- `/Widths [6 0 R 600 …]`, a
//!   form `/Matrix [1 0 0 1 0 (x) 300]`, a Type 3 `/FontMatrix` likewise -- read through its
//!   unparsed text, so the reference became two numbers and the string vanished, shifting every
//!   later item. PDFium reads each item, and a matrix that is not six numbers as the identity.
//!   Now read item by item: a reference resolves as PDFium resolves it, and anything else that is
//!   not a number refuses `[number-unreadable]`.
//! - a `/Differences` item that is neither a code nor a name -- `(x)`, `0.5`, `true` -- which
//!   PDFium reads as a code and burrow skipped, so the glyph names spelling the secret stayed.
//!   Refused `[differences-item-unreadable]`.
//! - `/BaseEncoding (WinAnsiEncoding)`, a string PDFium reads as WinAnsi and burrow read as
//!   Standard, so a standard-14 font's glyphs were placed outside the region. Refused
//!   `[base-encoding-not-a-name]`.
//!
//! Also the fourth and fifth reviews' width readers: `/FirstChar` that is not whole or is negative,
//! `/W` overlaps, fractional and negative bounds, the cost of reading `/W`, and #292's widths outside
//! 16 bits (the owner's decision: refused, LibreOffice's vertical CJK output with them).
//!
//! WHAT EACH TEST MEASURES, said rather than implied. The tests built on `reaches_and_refuses` /
//! `reaches_and_redacts` measure PDFium's ink in the region on the input and, for a twin, on the
//! output; the clamp test also requires no character to survive, since a glyph moved off the
//! region reads as clean ink. The CID-width tests and the width-range test built on `refuses_by`
//! pin the outcome only. The `/FontMatrix` and `/FontBBox` twins refuse `[type-three-image-cut]`
//! deliberately: the identity reading reaches the image, which is the rule those shapes hid from.

#![cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::collections::BTreeSet;

use burrow_engines::pdfsyntax::region::Region;
use support::char_box_oracle::dark_pixels_with_annotations;

/// The region, in display space: x 100..300, and y 250..300 in PDF space on a 400-point page.
const REGION: Region = Region {
    left: 100.0,
    top: 100.0,
    width: 200.0,
    height: 50.0,
};

/// The same rectangle for the pixel count, at one pixel a point.
const PIXELS: (i32, i32, i32, i32) = (100, 100, 200, 50);

/// A one-page document: catalog 1, pages 2, page 3 with `page_resources`, content 4, then
/// `objects` from 5.
fn document(page_resources: &str, content: &[u8], objects: &[&[u8]]) -> Vec<u8> {
    document_with("", page_resources, content, objects)
}

/// As [`document`], with `page_extra` written into the page dictionary.
fn document_with(
    page_extra: &str,
    page_resources: &str,
    content: &[u8],
    objects: &[&[u8]],
) -> Vec<u8> {
    let stream = [
        format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        content,
        b"\nendstream",
    ]
    .concat();
    let page = format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] {page_extra} \
         /Resources << {page_resources} >> /Contents 4 0 R >>"
    );
    let mut all: Vec<&[u8]> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        page.as_bytes(),
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

fn redact(pdf: &[u8]) -> burrow_types::Result<(Vec<u8>, burrow_engines::redact::Report)> {
    let covered: BTreeSet<usize> = [0].into_iter().collect();
    support::redact_page(pdf, 0, covered, REGION)
}

fn dark_in_region(pdf: &[u8]) -> u64 {
    dark_pixels_with_annotations(pdf, 0, PIXELS).0
}

/// `pdf` inks the region, and refuses under `rule`.
#[track_caller]
fn reaches_and_refuses(what: &str, pdf: &[u8], rule: &str) {
    assert!(
        dark_in_region(pdf) > 0,
        "{what}: the input must ink the region, and PDFium drew nothing there"
    );
    match redact(pdf) {
        Ok((out, _)) => panic!("{what}: Ok ({} bytes), expected [{rule}]", out.len()),
        Err(error) => {
            let message = error.to_string();
            assert!(message.contains(&format!("[{rule}]")), "{what}: {message}");
        }
    }
}

/// THE TWIN: `pdf` inks the region, redacts, and the region is clean afterwards.
#[track_caller]
fn reaches_and_redacts(what: &str, pdf: &[u8]) -> Vec<u8> {
    assert!(
        dark_in_region(pdf) > 0,
        "{what}: the twin must ink the region"
    );
    let (out, _) = redact(pdf).unwrap_or_else(|error| panic!("{what}: {error}"));
    assert_eq!(dark_in_region(&out), 0, "{what}: ink left in the region");
    out
}

/// Whether the emitted document contains `needle` once decompressed by qpdf.
///
/// A raw scan of the output finds nothing, because qpdf re-compresses every stream it writes.
/// A check that scanned the raw bytes would report silence for the wrong reason — the exact
/// shape ADR 0019's third correction was about.
fn decompressed(pdf: &[u8]) -> Vec<u8> {
    let dir = std::env::temp_dir().join(format!("burrow-strict-readers-{}", std::process::id()));
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

/// Helvetica from code 32, every width 600 except code 32's, which is `space_width`.
fn helvetica_with_space(space_width: &str) -> Vec<u8> {
    let rest = vec!["600"; 94].join(" ");
    format!(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 32 /LastChar 126 \
         /Widths [{space_width} {rest}] >>"
    )
    .into_bytes()
}

/// Ten spaces of 1,000 at size 10 put SECRET at x = 100, inside the region. Read as text,
/// `[6 0 R …]` was `[6 0 600 …]` -- the space 6 wide, SECRET at x = 0.6, outside the region, and
/// `Ok` with it on the page. Read item by item the reference resolves to 1,000, as PDFium reads
/// it, so it now REDACTS, cleanly; a string in the same place refuses.
#[test]
fn a_reference_inside_widths_resolves_and_a_string_refuses() {
    let content = b"BT /F1 10 Tf 0 260 Td (          SECRET) Tj ET";
    let by_reference = document(
        "/Font << /F1 5 0 R >>",
        content,
        &[&helvetica_with_space("6 0 R"), b"1000"],
    );
    reaches_and_redacts("/Widths [6 0 R …]", &by_reference);
    // The string where code 32's width belongs: PDFium reads it as 0 and code 33 (`!`) as 1,000,
    // so ten `!` put SECRET at 100; with the string dropped, `!` read as 600 and SECRET sat at 60,
    // short of the region.
    let leak = document(
        "/Font << /F1 5 0 R >>",
        b"BT /F1 10 Tf 0 260 Td (!!!!!!!!!!SECRET) Tj ET",
        &[&helvetica_with_space("(x) 1000"), b"1000"],
    );
    reaches_and_refuses("/Widths [(x) 1000 …]", &leak, "number-unreadable");
    let twin = document(
        "/Font << /F1 5 0 R >>",
        content,
        &[&helvetica_with_space("1000"), b"1000"],
    );
    reaches_and_redacts("/Widths [1000 …]", &twin);
    // THE CLOSEST TWIN: the string leak written as PDFium reads it, `/Widths [0 1000 …]`.
    let as_read = document(
        "/Font << /F1 5 0 R >>",
        b"BT /F1 10 Tf 0 260 Td (!!!!!!!!!!SECRET) Tj ET",
        &[&helvetica_with_space("0 1000"), b"1000"],
    );
    reaches_and_redacts("/Widths [0 1000 …]", &as_read);
}

/// A form drawing SECRET in the region under a `/Matrix` of seven items: PDFium reads the
/// identity; read as text, the string vanished and the form moved 300 points up, off the region.
#[test]
fn a_form_matrix_holding_a_string_or_a_name_refuses_and_the_identity_redacts() {
    let draw = b"BT /F1 20 Tf 110 260 Td (SECRET) Tj ET";
    let form = |matrix: &str| -> Vec<u8> {
        [
            format!(
                "<< /Type /XObject /Subtype /Form /BBox [-1000 -1000 1000 1000] \
                 /Matrix {matrix} /Resources << /Font << /F1 6 0 R >> >> /Length {} >>\nstream\n",
                draw.len()
            )
            .as_bytes(),
            draw,
            b"\nendstream",
        ]
        .concat()
    };
    let font = helvetica_with_space("600");
    for matrix in ["[1 0 0 1 0 (x) 300]", "[1 0 0 1 0 /N 300]"] {
        let leak = document(
            "/XObject << /X1 5 0 R >>",
            b"/X1 Do",
            &[&form(matrix), &font],
        );
        reaches_and_refuses(matrix, &leak, "number-unreadable");
    }
    let twin = document(
        "/XObject << /X1 5 0 R >>",
        b"/X1 Do",
        &[&form("[1 0 0 1 0 0]"), &font],
    );
    reaches_and_redacts("/Matrix [1 0 0 1 0 0]", &twin);
}

/// A Type 3 glyph drawn as an image inside its box, at size 20 from (50, 200). Under PDFium's
/// reading of a seven-item `/FontMatrix` -- the identity -- the image covers x 50..250 and
/// y 200..400, over the region; read as text it was `[.01 0 0 .01 0 0]`, two points square, and
/// the region missed it. The honest identity refuses `[type-three-image-cut]`, the rule this shape
/// was hiding from.
#[test]
fn a_type_three_font_matrix_holding_a_string_refuses() {
    let procedure = b"10 0 d0\nq 10 0 0 10 0 0 cm BI /W 1 /H 1 /BPC 8 /CS /G ID \x00 EI Q\n";
    let proc_stream = [
        format!("<< /Length {} >>\nstream\n", procedure.len()).as_bytes(),
        procedure,
        b"\nendstream",
    ]
    .concat();
    let font = |matrix: &str| -> Vec<u8> {
        format!(
            "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 10 10] /FontMatrix {matrix} \
             /CharProcs 6 0 R /Encoding << /Type /Encoding /Differences [97 /g] >> \
             /FirstChar 97 /LastChar 97 /Widths [10] /Resources << >> >>"
        )
        .into_bytes()
    };
    let content = b"BT /T3 20 Tf 50 200 Td (a) Tj ET";
    let build = |matrix: &str| {
        document(
            "/Font << /T3 5 0 R >>",
            content,
            &[&font(matrix), b"<< /g 7 0 R >>", &proc_stream],
        )
    };
    reaches_and_refuses(
        "/FontMatrix [.01 0 0 .01 0 (x) 0]",
        &build("[0.01 0 0 0.01 0 (x) 0]"),
        "number-unreadable",
    );
    reaches_and_refuses(
        "/FontMatrix [1 0 0 1 0 0]",
        &build("[1 0 0 1 0 0]"),
        "type-three-image-cut",
    );
}

/// Codes 0..5 drawn in the region, spelled S E C R E T by `/Differences`. An item that is neither
/// a code nor a name is a code to PDFium (its integer value), so the names land on 0..5 (1..6 after
/// `true`, whose integer value is 1); burrow
/// skipped it, narrowed codes 65..70, and kept every name.
#[test]
fn a_differences_item_that_is_neither_code_nor_name_refuses_and_the_twin_narrows() {
    let widths = vec!["600"; 256].join(" ");
    let font = |differences: &str| -> Vec<u8> {
        format!(
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 0 /LastChar 255 \
             /Widths [{widths}] /Encoding << /Type /Encoding /Differences [{differences}] >> >>"
        )
        .into_bytes()
    };
    // The second string, outside the region, keeps the font in use after the cut.
    let content =
        b"BT /F1 20 Tf 110 260 Td <000102030405> Tj ET BT /F1 20 Tf 50 50 Td (ABCDEF) Tj ET";
    for item in ["(x)", "0.5", "true"] {
        let leak = document(
            "/Font << /F1 5 0 R >>",
            content,
            &[&font(&format!("65 {item} /S /E /C /R /E /T"))],
        );
        reaches_and_refuses(item, &leak, "differences-item-unreadable");
    }
    let names = b"/S /E /C /R /E /T";
    let twin = document(
        "/Font << /F1 5 0 R >>",
        content,
        &[&font("0 /S /E /C /R /E /T")],
    );
    assert!(twin.windows(names.len()).any(|w| w == names));
    let out = reaches_and_redacts("[0 /S /E /C /R /E /T]", &twin);
    // EACH NAME, not the run: a narrowing that replaced only `/S` would pass a contiguous check.
    // The tokeniser splits a name off its neighbour too (`/S/E`), and is shown to see every name
    // in the INPUT's array first, so a reading that sees nothing cannot pass as "all removed".
    let names_in = |pdf: &[u8]| -> Vec<String> {
        let text = String::from_utf8_lossy(pdf).into_owned();
        let start = text.find("/Differences").expect("a /Differences");
        let array = text
            .get(start + "/Differences".len()..)
            .and_then(|rest| rest.find(']').and_then(|end| rest.get(..end)))
            .expect("the /Differences array closes")
            .to_owned();
        array
            .replace('/', " /")
            .split(|c: char| c.is_whitespace() || c == '[')
            .filter(|token| token.starts_with('/'))
            .map(str::to_owned)
            .collect()
    };
    let before = names_in(&decompressed(&twin));
    for name in ["/S", "/E", "/C", "/R", "/T"] {
        assert!(
            before.iter().any(|n| n == name),
            "the control cannot see {name} in {before:?}"
        );
    }
    let after = names_in(&decompressed(&out));
    for name in ["/S", "/E", "/C", "/R", "/T"] {
        assert!(
            !after.iter().any(|n| n == name),
            "{name} still stands in the twin's {after:?}"
        );
    }
}

/// `/FirstChar 65.5`: PDFium truncates to 65 and the old reader fell back to 0, so every width
/// sat 65 codes out -- and `steps::first_char`'s own refusal runs only for a font that is cut.
/// Refused; `/FirstChar 65` redacts.
#[test]
fn a_first_char_that_is_not_whole_refuses() {
    let font = |first: &str| -> Vec<u8> {
        let widths = vec!["600"; 26].join(" ");
        format!(
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar {first} \
             /LastChar 90 /Widths [{widths}] >>"
        )
        .into_bytes()
    };
    let content = b"BT /F1 20 Tf 110 260 Td (SECRET) Tj ET";
    let leak = document("/Font << /F1 5 0 R >>", content, &[&font("65.5")]);
    reaches_and_refuses("/FirstChar 65.5", &leak, "number-unreadable");
    let twin = document("/Font << /F1 5 0 R >>", content, &[&font("65")]);
    reaches_and_redacts("/FirstChar 65", &twin);
}

/// Fifty code-96 glyphs then SECRET, from x = -230 at size 20, in a standard-14 font with no
/// `/Widths`. WinAnsi's code 96 is 333 wide, so SECRET starts at 103, in the region; Standard's is
/// 222, so it starts at -8, outside it. A string `/BaseEncoding` is WinAnsi to PDFium and was
/// Standard to burrow.
#[test]
fn a_base_encoding_written_as_a_string_refuses_and_the_name_redacts() {
    let mut content = b"BT /F1 20 Tf -230 260 Td (".to_vec();
    content.extend_from_slice(&[b'`'; 50]);
    content.extend_from_slice(b"SECRET) Tj ET");
    let font = |base: &str| -> Vec<u8> {
        format!(
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica \
             /Encoding << /BaseEncoding {base} >> >>"
        )
        .into_bytes()
    };
    let leak = document(
        "/Font << /F1 5 0 R >>",
        &content,
        &[&font("(WinAnsiEncoding)")],
    );
    reaches_and_refuses("(WinAnsiEncoding)", &leak, "base-encoding-not-a-name");
    let twin = document(
        "/Font << /F1 5 0 R >>",
        &content,
        &[&font("/WinAnsiEncoding")],
    );
    reaches_and_redacts("/WinAnsiEncoding", &twin);
}

/// A Type 0 font over Identity-H with `dw` and `w`, drawing CIDs 1..6 at (110, 260).
fn cid_document(dw: &str, w: &str) -> Vec<u8> {
    document(
        "/Font << /F2 5 0 R >>",
        b"BT /F2 20 Tf 110 260 Td <000100020003000400050006> Tj ET",
        &[
            b"<< /Type /Font /Subtype /Type0 /BaseFont /Helvetica /Encoding /Identity-H \
              /DescendantFonts [6 0 R] >>",
            format!(
                "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Helvetica /CIDSystemInfo \
                 << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> {dw} {w} >>"
            )
            .as_bytes(),
        ],
    )
}

/// `/DW [500]`: an ARRAY where a number belongs, which PDFium reads as 0 and the old reader read
/// as its first number, 500. Refused; `/DW 500` redacts. Neither reaches the region through ink
/// PDFium is guaranteed to draw without a font program, so these assert the outcome only.
#[test]
fn a_default_width_written_as_an_array_refuses_and_the_number_redacts() {
    match redact(&cid_document("/DW [500]", "")) {
        Err(error) => assert!(error.to_string().contains("[number-unreadable]"), "{error}"),
        Ok(_) => panic!("/DW [500] redacted"),
    }
    redact(&cid_document("/DW 500", "")).expect("/DW 500 redacts");
}

/// `/W [1 (x) 500]`: a string where a CID range's end belongs. The old reader stopped there and
/// placed every glyph at the default width; refused now. `/W [1 6 500]` redacts.
#[test]
fn a_cid_width_array_holding_a_string_refuses_and_the_numbers_redact() {
    match redact(&cid_document("/DW 1000", "/W [1 (x) 500]")) {
        Err(error) => assert!(error.to_string().contains("[number-unreadable]"), "{error}"),
        Ok(_) => panic!("/W [1 (x) 500] redacted"),
    }
    redact(&cid_document("/DW 1000", "/W [1 6 500]")).expect("/W [1 6 500] redacts");
}

/// #181's `/CropBox` half: an indirect item in the page's box. The text scan read
/// `[0 0 400 6 0 R]` as five numbers and dropped the box, measuring the region from the
/// `/MediaBox`'s top, 50 points above where PDFium measures it from the 350-point crop -- so the
/// region missed SECRET at y 210. The page frame has read item by item since #224, through the
/// same `reading_of`: the reference resolves to 350 and the page redacts as its direct twin does.
#[test]
fn an_indirect_crop_box_item_resolves_as_its_direct_twin_does() {
    let content = b"BT /F1 20 Tf 110 210 Td (SECRET) Tj ET";
    let with_crop = |crop: &str| -> Vec<u8> {
        document_with(
            &format!("/CropBox {crop}"),
            "/Font << /F1 5 0 R >>",
            content,
            &[&helvetica_with_space("600"), b"350"],
        )
    };
    let indirect = with_crop("[0 0 400 6 0 R]");
    reaches_and_redacts("/CropBox [0 0 400 6 0 R]", &indirect);
    let direct = with_crop("[0 0 400 350]");
    reaches_and_redacts("/CropBox [0 0 400 350]", &direct);
}

/// Refused by `rule`, whatever PDFium draws -- for shapes whose ink these tests do not measure.
#[track_caller]
fn refuses_by(what: &str, pdf: &[u8], rule: &str) {
    match redact(pdf) {
        Err(error) => assert!(
            error.to_string().contains(&format!("[{rule}]")),
            "{what}: {error}"
        ),
        Ok(_) => panic!("{what}: redacted, where [{rule}] must refuse"),
    }
}

/// `/W` read as PDFium's `LoadMetricsArray` reads it (#125's fourth security review): a code given
/// two widths took the LAST here and the first there; a start that is not whole was skipped, or
/// stopped the whole array, where PDFium truncates it and reads on. Each `Ok` over the secret,
/// moved 180 points. Refused; the plain twin redacts.
#[test]
fn a_cid_width_array_read_otherwise_than_pdfium_refuses() {
    refuses_by(
        "[1 [100] 1 [3000]]",
        &cid_document("/DW 1000", "/W [1 [100] 1 [3000]]"),
        "widths-overlap",
    );
    refuses_by(
        "[0.5 0.5 1000 1 6 100]",
        &cid_document("/DW 1000", "/W [0.5 0.5 1000 1 6 100]"),
        "number-unreadable",
    );
    refuses_by(
        "[1.5 [100]]",
        &cid_document("/DW 1000", "/W [1.5 [100]]"),
        "number-unreadable",
    );
    redact(&cid_document("/DW 1000", "/W [1 [100] 2 6 100]")).expect("a plain /W redacts");
}

/// #241's `/FontBBox` half: `[[0 0] 10 10 []]` scanned as text was four numbers, `0 0 10 10`; PDFium
/// reads it item by item. Read item by item here, a nested item refuses. The plain box is the
/// Type 3 font the image test draws, whose glyph the region cuts.
#[test]
fn a_font_bbox_with_a_nested_item_refuses() {
    let procedure = b"10 0 d0\nq 10 0 0 10 0 0 cm BI /W 1 /H 1 /BPC 8 /CS /G ID \x00 EI Q\n";
    let proc_stream = [
        format!("<< /Length {} >>\nstream\n", procedure.len()).as_bytes(),
        procedure,
        b"\nendstream",
    ]
    .concat();
    let font = |bbox: &str| -> Vec<u8> {
        format!(
            "<< /Type /Font /Subtype /Type3 /FontBBox {bbox} /FontMatrix [1 0 0 1 0 0] \
             /CharProcs 6 0 R /Encoding << /Type /Encoding /Differences [97 /g] >> \
             /FirstChar 97 /LastChar 97 /Widths [10] /Resources << >> >>"
        )
        .into_bytes()
    };
    let build = |bbox: &str| {
        document(
            "/Font << /T3 5 0 R >>",
            b"BT /T3 20 Tf 50 200 Td (a) Tj ET",
            &[&font(bbox), b"<< /g 7 0 R >>", &proc_stream],
        )
    };
    refuses_by(
        "[[0 0] 10 10 []]",
        &build("[[0 0] 10 10 []]"),
        "number-unreadable",
    );
    refuses_by("[0 0 10 10]", &build("[0 0 10 10]"), "type-three-image-cut");
}

/// A Type 0 font over Identity-H with a font descriptor and an identity `/CIDToGIDMap`, so PDFium
/// draws substitute glyphs: `pads` copies of CID 1, then six glyphs, at size 10 from (110, 260).
fn cid_ink_document(dw: &str, w: &str, pads: usize) -> Vec<u8> {
    let mut content = b"BT /C0 10 Tf 110 260 Td <".to_vec();
    content.extend_from_slice("0001".repeat(pads).as_bytes());
    content.extend_from_slice(b"003600460044005500480057> Tj ET");
    document(
        "/Font << /C0 5 0 R >>",
        &content,
        &[
            b"<< /Type /Font /Subtype /Type0 /BaseFont /Helvetica /Encoding /Identity-H \
              /DescendantFonts [6 0 R] >>",
            format!(
                "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Helvetica /CIDSystemInfo \
                 << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor \
                 7 0 R {dw} {w} /CIDToGIDMap /Identity >>"
            )
            .as_bytes(),
            b"<< /Type /FontDescriptor /FontName /Helvetica /Flags 32 /FontBBox [0 -200 1000 900] \
              /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 >>",
        ],
    )
}

/// A RANGE STARTING BELOW 0 (#125's fifth security review): `[-70000 5 0]` gives codes 0..5 width
/// 0 to PDFium. Without the clamp the 65,536-code bound was spent below 0, codes 0..5 took `/DW`
/// 1,000, and twenty pads put the glyphs 200 points right, off the region -- `Ok` over them. With
/// it the pads are 0 wide, the glyphs sit at 110, and they are removed, as from the `[0 5 0]` twin.
///
/// INK IN THE REGION IS NOT ENOUGH HERE, and the first version of this test showed it: without
/// the clamp burrow removed the pads by its own widths, so PDFium redrew the six glyphs 200 points
/// right, off the region -- clean region, glyphs still in the file. So the output must carry no
/// characters at all, as PDFium reads them.
#[test]
fn a_cid_width_range_starting_below_zero_is_clamped_and_redacts() {
    for w in ["/W [-70000 5 0]", "/W [0 5 0]"] {
        let out = reaches_and_redacts(w, &cid_ink_document("/DW 1000", w, 20));
        let left = support::char_box_oracle::chars_on_page(&out, 0);
        assert!(
            left.is_empty(),
            "{w}: {} characters survive, moved",
            left.len()
        );
    }
}

/// `/W` CHARGED FOR EVERY ITEM IT READS (#125's fifth security review): runs below 0 assign
/// nothing, and 4,000 of them over one 65,535-item array took 83.8 s against a 2 s budget. Each run
/// now costs its items, so the read ceiling refuses within a few runs. The same with empty runs,
/// which cost one item each.
#[test]
fn a_cid_width_array_that_reads_too_much_is_refused_quickly() {
    let inner = format!("[{}]", "1 ".repeat(65_535));
    let runs = "-70000 8 0 R ".repeat(4_000);
    let pdf = document(
        "/Font << /C0 5 0 R >>",
        b"BT /C0 10 Tf 110 260 Td <0001> Tj ET",
        &[
            b"<< /Type /Font /Subtype /Type0 /BaseFont /Helvetica /Encoding /Identity-H \
              /DescendantFonts [6 0 R] >>",
            format!(
                "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Helvetica /CIDSystemInfo \
                 << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor \
                 7 0 R /DW 600 /W [{runs}] /CIDToGIDMap /Identity >>"
            )
            .as_bytes(),
            b"<< /Type /FontDescriptor /FontName /Helvetica /Flags 32 /FontBBox [0 -200 1000 900] \
              /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 >>",
            inner.as_bytes(),
        ],
    );
    let started = std::time::Instant::now();
    refuses_by("4,000 runs below 0", &pdf, "widths-too-long");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(3),
        "took {:?}",
        started.elapsed()
    );

    let empty_runs = format!("/W [{}]", "0 [] ".repeat(300_000));
    refuses_by(
        "300,000 empty runs",
        &cid_document("/DW 1000", &empty_runs),
        "widths-too-long",
    );
}

/// A range END that is not whole refuses as its start does (#125's fifth code review): the only
/// earlier fixture had a bad start, so a weakened end check would have survived.
#[test]
fn a_cid_width_range_end_that_is_not_whole_refuses() {
    refuses_by(
        "[1 6.5 500]",
        &cid_document("/DW 1000", "/W [1 6.5 500]"),
        "number-unreadable",
    );
}

/// THE SAME WIDTH TWICE is read alike by both readers, so it is not an overlap to refuse.
#[test]
fn a_cid_code_given_the_same_width_twice_redacts() {
    redact(&cid_document("/DW 1000", "/W [1 [100] 1 [100]]"))
        .expect("a repeated equal width redacts");
}

/// A NEGATIVE `/FirstChar` ON A FONT SHARED WITH ANOTHER PAGE (#125's fifth security review).
/// PDFium loads no `/Widths` for it; burrow indexed them from -1. `steps::first_char` refused it
/// only for a font that is cut, and a shared font is not. Refused where every font is read.
#[test]
fn a_negative_first_char_refuses_even_on_a_shared_font() {
    let widths = vec!["600"; 95].join(" ");
    let font = format!(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar -1 /LastChar 93 \
         /Widths [{widths}] >>"
    );
    // Two pages, both drawing with the font, so redacting page 0 does not cut it.
    let page0 = b"BT /F1 20 Tf 110 260 Td (SECRET) Tj ET";
    let page1 = b"BT /F1 20 Tf 50 50 Td (other page) Tj ET";
    let stream = |content: &[u8]| -> Vec<u8> {
        [
            format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(),
            content,
            b"\nendstream",
        ]
        .concat()
    };
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] /Resources << /Font << /F1 7 0 R \
          >> >> /Contents 4 0 R >>"
            .to_vec(),
        stream(page0),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] /Resources << /Font << /F1 7 0 R \
          >> >> /Contents 6 0 R >>"
            .to_vec(),
        stream(page1),
        font.into_bytes(),
    ];
    let mut out = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    refuses_by("/FirstChar -1, shared", &out, "first-char");
}

/// #292, THE OWNER'S DECISION (2026-10-08): a simple font's width below 0 or at 65,535 and above,
/// which PDFium wraps in 16 bits (-1000 is 64,536), refuses -- accepting that LibreOffice Writer's
/// vertical CJK output, which writes `/Widths [0 -1000 …]`, refuses with it. The in-range twin
/// redacts.
#[test]
fn a_simple_font_width_out_of_sixteen_bits_refuses() {
    let content = b"BT /F1 10 Tf 0 260 Td (          SECRET) Tj ET";
    for (what, space) in [("-1000", "-1000"), ("65536", "65536"), ("65535", "65535")] {
        let pdf = document(
            "/Font << /F1 5 0 R >>",
            content,
            &[&helvetica_with_space(space)],
        );
        refuses_by(what, &pdf, "width-out-of-range");
    }
    let missing = document(
        "/Font << /F1 5 0 R >>",
        content,
        &[
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 33 /LastChar 33 \
              /Widths [600] /FontDescriptor 6 0 R >>",
            b"<< /Type /FontDescriptor /FontName /Helvetica /Flags 32 /MissingWidth 65536 >>",
        ],
    );
    refuses_by("/MissingWidth 65536", &missing, "width-out-of-range");
    reaches_and_redacts(
        "in range",
        &document(
            "/Font << /F1 5 0 R >>",
            content,
            &[&helvetica_with_space("1000")],
        ),
    );
}

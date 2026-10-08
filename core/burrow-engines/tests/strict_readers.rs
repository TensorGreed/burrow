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
//! Each leaking shape inks the region on the input, measured by PDFium, and refuses by name; each
//! twin is the same document written as the specification writes it, and redacts.

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
    let stream = [
        format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        content,
        b"\nendstream",
    ]
    .concat();
    let page = format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] /Resources << {page_resources} >> \
         /Contents 4 0 R >>"
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
/// a code nor a name is a code to PDFium (its integer value), so the names land on 0..5; burrow
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
    let expanded = decompressed(&out);
    assert!(
        !expanded.windows(names.len()).any(|w| w == names),
        "the twin's names spelling the secret were narrowed"
    );
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

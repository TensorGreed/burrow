//! Every committed document with a Form XObject, walked and compared against PDFium.
//!
//! # Why a sweep rather than a fixture
//!
//! ADR 0029 §6 names `FPDFText_GetCharOrigin` as the calibration the redaction read-back
//! **inherits rather than performs** — the read-back cannot catch a placement defect, because
//! both of its passes call the same walk and agree. That makes this sweep the only thing
//! standing behind the disclosure, and until now it stood behind a handful of remembered cases:
//! `glyph_geometry.rs` pins hand-built fixtures and the four producer documents, and **not one
//! of them gave a Form XObject `/Resources` of its own**.
//!
//! A leak went through that gap. `draw_form` walked a form's content against the *page's*
//! resolver, so a `Tf /F1` inside a form resolved against the page's `/Font`. With a page `/F1`
//! of zero widths and a form-local `/F1` of real ones, PDFium rendered twelve characters across
//! 160 points and burrow placed all twelve at one point; a region over the rendered text reached
//! nothing and the redaction returned `Ok` with seven characters still inside the rectangle.
//!
//! So the calibration is run over **shapes** rather than over cases someone thought to write:
//! every corpus document that contains a form, every producer document, and the hand-built
//! two-font case that would have caught the defect. What disagrees is reported by name.
//!
//! # What agreement means here, and what it does not
//!
//! Origins only, within the oracle's pre-registered tolerance. Not boxes: `glyph_geometry.rs`
//! establishes separately what the two box functions measure and why burrow's must contain
//! PDFium's ink. Not "the secret is gone" — nothing here redacts anything.
//!
//! **A document PDFium reads as empty is skipped and counted**, not passed. A comparison over
//! zero characters agrees with everything.

#![cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "a test reading committed fixtures; the workspace lints are for attacker-controlled \
              input in library code, where they stay denied"
)]

mod support;

use support::char_box_oracle::{TOLERANCE_PT, chars_on_page};

/// The form shapes the committed corpus cannot supply, built here with declared widths.
///
/// # The corpus cannot calibrate a form, and the sweep measured that
///
/// Eight committed documents draw a Form XObject and **every one of them is refused with
/// `no-widths`** — they name a standard-14 font and stop, which ADR 0029 records as firing on
/// 38 of 43 corpus documents. So the form half of this sweep compared *nothing*, and the 305
/// origins it did compare were all from producer documents whose forms have no resources of
/// their own.
///
/// A calibration that reports agreement over a shape it never walked is the thing this file
/// exists to stop being, so the shapes are built here: a form with its own `/Font`, a form that
/// declares none and inherits, and a form inside a form where only the inner one declares.
///
/// This is not a substitute for the corpus. It is the difference between "no document
/// disagreed" and "no document was asked".
fn form_shapes() -> Vec<(String, Vec<u8>)> {
    let widths = "[556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 \
                   556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 \
                   556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 \
                   556 556 556 556 556 556 556 556 556 556 556 556 556]";
    let zeros = "[".to_owned() + &"0 ".repeat(63) + "]";
    let helvetica = |w: &str| {
        format!(
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding \
             /FirstChar 32 /LastChar 94 /Widths {w} >>"
        )
    };

    // 1. A form with its own /Font, DIFFERENT from the page's. The shape the leak went through:
    //    resolving against the page places every glyph at one point.
    vec![
        // 0. A font with NO /Widths whose `/Encoding` is a DICTIONARY rather than a name, drawing
        //    code 39 -- the one code where `WinAnsiEncoding` (quotesingle, 191) and
        //    `StandardEncoding` (quoteright, 222) disagree.
        //
        //    `base_encoding`'s `DICTIONARY` arm had no witness: every fixture wrote the name
        //    form, and a security review's mutation making the dictionary always answer
        //    `Standard` survived the entire suite. Under it burrow advances 222 where PDFium
        //    advances 191 -- the same silent drift as an uncalibrated width, and the reason it
        //    belongs against PDFium's own origins rather than against a number written here.
        (
            "encoding-dictionary-no-widths".to_owned(),
            assemble(&[
                "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
                "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 5 0 R >> /XObject << /Fm0 6 0 R >> >> \
             /Contents 4 0 R >>"
                    .to_owned(),
                stream("", "q 1 0 0 1 0 0 cm /Fm0 Do Q\n"),
                "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica \
                  /Encoding << /Type /Encoding /BaseEncoding /WinAnsiEncoding >> >>"
                    .to_owned(),
                stream(
                    "/Type /XObject /Subtype /Form /BBox [0 0 612 792] \
                 /Resources << /Font << /F1 5 0 R >> >>",
                    "BT /F1 24 Tf 72 700 Td ('''') Tj ET\n",
                ),
            ]),
        ),
        (
            "form-with-its-own-font".to_owned(),
            assemble(&[
                "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
                "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 5 0 R >> /XObject << /Fm0 6 0 R >> >> \
             /Contents 4 0 R >>"
                    .to_owned(),
                stream("", "q 1 0 0 1 0 0 cm /Fm0 Do Q\n"),
                helvetica(&zeros),
                stream(
                    "/Type /XObject /Subtype /Form /BBox [0 0 612 792] \
                 /Resources << /Font << /F1 7 0 R >> >>",
                    "BT /F1 24 Tf 72 700 Td (FORMLOCAL) Tj ET\n",
                ),
                helvetica(widths),
            ]),
        ),
        // 2. A form that declares NO /Resources: it must inherit the page's, and the near-miss for
        //    the fix -- a `within` that returned an empty dictionary instead of `None` would refuse
        //    every name this form uses.
        (
            "form-inheriting-the-pages-font".to_owned(),
            assemble(&[
                "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
                "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 5 0 R >> /XObject << /Fm0 6 0 R >> >> \
             /Contents 4 0 R >>"
                    .to_owned(),
                stream("", "q 1 0 0 1 0 0 cm /Fm0 Do Q\n"),
                helvetica(widths),
                stream(
                    "/Type /XObject /Subtype /Form /BBox [0 0 612 792]",
                    "BT /F1 24 Tf 72 700 Td (INHERITED) Tj ET\n",
                ),
            ]),
        ),
        // 3. A form inside a form, where only the INNER one declares resources. The outer inherits
        //    the page's and the inner overrides -- which is the scoping rule applied twice.
        (
            "nested-form-inner-declares".to_owned(),
            assemble(&[
                "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
                "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 5 0 R >> /XObject << /Fm0 6 0 R >> >> \
             /Contents 4 0 R >>"
                    .to_owned(),
                stream("", "q 1 0 0 1 0 0 cm /Fm0 Do Q\n"),
                helvetica(&zeros),
                stream(
                    "/Type /XObject /Subtype /Form /BBox [0 0 612 792] \
                 /Resources << /XObject << /Fm1 7 0 R >> >>",
                    "q /Fm1 Do Q\n",
                ),
                stream(
                    "/Type /XObject /Subtype /Form /BBox [0 0 612 792] \
                 /Resources << /Font << /F1 8 0 R >> >>",
                    "BT /F1 24 Tf 72 700 Td (NESTED) Tj ET\n",
                ),
                helvetica(widths),
            ]),
        ),
    ]
}

/// A stream object body.
fn stream(extra: &str, data: &str) -> String {
    format!(
        "<< /Length {}{extra} >>\nstream\n{data}endstream",
        data.len()
    )
}

/// Objects into a byte-exact PDF with object 1 as the catalogue.
fn assemble(objects: &[String]) -> Vec<u8> {
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

/// Every committed PDF that draws a Form XObject, plus the producer documents.
///
/// The form test is `Do` against an `/XObject` whose `/Subtype` is `/Form`; a raw scan for
/// `/Form` would also match an `/AcroForm` and a `/FormType`, and scanning the compressed bytes
/// would miss a form named inside a flate stream. So the filter is deliberately loose — it
/// selects **candidates** and the sweep reports how many of each kind it examined, rather than
/// claiming a count it cannot derive.
fn candidates() -> Vec<(String, Vec<u8>, bool)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut out = Vec::new();
    for directory in ["tests/redaction/fixtures", "tests/redaction/generated"] {
        let path = root.join(directory);
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        let mut files: Vec<std::path::PathBuf> = entries
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "pdf"))
            .collect();
        files.sort();
        for file in files {
            let Ok(bytes) = std::fs::read(&file) else {
                continue;
            };
            // `needle.len()`, NEVER A LITERAL. The first version wrote `windows(13)` against a
            // fourteen-byte needle, so it matched nothing and the sweep found only the four
            // producer documents -- a filter that silently selects nothing, which is the
            // failure the count gate below exists for and which it caught.
            let has_form = [
                b"/Subtype /Form".as_slice(),
                b"/Subtype/Form".as_slice(),
                b"/Subtype  /Form".as_slice(),
            ]
            .iter()
            .any(|needle| bytes.windows(needle.len()).any(|window| window == *needle));
            let producer = directory.ends_with("fixtures");
            if has_form || producer {
                let name = file
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                out.push((name, bytes, has_form));
            }
        }
    }
    out
}

#[test]
fn burrows_placement_agrees_with_pdfium_on_every_document_that_draws_a_form() {
    let candidates = candidates();
    assert!(
        candidates.len() >= 5,
        "the sweep found {} candidate document(s), which is not the corpus it was written \
         against -- the four producer fixtures alone are five short",
        candidates.len()
    );

    let mut examined = 0usize;
    let mut with_forms = 0usize;
    let mut empty = Vec::new();
    let mut substituted = Vec::new();
    let mut refused = Vec::new();
    let mut disagreed = Vec::new();
    let mut compared = 0usize;

    let synthetic: Vec<(String, Vec<u8>, bool)> = form_shapes()
        .into_iter()
        .map(|(name, bytes)| (name, bytes, true))
        .collect();
    let mut walked_forms = 0usize;

    for (name, bytes, has_form) in candidates.iter().chain(&synthetic) {
        if *has_form {
            with_forms += 1;
        }
        let oracle: Vec<_> = chars_on_page(bytes, 0)
            .into_iter()
            .filter(|char| !char.generated)
            .collect();
        if oracle.is_empty() {
            // A COMPARISON OVER ZERO CHARACTERS AGREES WITH EVERYTHING. Counted, not passed.
            empty.push(name.clone());
            continue;
        }
        // AN `/ActualText` SPAN MAKES THE ORACLE UNCOMPARABLE, and that is a fact about the
        // instrument rather than about the document. PDFium replaces a marked-content span's
        // decoded text with the `/ActualText` string and reports **every** character of it at
        // the span's starting origin -- 17 characters at one point over 16 glyphs, on
        // `09-actualtext.pdf`. Comparing burrow's per-glyph origins against that measures the
        // substitution, not the placement.
        //
        // Skipped and named rather than tolerated: these documents are refused by
        // `marked-content-carries-text` before any redaction reads their geometry, so nothing
        // downstream depends on the comparison this cannot make.
        if bytes
            .windows(b"/ActualText".len())
            .any(|window| window == b"/ActualText")
        {
            substituted.push(name.clone());
            continue;
        }
        let walked = match burrow_engines::glyphs_on_first_page(bytes, &support::walk_options()) {
            Ok(walked) => walked,
            Err(error) => {
                // A refusal is an outcome: the walk declining to place a shape it does not model
                // is the behaviour under test elsewhere. It is not agreement, so it is named.
                refused.push(format!("{name}: {error}"));
                continue;
            }
        };
        examined += 1;
        if *has_form {
            walked_forms += 1;
        }

        if walked.len() != oracle.len() {
            disagreed.push(format!(
                "{name}: burrow placed {} glyph(s), PDFium read {}",
                walked.len(),
                oracle.len()
            ));
            continue;
        }
        for (at, (glyph, char)) in walked.iter().zip(&oracle).enumerate() {
            let apart = (glyph.origin.0 - char.origin.0).hypot(glyph.origin.1 - char.origin.1);
            if apart >= TOLERANCE_PT {
                disagreed.push(format!(
                    "{name}: glyph {at} placed at {:?}, PDFium reads {:?} ({apart:.2} pt apart)",
                    glyph.origin, char.origin
                ));
                break;
            }
            compared += 1;
        }
    }

    eprintln!(
        "\n  calibration: {} committed candidate(s), {with_forms} containing a form, plus {} \
         built here; {examined} walked ({walked_forms} of them with a form), {compared} glyph \
         origin(s) compared",
        candidates.len(),
        synthetic.len()
    );
    if !substituted.is_empty() {
        eprintln!(
            "  the oracle substitutes /ActualText on {} document(s), so per-glyph origins are \
             not comparable:",
            substituted.len()
        );
        for name in &substituted {
            eprintln!("    {name}");
        }
    }
    if !empty.is_empty() {
        eprintln!("  PDFium reads no text on {} document(s):", empty.len());
        for name in &empty {
            eprintln!("    {name}");
        }
    }
    if !refused.is_empty() {
        eprintln!("  the walk refused {} document(s):", refused.len());
        for what in &refused {
            eprintln!("    {what}");
        }
    }

    assert!(
        compared > 0,
        "no glyph origin was compared on any document, so this sweep asserted nothing"
    );
    // THE COUNT THAT MATTERS, gated rather than printed. The committed corpus contributes ZERO
    // walkable form documents -- every one is refused by `no-widths` -- so without the shapes
    // built here this sweep would report agreement over a shape it never walked, which is the
    // failure it exists to stop being.
    assert!(
        walked_forms >= 3,
        "only {walked_forms} document(s) with a form were walked; the calibration is about \
         forms and a run that walks none of them agrees with everything"
    );
    assert!(
        disagreed.is_empty(),
        "burrow and PDFium disagree on {} document(s):\n  {}",
        disagreed.len(),
        disagreed.join("\n  ")
    );
}

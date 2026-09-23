//! Every bundled standard-14 width, against PDFium's own built-in metrics.
//!
//! # Why this exists, and why PDFium is the right instrument
//!
//! `pdfsyntax::standard14` is **hand-transcribed data**. The vendored PDFium is a prebuilt
//! binary with no source tables, so there was nothing in this tree to copy from — which is what
//! makes this a cross-check rather than a tautology: two independent expressions of the same
//! published metrics, compared.
//!
//! A transcription is a thing that can be wrong, and a wrong width is not a crash. It moves
//! every glyph after it along the line, so a region test lands somewhere else and a redaction
//! removes the wrong thing or nothing. That failure has no error message, which is exactly the
//! class this file exists to make loud.
//!
//! # How it measures
//!
//! For each tabulated font and each code in `32..=126`, a one-page document draws that single
//! character with **no `/Widths`** — so burrow must use the bundled table and PDFium must use
//! its built-in one. The advance is read as the distance between two consecutive origins:
//! `FPDFText_GetCharOrigin` on a two-glyph string, which is the same instrument
//! `glyph_geometry.rs` calibrates the walk with and carries no font-metric interpretation.
//!
//! # What disagreement means
//!
//! A failure here is a defect in `standard14`'s numbers, not in PDFium. The test names the
//! font, the code, both values and the difference, because "some width is wrong" is not
//! actionable and "Helvetica code 87 is 944 here and 722 there" is.

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

use burrow_engines::pdfsyntax::standard14::{
    ACCEPTED, BaseEncoding, DISPUTED, FIRST_CODE, LAST_CODE, width_of,
};
use support::char_box_oracle::chars_on_page;

/// The fonts this module tabulates, by `/BaseFont` name.
/// How many `(spelling, code)` pairs `DISPUTED` excludes across every spelling in `ACCEPTED`.
///
/// Not `DISPUTED.len()`: it is keyed on the style, and several spellings share one style, so a
/// single disputed pair excludes `@` under `Helvetica-Bold` **and** under `Arial-Bold`.
fn disputed_spellings() -> usize {
    ACCEPTED
        .iter()
        .map(|(_, style)| {
            DISPUTED
                .iter()
                .filter(|(disputed, _)| disputed == style)
                .count()
        })
        .sum()
}

/// Every `/BaseFont` spelling `width_of` answers for, taken from the module itself.
///
/// **Not written out here.** This was a hand-written list of the twelve canonical names while
/// `width_of` accepted twenty-one spellings, so the nine aliases were never measured — and a
/// security review found that `/Arial-Bold` drew `@` at a width PDFium disagrees with, while
/// `/Helvetica-Bold` correctly refused the same glyph. A list beside the thing it is supposed to
/// mirror is the shape that rots; the person adding an alias is the person who will not update
/// it. `burrow_engines::pdfsyntax::standard14::ACCEPTED` is now the single source, and adding a
/// spelling to it adds it to this sweep.
fn fonts() -> Vec<String> {
    ACCEPTED
        .iter()
        .map(|(spelling, _)| String::from_utf8_lossy(spelling).into_owned())
        .collect()
}

/// A one-page document drawing `code` twice at 100 pt, with **no `/Widths`**.
///
/// Twice, because the advance is the distance between the two origins. One glyph would give an
/// origin and nothing to measure against, and PDFium's box functions carry metric
/// interpretation the origin does not.
fn page_drawing(base: &str, encoding: &str, code: u32) -> Vec<u8> {
    page_drawing_with_encoding(base, &format!("/Encoding /{encoding}"), code)
}

/// As [`page_drawing`], with the whole `/Encoding` entry written out.
///
/// The `/Encoding` **dictionary** form needs it: `base_encoding` has a `DICTIONARY` arm reading
/// `/BaseEncoding`, every fixture here built the `NAME` form, and a security review's mutation
/// making that arm always answer `Standard` survived the entire sweep. Two spellings of the
/// same thing, one of them measured.
fn page_drawing_with_encoding(base: &str, encoding: &str, code: u32) -> Vec<u8> {
    let byte = u8::try_from(code).expect("a code in 32..=126");
    // Escaped, because `(`, `)` and `\` are the three bytes a literal string cannot carry raw.
    let mut text = Vec::new();
    for _ in 0..2 {
        if matches!(byte, b'(' | b')' | b'\\') {
            text.push(b'\\');
        }
        text.push(byte);
    }
    let content = format!(
        "BT /F1 100 Tf 50 400 Td ({}) Tj ET\n",
        String::from_utf8_lossy(&text)
    );
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 2000 792] \
         /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
            .to_owned(),
        format!(
            "<< /Length {} >>\nstream\n{content}endstream",
            content.len()
        ),
        format!("<< /Type /Font /Subtype /Type1 /BaseFont /{base} {encoding} >>"),
    ];
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

/// PDFium's advance for `code` in `base` at 100 pt, in glyph-space units, or `None` if PDFium
/// does not give two characters to measure between.
fn pdfium_advance(base: &str, encoding: &str, code: u32) -> Option<f64> {
    let pdf = page_drawing(base, encoding, code);
    let chars: Vec<_> = chars_on_page(&pdf, 0)
        .into_iter()
        .filter(|char| !char.generated)
        .collect();
    if chars.len() < 2 {
        return None;
    }
    // 100 pt font, so the advance in glyph space is the point distance times ten.
    Some((chars[1].origin.0 - chars[0].origin.0) * 10.0)
}

#[test]
fn every_bundled_width_agrees_with_pdfiums_own_metrics() {
    // Half a unit: the values are integers in glyph space and PDFium's origins come back in
    // points at 100 pt, so a correct pair differs only by float noise. A whole unit of
    // disagreement is a transcription error.
    const TOLERANCE: f64 = 0.5;

    let mut compared = 0usize;
    let mut unmeasurable = Vec::new();
    let mut excluded = Vec::new();
    let mut disagreed = Vec::new();

    let all_fonts = fonts();
    for base in &all_fonts {
        for (encoding_name, encoding) in [
            ("WinAnsiEncoding", BaseEncoding::WinAnsi),
            ("StandardEncoding", BaseEncoding::Standard),
        ] {
            for code in FIRST_CODE..=LAST_CODE {
                let Some(mine) = width_of(base.as_bytes(), code, encoding) else {
                    // A pair the module deliberately does not carry -- see `DISPUTED`. Counted
                    // so the excluded set cannot grow silently: if it ever covers the whole
                    // table, the count gate below fails rather than the run printing `OK`.
                    excluded.push(format!("{base}/{encoding_name} code {code}"));
                    continue;
                };
                let Some(theirs) = pdfium_advance(base, encoding_name, code) else {
                    // PDFium declining to read two characters is not agreement. Counted and
                    // reported rather than skipped silently.
                    unmeasurable.push(format!("{base}/{encoding_name} code {code}"));
                    continue;
                };
                compared += 1;
                if (mine - theirs).abs() > TOLERANCE {
                    disagreed.push(format!(
                        "{base}/{encoding_name} code {code}: burrow has {mine}, PDFium measures \
                         {theirs:.1} ({:.1} apart)",
                        (mine - theirs).abs()
                    ));
                }
            }
        }
    }

    eprintln!(
        "\n  standard-14 calibration: {} font(s) x 2 encoding(s) x {} code(s); \
         {compared} width(s) compared against PDFium, {} unmeasurable, {} deliberately not \
         carried",
        all_fonts.len(),
        LAST_CODE - FIRST_CODE + 1,
        unmeasurable.len(),
        excluded.len()
    );
    if !excluded.is_empty() {
        eprintln!("  not carried, because the two sources disagree (see `DISPUTED`):");
        for what in &excluded {
            eprintln!("    {what}");
        }
    }
    if !unmeasurable.is_empty() {
        eprintln!("  PDFium gave fewer than two characters for:");
        for what in unmeasurable.iter().take(20) {
            eprintln!("    {what}");
        }
        if unmeasurable.len() > 20 {
            eprintln!("    ... and {} more", unmeasurable.len() - 20);
        }
    }

    // THE EXACT COUNT, and every residual accounted for by name. This was a 95 %% tolerance,
    // and a mutation that made `width_of` return `None` for sixty Helvetica codes passed it —
    // the slack was about 114 widths wide. Both residuals are derivable, so neither needs slack:
    //
    //   - `excluded` is exactly the `DISPUTED` pairs, seen once per encoding;
    //   - `unmeasurable` is exactly code 32, the space, which PDFium does not report as a
    //     character, seen once per font per encoding.
    //
    // With both pinned, `compared` is forced and a silently uncarried width has nowhere to hide.
    let expected = all_fonts.len() * 2 * usize::try_from(LAST_CODE - FIRST_CODE + 1).unwrap();
    assert_eq!(
        excluded.len(),
        disputed_spellings() * 2,
        "the not-carried set is exactly `DISPUTED`, once per encoding:\n  {}",
        excluded.join("\n  ")
    );
    assert_eq!(
        unmeasurable.len(),
        all_fonts.len() * 2,
        "the unmeasurable set is exactly code 32 once per font per encoding:\n  {}",
        unmeasurable.join("\n  ")
    );
    assert!(
        unmeasurable.iter().all(|what| what.ends_with("code 32")),
        "something other than the space was unmeasurable:\n  {}",
        unmeasurable.join("\n  ")
    );
    assert_eq!(
        compared,
        expected - excluded.len() - unmeasurable.len(),
        "only {compared} of {expected} widths were measurable against PDFium, and the \
         residuals do not account for the difference"
    );
    assert!(
        disagreed.is_empty(),
        "{} width(s) disagree with PDFium:\n  {}",
        disagreed.len(),
        disagreed.join("\n  ")
    );
}

#[test]
fn an_encoding_dictionary_selects_winansi_just_as_the_name_does() {
    // CODE 39, where `WinAnsiEncoding` has `quotesingle` (191) and `StandardEncoding` has
    // `quoteright` (222) — the one code where the encoding changes the answer, so the only
    // place this branch can be measured at all.
    //
    // Against PDFium rather than against 191, so the test cannot agree with a table that is
    // itself wrong; that is the whole argument of this file.
    let pdf = page_drawing_with_encoding(
        "Helvetica",
        "/Encoding << /BaseEncoding /WinAnsiEncoding >>",
        39,
    );
    let chars: Vec<_> = chars_on_page(&pdf, 0)
        .into_iter()
        .filter(|char| !char.generated)
        .collect();
    assert!(
        chars.len() >= 2,
        "PDFium must read both characters for there to be an advance to measure"
    );
    let theirs = (chars[1].origin.0 - chars[0].origin.0) * 10.0;
    let mine = f64::from(WIN_ANSI_39_HELVETICA);
    assert!(
        (mine - theirs).abs() <= 0.5,
        "an /Encoding dictionary must select WinAnsi as the name form does: the override is \
         {mine} and PDFium measures {theirs:.1}"
    );
}

/// `WinAnsiEncoding`'s width at code 39 in Helvetica, named here so the test above says which
/// branch it is asserting rather than only that two numbers agree.
const WIN_ANSI_39_HELVETICA: u16 = 191;

/// A one-page document naming `base` with the `/Widths` fragment `widths` — empty for none.
fn page_with(base: &str, widths: &str) -> Vec<u8> {
    page_with_encoding(base, widths, "/Encoding /WinAnsiEncoding")
}

/// As [`page_with`], with the `/Encoding` entry written out.
///
/// The `/Encoding` **dictionary** form needs this: `base_encoding` has a `DICTIONARY` arm that
/// reads `/BaseEncoding`, and nothing built one, so a mutation making that arm always answer
/// `Standard` survived a security review's whole sweep. The two forms mean the same thing to a
/// reader and only one of them was ever measured.
fn page_with_encoding(base: &str, widths: &str, encoding: &str) -> Vec<u8> {
    let content = "BT /F1 100 Tf 50 400 Td (AA) Tj ET\n";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 2000 792] \
         /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
            .to_owned(),
        format!(
            "<< /Length {} >>\nstream\n{content}endstream",
            content.len()
        ),
        format!("<< /Type /Font /Subtype /Type1 /BaseFont /{base} {encoding} {widths} >>"),
    ];
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

/// The advance burrow computes between the two `A`s on the page.
fn burrow_advance(pdf: &[u8]) -> f64 {
    let glyphs = burrow_engines::glyphs_on_first_page(pdf, &support::walk_options())
        .expect("the page walks");
    assert_eq!(glyphs.len(), 2, "the fixture draws two glyphs");
    glyphs[1].origin.0 - glyphs[0].origin.0
}

#[test]
fn a_declared_width_wins_over_the_bundled_table() {
    // THE PRECEDENCE, and the whole point of it: a document may declare widths that differ from
    // the published metrics, and it is entitled to. Drawing with the table would then place
    // every glyph where the file does not.
    //
    // `A` in Helvetica is 667 published -- confirmed against PDFium by the calibration above,
    // which is how the first version of this test was caught claiming 722 (that is
    // Helvetica-Bold's). The fixture declares 250, a value no standard-14 font has for `A`, so
    // the two answers cannot be confused.
    let declared = "[".to_owned() + &"250 ".repeat(63) + "]";
    let pdf = page_with(
        "Helvetica",
        &format!("/FirstChar 32 /LastChar 94 /Widths {declared}"),
    );
    let advance = burrow_advance(&pdf);
    assert!(
        (advance - 25.0).abs() < 0.01,
        "the DOCUMENT's 250 must win at 100 pt, and the advance is {advance}"
    );

    // THE NEAR-MISS. The same font with no `/Widths` uses the table, so this test cannot pass
    // by the table being unreachable altogether.
    let pdf = page_with("Helvetica", "");
    let advance = burrow_advance(&pdf);
    assert!(
        (advance - 66.7).abs() < 0.01,
        "with no /Widths the bundled 667 must be used, and the advance is {advance}"
    );
}

#[test]
fn a_font_the_table_does_not_carry_still_refuses() {
    // FAILING CLOSED IS THE POINT. Bundling metrics for the standard 14 must not turn into
    // guessing for everything else: a font this module does not tabulate, and a code outside
    // the tabulated range, both refuse exactly as they did before.
    let pdf = page_with("Symbol", "");
    let error = burrow_engines::glyphs_on_first_page(&pdf, &support::walk_options())
        .expect_err("Symbol is not tabulated");
    assert!(
        format!("{error}").contains("no-widths"),
        "an untabulated font refuses by the same rule as before: {error}"
    );

    // A subset name is not a standard-14 font, whatever it is a subset OF.
    let pdf = page_with("ABCDEF+Helvetica", "");
    let error = burrow_engines::glyphs_on_first_page(&pdf, &support::walk_options())
        .expect_err("a subset carries its own widths or none");
    assert!(format!("{error}").contains("no-widths"), "{error}");
}

#[test]
fn a_code_outside_the_tabulated_range_refuses() {
    // The range is 32..=126 and says so. Code 200 is inside WinAnsi and outside the table.
    let content = "BT /F1 100 Tf 50 400 Td (\u{c8}) Tj ET\n";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 2000 792] \
         /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
            .to_owned(),
        format!(
            "<< /Length {} >>\nstream\n{content}endstream",
            content.len()
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_owned(),
    ];
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
    let error = burrow_engines::glyphs_on_first_page(out.as_bytes(), &support::walk_options())
        .expect_err("code 200 is outside 32..=126");
    assert!(format!("{error}").contains("no-width"), "{error}");
}

#[test]
fn a_disputed_pair_refuses_rather_than_picking_a_side() {
    // `Helvetica-Bold` code 64 is where the published metrics say 975 and PDFium measures 1072.
    // Neither is drawn with; the page refuses. Without this, shrinking `DISPUTED` to nothing
    // would silently start drawing with one of two numbers that disagree by 10%.
    let pdf = page_with("Helvetica-Bold", "");
    // The `A` this fixture draws is not disputed, so the page walks.
    assert!(burrow_advance(&pdf) > 0.0);

    let content = "BT /F1 100 Tf 50 400 Td (@) Tj ET\n";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 2000 792] \
         /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
            .to_owned(),
        format!(
            "<< /Length {} >>\nstream\n{content}endstream",
            content.len()
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>"
            .to_owned(),
    ];
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
    let error = burrow_engines::glyphs_on_first_page(out.as_bytes(), &support::walk_options())
        .expect_err("a disputed width is not drawn with");
    assert!(format!("{error}").contains("no-width"), "{error}");
}

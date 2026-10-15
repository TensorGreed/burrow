//! A pin on PDFium, not on burrow: its page load parses a Type 3 glyph procedure only for a code
//! the page draws (#206).
//!
//! `redact::steps::type_three_marked_work` charges the second reading's work ceiling
//! (`redact_read_back::MAX_MARKED_WORK`) only for drawn codes, on this measured fact. Measured
//! 2026-10-15, release build: with one code drawn, fonts of 1, 51 and 201 procedures nested 900
//! deep loaded in about 6, 10 and 8 ms; with every code drawn, about 7, 99 and 226 ms. If a PDFium
//! bump starts parsing the undrawn ones, the first assertion here fails, and the charge must then
//! cover every procedure in the font.
//!
//! # Its own binary, so nothing else runs PDFium while it times
//!
//! The pin is a wall-clock comparison. Inside a test binary, tests run in parallel and every PDFium
//! call queues on one engine thread, so a timing taken there also measures whatever else is
//! queued. Cargo runs test binaries one at a time, and this one holds a single test, so its
//! timings are PDFium's alone. They are taken through the test oracle (`chars_on_page`: the page
//! load and its text layer, the calls the second reading makes), with no redaction around them.
//!
//! # It proves on every run that it can fail
//!
//! The same font with every code drawn must be measurably slower, so a fixture PDFium stopped
//! parsing at all, or a timing that measured nothing, fails here rather than passing the pin.
#![cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::time::{Duration, Instant};

use support::char_box_oracle::chars_on_page;

/// A one-page document: a line of Helvetica, and a Type 3 font of `procedures` procedures, each
/// nested `depth` deep in marked content, of which the page draws the first `drawn` codes. Every
/// glyph is visible text; the procedures carry marks and nothing else.
fn document(procedures: usize, drawn: usize, depth: usize) -> Vec<u8> {
    let names: String = (0..procedures).map(|i| format!("/g{i} ")).collect();
    let procs: String = (0..procedures)
        .map(|i| format!("/g{i} {} 0 R ", 7 + i))
        .collect();
    let widths = "1000 ".repeat(procedures);
    let hex: String = (1..=drawn).map(|code| format!("{code:02X}")).collect();
    let content =
        format!("BT /F1 24 Tf 72 700 Td (K) Tj ET\nBT /T0 1 Tf 300 300 Td <{hex}> Tj ET\n");
    let procedure = format!(
        "1 0 d0\n{}{}",
        "/Span BMC\n".repeat(depth),
        "EMC\n".repeat(depth)
    );
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
         /Resources << /Font << /F1 5 0 R /T0 4 0 R >> >> /Contents 6 0 R >>"
            .to_owned(),
        format!(
            "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] \
             /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << {procs}>> \
             /Encoding << /Type /Encoding /Differences [1 {names}] >> \
             /FirstChar 1 /LastChar {procedures} /Widths [{widths}] >>"
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
    ];
    // Object 6 is the content stream, and the procedures follow it from 7.
    objects.push(format!(
        "<< /Length {} >>\nstream\n{content}endstream",
        content.len()
    ));
    for _ in 0..procedures {
        objects.push(format!(
            "<< /Length {} >>\nstream\n{procedure}endstream",
            procedure.len()
        ));
    }
    let mut out = String::from("%PDF-1.7\n");
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.push_str(&format!("{} 0 obj\n{object}\nendobj\n", index + 1));
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

/// PDFium's time to load page 0 of `pdf` and read its text layer: the least of five tries.
fn load_time(pdf: &[u8], expected_chars: usize) -> Duration {
    (0..5)
        .map(|_| {
            let started = Instant::now();
            let chars = chars_on_page(pdf, 0);
            let took = started.elapsed();
            assert!(
                chars.len() >= expected_chars,
                "PDFium read {} characters, expected at least {expected_chars}: the fixture is not \
                 being drawn as written",
                chars.len()
            );
            took
        })
        .min()
        .unwrap_or_default()
}

#[test]
fn pdfium_parses_a_type_three_procedure_only_for_a_drawn_code() {
    const DEPTH: usize = 900;
    const UNDRAWN: usize = 200;
    // The `K` in Helvetica is always read; the Type 3 glyphs draw nothing, so PDFium reports no
    // character for them, and only the `K` is counted here.
    let alone = load_time(&document(1, 1, DEPTH), 1);
    let beside_undrawn = load_time(&document(1 + UNDRAWN, 1, DEPTH), 1);
    let all_drawn = load_time(&document(1 + UNDRAWN, 1 + UNDRAWN, DEPTH), 1);
    println!(
        "PDFium's load: {alone:?} alone, {beside_undrawn:?} beside {UNDRAWN} undrawn procedures, \
         {all_drawn:?} with every code drawn"
    );
    // THE CONTROL, ON EVERY RUN: parsing the 201 must show. Measured about 220 ms in a release
    // build; 100 ms is the line, so a fixture PDFium does not parse, or a timing of nothing, fails.
    assert!(
        all_drawn > alone + Duration::from_millis(100),
        "with every code drawn PDFium took {all_drawn:?} against {alone:?} alone: the fixture no \
         longer makes parsing visible, so the pin below would pass over nothing"
    );
    // THE PIN: the undrawn 200 add next to nothing. Parsed, they would add what the control adds.
    assert!(
        beside_undrawn < alone + Duration::from_millis(50),
        "{UNDRAWN} undrawn Type 3 procedures added {:?} to PDFium's page load ({alone:?} alone): \
         PDFium now parses procedures no drawn code names, and `type_three_marked_work` must \
         charge every procedure in the font",
        beside_undrawn.saturating_sub(alone)
    );
}

//! What the pinned PDFium does with the standard 14 fonts, measured -- the half of #290's table that
//! is not the AFM files.
//!
//! # Two jobs
//!
//! 1. **The face gate.** `pdfium_draws_each_standard_fourteen_name_from_its_bundled_face` asks
//!    PDFium which face it actually loaded for every `/BaseFont` spelling burrow accepts -- the
//!    family name and a sha256 of the font data -- and requires the face recorded in the generated
//!    table. A table measured from one face says nothing about another: natively PDFium prefers a
//!    matching system font when one is installed (#290's round-0 spec review measured it), and on
//!    that day the widths are not these. This test is the owner's condition 1 (2026-10-09): it
//!    checks which face LOADED, not that a test ran, and `tools/test-standard14-face-gate.sh` hands
//!    PDFium a substitute face and requires this test to go red naming it.
//!
//! 2. **The measurement.** With `BURROW_STANDARD14_MEASURE=<path>`, `measure_into_a_file` writes
//!    PDFium's advance for every glyph name in every AFM (drawn through `/Differences`) and for every
//!    code 32..=126 under each base encoding, per style -- under `/Subtype /Type1` AND `/TrueType`,
//!    and each name at two codes -- and the faces it loaded. That file and the
//!    AFMs are all `tools/make-standard14-table.py` reads; `tools/check-standard14-table.sh` runs
//!    both and diffs the result against the committed table, so a PDFium bump that moves a bundled
//!    width goes red rather than stale (the owner's condition 2).
//!
//! # Why its own PDFium, not the oracle's
//!
//! The gate's mutation needs PDFium initialised with a user font path, which only
//! `FPDF_InitLibraryWithConfig` can give, and the oracle initialises with `FPDF_InitLibrary`. This
//! binary initialises exactly once, on one thread, behind a lock, as `core/CLAUDE.md` requires.

#![cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "a test measuring fixtures it built; the workspace lints are for attacker-controlled \
              input in library code, where they stay denied"
)]

use std::collections::BTreeMap;
use std::ffi::{CString, c_char, c_double, c_int, c_uint, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use burrow_engines::pdfsyntax::standard14::ACCEPTED;
use burrow_engines::pdfsyntax::standard14_table::FACES;

#[repr(C)]
struct LibraryConfig {
    version: c_int,
    user_font_paths: *const *const c_char,
    isolate: *mut c_void,
    v8_embedder_slot: c_uint,
}

// Declared here, not in the crate: nothing that ships may reach these. `#[link]` is explicit for
// the reason `tests/support/char_box_oracle.rs` records.
#[link(name = "pdfium")]
unsafe extern "C" {
    fn FPDF_InitLibraryWithConfig(config: *const LibraryConfig);
    fn FPDF_LoadMemDocument64(
        data: *const c_void,
        size: usize,
        password: *const c_char,
    ) -> *mut c_void;
    fn FPDF_CloseDocument(document: *mut c_void);
    fn FPDF_LoadPage(document: *mut c_void, index: c_int) -> *mut c_void;
    fn FPDF_ClosePage(page: *mut c_void);
    fn FPDFText_LoadPage(page: *mut c_void) -> *mut c_void;
    fn FPDFText_ClosePage(text_page: *mut c_void);
    fn FPDFText_CountChars(text_page: *mut c_void) -> c_int;
    fn FPDFText_IsGenerated(text_page: *mut c_void, index: c_int) -> c_int;
    fn FPDFText_GetCharOrigin(
        text_page: *mut c_void,
        index: c_int,
        x: *mut c_double,
        y: *mut c_double,
    ) -> c_int;
    fn FPDFText_GetTextObject(text_page: *mut c_void, index: c_int) -> *mut c_void;
    fn FPDFTextObj_GetFont(text_object: *mut c_void) -> *mut c_void;
    fn FPDFFont_GetFamilyName(font: *mut c_void, buffer: *mut c_char, length: usize) -> usize;
    fn FPDFFont_GetFontData(
        font: *mut c_void,
        buffer: *mut u8,
        length: usize,
        out_length: *mut usize,
    ) -> c_int;
}

/// One lock for every PDFium call in this binary, and the one initialisation behind it.
fn pdfium() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let lock = LOCK.get_or_init(|| {
        // THE SUBSTITUTE FACE, for the gate's mutation only: a directory PDFium searches as a user
        // font path. Unset in every ordinary run.
        let paths: Vec<CString> = std::env::var("BURROW_PDFIUM_FONT_PATHS")
            .ok()
            .map(|value| {
                value
                    .split(':')
                    .filter(|p| !p.is_empty())
                    .map(|p| CString::new(p).expect("a path with no NUL"))
                    .collect()
            })
            .unwrap_or_default();
        let mut pointers: Vec<*const c_char> = paths.iter().map(|p| p.as_ptr()).collect();
        pointers.push(std::ptr::null());
        let config = LibraryConfig {
            version: 2,
            user_font_paths: if paths.is_empty() {
                std::ptr::null()
            } else {
                pointers.as_ptr()
            },
            isolate: std::ptr::null_mut(),
            v8_embedder_slot: 0,
        };
        // SAFETY: the only initialisation in this binary, made once under the lock every later
        // PDFium call takes. `paths` and `pointers` outlive the call; PDFium copies the paths.
        unsafe { FPDF_InitLibraryWithConfig(&config) };
        Mutex::new(())
    });
    lock.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A one-page document drawing `codes` at 100 pt from x = 50 in a font of `subtype` and `base`,
/// with `encoding` written as the font's `/Encoding` entry (or none).
fn page(subtype: &str, base: &str, encoding: &str, codes: &[u8]) -> Vec<u8> {
    let hex: String = codes.iter().map(|c| format!("{c:02X}")).collect();
    let content = format!("BT /F1 100 Tf 50 400 Td <{hex}> Tj ET\n");
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
        format!("<< /Type /Font /Subtype /{subtype} /BaseFont /{base} {encoding} >>"),
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

/// What PDFium did with a page: the origins of the characters it did not generate, and the face of
/// the first one.
struct Drawn {
    origins: Vec<f64>,
    face: Option<(String, String)>,
}

fn draw(pdf: &[u8]) -> Drawn {
    let _guard = pdfium();
    let mut origins = Vec::new();
    let mut face = None;
    // SAFETY: every handle is used only between its open and its close below, under the lock, and
    // every buffer is sized from PDFium's own answer for it. A null page or text page is passed on
    // to PDFium, which answers a null handle with zero characters and closes nothing.
    unsafe {
        let doc = FPDF_LoadMemDocument64(pdf.as_ptr().cast(), pdf.len(), std::ptr::null());
        assert!(!doc.is_null(), "PDFium refused a document this test built");
        let page = FPDF_LoadPage(doc, 0);
        let text = FPDFText_LoadPage(page);
        for index in 0..FPDFText_CountChars(text) {
            if FPDFText_IsGenerated(text, index) == 1 {
                continue;
            }
            let (mut x, mut y) = (0.0, 0.0);
            if FPDFText_GetCharOrigin(text, index, &mut x, &mut y) == 1 {
                origins.push(x);
            }
            if face.is_none() {
                let object = FPDFText_GetTextObject(text, index);
                let font = if object.is_null() {
                    std::ptr::null_mut()
                } else {
                    FPDFTextObj_GetFont(object)
                };
                if !font.is_null() {
                    let needed = FPDFFont_GetFamilyName(font, std::ptr::null_mut(), 0);
                    let mut name = vec![0_u8; needed.max(1)];
                    FPDFFont_GetFamilyName(font, name.as_mut_ptr().cast(), name.len());
                    let family =
                        String::from_utf8_lossy(name.split(|b| *b == 0).next().unwrap_or_default())
                            .into_owned();
                    let mut length = 0_usize;
                    FPDFFont_GetFontData(font, std::ptr::null_mut(), 0, &mut length);
                    let mut data = vec![0_u8; length];
                    let mut written = 0_usize;
                    FPDFFont_GetFontData(font, data.as_mut_ptr(), data.len(), &mut written);
                    data.truncate(written);
                    // A FACE THAT CANNOT BE NAMED OR READ IS NOT A FACE: recorded as `("", sha256 of
                    // nothing)` it would let the gate compare equal against itself (#290's round-1
                    // code review).
                    assert!(
                        !family.is_empty() && !data.is_empty(),
                        "PDFium gave no family name or no font data for the face it loaded"
                    );
                    face = Some((family, sha256_hex(&data)));
                }
            }
        }
        FPDFText_ClosePage(text);
        FPDF_ClosePage(page);
        FPDF_CloseDocument(doc);
    }
    Drawn { origins, face }
}

/// sha256, in the test only, through the `sha2` the workspace already carries for tests.
fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(data))
}

/// The distance from the first drawn origin to the last, in glyph-space units at 100 pt.
fn span(subtype: &str, base: &str, encoding: &str, codes: &[u8]) -> Option<f64> {
    let drawn = draw(&page(subtype, base, encoding, codes));
    match (drawn.origins.first(), drawn.origins.last()) {
        (Some(first), Some(last)) if drawn.origins.len() >= 2 => Some((last - first) * 10.0),
        _ => None,
    }
}

/// The advance of `code`, measured BETWEEN TWO `A`s (code 65): span(A code A) - span(A A).
///
/// PDFium does not report every code as a character -- the space is the one that matters -- so a
/// code is never measured between two copies of itself. `A` is code 65 under all three base
/// encodings, and a `/Differences` remapping puts the measured name at code 66, never at 65.
fn advance(subtype: &str, base: &str, encoding: &str, code: u8) -> Option<f64> {
    let with = span(subtype, base, encoding, &[65, code, 65])?;
    let without = span(subtype, base, encoding, &[65, 65])?;
    Some(((with - without) * 1000.0).round() / 1000.0)
}

/// The twelve styles, each by its canonical `/BaseFont` name.
fn styles() -> Vec<&'static str> {
    let mut styles: Vec<&'static str> = ACCEPTED
        .iter()
        .map(|(_, style)| std::str::from_utf8(style).expect("an ASCII style"))
        .collect();
    styles.sort_unstable();
    styles.dedup();
    styles
}

fn afm_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/adobe-core14-afm")
}

/// The glyph names an AFM file carries, in file order.
fn afm_names(style: &str) -> Vec<String> {
    let text = std::fs::read_to_string(afm_dir().join(format!("{style}.afm")))
        .unwrap_or_else(|error| panic!("{style}.afm: {error}"));
    text.lines()
        .filter(|line| line.starts_with("C "))
        .filter_map(|line| {
            line.split(';')
                .map(str::trim)
                .find_map(|field| field.strip_prefix("N "))
                .map(str::to_owned)
        })
        .collect()
}

/// THE FACE GATE (the owner's condition 1). Every `/BaseFont` spelling burrow accepts must be
/// drawn from the face the generated table was measured on -- by family name and by font data --
/// in both subtypes burrow reads it under. A system face substituted for any of them fails here,
/// naming the spelling and both faces, and `tools/test-standard14-face-gate.sh` proves it does.
#[test]
fn pdfium_draws_each_standard_fourteen_name_from_its_bundled_face() {
    let mut examined = 0usize;
    let mut wrong = Vec::new();
    for (spelling, style) in ACCEPTED {
        let spelling = std::str::from_utf8(spelling).unwrap();
        let style = std::str::from_utf8(style).unwrap();
        let expected = FACES
            .iter()
            .find(|(name, _, _)| *name == style)
            .unwrap_or_else(|| panic!("the generated table records no face for {style}"));
        for subtype in ["Type1", "TrueType"] {
            examined += 1;
            let drawn = draw(&page(subtype, spelling, "", b"AA"));
            let face = drawn.face.unwrap_or_else(|| {
                panic!("{subtype} /{spelling}: PDFium drew no text to ask the face of")
            });
            if face.0 != expected.1 || face.1 != expected.2 {
                wrong.push(format!(
                    "{subtype} /{spelling}: PDFium loaded {:?} (sha256 {}), the table was \
                     measured on {:?} (sha256 {})",
                    face.0, face.1, expected.1, expected.2
                ));
            }
        }
    }
    eprintln!("face gate: {examined} (subtype, spelling) pairs examined");
    assert_eq!(
        examined,
        ACCEPTED.len() * 2,
        "every accepted spelling, both subtypes"
    );
    assert!(wrong.is_empty(), "faces differ:\n{}", wrong.join("\n"));
}

/// THE MEASUREMENT, written only when `BURROW_STANDARD14_MEASURE` names a file. A plain run
/// measures nothing and passes, so `cargo test` stays fast; `tools/check-standard14-table.sh`
/// sets it.
#[test]
fn measure_into_a_file() {
    let Ok(out) = std::env::var("BURROW_STANDARD14_MEASURE") else {
        return;
    };
    let mut names: BTreeMap<String, BTreeMap<String, Option<f64>>> = BTreeMap::new();
    let mut codes: BTreeMap<String, BTreeMap<String, BTreeMap<u8, Option<f64>>>> = BTreeMap::new();
    let mut faces: BTreeMap<String, (String, String)> = BTreeMap::new();
    for style in styles() {
        let face = draw(&page("Type1", style, "", b"AA"))
            .face
            .expect("PDFium drew text to ask the face of");
        faces.insert(style.to_owned(), face);
        // EACH NAME THROUGH `/Differences`, between two `A`s, at TWO codes (66 and 200) and under
        // BOTH subtypes the resolver reads it under (#290's round-1 code review): a name is
        // recorded with a width only when all four agree, so a PDFium whose TrueType path or a
        // code-dependent fallback diverged would drop the name rather than keep a wrong width.
        let by_name = names.entry(style.to_owned()).or_default();
        for name in afm_names(style) {
            let mut seen = Vec::new();
            for subtype in ["Type1", "TrueType"] {
                for code in [66_u8, 200] {
                    let encoding = format!("/Encoding << /Differences [{code} /{name}] >>");
                    seen.push(advance(subtype, style, &encoding, code));
                }
            }
            let agreed = seen.first().copied().flatten().filter(|first| {
                seen.iter()
                    .all(|other| other.is_some_and(|w| (w - first).abs() < 1e-6))
            });
            by_name.insert(name, agreed);
        }
        // EACH CODE under each base encoding burrow may admit.
        let by_encoding = codes.entry(style.to_owned()).or_default();
        for base in ["StandardEncoding", "WinAnsiEncoding", "MacRomanEncoding"] {
            let encoding = format!("/Encoding /{base}");
            let measured = by_encoding.entry(base.to_owned()).or_default();
            for code in 32..=126_u8 {
                // Both subtypes, as for names: recorded only where they agree.
                let type1 = advance("Type1", style, &encoding, code);
                let truetype = advance("TrueType", style, &encoding, code);
                let agreed = match (type1, truetype) {
                    (Some(a), Some(b)) if (a - b).abs() < 1e-6 => Some(a),
                    _ => None,
                };
                measured.insert(code, agreed);
            }
        }
    }
    let json = serde_json::json!({
        "pdfium": "the vendored build; see engines/",
        "faces": faces.iter().map(|(s, (f, h))| (s.clone(), serde_json::json!({"family": f, "sha256": h}))).collect::<serde_json::Map<_, _>>(),
        "names": names,
        "codes": codes,
    });
    std::fs::write(&out, serde_json::to_vec_pretty(&json).unwrap())
        .unwrap_or_else(|error| panic!("{out}: {error}"));
    eprintln!(
        "measured {} styles, {} names, {} codes",
        faces.len(),
        names.values().map(BTreeMap::len).sum::<usize>(),
        codes
            .values()
            .flat_map(BTreeMap::values)
            .map(BTreeMap::len)
            .sum::<usize>()
    );
}

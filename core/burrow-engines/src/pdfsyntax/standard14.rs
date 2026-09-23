//! The standard 14 fonts' advance widths, for documents that declare none.
//!
//! # Why this is here at all
//!
//! A font with no `/Widths` array has **no advances in the document**: the standard 14 —
//! Helvetica, Times, Courier and their variants — are defined by tables every viewer bundles
//! and no file repeats. burrow did not bundle them, so the resolver refused rather than
//! guessing, and it was right to: an invented width misplaces every glyph after it on the line,
//! and a region test over a misplaced glyph removes the wrong thing or nothing.
//!
//! ADR 0029 recorded the refusal as the cheapest of five conditions for revisiting, and then a
//! measurement changed what "cheapest" meant: **it fires on 38 of 43 corpus documents**, and on
//! every document in the survival corpus this milestone is measured against. A refusal that
//! common stops most documents at the door, which makes every refusal behind it — the ones
//! #125 is about — a guard on a path almost nothing reaches.
//!
//! # Provenance
//!
//! The numbers are the published Adobe Font Metrics values for the standard 14, which PDF
//! 32000-1 Annex D names as the metrics a conforming reader supplies. They are **transcribed
//! here by hand** rather than derived from any engine in this tree: the vendored PDFium is a
//! prebuilt binary and carries no source tables, so there was nothing to copy from — which is
//! what makes the calibration below a real cross-check rather than a tautology.
//!
//! **A transcription is a thing that can be wrong**, and the whole table is checked against
//! PDFium's own built-in metrics on every run: `tests/standard14_calibration.rs` renders each
//! font at each tabulated code and compares the advance PDFium reports against the value here.
//! A digit typed wrongly fails that test rather than misplacing a glyph in a redaction.
//!
//! "The whole table" means **every spelling in [`ACCEPTED`]**, not every canonical name. It used
//! to mean the latter, and the gap was the nine alias spellings — measured, and it had already
//! let `/Arial-Bold` draw a width `/Helvetica-Bold` refuses. The calibration iterates `ACCEPTED`
//! now, so a spelling cannot be added without being measured. See [`DISPUTED`].
//!
//! # What is tabulated, and what still refuses
//!
//! **Codes 32 to 126 only.** That is the range every corpus document draws in, and it is a
//! range whose encoding is unambiguous: `StandardEncoding` and `WinAnsiEncoding` name the same
//! glyph at every code in it except two, handled below. A code outside the range is **not
//! guessed** — the resolver refuses exactly as it did before, with the same rule name.
//!
//! Symbol and ZapfDingbats are **not** tabulated. Their encodings are built into the fonts
//! rather than shared, so they need their own code-to-glyph tables, and no corpus document uses
//! them. They refuse, as before.
//!
//! Failing closed outside the tabulated range is the direction that does not leak: a document
//! burrow cannot measure is refused, and a refusal is a correct answer.
//!
//! # The two codes where the encoding changes the width
//!
//! `StandardEncoding` names code 39 `quoteright` and code 96 `quoteleft`; `WinAnsiEncoding`
//! names them `quotesingle` and `grave`. The glyphs differ and so do their widths, so the base
//! tables hold the `StandardEncoding` values and [`width_of`] substitutes for `WinAnsiEncoding`.
//! Every other code in 32..=126 is the same glyph under both.

/// Which base encoding a simple font's `/Encoding` names.
///
/// **Not [`crate::pdfsyntax::geometry::Encoding`]**, which describes a CMap and answers a
/// different question — a simple font has no CMap. This is the two-valued distinction the
/// widths actually turn on, and it is two-valued because only codes 39 and 96 differ across
/// 32..=126; `MacRomanEncoding` agrees with `StandardEncoding` on both of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseEncoding {
    /// `/WinAnsiEncoding`.
    WinAnsi,
    /// `/StandardEncoding`, `/MacRomanEncoding`, or none stated.
    Standard,
}

/// The lowest code this module has a width for.
pub const FIRST_CODE: u32 = 32;
/// The highest code this module has a width for.
pub const LAST_CODE: u32 = 126;

const HELVETICA: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 222, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667,
    611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 278, 278, 278, 469, 556, 222, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500,
    222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];

const HELVETICA_BOLD: [u16; 95] = [
    278, 333, 474, 556, 556, 889, 722, 278, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611, 975, 722, 722, 722, 722, 667,
    611, 778, 722, 278, 556, 722, 611, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 333, 278, 333, 584, 556, 278, 556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556,
    278, 889, 611, 611, 611, 611, 389, 556, 333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584,
];

const TIMES_ROMAN: [u16; 95] = [
    250, 333, 408, 500, 500, 833, 778, 333, 333, 333, 500, 564, 250, 333, 250, 278, 500, 500, 500,
    500, 500, 500, 500, 500, 500, 500, 278, 278, 564, 564, 564, 444, 921, 722, 667, 667, 722, 611,
    556, 722, 722, 333, 389, 722, 611, 889, 722, 722, 556, 722, 667, 556, 611, 722, 722, 944, 722,
    722, 611, 333, 278, 333, 469, 500, 333, 444, 500, 444, 500, 444, 333, 500, 500, 278, 278, 500,
    278, 778, 500, 500, 500, 500, 333, 389, 278, 500, 500, 722, 500, 500, 444, 480, 200, 480, 541,
];

const TIMES_BOLD: [u16; 95] = [
    250, 333, 555, 500, 500, 1000, 833, 333, 333, 333, 500, 570, 250, 333, 250, 278, 500, 500, 500,
    500, 500, 500, 500, 500, 500, 500, 333, 333, 570, 570, 570, 500, 930, 722, 667, 722, 722, 667,
    611, 778, 778, 389, 500, 778, 667, 944, 722, 778, 611, 778, 722, 556, 667, 722, 722, 1000, 722,
    722, 667, 333, 278, 333, 581, 500, 333, 500, 556, 444, 556, 444, 333, 500, 556, 278, 333, 556,
    278, 833, 556, 500, 556, 556, 444, 389, 333, 556, 500, 722, 500, 500, 444, 394, 220, 394, 520,
];

const TIMES_ITALIC: [u16; 95] = [
    250, 333, 420, 500, 500, 833, 778, 333, 333, 333, 500, 675, 250, 333, 250, 278, 500, 500, 500,
    500, 500, 500, 500, 500, 500, 500, 333, 333, 675, 675, 675, 500, 920, 611, 611, 667, 722, 611,
    611, 722, 722, 333, 444, 667, 556, 833, 667, 722, 611, 722, 611, 500, 556, 722, 611, 833, 611,
    556, 556, 389, 278, 389, 422, 500, 333, 500, 500, 444, 500, 444, 278, 500, 500, 278, 278, 444,
    278, 722, 500, 500, 500, 500, 389, 389, 278, 500, 444, 667, 444, 444, 389, 400, 275, 400, 541,
];

const TIMES_BOLD_ITALIC: [u16; 95] = [
    250, 389, 555, 500, 500, 833, 778, 333, 333, 333, 500, 570, 250, 333, 250, 278, 500, 500, 500,
    500, 500, 500, 500, 500, 500, 500, 333, 333, 570, 570, 570, 500, 832, 667, 667, 667, 722, 667,
    667, 722, 778, 389, 500, 667, 611, 889, 722, 722, 611, 722, 667, 556, 611, 722, 667, 889, 667,
    611, 611, 333, 278, 333, 570, 500, 333, 500, 500, 444, 500, 444, 333, 500, 556, 278, 278, 500,
    278, 778, 556, 500, 500, 500, 389, 389, 278, 556, 444, 667, 500, 444, 389, 348, 220, 348, 570,
];

/// The `(style, code)` pairs where the published metrics and PDFium **disagree**, and which
/// this module therefore does not carry.
///
/// Keyed on the **style** [`ACCEPTED`] canonicalises to, not on the `/BaseFont` as written, so
/// every alias spelling of a disputed font is excluded with it. It was keyed on the raw name,
/// and `/Arial-Bold` drew a width `/Helvetica-Bold` refused.
///
/// # A width two sources disagree on is a width burrow does not draw with
///
/// The calibration is not decoration: it found these. Measured at 100 pt, as the advance
/// between two consecutive origins — the same instrument the calibration uses for every other
/// width, so a disagreement here is a disagreement on the same footing as an agreement there:
///
/// | font | code | published | PDFium |
/// |---|--:|--:|--:|
/// | `Helvetica-Bold` | 64 `@` | 975 | 1072 |
/// | `Helvetica-BoldOblique` | 64 `@` | 975 | 1072 |
/// | `Helvetica-Oblique` | 64 `@` | 1015 | 1116 |
/// | `Helvetica-BoldOblique` | 53 `5` | 556 | 528 |
///
/// Plain `Helvetica` agrees at code 64 exactly, and `Helvetica-Bold` agrees at every other code
/// in the range — so this is not a transcription slip in one column, and it is not PDFium
/// substituting a whole font either. It is four specific glyphs where the engine burrow renders
/// with reports a different advance from the metrics the specification names.
///
/// **Which is right cannot be decided from inside this tree**, and guessing is the thing this
/// module exists to avoid. So the pairs are excluded: a page drawing `@` in Helvetica-Bold with
/// no `/Widths` refuses, exactly as it did before any of this. The rule the module can then
/// state is a real one — **every width here is one two independent sources agree on** — which is
/// stronger than "transcribed carefully".
///
/// # Keyed on the style, and what keying it on the name cost
///
/// These were matched against the `/BaseFont` as the file spells it, above the canonicalisation
/// rather than below it, so an alias walked straight past the exclusion its own table carried. A
/// security review measured it: `/Arial-Bold` drew `@` at 975 where PDFium places 1072, nine
/// tenths of an em out, while the byte-identical page named `/Helvetica-Bold` refused. Every
/// glyph after it on the line drifted, so the region test then reached a neighbour or nothing —
/// a redaction removing the wrong thing, with no error.
///
/// The exclusion is now keyed on the style [`ACCEPTED`] maps a spelling to, and the calibration
/// iterates `ACCEPTED` rather than its own list of names.
pub const DISPUTED: [(&[u8], u32); 4] = [
    (b"Helvetica-Bold", 64),
    (b"Helvetica-BoldOblique", 64),
    (b"Helvetica-Oblique", 64),
    (b"Helvetica-BoldOblique", 53),
];

/// Every Courier variant is monospaced at 600 units.
const COURIER_WIDTH: u16 = 600;

/// `WinAnsiEncoding`'s width at code 39 (`quotesingle`) for each family, where
/// `StandardEncoding` has `quoteright`.
const WIN_ANSI_39: [(&[u8], u16); 6] = [
    (b"Helvetica", 191),
    (b"Helvetica-Bold", 238),
    (b"Times-Roman", 180),
    (b"Times-Bold", 278),
    (b"Times-Italic", 214),
    (b"Times-BoldItalic", 278),
];

/// `WinAnsiEncoding`'s width at code 96 (`grave`) for each family, where `StandardEncoding` has
/// `quoteleft`.
const WIN_ANSI_96: [(&[u8], u16); 6] = [
    (b"Helvetica", 333),
    (b"Helvetica-Bold", 333),
    (b"Times-Roman", 333),
    (b"Times-Bold", 333),
    (b"Times-Italic", 333),
    (b"Times-BoldItalic", 333),
];

/// The table for `base`, or `None` for a font this module does not carry.
///
/// # A subset tag is not a standard-14 font
///
/// `/BaseFont /ABCDEF+Helvetica` names an **embedded subset**, which carries its own `/Widths`
/// and must never reach this table. The prefix is six uppercase letters and a `+` (PDF 32000-1
/// §9.6.4), and a name carrying one is rejected here rather than stripped: a subset that
/// somehow lacked `/Widths` is a document whose metrics genuinely are not knowable, and
/// borrowing the base font's would be the invention this module exists to avoid.
/// Every `/BaseFont` spelling this module answers for, and the **style** each canonicalises to.
///
/// # One table, because a hand-written list beside a `match` rots
///
/// This was a `match` with alias arms, and `standard14_calibration.rs` had its own list of the
/// twelve canonical names. The `match` accepted twenty-one spellings; the calibration measured
/// twelve. The nine aliases were never compared against PDFium — and a security review measured
/// what that cost: `DISPUTED` was keyed on the raw `/BaseFont`, so `/Arial-Bold` walked past the
/// exclusion its own table had and drew `@` at 975 where PDFium places 1072. A page named
/// `/Helvetica-Bold` refused; the byte-identical page named `/Arial-Bold` redacted with every
/// glyph after the `@` about 10 % of an em out of place.
///
/// So the calibration iterates **this**, and an alias cannot be added without being measured.
///
/// # Style, not table
///
/// The style is finer than the width table: `Helvetica` and `Helvetica-Oblique` share
/// one width table because a slant does not change an advance — and PDFium nonetheless reports a
/// different width for `@` in each. So [`DISPUTED`] is keyed on the style, which keeps
/// `Helvetica-Oblique`'s disputed `@` from excluding plain `Helvetica`'s agreeing one.
pub const ACCEPTED: [(&[u8], &[u8]); 21] = [
    (b"Courier", b"Courier"),
    (b"Courier-Bold", b"Courier-Bold"),
    (b"Courier-Oblique", b"Courier-Oblique"),
    (b"Courier-BoldOblique", b"Courier-BoldOblique"),
    (b"Helvetica", b"Helvetica"),
    (b"Arial", b"Helvetica"),
    (b"Helvetica-Oblique", b"Helvetica-Oblique"),
    (b"Helvetica-Italic", b"Helvetica-Oblique"),
    (b"Arial-Italic", b"Helvetica-Oblique"),
    (b"Helvetica-Bold", b"Helvetica-Bold"),
    (b"Arial-Bold", b"Helvetica-Bold"),
    (b"Helvetica-BoldOblique", b"Helvetica-BoldOblique"),
    (b"Arial-BoldItalic", b"Helvetica-BoldOblique"),
    (b"Times-Roman", b"Times-Roman"),
    (b"TimesNewRoman", b"Times-Roman"),
    (b"Times-Bold", b"Times-Bold"),
    (b"TimesNewRoman-Bold", b"Times-Bold"),
    (b"Times-Italic", b"Times-Italic"),
    (b"TimesNewRoman-Italic", b"Times-Italic"),
    (b"Times-BoldItalic", b"Times-BoldItalic"),
    (b"TimesNewRoman-BoldItalic", b"Times-BoldItalic"),
];

/// The style `base` canonicalises to, or `None` for a font this module does not carry.
fn style_of(base: &[u8]) -> Option<&'static [u8]> {
    if base.len() > 7 && base.get(6) == Some(&b'+') {
        return None;
    }
    ACCEPTED
        .iter()
        .find(|(spelling, _)| *spelling == base)
        .map(|(_, style)| *style)
}

/// The width table for a style, and the name [`WIN_ANSI_39`] and [`WIN_ANSI_96`] key on.
///
/// `None` for the Courier styles, which are monospaced and need no table — [`width_of`] answers
/// them before this is reached.
fn table_of(style: &[u8]) -> Option<(&'static [u16; 95], &'static [u8])> {
    let table: (&'static [u16; 95], &'static [u8]) = match style {
        b"Helvetica" | b"Helvetica-Oblique" => (&HELVETICA, b"Helvetica"),
        b"Helvetica-Bold" | b"Helvetica-BoldOblique" => (&HELVETICA_BOLD, b"Helvetica-Bold"),
        b"Times-Roman" => (&TIMES_ROMAN, b"Times-Roman"),
        b"Times-Bold" => (&TIMES_BOLD, b"Times-Bold"),
        b"Times-Italic" => (&TIMES_ITALIC, b"Times-Italic"),
        b"Times-BoldItalic" => (&TIMES_BOLD_ITALIC, b"Times-BoldItalic"),
        _ => return None,
    };
    Some(table)
}

/// The advance for `code` in the standard-14 font named `base`, in glyph-space units.
///
/// `None` when this module carries no width for that font and code — a font it does not
/// tabulate, a code outside [`FIRST_CODE`]`..=`[`LAST_CODE`], or a subset name. **The caller
/// refuses on `None`**; it must not substitute a default, which is the guessing this module
/// exists to replace.
#[must_use]
pub fn width_of(base: &[u8], code: u32, encoding: BaseEncoding) -> Option<f64> {
    if !(FIRST_CODE..=LAST_CODE).contains(&code) {
        return None;
    }
    // CANONICALISED FIRST, AND EVERY LATER TEST KEYS ON THE STYLE. `DISPUTED` used to be
    // consulted against the raw `/BaseFont`, above this line, so every alias spelling walked
    // past it -- see `ACCEPTED` for what that measured.
    let style = style_of(base)?;

    // Courier is monospaced, so it needs no table and no encoding question.
    if style.starts_with(b"Courier") {
        return Some(f64::from(COURIER_WIDTH));
    }
    // EXCLUDED BEFORE THE TABLE IS READ. See `DISPUTED`.
    if DISPUTED
        .iter()
        .any(|(disputed, at)| *disputed == style && *at == code)
    {
        return None;
    }
    let (table, canonical) = table_of(style)?;

    // THE TWO CODES WHERE THE ENCODING DECIDES THE GLYPH. See the module header.
    if encoding == BaseEncoding::WinAnsi {
        let overrides: &[(&[u8], u16)] = match code {
            39 => &WIN_ANSI_39,
            96 => &WIN_ANSI_96,
            _ => &[],
        };
        for (name, width) in overrides {
            if *name == canonical {
                return Some(f64::from(*width));
            }
        }
    }

    // CHECKED, though the range guard above already makes it impossible. An arithmetic
    // overflow here would be a panic in library code rather than a `None`, and the guard is
    // four lines away from the subtraction that depends on it.
    let at = usize::try_from(code.checked_sub(FIRST_CODE)?).ok()?;
    table.get(at).map(|width| f64::from(*width))
}

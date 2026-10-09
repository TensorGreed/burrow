//! The standard 14 fonts' advance widths, for documents that declare none.
//!
//! # Why this is here at all
//!
//! A font with no `/Widths` array has **no advances in the document**: the standard 14 --
//! Helvetica, Times, Courier and their variants -- are defined by tables every viewer bundles and
//! no file repeats. A guessed width misplaces every glyph after it on the line, and a region test
//! over a misplaced glyph removes the wrong thing or nothing, so a width burrow cannot vouch for
//! refuses.
//!
//! # By glyph NAME, from a generated table (#290, ADR 0030)
//!
//! The table was keyed by CODE, for codes 32 to 126, transcribed by hand. A font whose
//! `/Differences`, base encoding or descriptor flags selected another glyph drew it at a width
//! burrow did not compute: `Ok` with the secret in the region (#290). It is now keyed by glyph
//! name, and GENERATED ([`super::standard14_table`]) as the intersection of Adobe's published
//! metrics and the pinned PDFium's own measured advances, per style -- the owner's decision of
//! 2026-10-09. A name the two disagree on, or that PDFium's bundled face lacks at the AFM's width,
//! is absent, and refuses. That subsumes the four `DISPUTED` pairs the calibration had found by
//! hand (`@` in three Helvetica styles, `5` in Helvetica-BoldOblique): they are among the names
//! the measurement excludes.
//!
//! # What the widths are exact for, and the residual (DECISIONS.md rule 14)
//!
//! Exact for a viewer that uses the AFM metrics, and for PDFium drawing from its bundled faces --
//! which `tests/standard14_measure.rs` requires, by family name and font data, for every spelling
//! below. A viewer that substitutes a system font for a standard-14 name (desktop Chrome's PDFium
//! among them, when a matching face is installed) may lay text out otherwise. That residual is not
//! new: the hand-transcribed table had it too.
//!
//! # Under a base encoding: codes 32 to 126
//!
//! StandardEncoding, WinAnsiEncoding and MacRomanEncoding name the same glyph at every code in
//! 32..=126 except 39 and 96. A code past 126 under a base encoding is not tabulated and refuses;
//! a glyph past it is reached through `/Differences`, by name. Symbol and ZapfDingbats carry their
//! own built-in encodings and are not tabulated: they refuse.

use super::standard14_table as table;

/// Which base encoding a simple font's `/Encoding` names.
///
/// **Not [`crate::pdfsyntax::geometry::Encoding`]**, which describes a CMap and answers a
/// different question -- a simple font has no CMap. Any other base encoding refuses before this is
/// reached (`resources::base_encoding`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseEncoding {
    /// `/StandardEncoding`, or none stated.
    Standard,
    /// `/WinAnsiEncoding`.
    WinAnsi,
    /// `/MacRomanEncoding`: measured to agree with PDF 32000-1 Annex D at every code PDFium's
    /// bundled faces carry (#290's round-0 spec review), and admitted by the owner.
    MacRoman,
}

impl BaseEncoding {
    const fn index(self) -> usize {
        match self {
            Self::Standard => 0,
            Self::WinAnsi => 1,
            Self::MacRoman => 2,
        }
    }
}

/// The lowest code a base encoding is tabulated for.
pub const FIRST_CODE: u32 = table::FIRST_CODE;
/// The highest code a base encoding is tabulated for.
pub const LAST_CODE: u32 = table::LAST_CODE;

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
/// Each style is measured and accepted on its own, so `Helvetica` and `Helvetica-Oblique` --
/// which PDFium draws `@` at different widths in -- are judged apart, and an alias spelling is
/// judged as the style it canonicalises to. `tests/standard14_measure.rs` requires every spelling
/// here to load its style's measured face.
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

/// The index of `style` in the generated table.
fn style_index(style: &[u8]) -> Option<usize> {
    table::STYLES
        .iter()
        .position(|candidate| *candidate == style)
}

/// The width of `name` in the style `index`, if the table accepts it.
fn width_at(index: usize, name: &[u8]) -> Option<f64> {
    let at = table::NAMES.binary_search(&name).ok()?;
    let width = *table::WIDTHS.get(index)?.get(at)?;
    (width != table::NONE).then(|| f64::from(width))
}

/// The advance of the glyph NAMED `name` in the standard-14 font `base`, in glyph-space units.
///
/// `None` when the table does not accept that name in that font -- a font it does not carry, a
/// subset name, a name the AFM and PDFium disagree on, or a name outside the AFM. **The caller
/// refuses on `None`.**
#[must_use]
pub fn width_of_name(base: &[u8], name: &[u8]) -> Option<f64> {
    let index = style_index(style_of(base)?)?;
    width_at(index, name)
}

/// The advance for `code` under `encoding` in the standard-14 font named `base`, in glyph-space
/// units.
///
/// `None` when this module carries no width for that font and code -- a font it does not
/// tabulate, a code outside [`FIRST_CODE`]`..=`[`LAST_CODE`], a subset name, or a code whose glyph
/// the measurement did not accept. **The caller refuses on `None`**; it must not substitute a
/// default, which is the guessing this module exists to replace.
#[must_use]
pub fn width_of(base: &[u8], code: u32, encoding: BaseEncoding) -> Option<f64> {
    if !(FIRST_CODE..=LAST_CODE).contains(&code) {
        return None;
    }
    let index = style_index(style_of(base)?)?;
    let bits = table::ACCEPTED_CODES.get(index)?.get(encoding.index())?;
    // A bit per code: word `code >> 6`, bit `code & 63`.
    let word = bits.get(usize::try_from(code >> 6).ok()?)?;
    if word & (1_u64 << (code & 63)) == 0 {
        return None;
    }
    let at = usize::try_from(code.checked_sub(FIRST_CODE)?).ok()?;
    let name_index = *table::ENCODINGS.get(encoding.index())?.get(at)?;
    let name = table::NAMES.get(usize::from(name_index))?;
    width_at(index, name)
}

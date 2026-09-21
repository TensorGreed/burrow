//! Where each glyph is on the page.
//!
//! Deciding what falls inside a redaction region means computing where each glyph sits, and
//! nothing in this tree did that before #129. ADR 0029 §6's geometry half.
//!
//! # A matrix, not a pair of coordinates
//!
//! Every term in PDF's text machinery is a transform, and they compose. Writing them as offsets
//! added to an `(x, y)` works until the first `cm` with a rotation in it, at which point a
//! "horizontal" advance is no longer horizontal. So the state here is matrices throughout, and
//! the glyph origin falls out of them rather than being tracked beside them.
//!
//! # What a box here means, and why it is not the advance box
//!
//! **The advance box is where the glyph's pen is. Redaction is about where its ink is.** A Type 3
//! `/CharProcs` entry may draw anywhere at all, and CFF and TrueType outlines overhang their
//! advances routinely — accents, swashes, italic descenders. Measured in
//! `tests/glyph_geometry.rs`: a Type 3 glyph whose advance box is 100…106 while its ink is
//! 124…130, so a region drawn tightly around the ink meets no part of the advance box.
//!
//! So [`Glyph::conservative_box`] is the advance box **unioned with the scaled `/FontBBox`**, and
//! that is what a region test uses. Uncertainty removes more rather than less.
//!
//! **A `/FontBBox` that lies is a residue**, stated rather than implied: a font may declare a box
//! its glyphs exceed and nothing in the file contradicts it. The oracle catches it in the suite;
//! at run time it belongs with the refusals.
//!
//! # Vertical writing is refused, not approximated
//!
//! An Identity-V font advances **downwards**, takes its metrics from `/W2` and `/DW2`, and
//! displaces the glyph origin by a vertical origin vector. Treating it as horizontal computes
//! every box in the wrong place, and in the dangerous direction: text inside the region gets
//! boxes outside it and is missed. [`Error::Unsupported`] rather than a wrong answer.

use burrow_types::{Error, Result};

/// A 2-D affine transform, in PDF's `[a b c d e f]` order.
///
/// ```text
/// | a  b  0 |
/// | c  d  0 |
/// | e  f  1 |
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix {
    /// x scale.
    pub a: f64,
    /// y skew.
    pub b: f64,
    /// x skew.
    pub c: f64,
    /// y scale.
    pub d: f64,
    /// x translation.
    pub e: f64,
    /// y translation.
    pub f: f64,
}

impl Matrix {
    /// The identity.
    pub const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    /// `self` then `then` — PDF's order, in which the left operand applies first.
    ///
    /// **The order is the whole of it.** `cm` premultiplies the CTM, `Tm` replaces the text
    /// matrix, and a Form's `/Matrix` composes with the CTM at the `Do`. Getting any of those
    /// backwards puts glyphs somewhere plausible and wrong, which is why every one of them has
    /// its own fixture rather than being read off this comment.
    #[must_use]
    pub const fn then(&self, then: &Self) -> Self {
        Self {
            a: self.a * then.a + self.b * then.c,
            b: self.a * then.b + self.b * then.d,
            c: self.c * then.a + self.d * then.c,
            d: self.c * then.b + self.d * then.d,
            e: self.e * then.a + self.f * then.c + then.e,
            f: self.e * then.b + self.f * then.d + then.f,
        }
    }

    /// Where `(x, y)` lands.
    #[must_use]
    pub const fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    /// A translation.
    #[must_use]
    pub const fn translate(x: f64, y: f64) -> Self {
        Self {
            e: x,
            f: y,
            ..Self::IDENTITY
        }
    }

    /// A scale.
    #[must_use]
    pub const fn scale(x: f64, y: f64) -> Self {
        Self {
            a: x,
            d: y,
            ..Self::IDENTITY
        }
    }
}

/// A rectangle in the space its producer was working in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    /// Smallest x.
    pub left: f64,
    /// Smallest y.
    pub bottom: f64,
    /// Largest x.
    pub right: f64,
    /// Largest y.
    pub top: f64,
}

impl Rect {
    /// The smallest rectangle containing both.
    #[must_use]
    pub fn union(&self, other: &Self) -> Self {
        Self {
            left: self.left.min(other.left),
            bottom: self.bottom.min(other.bottom),
            right: self.right.max(other.right),
            top: self.top.max(other.top),
        }
    }

    /// The axis-aligned box containing this rectangle's four corners after `by`.
    ///
    /// **The corners, not the two opposite ones.** A rotated or skewed transform sends a
    /// rectangle to a parallelogram, and taking `apply` of only `(left, bottom)` and
    /// `(right, top)` produces a box that misses two of its corners — smaller than the shape it
    /// claims to contain, which for a conservative box is the failing direction.
    #[must_use]
    pub fn transformed(&self, by: &Matrix) -> Self {
        let corners = [
            by.apply(self.left, self.bottom),
            by.apply(self.right, self.bottom),
            by.apply(self.left, self.top),
            by.apply(self.right, self.top),
        ];
        let mut out = Self {
            left: f64::INFINITY,
            bottom: f64::INFINITY,
            right: f64::NEG_INFINITY,
            top: f64::NEG_INFINITY,
        };
        for (x, y) in corners {
            out.left = out.left.min(x);
            out.right = out.right.max(x);
            out.bottom = out.bottom.min(y);
            out.top = out.top.max(y);
        }
        out
    }

    /// Whether the two overlap, edges excluded.
    #[must_use]
    pub fn intersects(&self, other: &Self) -> bool {
        self.left < other.right
            && other.left < self.right
            && self.bottom < other.top
            && other.bottom < self.top
    }
}

/// Which way a font lays its glyphs out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WritingMode {
    /// `WMode 0` — advances along x.
    Horizontal,
    /// `WMode 1` — advances down y. Refused; see the module header.
    Vertical,
}

/// The parameters of PDF's text state that move a glyph without changing what it says.
///
/// Every field here is something a content stream sets and a later glyph reads. They are
/// separate from [`TextPosition`] because the specification is: `BT` resets the matrices and
/// leaves these alone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextState {
    /// `Tf`'s size.
    pub font_size: f64,
    /// `Tc`, added to every glyph's advance.
    pub char_spacing: f64,
    /// `Tw`, added to the advance of **single-byte code 32 only**.
    ///
    /// See [`Glyph::advance`] for why that qualification is load-bearing.
    pub word_spacing: f64,
    /// `Tz`, as a percentage — 100 is unscaled.
    pub horizontal_scale: f64,
    /// `Ts`, the rise above the baseline.
    pub rise: f64,
    /// `TL`, the leading `T*` and `TD` use.
    pub leading: f64,
}

impl Default for TextState {
    fn default() -> Self {
        // THE SPECIFICATION'S INITIAL VALUES, not zeroes. `Tz` defaults to 100 and not 0 -- a
        // zero horizontal scale collapses every glyph to a point, which is a box that intersects
        // almost nothing and so a redaction that removes almost nothing.
        Self {
            font_size: 0.0,
            char_spacing: 0.0,
            word_spacing: 0.0,
            horizontal_scale: 100.0,
            rise: 0.0,
            leading: 0.0,
        }
    }
}

/// The two matrices `BT` resets and the text operators move.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextPosition {
    /// `Tm`, where the next glyph goes.
    pub text: Matrix,
    /// The line matrix `Td`, `TD` and `T*` are relative to.
    pub line: Matrix,
}

impl Default for TextPosition {
    fn default() -> Self {
        Self {
            text: Matrix::IDENTITY,
            line: Matrix::IDENTITY,
        }
    }
}

impl TextPosition {
    /// `Td` — move to the start of the next line, offset from the **line** matrix.
    ///
    /// **From the line matrix, not the text matrix.** Applying it to the text matrix compounds
    /// every offset along a run, so the second `Td` lands twice as far as it should. Its own
    /// fixture, because the two are the same for a single `Td` and differ for every one after.
    pub fn next_line_at(&mut self, tx: f64, ty: f64) {
        self.line = Matrix::translate(tx, ty).then(&self.line);
        self.text = self.line;
    }
}

/// One glyph, placed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glyph {
    /// The pen position in page space — what `FPDFText_GetCharOrigin` reports.
    pub origin: (f64, f64),
    /// The transform from glyph space to page space, for boxing the glyph's own extents.
    pub to_page: Matrix,
    /// The advance this glyph contributes, in unscaled text-space units.
    pub advance: f64,
    /// The font's `/FontBBox`, in glyph space, if the font declared one.
    pub font_bbox: Option<Rect>,
    /// The font size in force, for the advance box's height.
    pub font_size: f64,
}

impl Glyph {
    /// The box a region test must use: the advance box **unioned with the scaled `/FontBBox`**.
    ///
    /// See the module header for the measurement this exists for. A font that declares no
    /// `/FontBBox` gets the advance box alone, which is the best available answer and is
    /// recorded as such rather than silently treated as complete.
    #[must_use]
    pub fn conservative_box(&self) -> Rect {
        let advance = Rect {
            left: 0.0,
            bottom: 0.0,
            right: self.advance,
            top: self.font_size,
        }
        .transformed(&self.to_page);
        match self.font_bbox {
            Some(bbox) => advance.union(&bbox.transformed(&self.to_page)),
            None => advance,
        }
    }
}

/// Whether `code` takes word spacing.
///
/// # The qualification is the whole function
///
/// PDF 32000-1 §9.3.3: word spacing applies to **byte 32 in a single-byte encoding**, and to
/// nothing else. A two-byte CID font whose code happens to be `0x0020` does **not** take it —
/// that code is a glyph selector, not a space, and the byte 32 inside it is half of a number.
///
/// Applying `Tw` there moves every glyph after it along the run by the word spacing, so the
/// error compounds: a page with a `Tw` of 3 and twenty such codes ends up sixty points out.
/// It is the classic miss in this corner of the specification, and it has its own fixture.
#[must_use]
pub const fn takes_word_spacing(code: u32, bytes_per_code: u8) -> bool {
    bytes_per_code == 1 && code == 32
}

/// Refuse a vertical writing mode rather than computing boxes in the wrong place.
///
/// # Errors
///
/// [`Error::Unsupported`] for [`WritingMode::Vertical`]. See the module header.
pub const fn check_writing_mode(mode: WritingMode) -> Result<()> {
    match mode {
        WritingMode::Horizontal => Ok(()),
        WritingMode::Vertical => Err(Error::Unsupported(String::new())),
    }
}

#[cfg(test)]
mod tests {
    use super::{Matrix, Rect, TextPosition, TextState, takes_word_spacing};

    #[test]
    fn composition_applies_the_left_operand_first() {
        // Scale then translate is not translate then scale, and the difference is where every
        // glyph on a page with a `cm` lands.
        let scale = Matrix::scale(2.0, 2.0);
        let shift = Matrix::translate(10.0, 0.0);
        assert_eq!(scale.then(&shift).apply(1.0, 0.0), (12.0, 0.0));
        assert_eq!(shift.then(&scale).apply(1.0, 0.0), (22.0, 0.0));
    }

    #[test]
    fn a_rotated_box_keeps_all_four_corners() {
        // THE FAILING DIRECTION FOR A CONSERVATIVE BOX. Transforming only two opposite corners
        // of a rectangle under a rotation gives a box that does not contain the other two --
        // smaller than the shape it claims to hold, which is how ink ends up outside it.
        let cos45 = core::f64::consts::FRAC_1_SQRT_2;
        let rotate_45 = Matrix {
            a: cos45,
            b: cos45,
            c: -cos45,
            d: cos45,
            e: 0.0,
            f: 0.0,
        };
        let unit = Rect {
            left: 0.0,
            bottom: 0.0,
            right: 1.0,
            top: 1.0,
        };
        let boxed = unit.transformed(&rotate_45);

        // The rotated unit square spans -cos45..cos45 in x and 0..2*cos45 in y. A two-corner
        // implementation would report 0..0 in x, which contains none of it.
        assert!((boxed.left + cos45).abs() < 1e-9, "left was {}", boxed.left);
        assert!(
            (boxed.right - cos45).abs() < 1e-9,
            "right was {}",
            boxed.right
        );
        assert!(
            (boxed.top - cos45 * 2.0).abs() < 1e-9,
            "top was {}",
            boxed.top
        );
    }

    #[test]
    fn td_is_relative_to_the_line_matrix_and_not_the_text_matrix() {
        // The two agree for one `Td` and diverge for every one after, so a single-offset test
        // would pass against the wrong implementation.
        let mut position = TextPosition::default();
        position.next_line_at(10.0, 0.0);
        position.next_line_at(10.0, 0.0);
        assert_eq!(position.text.apply(0.0, 0.0), (20.0, 0.0));
    }

    #[test]
    fn horizontal_scale_defaults_to_a_hundred_rather_than_zero() {
        // A zero `Tz` collapses every glyph to a point, and a box that is a point intersects
        // almost nothing -- so the defect reads as "the redaction removed nothing" rather than
        // as a crash.
        assert_eq!(TextState::default().horizontal_scale, 100.0);
    }

    #[test]
    fn word_spacing_applies_to_single_byte_thirty_two_and_nothing_else() {
        assert!(takes_word_spacing(32, 1));
        // A two-byte code that happens to equal 32 is a glyph selector, not a space.
        assert!(!takes_word_spacing(32, 2));
        assert!(!takes_word_spacing(0x0020, 2));
        assert!(!takes_word_spacing(33, 1));
    }
}

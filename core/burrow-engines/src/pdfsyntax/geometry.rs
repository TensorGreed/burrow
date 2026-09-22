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
//! # Vertical writing is refused, and it is keyed on `WMode` rather than on a name
//!
//! A vertical font advances **downwards**, takes its metrics from `/W2` and `/DW2`, and
//! displaces the glyph origin by a vertical origin vector. Treating it as horizontal computes
//! every box in the wrong place, and in the dangerous direction: text inside the region gets
//! boxes outside it and is missed. A refusal rather than a wrong answer.
//!
//! **The name `Identity-V` is not the key.** It is the obvious check and it is wrong twice over:
//! it misses every other vertical CMap in Adobe's registry (`UniJIS-UCS2-V`, `90ms-RKSJ-V`,
//! `ETen-B5-V`, …), and it misses an *embedded* CMap entirely, because there the producer names
//! the stream. A CMap called `/Identity-H` that declares `/WMode 1 def` is vertical. So
//! [`writing_mode_of`] reads `WMode` — from the stream dictionary, from the program, or
//! inherited through `usecmap` — and [`CMap::Embedded`] does not carry a name at all, so it
//! cannot be keyed on by accident.
//!
//! # And a vertical document need not say any of that
//!
//! Measured: asked for a vertically-written Japanese paragraph, LibreOffice emitted a **subset
//! simple font and one `Tm` per glyph**, stepping `y` down the page. No CID font, no `WMode`,
//! nothing to refuse — and the ordinary horizontal walk places every glyph correctly.
//! `tests/glyph_geometry.rs` pins that shape. The `WMode` refusal covers the other case, and
//! saying so here is the difference between a bound and a hope.

use burrow_types::{Error, Result};

use super::ops::{Operand, Operation};

/// Why a walk refused, named.
///
/// # Why this exists, and why it is an enum
///
/// Three times in this milestone a refusal test accepted *some* refusal rather than *the*
/// refusal, and twice that hid a deleted defence: the Form-XObject depth cap refuses a
/// self-drawing form too, so `matches!(outcome, Err(Error::Unsupported(_)))` stayed green with
/// cycle detection removed. Asserting on a hand-written substring fixes the test and not the
/// habit — the next test still has a shorter, looser spelling available.
///
/// So the rule name is a value rather than a string. Every refusal in this module is built
/// through [`Refusal::refuse`], which stamps the rule into the message; every refusal test
/// names the variant it expects. The loose assertion is now the *longer* one to write, and
/// `refusals_are_the_only_way_to_refuse` fails the build if a bare `Error::Malformed` or
/// `Error::Unsupported` reappears here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Refusal {
    /// A `Q` with nothing on the stack.
    UnmatchedRestore,
    /// `q` without a matching `Q` by the end of the stream.
    UnbalancedSave,
    /// A `BT` inside a text object.
    NestedTextObject,
    /// A `BT` the stream ends without closing.
    UnterminatedTextObject,
    /// An `ET` with no `BT`.
    UnmatchedEndText,
    /// A text operator outside `BT` … `ET`.
    TextOutsideTextObject,
    /// A `Do` whose operand is not a name.
    FormOperandNotAName,
    /// A `TJ` whose operand is not an array.
    ShowArrayOperandNotAnArray,
    /// A `TJ` array item that is neither a string nor a number.
    ShowArrayItemNotShowable,
    /// A text-showing operator whose operand is not a string.
    ShowOperandNotAString,
    /// Text shown with no font selected by `Tf`.
    NoFontSelected,
    /// A font claiming zero bytes per code.
    ZeroBytesPerCode,
    /// A Form XObject that draws itself, directly or through another form.
    FormCycle,
    /// Form XObjects nested deeper than [`MAX_FORM_DEPTH`].
    FormDepth,
    /// A font whose CMap declares vertical writing (`WMode 1`).
    VerticalWriting,
    /// A CMap whose writing mode the document does not determine.
    UndeterminedWritingMode,
}

impl Refusal {
    /// Every variant, so the uniqueness and message tests cannot drift from the enum.
    pub const ALL: &'static [Self] = &[
        Self::UnmatchedRestore,
        Self::UnbalancedSave,
        Self::NestedTextObject,
        Self::UnterminatedTextObject,
        Self::UnmatchedEndText,
        Self::TextOutsideTextObject,
        Self::FormOperandNotAName,
        Self::ShowArrayOperandNotAnArray,
        Self::ShowArrayItemNotShowable,
        Self::ShowOperandNotAString,
        Self::NoFontSelected,
        Self::ZeroBytesPerCode,
        Self::FormCycle,
        Self::FormDepth,
        Self::VerticalWriting,
        Self::UndeterminedWritingMode,
    ];

    /// The rule's name, as it appears in the error message.
    #[must_use]
    pub const fn rule(self) -> &'static str {
        match self {
            Self::UnmatchedRestore => "q-without-save",
            Self::UnbalancedSave => "q-unbalanced",
            Self::NestedTextObject => "bt-nested",
            Self::UnterminatedTextObject => "bt-unterminated",
            Self::UnmatchedEndText => "et-without-bt",
            Self::TextOutsideTextObject => "text-outside-text-object",
            Self::FormOperandNotAName => "do-operand-not-a-name",
            Self::ShowArrayOperandNotAnArray => "tj-operand-not-an-array",
            Self::ShowArrayItemNotShowable => "tj-array-item-not-showable",
            Self::ShowOperandNotAString => "show-operand-not-a-string",
            Self::NoFontSelected => "no-font-selected",
            Self::ZeroBytesPerCode => "zero-bytes-per-code",
            Self::FormCycle => "form-cycle",
            Self::FormDepth => "form-depth",
            Self::VerticalWriting => "vertical-writing",
            Self::UndeterminedWritingMode => "writing-mode-undetermined",
        }
    }

    /// Whether this is a refusal to walk a well-formed file, rather than a malformed one.
    const fn is_unsupported(self) -> bool {
        matches!(
            self,
            Self::FormCycle | Self::FormDepth | Self::VerticalWriting
        )
    }

    /// Build the refusal, with the rule name stamped into the message.
    fn refuse<T>(self, detail: &str) -> Result<T> {
        // PROBE-EXEMPT BEGIN -- `refuse` and `caught` are this module's entire `Error`
        // vocabulary: one builds the two variants, the other matches on them, and a probe that
        // could tell a construction from a pattern would be a parser.
        // `refusals_are_the_only_way_to_refuse_in_this_module` skips exactly this window and
        // checks every other line in the file, production and tests alike.
        let message = format!("pdf geometry [{}]: {detail}", self.rule());
        Err(if self.is_unsupported() {
            Error::Unsupported(message)
        } else {
            Error::Malformed(message)
        })
    }

    /// Whether `error` is this refusal.
    ///
    /// Public so the integration suites assert on the rule too, rather than on the variant of
    /// [`Error`] — which every refusal here shares with a dozen others.
    #[must_use]
    pub fn caught(self, error: &Error) -> bool {
        // (still inside the probe-exempt window: see `refuse`)
        let message = match error {
            Error::Malformed(message) if !self.is_unsupported() => message,
            Error::Unsupported(message) if self.is_unsupported() => message,
            _ => return false,
        };
        message.contains(&format!("[{}]", self.rule()))
        // PROBE-EXEMPT END
    }
}

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
pub fn check_writing_mode(mode: WritingMode) -> Result<()> {
    match mode {
        WritingMode::Horizontal => Ok(()),
        // KEYED ON `WMode`, NOT ON THE NAME. See `WritingMode::from_wmode`.
        WritingMode::Vertical => Refusal::VerticalWriting.refuse(
            "the font's CMap declares 'WMode 1', and burrow places vertical runs nowhere              rather than somewhere wrong",
        ),
    }
}

// ---- where the writing mode actually comes from ---------------------------------------------

/// A font's CMap, as the file gives it.
///
/// # The name is not the key, and in one arm it is not even representable
///
/// `Identity-V` is the CMap everyone reaches for when writing this check, and keying on it is
/// wrong in both directions. It misses every other vertical CMap in Adobe's registry —
/// `UniJIS-UCS2-V`, `90ms-RKSJ-V`, `ETen-B5-V` and a dozen more — and, worse, it misses an
/// **embedded** CMap, whose name the producer chooses. A stream called `/Whatever` that declares
/// `/WMode 1 def` is vertical, and a stream called `/Identity-H` that declares `/WMode 1 def` is
/// vertical too. There is a fixture for each.
///
/// So the two cases are different types rather than one string:
///
/// - [`Self::Predefined`] carries a name **because the name is all the file carries**. The
///   program lives in Adobe's registry, not in the document, so the name cannot lie about it: it
///   either resolves to a published CMap or it does not resolve at all.
/// - [`Self::Embedded`] carries **no name at all**. The program is right there, so the name is
///   both untrustworthy and unnecessary — and leaving it out of the type means a future reader
///   cannot key on it by accident. That is the shape of the whole finding, expressed where the
///   compiler can hold it.
#[derive(Debug, Clone, Copy)]
pub enum CMap<'a> {
    /// A predefined CMap, referenced by registry name. The document carries no program.
    Predefined(&'a [u8]),
    /// An embedded CMap stream: its dictionary's `/WMode`, if present, and its program.
    Embedded {
        /// `/WMode` on the CMap **stream dictionary**, if the file states one.
        dictionary_wmode: Option<i64>,
        /// The CMap program itself, between `begincmap` and `endcmap`.
        program: &'a [u8],
    },
}

impl WritingMode {
    /// PDF 32000-1 §9.7.5.1: `WMode` is 0 for horizontal and 1 for vertical.
    ///
    /// # Errors
    ///
    /// [`Refusal::UndeterminedWritingMode`] for any other value. A third mode is not something
    /// to guess at: the guess that it means horizontal is the one that places vertical text
    /// where it is not and misses it.
    pub fn from_wmode(wmode: i64) -> Result<Self> {
        match wmode {
            0 => Ok(Self::Horizontal),
            1 => Ok(Self::Vertical),
            other => Refusal::UndeterminedWritingMode.refuse(&format!(
                "a CMap declaring 'WMode {other}', which is neither 0 nor 1"
            )),
        }
    }
}

/// Derive a CMap's writing mode from what the file actually says.
///
/// # Errors
///
/// [`Refusal::UndeterminedWritingMode`] when the mode cannot be derived — a predefined name
/// outside the registry's `-H`/`-V` convention, a `usecmap` of such a name, or a stream
/// dictionary and program that disagree. Vertical writing is refused later, by
/// [`check_writing_mode`], so that the two questions stay separate: *what does the file say* and
/// *what will burrow do about it*.
pub fn writing_mode_of(cmap: &CMap<'_>) -> Result<WritingMode> {
    match *cmap {
        CMap::Predefined(name) => predefined_writing_mode(name),
        CMap::Embedded {
            dictionary_wmode,
            program,
        } => {
            let declared = match dictionary_wmode {
                Some(wmode) => Some(WritingMode::from_wmode(wmode)?),
                None => None,
            };
            let in_program = program_writing_mode(program)?;
            match (declared, in_program) {
                // THEY DISAGREE, so the file says two things. §9.7.5.1 requires them to match,
                // and picking one means picking which half of a contradiction to believe.
                (Some(a), Some(b)) if a != b => Refusal::UndeterminedWritingMode.refuse(
                    "a CMap whose stream dictionary and whose program declare different \
                     writing modes",
                ),
                (Some(mode), _) | (None, Some(mode)) => Ok(mode),
                // ABSENT MEANS HORIZONTAL, per the specification's default of 0. A vertical
                // CMap has to say so; this is the one place the absence of a statement is
                // allowed to settle the question, and it is allowed because the specification
                // settles it rather than because nothing was found.
                (None, None) => Ok(WritingMode::Horizontal),
            }
        }
    }
}

/// A predefined CMap's mode, from the registry's naming convention.
fn predefined_writing_mode(name: &[u8]) -> Result<WritingMode> {
    // Every predefined CMap in Adobe's registry ends `-H` or `-V`, and that suffix *is* the
    // mode. Trusting a name is sound HERE and nowhere else: a predefined CMap's program is not
    // in the document, so the name is a reference rather than a claim about bytes beside it.
    if name.ends_with(b"-V") {
        return Ok(WritingMode::Vertical);
    }
    if name.ends_with(b"-H") {
        return Ok(WritingMode::Horizontal);
    }
    Refusal::UndeterminedWritingMode.refuse(
        "a predefined CMap whose name follows neither the '-H' nor the '-V' convention, so its \
         writing mode is not derivable from the document",
    )
}

/// `/WMode` as the CMap program declares it, and `usecmap` as a way of inheriting one.
fn program_writing_mode(program: &[u8]) -> Result<Option<WritingMode>> {
    // TOKENISED, NOT SEARCHED. `/WMode` inside a string literal or after a `%` comment is not a
    // declaration, and a byte scan cannot tell the difference -- the lexer already can, and its
    // `a_string_hides_what_looks_like_a_name` test is exactly this case.
    let mut lexer = super::lexer::Lexer::new(program);
    let mut previous_name: Option<Vec<u8>> = None;
    let mut wanted_number = false;
    let mut declared: Option<WritingMode> = None;
    let mut inherited: Option<WritingMode> = None;

    while let Some(token) = lexer.next_token()? {
        let span = lexer.span();
        if wanted_number {
            wanted_number = false;
            if let super::lexer::Token::Number = token {
                let digits = program.get(span.0..span.1).unwrap_or_default();
                let text = core::str::from_utf8(digits).unwrap_or_default();
                let Ok(value) = text.parse::<i64>() else {
                    return Refusal::UndeterminedWritingMode
                        .refuse("a 'WMode' whose value is not an integer");
                };
                declared = Some(WritingMode::from_wmode(value)?);
                continue;
            }
            // A `WMode` FOLLOWED BY SOMETHING THAT IS NOT A NUMBER is a writing mode the
            // document states and this code cannot read. Passing over it reports the CMap as
            // horizontal on the strength of having failed to parse it, which is the one reading
            // that places vertical text where it is not.
            return Refusal::UndeterminedWritingMode
                .refuse("a 'WMode' whose value is not a number");
        }
        match token {
            super::lexer::Token::Name(name) => {
                if name == b"WMode" {
                    wanted_number = true;
                }
                previous_name = Some(name);
            }
            // `/UniJIS-UCS2-V usecmap` INHERITS A VERTICAL MODE WITHOUT DECLARING ONE. An
            // embedded CMap that does this and states no `/WMode` of its own is vertical, and a
            // check that read only `/WMode` would pass it as horizontal.
            super::lexer::Token::Keyword(word) if word == b"usecmap" => {
                let Some(name) = previous_name.take() else {
                    return Refusal::UndeterminedWritingMode
                        .refuse("a 'usecmap' with no CMap named before it");
                };
                inherited = Some(predefined_writing_mode(&name)?);
            }
            _ => previous_name = None,
        }
    }
    if wanted_number {
        return Refusal::UndeterminedWritingMode
            .refuse("a 'WMode' at the end of a CMap program, with no value after it");
    }
    Ok(declared.or(inherited))
}

// ---- the content-stream walk ---------------------------------------------------------------

/// How deep Form XObjects may nest before the walk refuses.
///
/// A form may draw another form, and nothing in a file bounds that. Matching
/// [`MAX_NESTING`](super::ops) would be arbitrary here — that one is about brackets — so this is
/// its own number: deeper than any producer nests (a form inside a form inside a stamp is three)
/// and shallow enough that the work is bounded well before the stack is.
pub const MAX_FORM_DEPTH: usize = 16;

/// A Form XObject, resolved.
#[derive(Debug, Clone)]
pub struct Form {
    /// The object's identity — object number and generation, packed as qpdf packs them.
    ///
    /// **Cycle detection keys on this, not on the name.** The same name means different objects
    /// at different nesting levels, because each form carries its own `/Resources`; a walk that
    /// remembered names would refuse a legitimate document that reuses `/Fm0` inside a form, and
    /// would miss a cycle that reaches the same object by two names.
    pub id: u64,
    /// The form's `/Matrix`, composed with the CTM at the `Do`.
    pub matrix: Matrix,
    /// Its decoded content stream.
    pub content: Vec<u8>,
}

/// What a glyph's font says about it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphMetrics {
    /// The advance, in glyph-space units before the font matrix.
    pub width: f64,
    /// How many bytes of the string this code took — 1 for a simple font, 2 for Identity-H.
    ///
    /// Carried because word spacing depends on it; see [`takes_word_spacing`].
    pub bytes_per_code: u8,
    /// The font's `/FontBBox`, in glyph space.
    pub font_bbox: Option<Rect>,
    /// Glyph space to text space — `/FontMatrix` for a Type 3, 0.001 otherwise.
    pub font_matrix: Matrix,
    /// Which way the font lays glyphs out.
    pub writing_mode: WritingMode,
}

/// What the walk needs from the document, so `pdfsyntax` still holds none.
///
/// The rest of this module is document-free by design — it lexes bytes and composes matrices.
/// Resolving `/Fm0` to a stream, or `/F1` to a width, needs the object graph, so those arrive
/// through here rather than dragging a document into the tokeniser.
pub trait Resources {
    /// The Form XObject named `name`, or `None` if `Do` names something that is not one.
    ///
    /// An image `Do` is the ordinary `None`: it draws no glyphs, so the walk steps over it.
    ///
    /// # Errors
    ///
    /// Whatever resolving the object failed with.
    fn form(&self, name: &[u8]) -> Result<Option<Form>>;

    /// The metrics for `code` in the font named `name`.
    ///
    /// # Errors
    ///
    /// Whatever resolving the font failed with, and [`Error::Malformed`] if the content stream
    /// selects a font the resources do not have.
    fn glyph(&self, name: &[u8], code: u32) -> Result<GlyphMetrics>;

    /// How many bytes per code the font named `name` uses, to split a string into codes.
    ///
    /// # Errors
    ///
    /// As [`Self::glyph`].
    fn bytes_per_code(&self, name: &[u8]) -> Result<u8>;
}

/// Everything `q` saves and `Q` restores.
///
/// **The text state is in here, and that is the part that is easy to get wrong.** `Tc`, `Tw`,
/// `Tz`, `TL`, `Tf`, `Tr` and `Ts` are graphics-state parameters, so `q` saves them with the CTM
/// — a walk that saved only the matrix would let a `Tz` set inside `q`…`Q` escape and scale
/// every glyph after it.
#[derive(Debug, Clone)]
struct GraphicsState {
    ctm: Matrix,
    text: TextState,
    font: Option<Vec<u8>>,
}

/// Walk a content stream and place every glyph it draws.
///
/// # Every early exit is a refusal
///
/// A walk that stops partway has not examined the rest of the page, and a redaction built on a
/// partial glyph list removes what it saw and leaves what it did not. So there is no path out of
/// here that returns the glyphs collected so far: an unbalanced `q`/`Q`, an unterminated `BT`, a
/// text operator outside a text object, a form cycle and a form nested past
/// [`MAX_FORM_DEPTH`] are all [`Err`].
///
/// # Errors
///
/// - [`Error::Malformed`] — the stream will not tokenise, brackets or text objects do not
///   balance, or a text operator appears outside `BT`…`ET`.
/// - [`Error::Unsupported`] — a form cycle, a form nested too deep, or a vertical writing mode.
pub fn glyphs_in(content: &[u8], resources: &dyn Resources) -> Result<Vec<Glyph>> {
    let mut out = Vec::new();
    let mut open_forms = Vec::new();
    walk(
        content,
        resources,
        GraphicsState {
            ctm: Matrix::IDENTITY,
            text: TextState::default(),
            font: None,
        },
        &mut open_forms,
        &mut out,
    )?;
    Ok(out)
}

/// One content stream, at one nesting level.
fn walk(
    content: &[u8],
    resources: &dyn Resources,
    // THE WHOLE GRAPHICS STATE, not the CTM and the text parameters separately. The SELECTED
    // FONT is part of it: `Tf` sets the font and the size together, and a form that inherited
    // the size but not the font refused every string it drew. Found by the form-inheritance
    // test, which is the only place the two are set in different streams.
    initial: GraphicsState,
    open_forms: &mut Vec<u64>,
    out: &mut Vec<Glyph>,
) -> Result<()> {
    let operations = super::ops::operations(content)?;

    let mut state = initial;
    let mut stack: Vec<GraphicsState> = Vec::new();
    // `Some` between `BT` and `ET`. The matrices live here rather than in `GraphicsState`
    // because `q`/`Q` do NOT save them -- they are reset by `BT` and nothing else touches them.
    let mut position: Option<TextPosition> = None;

    for operation in &operations {
        let number = |at: usize| -> f64 {
            operation
                .operands
                .get(at)
                .and_then(Operand::as_number)
                .unwrap_or(0.0)
        };
        match operation.operator.as_slice() {
            b"q" => stack.push(state.clone()),
            b"Q" => {
                // A `Q` WITH NOTHING SAVED IS A REFUSAL. Tolerating it means guessing what the
                // producer meant by it, and every guess puts later glyphs somewhere.
                state = match stack.pop() {
                    Some(saved) => saved,
                    None => {
                        return Refusal::UnmatchedRestore.refuse(
                            "a 'Q' with no matching 'q', so the graphics state after it is \
                             undefined",
                        );
                    }
                };
            }
            b"cm" => {
                let m = Matrix {
                    a: number(0),
                    b: number(1),
                    c: number(2),
                    d: number(3),
                    e: number(4),
                    f: number(5),
                };
                state.ctm = m.then(&state.ctm);
            }
            b"BT" => {
                if position.is_some() {
                    return Refusal::NestedTextObject.refuse(
                        "a 'BT' inside a text object, which the specification does not allow to \
                         nest",
                    );
                }
                // ONLY THE MATRICES. `Tc`, `Tw`, `Tz`, `TL`, `Tf`, `Tr` and `Ts` are graphics
                // state and survive `BT` -- a walk that reset them would place every glyph in a
                // second text object as though the first had never set anything.
                position = Some(TextPosition::default());
            }
            b"ET" => {
                if position.take().is_none() {
                    return Refusal::UnmatchedEndText.refuse("an 'ET' with no 'BT'");
                }
            }
            b"Tc" => state.text.char_spacing = number(0),
            b"Tw" => state.text.word_spacing = number(0),
            b"Tz" => state.text.horizontal_scale = number(0),
            b"TL" => state.text.leading = number(0),
            b"Ts" => state.text.rise = number(0),
            b"Tf" => {
                state.text.font_size = number(1);
                state.font = match operation.operands.first() {
                    Some(Operand::Name { value, .. }) => Some(value.clone()),
                    _ => None,
                };
            }
            b"Do" => {
                let Some(Operand::Name { value, .. }) = operation.operands.first() else {
                    return Refusal::FormOperandNotAName
                        .refuse("a 'Do' whose operand is not a name");
                };
                draw_form(value, resources, &state, open_forms, out)?;
            }
            // The text-placing and text-showing operators, which need a text object.
            b"Tm" | b"Td" | b"TD" | b"T*" | b"Tj" | b"TJ" | b"'" | b"\"" => {
                let Some(place) = position.as_mut() else {
                    // OUTSIDE A TEXT OBJECT IS A REFUSAL, not a no-op. There is no text matrix
                    // to place against, so a walk that skipped it would silently not see
                    // whatever the operator draws.
                    return Refusal::TextOutsideTextObject
                        .refuse("a text operator outside a 'BT' ... 'ET' text object");
                };
                text_operator(content, operation, &mut state, place, resources, out)?;
            }
            _ => {}
        }
    }

    if !stack.is_empty() {
        return Refusal::UnbalancedSave.refuse(&format!(
            "{} unbalanced 'q' at the end of a content stream",
            stack.len()
        ));
    }
    if position.is_some() {
        return Refusal::UnterminatedTextObject
            .refuse("a 'BT' with no 'ET' -- the text object runs off the end of the stream");
    }
    Ok(())
}

/// `Do` on a Form XObject: compose, recurse, and refuse a cycle or an over-deep nest.
fn draw_form(
    name: &[u8],
    resources: &dyn Resources,
    state: &GraphicsState,
    open_forms: &mut Vec<u64>,
    out: &mut Vec<Glyph>,
) -> Result<()> {
    let Some(form) = resources.form(name)? else {
        // An image, or anything else that is not a form. It draws no glyphs.
        return Ok(());
    };

    // A CYCLE IS A REFUSAL, not a stop. A form that draws itself has no finite glyph list, and
    // returning the glyphs found before the loop was noticed would report a page as containing
    // less than it does. Keyed on object identity, not on the name -- see `Form::id`.
    if open_forms.contains(&form.id) {
        return Refusal::FormCycle
            .refuse("a Form XObject draws itself, directly or through another form");
    }
    if open_forms.len() >= MAX_FORM_DEPTH {
        return Refusal::FormDepth.refuse("Form XObjects nested deeper than burrow will walk");
    }

    open_forms.push(form.id);
    // THE FORM'S MATRIX COMPOSES WITH THE CTM AT THE `Do`, in that order. The other order puts
    // the form's own transform outside the page's, which is plausible and wrong.
    let result = walk(
        &form.content,
        resources,
        GraphicsState {
            ctm: form.matrix.then(&state.ctm),
            ..state.clone()
        },
        open_forms,
        out,
    );
    open_forms.pop();
    result
}

/// One text-placing or text-showing operator.
fn text_operator(
    content: &[u8],
    operation: &Operation,
    state: &mut GraphicsState,
    place: &mut TextPosition,
    resources: &dyn Resources,
    out: &mut Vec<Glyph>,
) -> Result<()> {
    let number = |at: usize| -> f64 {
        operation
            .operands
            .get(at)
            .and_then(Operand::as_number)
            .unwrap_or(0.0)
    };
    match operation.operator.as_slice() {
        b"Tm" => {
            let m = Matrix {
                a: number(0),
                b: number(1),
                c: number(2),
                d: number(3),
                e: number(4),
                f: number(5),
            };
            place.text = m;
            place.line = m;
        }
        b"Td" => place.next_line_at(number(0), number(1)),
        b"TD" => {
            // `TD` SETS THE LEADING TOO, to the NEGATIVE of its second operand. A walk that
            // treated it as `Td` would leave `TL` at whatever it was, so every later `T*` on the
            // page moves by the wrong amount.
            state.text.leading = -number(1);
            place.next_line_at(number(0), number(1));
        }
        b"T*" => place.next_line_at(0.0, -state.text.leading),
        b"'" => {
            place.next_line_at(0.0, -state.text.leading);
            show(
                content,
                operation.operands.first(),
                state,
                place,
                resources,
                out,
            )?;
        }
        b"\"" => {
            // `aw ac string "` sets word spacing, then character spacing, then does `'`.
            state.text.word_spacing = number(0);
            state.text.char_spacing = number(1);
            place.next_line_at(0.0, -state.text.leading);
            show(
                content,
                operation.operands.get(2),
                state,
                place,
                resources,
                out,
            )?;
        }
        b"Tj" => show(
            content,
            operation.operands.first(),
            state,
            place,
            resources,
            out,
        )?,
        b"TJ" => {
            let Some(Operand::Array { items, .. }) = operation.operands.first() else {
                return Refusal::ShowArrayOperandNotAnArray
                    .refuse("a 'TJ' whose operand is not an array");
            };
            for item in items {
                match item {
                    Operand::Str { .. } => {
                        show(content, Some(item), state, place, resources, out)?;
                    }
                    Operand::Number { value, .. } => {
                        // A KERN, in thousandths of text space, SUBTRACTED from the advance --
                        // and scaled by `Tz` like every other horizontal displacement. Adding it
                        // instead reverses every kern on the page, which moves glyphs by a
                        // point or two each: wrong, and small enough to look like rounding.
                        let shift = -value / 1000.0
                            * state.text.font_size
                            * (state.text.horizontal_scale / 100.0);
                        place.text = Matrix::translate(shift, 0.0).then(&place.text);
                    }
                    _ => {
                        return Refusal::ShowArrayItemNotShowable.refuse(
                            "a 'TJ' array holding something that is neither a string nor a \
                             number",
                        );
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Place every glyph of one shown string, advancing the text matrix as it goes.
fn show(
    content: &[u8],
    operand: Option<&Operand>,
    state: &GraphicsState,
    place: &mut TextPosition,
    resources: &dyn Resources,
    out: &mut Vec<Glyph>,
) -> Result<()> {
    let Some(Operand::Str { span }) = operand else {
        return Refusal::ShowOperandNotAString
            .refuse("a text-showing operator whose operand is not a string");
    };
    let Some(font) = state.font.clone() else {
        return Refusal::NoFontSelected.refuse("text shown with no font selected by 'Tf'");
    };
    // THE STRING'S BYTES, decoded from its span. `Token::Str` carries no value -- #128's note
    // that nothing `names_in_content` answers depends on what a string SAYS -- so the value
    // comes from slicing and decoding, which is exactly what that span exists for.
    let raw = content.get(span.0..span.1).ok_or_else(|| {
        Error::Internal("pdf geometry: a string's span left its content stream".to_owned())
    })?;
    let bytes = super::strings::decode_string(raw)?;
    let per_code = resources.bytes_per_code(&font)?;
    if per_code == 0 {
        return Refusal::ZeroBytesPerCode.refuse("a font claiming zero bytes per code");
    }

    for chunk in bytes.chunks(usize::from(per_code)) {
        let mut code = 0_u32;
        for byte in chunk {
            code = (code << 8) | u32::from(*byte);
        }
        let metrics = resources.glyph(&font, code)?;
        check_writing_mode(metrics.writing_mode)?;

        let scale = state.text.horizontal_scale / 100.0;
        // GLYPH SPACE -> TEXT SPACE -> PAGE SPACE, composed once per glyph. `Tz` scales x only
        // and the rise moves y only, and both sit between the font matrix and the text matrix.
        let to_page = metrics
            .font_matrix
            .then(&Matrix::scale(
                state.text.font_size * scale,
                state.text.font_size,
            ))
            .then(&Matrix::translate(0.0, state.text.rise))
            .then(&place.text)
            .then(&state.ctm);
        let origin = place.text.then(&state.ctm).apply(0.0, state.text.rise);

        let width = metrics.width * metrics.font_matrix.a;
        out.push(Glyph {
            origin,
            to_page,
            advance: width * state.text.font_size,
            font_bbox: metrics.font_bbox,
            font_size: state.text.font_size,
        });

        // THE ADVANCE, with word spacing applied ONLY to single-byte code 32. See
        // `takes_word_spacing` for what applying it to a two-byte 0x0020 costs.
        let word = if takes_word_spacing(code, metrics.bytes_per_code) {
            state.text.word_spacing
        } else {
            0.0
        };
        let advance = (width * state.text.font_size + state.text.char_spacing + word) * scale;
        place.text = Matrix::translate(advance, 0.0).then(&place.text);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use burrow_types::Result;

    use super::{
        CMap, Form, Glyph, GlyphMetrics, MAX_FORM_DEPTH, Matrix, Rect, Refusal, Resources,
        TextPosition, TextState, WritingMode, check_writing_mode, glyphs_in, takes_word_spacing,
        writing_mode_of,
    };

    /// A resources table with one font of known width and whatever forms a test names.
    ///
    /// Widths are 500/1000 so an advance is half the font size -- a number a reader can check in
    /// their head, which is what makes a failure message useful rather than a pair of decimals.
    struct Fake {
        forms: Vec<(Vec<u8>, Form)>,
        writing_mode: WritingMode,
        bytes_per_code: u8,
    }

    impl Fake {
        fn new() -> Self {
            Self {
                forms: Vec::new(),
                writing_mode: WritingMode::Horizontal,
                bytes_per_code: 1,
            }
        }

        fn with_form(mut self, name: &[u8], id: u64, matrix: Matrix, content: &str) -> Self {
            self.forms.push((
                name.to_vec(),
                Form {
                    id,
                    matrix,
                    content: content.as_bytes().to_vec(),
                },
            ));
            self
        }
    }

    impl Resources for Fake {
        fn form(&self, name: &[u8]) -> Result<Option<Form>> {
            Ok(self
                .forms
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, f)| f.clone()))
        }

        fn glyph(&self, _name: &[u8], _code: u32) -> Result<GlyphMetrics> {
            Ok(GlyphMetrics {
                width: 500.0,
                bytes_per_code: self.bytes_per_code,
                font_bbox: Some(Rect {
                    left: 0.0,
                    bottom: 0.0,
                    right: 1000.0,
                    top: 1000.0,
                }),
                font_matrix: Matrix::scale(0.001, 0.001),
                writing_mode: self.writing_mode,
            })
        }

        fn bytes_per_code(&self, _name: &[u8]) -> Result<u8> {
            Ok(self.bytes_per_code)
        }
    }

    fn placed(content: &str) -> Vec<Glyph> {
        glyphs_in(content.as_bytes(), &Fake::new()).expect("the fixture should walk")
    }

    // ---- the refusal names themselves are checked ------------------------------------------

    /// Every variant, spelled as it appears in source.
    ///
    /// Derived from `ALL` rather than listed again, so a variant added to the enum and to
    /// nothing else fails the two probes below instead of quietly joining an unchecked set.
    fn variant_names() -> Vec<String> {
        Refusal::ALL
            .iter()
            .map(|rule| format!("{rule:?}"))
            .collect()
    }

    #[test]
    fn every_rule_name_is_distinct() {
        // Two variants sharing a name makes `caught` accept the wrong one, which is exactly the
        // looseness this enum exists to remove.
        let mut names: Vec<&str> = Refusal::ALL.iter().map(|rule| rule.rule()).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "two refusals share a rule name");
        assert_eq!(
            total, 16,
            "a refusal was added or removed without updating the probes"
        );
    }

    #[test]
    fn a_rule_catches_its_own_refusal_and_rejects_every_other() {
        // THE PER-RULE PROBE. A `caught` that matched everything would make every refusal test
        // in this module vacuous, and a `caught` that matched nothing would make them all fail
        // for the wrong reason. Both directions, for all 15 x 15 pairs.
        for rule in Refusal::ALL {
            let error = rule
                .refuse::<()>("a detail")
                .expect_err("refuse must refuse");
            assert!(rule.caught(&error), "{} did not catch itself", rule.rule());
            for other in Refusal::ALL {
                if other != rule {
                    assert!(
                        !other.caught(&error),
                        "{} caught {}'s refusal",
                        other.rule(),
                        rule.rule()
                    );
                }
            }
        }
    }

    #[test]
    fn refusals_are_the_only_way_to_refuse_in_this_module() {
        // THE STRUCTURAL HALF. Naming rules helps only while every refusal has one; a single
        // bare `Error::Malformed` here would be a refusal no test could assert on, and the next
        // test written against it would fall back to the loose shape. So the module's own
        // source is the fixture.
        let source = include_str!("geometry.rs");
        // THE WHOLE FILE, minus one marked window. An earlier draft checked only the part
        // before `refuse` and the test module, which left everything after `refuse` -- most of
        // the module, including the CMap code added the same day -- unexamined. A check that
        // silently examines part of its subject reads as coverage.
        let begin = concat!("// PROBE-", "EXEMPT BEGIN");
        let end = concat!("// PROBE-", "EXEMPT END");
        let (before, rest) = source.split_once(begin).expect("the exempt window opens");
        let (exempt, after) = rest.split_once(end).expect("the exempt window closes");
        assert!(
            exempt.lines().count() <= 30,
            "the exempt window has grown past the two functions it is for: {} lines",
            exempt.lines().count()
        );
        for (half, label) in [(before, "before the exempt window"), (after, "after it")] {
            // COMMENTS ARE NOT CONSTRUCTION SITES. The doc on `Refusal` quotes the loose shape
            // in order to argue against it, and a probe that could not tell the two apart would
            // have to be weakened somewhere instead.
            for line in half.lines().filter(|l| !l.trim_start().starts_with("//")) {
                // SPELLED IN PIECES so this probe does not match itself -- it did, on the
                // second run, which is the same self-matching shape as `pgrep -f` in CLAUDE.md.
                for bare in [
                    concat!("Error::", "Malformed("),
                    concat!("Error::", "Unsupported("),
                ] {
                    assert!(
                        !line.contains(bare),
                        "a bare `{bare}` in {label}: build it through `Refusal` so a test can \
                         name it -- {line}"
                    );
                }
            }
        }
    }

    #[test]
    fn every_refusal_is_both_raised_and_tested() {
        // A variant that nothing raises is a rule that cannot fire; one that no test names is a
        // rule nobody checks. Neither is caught by the compiler.
        let source = include_str!("geometry.rs");
        let (production, tests) = source
            .split_once("mod tests {")
            .expect("the test module marks the split");
        // Whitespace-insensitive: rustfmt wraps `Refusal::X.refuse(..)` onto two lines when the
        // detail is long, and a probe that missed those would report the longest refusals as
        // unraised -- which it did, on the first run.
        let dense: String = production.chars().filter(|c| !c.is_whitespace()).collect();
        for name in variant_names() {
            assert!(
                dense.contains(&format!("Refusal::{name}.refuse(")),
                "`Refusal::{name}` is never raised"
            );
            assert!(
                tests.contains(&format!("Refusal::{name})"))
                    || tests.contains(&format!("Refusal::{name},")),
                "no test asserts `Refusal::{name}`"
            );
        }
    }

    /// Assert a walk was refused, **and by which rule**.
    ///
    /// There is deliberately no helper that asserts "refused somehow". Writing one out by hand
    /// is three lines of `matches!` against an `Error` variant a dozen rules share; naming the
    /// rule is one line. The shorter spelling is the correct one, which is the point -- the
    /// habit that produced three loose refusal tests in this milestone now produces the right
    /// check by default.
    #[track_caller]
    fn assert_refused(outcome: Result<Vec<Glyph>>, rule: Refusal) {
        match outcome {
            Err(error) => assert!(
                rule.caught(&error),
                "refused, but by a different rule: wanted `{}`, got {error:?}",
                rule.rule()
            ),
            Ok(glyphs) => panic!(
                "expected a refusal by `{}`, got {} glyphs",
                rule.rule(),
                glyphs.len()
            ),
        }
    }

    /// Walk a fixture against the default resources, for a walk that must be refused.
    #[track_caller]
    fn refusing(content: &str, rule: Refusal) {
        assert_refused(glyphs_in(content.as_bytes(), &Fake::new()), rule);
    }

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
    // ---- (1) text state persists across BT/ET; BT resets only the matrices ----------------

    #[test]
    fn text_state_survives_bt_while_the_matrices_do_not() {
        // `Tc`, `Tw`, `Tz`, `TL`, `Tf`, `Tr` and `Ts` are GRAPHICS state. A walk that reset them
        // at `BT` would place every glyph in the second text object as though the first had set
        // nothing -- and the page still renders, so nothing else would say.
        let glyphs = placed("/F1 10 Tf 4 Tz BT 100 700 Td (A) Tj ET BT 200 700 Td (A) Tj ET");
        assert_eq!(glyphs.len(), 2);

        // The matrices DID reset: the second glyph is at its own `Td`, not 200 past the first.
        assert!(
            (glyphs[1].origin.0 - 200.0).abs() < 1e-9,
            "{:?}",
            glyphs[1].origin
        );

        // And the state did NOT: both glyphs carry the 4% horizontal scale, so both boxes are
        // the same narrow width. A reset `Tz` would give the second one a box 25x wider.
        let width = |g: &Glyph| g.conservative_box().right - g.conservative_box().left;
        assert!(
            (width(&glyphs[0]) - width(&glyphs[1])).abs() < 1e-9,
            "the second text object lost the Tz: {} vs {}",
            width(&glyphs[0]),
            width(&glyphs[1])
        );
    }

    #[test]
    fn bt_resets_the_line_matrix_as_well_as_the_text_matrix() {
        // Resetting only `Tm` leaves `Tlm` holding the previous object's position, so the first
        // `Td` of the second object lands relative to the old line rather than the origin.
        let glyphs = placed("/F1 10 Tf BT 100 700 Td (A) Tj ET BT 5 0 Td (A) Tj ET");
        assert_eq!(glyphs.len(), 2);
        assert!(
            (glyphs[1].origin.0 - 5.0).abs() < 1e-9,
            "the line matrix survived BT: second glyph at {:?}",
            glyphs[1].origin
        );
    }

    // ---- (2) q/Q save and restore the text state along with the CTM -----------------------

    #[test]
    fn q_and_q_save_the_text_state_and_not_only_the_ctm() {
        // THE EASY HALF IS THE CTM. The half that gets missed is that `Tz`, `Tc` and the rest
        // are graphics state too -- so a `Tz` set inside `q` ... `Q` must not survive the `Q`.
        let glyphs = placed(
            "/F1 10 Tf BT 100 700 Td (A) Tj ET q 4 Tz BT 200 700 Td (A) Tj ET Q \
             BT 300 700 Td (A) Tj ET",
        );
        assert_eq!(glyphs.len(), 3);
        let width = |g: &Glyph| g.conservative_box().right - g.conservative_box().left;

        assert!(
            width(&glyphs[1]) < width(&glyphs[0]),
            "the Tz inside q...Q did not apply"
        );
        assert!(
            (width(&glyphs[2]) - width(&glyphs[0])).abs() < 1e-9,
            "the Tz escaped the Q: {} against {}",
            width(&glyphs[2]),
            width(&glyphs[0])
        );
    }

    #[test]
    fn q_and_q_restore_the_ctm() {
        let glyphs =
            placed("/F1 10 Tf q 2 0 0 2 0 0 cm BT 100 700 Td (A) Tj ET Q BT 100 700 Td (A) Tj ET");
        assert_eq!(glyphs.len(), 2);
        assert!(
            (glyphs[0].origin.0 - 200.0).abs() < 1e-9,
            "{:?}",
            glyphs[0].origin
        );
        assert!(
            (glyphs[1].origin.0 - 100.0).abs() < 1e-9,
            "{:?}",
            glyphs[1].origin
        );
    }

    // ---- (3) Form recursion: cycles and depth are refusals ---------------------------------

    #[test]
    fn a_form_that_draws_itself_is_refused_rather_than_walked() {
        let resources = Fake::new().with_form(b"Fm0", 7, Matrix::IDENTITY, "/Fm0 Do");
        // ASSERTED ON THE RULE, not on "some Unsupported". The depth cap refuses a
        // self-drawing form too -- it recurses to MAX_FORM_DEPTH and stops -- so a test that
        // accepted either would pass with cycle detection deleted. Measured: it did.
        assert_refused(glyphs_in(b"/Fm0 Do", &resources), Refusal::FormCycle);
    }

    #[test]
    fn a_cycle_through_two_forms_is_refused_by_identity_and_not_by_name() {
        // KEYED ON THE OBJECT, not the name. The two forms name each other through DIFFERENT
        // resource names, so a walk remembering names would have to see `/FmA` twice to notice
        // -- and would also refuse a legitimate document that reuses one name inside a form.
        let resources = Fake::new()
            .with_form(b"FmA", 11, Matrix::IDENTITY, "/FmB Do")
            .with_form(b"FmB", 12, Matrix::IDENTITY, "/FmA Do");
        assert_refused(glyphs_in(b"/FmA Do", &resources), Refusal::FormCycle);
    }

    #[test]
    fn forms_nested_past_the_cap_are_refused_rather_than_truncated() {
        // Each form draws the next, all distinct objects, so this is depth rather than a cycle.
        let mut resources = Fake::new();
        for level in 0..=MAX_FORM_DEPTH {
            let name = format!("Fm{level}");
            let next = format!("/Fm{} Do", level + 1);
            resources =
                resources.with_form(name.as_bytes(), 100 + level as u64, Matrix::IDENTITY, &next);
        }
        assert_refused(glyphs_in(b"/Fm0 Do", &resources), Refusal::FormDepth);
    }

    #[test]
    fn a_form_composes_its_matrix_with_the_ctm_at_the_do() {
        // The other order puts the form's transform outside the page's, which is plausible and
        // wrong: here it would place the glyph at 20 rather than 110.
        let resources = Fake::new().with_form(
            b"Fm0",
            7,
            Matrix::translate(10.0, 0.0),
            "/F1 10 Tf BT 0 0 Td (A) Tj ET",
        );
        let glyphs = glyphs_in(b"q 1 0 0 1 100 0 cm /Fm0 Do Q", &resources).expect("walks");
        assert_eq!(glyphs.len(), 1);
        assert!(
            (glyphs[0].origin.0 - 110.0).abs() < 1e-9,
            "form matrix composed in the wrong order: {:?}",
            glyphs[0].origin
        );
    }

    #[test]
    fn a_form_inherits_the_text_state_in_force_at_the_do() {
        let resources = Fake::new().with_form(b"Fm0", 7, Matrix::IDENTITY, "BT 0 0 Td (A) Tj ET");
        // The font and size are set OUTSIDE the form and never inside it.
        let glyphs = glyphs_in(b"/F1 10 Tf /Fm0 Do", &resources).expect("walks");
        assert_eq!(glyphs.len(), 1);
        assert!((glyphs[0].font_size - 10.0).abs() < 1e-9);
    }

    // ---- (4) every early exit is a refusal ------------------------------------------------

    #[test]
    fn an_unbalanced_q_is_refused() {
        // A WALK THAT STOPS EARLY HAS NOT EXAMINED THE REST OF THE PAGE. Returning the glyphs
        // found so far would report a page as holding less than it does, and a redaction built
        // on that removes what it saw and leaves what it did not.
        refusing("q /F1 10 Tf BT 0 0 Td (A) Tj ET", Refusal::UnbalancedSave);
    }

    #[test]
    fn a_q_with_nothing_saved_is_refused() {
        refusing("Q /F1 10 Tf BT 0 0 Td (A) Tj ET", Refusal::UnmatchedRestore);
    }

    #[test]
    fn an_unterminated_text_object_is_refused() {
        refusing(
            "/F1 10 Tf BT 0 0 Td (A) Tj",
            Refusal::UnterminatedTextObject,
        );
    }

    #[test]
    fn a_nested_bt_is_refused() {
        refusing(
            "/F1 10 Tf BT BT 0 0 Td (A) Tj ET ET",
            Refusal::NestedTextObject,
        );
    }

    #[test]
    fn a_text_operator_outside_a_text_object_is_refused() {
        // Not a no-op: there is no text matrix to place against, so skipping it means silently
        // not seeing whatever it draws.
        refusing("/F1 10 Tf 0 0 Td (A) Tj", Refusal::TextOutsideTextObject);
        refusing("/F1 10 Tf (A) Tj", Refusal::TextOutsideTextObject);
    }

    #[test]
    fn an_et_with_no_bt_is_refused() {
        refusing("ET", Refusal::UnmatchedEndText);
    }

    #[test]
    fn text_shown_with_no_font_is_refused() {
        refusing("BT 0 0 Td (A) Tj ET", Refusal::NoFontSelected);
    }

    #[test]
    fn an_operator_whose_operand_is_the_wrong_shape_is_refused() {
        // FOUND BY `every_refusal_is_both_raised_and_tested`, which reported five rules nothing
        // asserted. They are the ones a malformed file reaches first, so they were exactly the
        // wrong five to leave unchecked.
        refusing("5 Do", Refusal::FormOperandNotAName);
        refusing(
            "/F1 10 Tf BT (A) TJ ET",
            Refusal::ShowArrayOperandNotAnArray,
        );
        refusing(
            "/F1 10 Tf BT [/Name] TJ ET",
            Refusal::ShowArrayItemNotShowable,
        );
        refusing("/F1 10 Tf BT /Name Tj ET", Refusal::ShowOperandNotAString);
    }

    #[test]
    fn a_font_claiming_zero_bytes_per_code_is_refused() {
        // Not a division by zero but a non-terminating chunk: `chunks(0)` panics, and a walk
        // that clamped it to 1 would decode a two-byte font's codes as bytes and place every
        // glyph on the page somewhere else.
        let mut resources = Fake::new();
        resources.bytes_per_code = 0;
        assert_refused(
            glyphs_in(b"/F1 10 Tf BT 0 0 Td (A) Tj ET", &resources),
            Refusal::ZeroBytesPerCode,
        );
    }

    // ---- the writing mode comes from `WMode`, not from a name -----------------------------

    #[test]
    fn a_vertical_cmap_is_caught_by_wmode_under_any_name() {
        // THE EVASION. An embedded CMap's name is the producer's to choose, so `/Identity-V` is
        // the one spelling an attacker will not use. A check keyed on the name passes all three
        // of these; the check keyed on `WMode` fails all three.
        for program in [
            b"/CIDInit /ProcSet findresource begin /WMode 1 def".as_slice(),
            b"%!PS-Adobe-3.0 Resource-CMap\n/CMapName /Perfectly-Ordinary-H def /WMode 1 def"
                .as_slice(),
            b"/CMapName /Identity-H def /WMode 1 def".as_slice(),
        ] {
            let cmap = CMap::Embedded {
                dictionary_wmode: None,
                program,
            };
            assert_eq!(
                writing_mode_of(&cmap).expect("derives"),
                WritingMode::Vertical,
                "a program declaring 'WMode 1' read as horizontal"
            );
        }
    }

    #[test]
    fn the_identity_h_twin_is_not_refused() {
        // THE NEAR-MISS. A rule that refuses too much is not safe either -- it refuses the
        // ordinary horizontal document this fixture is, and a redaction nobody can run leaks
        // nothing only because it never runs.
        for cmap in [
            CMap::Predefined(b"Identity-H"),
            CMap::Embedded {
                dictionary_wmode: Some(0),
                program: b"/CMapName /Identity-H def /WMode 0 def",
            },
            CMap::Embedded {
                dictionary_wmode: None,
                // NO `WMode` AT ALL. The specification's default is 0, so this is horizontal
                // because the specification says so, not because nothing was found.
                program: b"/CMapName /Identity-H def",
            },
        ] {
            assert_eq!(
                writing_mode_of(&cmap).expect("derives"),
                WritingMode::Horizontal
            );
            check_writing_mode(writing_mode_of(&cmap).expect("derives")).expect("not refused");
        }
    }

    #[test]
    fn a_predefined_cmap_is_read_from_its_registry_suffix() {
        // The `-V` CMaps a name check spelled `Identity-V` would miss.
        for name in [
            b"Identity-V".as_slice(),
            b"UniJIS-UCS2-V".as_slice(),
            b"90ms-RKSJ-V".as_slice(),
            b"ETen-B5-V".as_slice(),
        ] {
            assert_eq!(
                writing_mode_of(&CMap::Predefined(name)).expect("derives"),
                WritingMode::Vertical,
                "{}",
                String::from_utf8_lossy(name)
            );
        }
        assert_refused_mode(
            writing_mode_of(&CMap::Predefined(b"SomethingElse")),
            Refusal::UndeterminedWritingMode,
        );
    }

    #[test]
    fn a_usecmap_inherits_a_vertical_mode_without_declaring_one() {
        // A REAL HOLE IN THE OBVIOUS CHECK. This program states no `WMode` of its own; it
        // inherits one. Reading only `/WMode` reports it horizontal.
        let cmap = CMap::Embedded {
            dictionary_wmode: None,
            program: b"/CMapName /Custom def /UniJIS-UCS2-V usecmap",
        };
        assert_eq!(
            writing_mode_of(&cmap).expect("derives"),
            WritingMode::Vertical
        );
    }

    #[test]
    fn a_wmode_inside_a_string_or_a_comment_is_not_a_declaration() {
        // TOKENISED, NOT SCANNED. A byte search for "/WMode 1" matches both of these, and
        // refusing an ordinary document because a comment mentions the key is a check that
        // cannot be left on.
        for program in [
            b"% /WMode 1 def
/CMapName /Plain-H def"
                .as_slice(),
            b"(/WMode 1 def) pop /CMapName /Plain-H def".as_slice(),
        ] {
            let cmap = CMap::Embedded {
                dictionary_wmode: None,
                program,
            };
            assert_eq!(
                writing_mode_of(&cmap).expect("derives"),
                WritingMode::Horizontal,
                "{}",
                String::from_utf8_lossy(program)
            );
        }
    }

    #[test]
    fn a_cmap_that_says_two_things_is_refused_rather_than_believed_once() {
        assert_refused_mode(
            writing_mode_of(&CMap::Embedded {
                dictionary_wmode: Some(0),
                program: b"/WMode 1 def",
            }),
            Refusal::UndeterminedWritingMode,
        );
        assert_refused_mode(
            writing_mode_of(&CMap::Embedded {
                dictionary_wmode: None,
                program: b"/WMode 2 def",
            }),
            Refusal::UndeterminedWritingMode,
        );
        assert_refused_mode(
            writing_mode_of(&CMap::Embedded {
                dictionary_wmode: None,
                program: b"/WMode (one) def",
            }),
            Refusal::UndeterminedWritingMode,
        );
    }

    /// The rule-naming assertion, for the derivation rather than the walk.
    #[track_caller]
    fn assert_refused_mode(outcome: Result<WritingMode>, rule: Refusal) {
        match outcome {
            Err(error) => assert!(
                rule.caught(&error),
                "refused, but by a different rule: wanted `{}`, got {error:?}",
                rule.rule()
            ),
            Ok(mode) => panic!("expected a refusal by `{}`, got {mode:?}", rule.rule()),
        }
    }

    #[test]
    fn a_vertical_writing_mode_is_refused() {
        let mut resources = Fake::new();
        resources.writing_mode = WritingMode::Vertical;
        assert_refused(
            glyphs_in(b"/F1 10 Tf BT 0 0 Td (A) Tj ET", &resources),
            Refusal::VerticalWriting,
        );
    }
}

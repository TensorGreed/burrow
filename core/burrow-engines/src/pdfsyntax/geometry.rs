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

use super::ops::{Operand, Operation};

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
                state = stack.pop().ok_or_else(|| {
                    Error::Malformed(
                        "pdf geometry: a 'Q' with no matching 'q', so the graphics state after \
                         it is undefined"
                            .to_owned(),
                    )
                })?;
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
                    return Err(Error::Malformed(
                        "pdf geometry: a 'BT' inside a text object, which the specification does \
                         not allow to nest"
                            .to_owned(),
                    ));
                }
                // ONLY THE MATRICES. `Tc`, `Tw`, `Tz`, `TL`, `Tf`, `Tr` and `Ts` are graphics
                // state and survive `BT` -- a walk that reset them would place every glyph in a
                // second text object as though the first had never set anything.
                position = Some(TextPosition::default());
            }
            b"ET" => {
                if position.take().is_none() {
                    return Err(Error::Malformed(
                        "pdf geometry: an 'ET' with no 'BT'".to_owned(),
                    ));
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
                    return Err(Error::Malformed(
                        "pdf geometry: a 'Do' whose operand is not a name".to_owned(),
                    ));
                };
                draw_form(value, resources, &state, open_forms, out)?;
            }
            // The text-placing and text-showing operators, which need a text object.
            b"Tm" | b"Td" | b"TD" | b"T*" | b"Tj" | b"TJ" | b"'" | b"\"" => {
                let Some(place) = position.as_mut() else {
                    // OUTSIDE A TEXT OBJECT IS A REFUSAL, not a no-op. There is no text matrix
                    // to place against, so a walk that skipped it would silently not see
                    // whatever the operator draws.
                    return Err(Error::Malformed(
                        "pdf geometry: a text operator outside a 'BT' ... 'ET' text object"
                            .to_owned(),
                    ));
                };
                text_operator(content, operation, &mut state, place, resources, out)?;
            }
            _ => {}
        }
    }

    if !stack.is_empty() {
        return Err(Error::Malformed(format!(
            "pdf geometry: {} unbalanced 'q' at the end of a content stream",
            stack.len()
        )));
    }
    if position.is_some() {
        return Err(Error::Malformed(
            "pdf geometry: a 'BT' with no 'ET' -- the text object runs off the end of the stream"
                .to_owned(),
        ));
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
        return Err(Error::Unsupported(
            "a Form XObject draws itself, directly or through another form (cycle)".to_owned(),
        ));
    }
    if open_forms.len() >= MAX_FORM_DEPTH {
        return Err(Error::Unsupported(
            "Form XObjects nested deeper than burrow will walk (depth)".to_owned(),
        ));
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
                return Err(Error::Malformed(
                    "pdf geometry: a 'TJ' whose operand is not an array".to_owned(),
                ));
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
                        return Err(Error::Malformed(
                            "pdf geometry: a 'TJ' array holding something that is neither a \
                             string nor a number"
                                .to_owned(),
                        ));
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
        return Err(Error::Malformed(
            "pdf geometry: a text-showing operator whose operand is not a string".to_owned(),
        ));
    };
    let Some(font) = state.font.clone() else {
        return Err(Error::Malformed(
            "pdf geometry: text shown with no font selected by 'Tf'".to_owned(),
        ));
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
        return Err(Error::Malformed(
            "pdf geometry: a font claiming zero bytes per code".to_owned(),
        ));
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
    use burrow_types::{Error, Result};

    use super::{
        Form, Glyph, GlyphMetrics, MAX_FORM_DEPTH, Matrix, Rect, Resources, TextPosition,
        TextState, WritingMode, glyphs_in, takes_word_spacing,
    };

    /// A resources table with one font of known width and whatever forms a test names.
    ///
    /// Widths are 500/1000 so an advance is half the font size -- a number a reader can check in
    /// their head, which is what makes a failure message useful rather than a pair of decimals.
    struct Fake {
        forms: Vec<(Vec<u8>, Form)>,
        writing_mode: WritingMode,
    }

    impl Fake {
        fn new() -> Self {
            Self {
                forms: Vec::new(),
                writing_mode: WritingMode::Horizontal,
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
                bytes_per_code: 1,
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
            Ok(1)
        }
    }

    fn placed(content: &str) -> Vec<Glyph> {
        glyphs_in(content.as_bytes(), &Fake::new()).expect("the fixture should walk")
    }

    fn refused(content: &str) -> Error {
        glyphs_in(content.as_bytes(), &Fake::new()).expect_err("this fixture must be refused")
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
        assert_refused_by(glyphs_in(b"/Fm0 Do", &resources), "cycle");
    }

    /// Assert a walk was refused, by the rule whose name appears in the message.
    fn assert_refused_by(outcome: Result<Vec<Glyph>>, rule: &str) {
        match outcome {
            Err(Error::Unsupported(message)) => assert!(
                message.contains(rule),
                "refused, but by a different rule: wanted `{rule}`, got `{message}`"
            ),
            other => panic!("expected a refusal naming `{rule}`, got {other:?}"),
        }
    }

    #[test]
    fn a_cycle_through_two_forms_is_refused_by_identity_and_not_by_name() {
        // KEYED ON THE OBJECT, not the name. The two forms name each other through DIFFERENT
        // resource names, so a walk remembering names would have to see `/FmA` twice to notice
        // -- and would also refuse a legitimate document that reuses one name inside a form.
        let resources = Fake::new()
            .with_form(b"FmA", 11, Matrix::IDENTITY, "/FmB Do")
            .with_form(b"FmB", 12, Matrix::IDENTITY, "/FmA Do");
        assert_refused_by(glyphs_in(b"/FmA Do", &resources), "cycle");
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
        assert_refused_by(glyphs_in(b"/Fm0 Do", &resources), "depth");
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
        assert!(matches!(
            refused("q /F1 10 Tf BT 0 0 Td (A) Tj ET"),
            Error::Malformed(_)
        ));
    }

    #[test]
    fn a_q_with_nothing_saved_is_refused() {
        assert!(matches!(
            refused("Q /F1 10 Tf BT 0 0 Td (A) Tj ET"),
            Error::Malformed(_)
        ));
    }

    #[test]
    fn an_unterminated_text_object_is_refused() {
        assert!(matches!(
            refused("/F1 10 Tf BT 0 0 Td (A) Tj"),
            Error::Malformed(_)
        ));
    }

    #[test]
    fn a_nested_bt_is_refused() {
        assert!(matches!(
            refused("/F1 10 Tf BT BT 0 0 Td (A) Tj ET ET"),
            Error::Malformed(_)
        ));
    }

    #[test]
    fn a_text_operator_outside_a_text_object_is_refused() {
        // Not a no-op: there is no text matrix to place against, so skipping it means silently
        // not seeing whatever it draws.
        assert!(matches!(
            refused("/F1 10 Tf 0 0 Td (A) Tj"),
            Error::Malformed(_)
        ));
        assert!(matches!(refused("/F1 10 Tf (A) Tj"), Error::Malformed(_)));
    }

    #[test]
    fn an_et_with_no_bt_is_refused() {
        assert!(matches!(refused("ET"), Error::Malformed(_)));
    }

    #[test]
    fn text_shown_with_no_font_is_refused() {
        assert!(matches!(
            refused("BT 0 0 Td (A) Tj ET"),
            Error::Malformed(_)
        ));
    }

    #[test]
    fn a_vertical_writing_mode_is_refused() {
        let mut resources = Fake::new();
        resources.writing_mode = WritingMode::Vertical;
        let outcome = glyphs_in(b"/F1 10 Tf BT 0 0 Td (A) Tj ET", &resources);
        assert!(
            matches!(outcome, Err(Error::Unsupported(_))),
            "vertical writing must be refused rather than laid out horizontally: {outcome:?}"
        );
    }
}

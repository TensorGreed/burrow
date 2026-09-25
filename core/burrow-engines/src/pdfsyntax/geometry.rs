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
//! **And the walk derives it rather than being told.** That is the difference between a rule
//! that holds in this module's tests and one that holds over a document. [`GlyphMetrics`] used
//! to carry a [`WritingMode`] the resolver supplied, so a resolver that could not decode a CMap
//! stream — or never tried — returned `Horizontal` by default, with no refusal and no compile
//! error: the whole rule was true at unit level and vacuous at document level. It now carries
//! an [`Encoding`], every arm of which is an answer someone had to give, and the arm for "I
//! could not read it" is [`Encoding::UnreadableCMap`], which is refused. There is no way left
//! to reach horizontal by omission. `tests/glyph_geometry.rs` pins it on a real PDF whose
//! `/Encoding` is a CMap **stream** named `/Ordinary-H` and declaring `WMode 1`, against a twin
//! differing in one digit that must not be refused.
//!
//! # What this walk does not reach
//!
//! "Every early exit is a refusal" is a claim about the operators the walk **models**. It was
//! not, until review, a claim about the ones it does not -- and the gap was measured: a page
//! whose only text lived in a tiling pattern walked to `Ok(0)` while PDFium inked 740 pixels of
//! it, and `FPDFText_*` reported zero characters, so ADR 0029 §6's read-back was blind to it
//! too. An `Ok` over text nothing observed is the exact shape §8 forbids.
//!
//! So a pattern fill is now refused ([`Refusal::PatternMayDrawText`]) rather than walked past.
//! The residue, stated rather than implied: an ExtGState naming a `/Font` sets the size and
//! face without a `Tf`, and this walk does not resolve `gs`. Refusing every `gs` would refuse
//! most real documents, and resolving it needs a seam [`Resources`] does not have yet. That is
//! #152, and until it is closed the walk's completeness claim is "every operator it models,
//! plus patterns refused".
//!
//! # And a vertical document need not say any of that
//!
//! Measured: asked for a vertically-written Japanese paragraph, LibreOffice emitted a **subset
//! simple font and one `Tm` per glyph**, stepping `y` down the page. No CID font, no `WMode`,
//! nothing to refuse — and the ordinary horizontal walk places every glyph correctly.
//! `tests/glyph_geometry.rs` pins that shape. The `WMode` refusal covers the other case, and
//! saying so here is the difference between a bound and a hope.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};

use burrow_types::{Clock, Deadline, Error, Result};

use super::ops::{Operand, Operation, Span};

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
/// through `Refusal::refuse` (private, so no intra-doc link), which stamps the rule into the
/// message; every refusal test names the variant it expects. The loose assertion is now the
/// *longer* one to write, and `refusals_are_the_only_way_to_refuse_in_this_module` fails if a
/// bare `Error::Malformed` or `Error::Unsupported` reappears here.
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
    /// An operator whose numeric operand is missing or is not a number.
    NumericOperandNotANumber,
    /// An operator given a number of operands the specification does not define for it.
    OperandCountMismatch,
    /// A coordinate or matrix entry that is not finite.
    NonFiniteGeometry,
    /// More glyphs on one page than burrow will place.
    TooManyGlyphs,
    /// More Form XObject draws in one walk than burrow will follow.
    TooManyFormDraws,
    /// A pattern fill, which can draw text burrow's walk does not reach.
    PatternMayDrawText,
    /// An embedded CMap stream the resolver could not read.
    UnreadableCMap,
    /// A simple font whose codes are not single bytes.
    SimpleFontWithMultiByteCodes,
    /// A glyph whose span indexes a stream other than the one being edited.
    GlyphFromAnotherStream,
    /// A glyph attributed to an operation the stream being edited does not contain.
    GlyphWithoutItsOperation,
    /// A glyph attributed to an operation that shows no text.
    GlyphOnANonShowingOperation,
    /// A displacement no `TJ` adjustment reproduces.
    AdjustmentNotExpressible,
    /// The region reaches text inside a Form XObject drawn in more than one place.
    SharedFormWouldChangeElsewhere,
    /// A Type 3 glyph procedure that shows text the walk does not reach.
    TypeThreeProcedureShowsText,
    /// A shown string that does not divide into whole codes.
    StringNotWholeCodes,
    /// Glyphs cut from one string disagreeing on the font's code width.
    MixedCodeWidths,
    /// A Form XObject that draws itself, directly or through another form.
    FormCycle,
    /// Form XObjects nested deeper than [`MAX_FORM_DEPTH`].
    FormDepth,
    /// A font whose CMap declares vertical writing (`WMode 1`).
    VerticalWriting,
    /// A CMap whose writing mode the document does not determine.
    UndeterminedWritingMode,
    /// A marked-content span reaching the region names a property list burrow could not read.
    ///
    /// **It says burrow could not tell**, which is not the same as saying the span carries
    /// text. Folding the two would put a sentence in front of the user asserting a copy of
    /// their text exists when what happened is that nothing could look — and it would make the
    /// corpus census read as though every one of these documents carried the channel.
    ///
    /// # Narrower since #166, and what is left in it
    ///
    /// This used to fire on **every** named property list, because nothing resolved
    /// `/Properties`. `13-optional-content.pdf` refused here for a reason that was not its
    /// reason, and every ordinary tagged span written as `/P /MC0 BDC` refused with it. A name
    /// is now resolved through the `/Properties` of the scope that drew the stream
    /// ([`NamedProperties`]), and what remains is what genuinely cannot be read: a name the
    /// scope does not define, an entry that is not a dictionary, and a dictionary holding an
    /// indirect reference — whose target could hold the text and is not in the bytes resolved.
    MarkedContentPropertiesUnresolved,
    /// A marked-content span reaching the region names, through `/Properties`, a property list
    /// that carries its own copy of the text.
    ///
    /// **Refused, not handled, and not for want of a rewriter.** An inline list lives in the
    /// content stream, and dropping its `/ActualText` touches exactly the span it describes. A
    /// named list lives in a **resource dictionary**, which is shared by construction: other
    /// spans may name the same entry, and a page's `/Resources` is very often the one every
    /// page inherits. Dropping the key there removes alternative text from content nobody
    /// asked to redact — the damage-elsewhere hazard ADR 0029 answers with a refusal for shared
    /// forms and a disclosure for shared fonts. Rewriting only the `BDC` operand to an inline
    /// dictionary would leave the string in the resource, and a raw byte scan would still find
    /// it.
    ///
    /// Scoped to spans the removal is inside, as the form rule is scoped to glyphs being
    /// removed: a named carrying span elsewhere on the page does not refuse it. ADR 0029 records
    /// this under the same condition for revisiting as shared forms and fonts, since all three
    /// want the same thing — a count of who else uses the object.
    MarkedContentNamedPropertiesCarryText,
    /// A marked-content property list that must be rewritten spans two `/Contents` elements.
    ///
    /// Legal — §7.8.2 divides the array between lexical tokens, not between operations — and
    /// `Contents::apply` already refuses to replace across a boundary because deleting across
    /// one is defined and replacing is not. This gives that outcome a **name**: without it the
    /// operation refused with a `Malformed` about content-stream edits, which blames the
    /// document's syntax for a limit of burrow's rewriter and which the corpus sweep would
    /// panic on rather than record.
    MarkedContentSplitAcrossElements,
    /// A covering span's property list holds a string burrow cannot account for.
    ///
    /// The rewriter removes `TEXT_CARRYING_KEYS` where they are **keys**, because that is
    /// where assistive technology reads them. A producer may also write one of those names
    /// somewhere else — §14.6.2 restricts a property list's contents not at all. Then the name is
    /// there, a string is beside it, and the rewriter removes nothing.
    ///
    /// Left alone that is a leak a raw byte scan finds, which is the one outcome redaction may
    /// not have. Measured on `/Span << /MCID 0 /K [ /ActualText (secret) ] >>`: a name in array
    /// position is not a key, nothing was removed, and the string reached the output.
    ///
    /// So the span is refused, and the check runs on the **rewritten** bytes. See
    /// `names_a_text_key` for why this is the gap between the wide detector and the narrow
    /// rewriter rather than "the list holds a string", which refuses `/Lang (en-US)`.
    MarkedContentCarriesOpaqueString,
    /// A covering span's property list is not a sequence of key/value pairs.
    ///
    /// `as_chunks::<2>()` walks a dictionary's flat `items` in key/value pairs and **silently
    /// drops a trailing odd one**, which is fine for reading and wrong for rewriting: the
    /// rebuild then emits a dictionary missing that item. Measured, end to end —
    /// `/Span << /MCID 0 /ActualText 4 0 R >> BDC` became `/Span << /MCID 0 0 R >>`, a
    /// dictionary keyed by the number `0`, and
    /// `/Span << /MCID 0 /Pad << /ActualText (X) >> /Tail >>` lost `/Tail` without a word.
    ///
    /// Neither leaked — `names_a_text_key` iterates every item, so a stray carried name is still
    /// seen. But burrow emitted a document it had corrupted while reporting success, and
    /// silently producing worse output than it was given is the thing this operation may least
    /// afford. An odd property list is refused instead.
    MarkedContentPropertyListMalformed,
    /// A content stream the page draws marks a span as optional content: `/OC … BDC`.
    ///
    /// The redaction steps refuse a page whose **resources** reference an optional-content group
    /// (ADR 0029 §3). That signal reads object types, and a security review measured it walking
    /// past a membership dictionary written without `/Type` — `/OC1 << /OCGs [n 0 R] >>` — which
    /// PDFium honours as one under an `/OC` mark, hiding the content. The mark itself is the part
    /// a reader keys on, so it is the signal here: every stream this walk draws, whatever the
    /// named list turns out to be.
    ///
    /// Its own rule rather than the steps' `optional-content`, so that each signal has a fixture
    /// only it catches: two defences that raise the same name mask each other's deletion.
    OptionalContentMarked,
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
        Self::NumericOperandNotANumber,
        Self::OperandCountMismatch,
        Self::NonFiniteGeometry,
        Self::TooManyGlyphs,
        Self::TooManyFormDraws,
        Self::PatternMayDrawText,
        Self::UnreadableCMap,
        Self::SimpleFontWithMultiByteCodes,
        Self::GlyphFromAnotherStream,
        Self::GlyphWithoutItsOperation,
        Self::GlyphOnANonShowingOperation,
        Self::AdjustmentNotExpressible,
        Self::SharedFormWouldChangeElsewhere,
        Self::TypeThreeProcedureShowsText,
        Self::StringNotWholeCodes,
        Self::MixedCodeWidths,
        Self::FormCycle,
        Self::FormDepth,
        Self::VerticalWriting,
        Self::UndeterminedWritingMode,
        Self::MarkedContentPropertiesUnresolved,
        Self::MarkedContentNamedPropertiesCarryText,
        Self::MarkedContentSplitAcrossElements,
        Self::MarkedContentCarriesOpaqueString,
        Self::MarkedContentPropertyListMalformed,
        Self::OptionalContentMarked,
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
            Self::NumericOperandNotANumber => "numeric-operand-not-a-number",
            Self::OperandCountMismatch => "operand-count-mismatch",
            Self::NonFiniteGeometry => "non-finite-geometry",
            Self::TooManyGlyphs => "too-many-glyphs",
            Self::TooManyFormDraws => "too-many-form-draws",
            Self::PatternMayDrawText => "pattern-may-draw-text",
            Self::UnreadableCMap => "unreadable-cmap",
            Self::SimpleFontWithMultiByteCodes => "simple-font-multi-byte-codes",
            Self::GlyphFromAnotherStream => "glyph-from-another-stream",
            Self::GlyphWithoutItsOperation => "glyph-without-its-operation",
            Self::GlyphOnANonShowingOperation => "glyph-on-a-non-showing-operation",
            Self::AdjustmentNotExpressible => "adjustment-not-expressible",
            Self::SharedFormWouldChangeElsewhere => "shared-form-would-change-elsewhere",
            Self::TypeThreeProcedureShowsText => "type-three-procedure-shows-text",
            Self::StringNotWholeCodes => "string-not-whole-codes",
            Self::MixedCodeWidths => "mixed-code-widths",
            Self::FormCycle => "form-cycle",
            Self::FormDepth => "form-depth",
            Self::VerticalWriting => "vertical-writing",
            Self::UndeterminedWritingMode => "writing-mode-undetermined",
            Self::MarkedContentPropertiesUnresolved => "marked-content-properties-unresolved",
            Self::MarkedContentNamedPropertiesCarryText => {
                "marked-content-named-properties-carry-text"
            }
            Self::MarkedContentSplitAcrossElements => "marked-content-split-across-elements",
            Self::MarkedContentCarriesOpaqueString => "marked-content-carries-opaque-string",
            Self::MarkedContentPropertyListMalformed => "marked-content-property-list-malformed",
            Self::OptionalContentMarked => "optional-content-marked",
        }
    }

    /// Whether this is a refusal to walk a well-formed file, rather than a malformed one.
    const fn is_unsupported(self) -> bool {
        matches!(
            self,
            Self::FormCycle
                | Self::FormDepth
                | Self::VerticalWriting
                | Self::TooManyGlyphs
                | Self::TooManyFormDraws
                | Self::PatternMayDrawText
                | Self::MarkedContentPropertiesUnresolved
                | Self::MarkedContentNamedPropertiesCarryText
                | Self::MarkedContentSplitAcrossElements
                | Self::MarkedContentCarriesOpaqueString
                | Self::MarkedContentPropertyListMalformed
                | Self::OptionalContentMarked
                | Self::UnreadableCMap
                | Self::SharedFormWouldChangeElsewhere
                | Self::TypeThreeProcedureShowsText
        )
    }

    /// Build the refusal, with the rule name stamped into the message.
    ///
    /// Under test it also records **who called it**, so the reachability gate can require that a
    /// rule was raised by this module's production code rather than named in a test (#177).
    ///
    /// The caller is read in this function's own body. A first draft read it inside the closure
    /// handed to the recorder, and a closure is not `track_caller`, so `Location::caller()`
    /// named a line in here -- in production -- for every call, and every rule passed. The
    /// gate's own probe caught that before anything was committed.
    #[cfg_attr(test, track_caller)]
    fn refuse<T>(self, detail: &str) -> Result<T> {
        // THE RECORDER, outside the exempt window because it builds no error. The caller is read
        // here, not in the closure: see above.
        #[cfg(test)]
        let caller = core::panic::Location::caller();
        #[cfg(test)]
        RAISED.with(|raised| {
            raised
                .borrow_mut()
                .push((self, caller.file(), caller.line()))
        });
        // PROBE-EXEMPT BEGIN -- `refuse` and `caught` are this module's entire *refusal*
        // vocabulary: one builds the two variants a refusal uses, the other matches on them,
        // and a probe that could tell a construction from a pattern would be a parser.
        // `Error::Internal` in `show` is deliberately outside this: it reports a bug in this
        // code rather than a judgement about a file, so it is not a rule anything asserts on.
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
        // A PREFIX, NOT A SEARCH. `contains` accepted an error this module never built --
        // `Error::Unsupported("qpdf: /Annot [form-cycle] in the document")` matched -- and
        // `caught` is the oracle every refusal test here rests on, so it must not be
        // satisfiable by a tag that happens to appear anywhere in a message.
        message.starts_with(&format!("pdf geometry [{}]: ", self.rule()))
        // PROBE-EXEMPT END
    }
}

#[cfg(test)]
std::thread_local! {
    /// Every refusal built on this thread, with the source line that called `Refusal::refuse`.
    ///
    /// Thread-local because libtest runs tests in parallel threads: a process-wide set would mix
    /// one test's refusals into another's. The gate clears it, drives one witness, and reads it.
    static RAISED: core::cell::RefCell<Vec<(Refusal, &'static str, u32)>> =
        const { core::cell::RefCell::new(Vec::new()) };
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
#[derive(Debug, Clone, PartialEq)]
pub struct Glyph {
    /// The pen position in page space — what `FPDFText_GetCharOrigin` reports.
    pub origin: (f64, f64),
    /// The transform from glyph space to page space, for boxing the glyph's own extents.
    ///
    /// **Only `/FontBBox` is in glyph space.** `advance` and `font_size` are in text space, so
    /// putting them through this matrix scales them by the font matrix and the font size a
    /// second time — see [`Self::text_to_page`] for the measurement.
    pub to_page: Matrix,
    /// The transform from text space to page space, for boxing the advance.
    ///
    /// # Why there are two matrices, and what having one cost
    ///
    /// `conservative_box` built its advance rectangle from `advance` and `font_size` and put it
    /// through [`Self::to_page`]. Both are already in text space, and `to_page` begins with the
    /// font matrix and the font size — so every advance box came out scaled by `0.001 × size`
    /// twice. Measured on `producer-writer.pdf` at 18 pt: a glyph PDFium boxes at 2.5 × 13 pt
    /// came back as **0.25 × 0.32 pt**, a box barely larger than the origin point.
    ///
    /// That is the defect the conservative box exists to prevent, in the function that exists
    /// to prevent it. A region overlapping a glyph's ink but not its pen position intersected
    /// nothing, so the redaction removed nothing and returned `Ok` — and no test caught it,
    /// because every hand-built fixture puts its region around the origin. It took a real
    /// producer document and a region derived from PDFium's own boxes.
    ///
    /// `Tz` is folded in here rather than into `advance`, which is recorded without it for the
    /// same reason `displacement` is recorded with it: each is used where it is, and neither is
    /// recomputed.
    pub text_to_page: Matrix,
    /// The advance this glyph contributes, in unscaled text-space units.
    pub advance: f64,
    /// The font's `/FontBBox`, in glyph space, if the font declared one.
    pub font_bbox: Option<Rect>,
    /// The font size in force, for the advance box's height.
    pub font_size: f64,
    /// The font size in force, scaled by `Tz` -- the denominator a `TJ` adjustment divides by.
    ///
    /// Recorded rather than recomputed for the same reason as `displacement`: the two must
    /// cancel exactly when a removal converts one into an adjustment, and two expressions that
    /// are meant to agree, written twice, is how they stop agreeing.
    pub scaled_font_size: f64,
    /// The text-space displacement this glyph caused, **including** `Tc`, `Tw` and `Tz`.
    ///
    /// # Why this is carried rather than recomputed
    ///
    /// Removing a glyph has to put back exactly what it displaced, or the glyphs after the cut
    /// slide along and the page reflows -- a redaction that changed the evidence rather than
    /// removing a secret from it. The value to put back is this one, computed by the same
    /// expression that moved the pen, on the same line. Recomputing it at removal time from
    /// `advance`, `char_spacing` and `word_spacing` would be the same arithmetic written twice,
    /// and a test comparing the two would be measuring them against each other rather than
    /// against the renderer.
    ///
    /// `advance` is deliberately **not** this: it is the glyph's own width times the font size,
    /// with no spacing terms, which is what the advance box needs.
    pub displacement: f64,
    /// Where in the content stream this glyph's code came from.
    pub source: GlyphSource,
}

/// A font resource name together with the stream that named it.
///
/// # A name with no scope was the bug, five times
///
/// `Tf /F1` means whatever `/F1` means **in the resources in force where it was read**, and a
/// glyph drawn inside a Form XObject was named in that form's. This was a bare `Vec<u8>`, and
/// five separate consumers took one and resolved it against the **page's** `/Font`:
///
/// | | consequence |
/// |---|---|
/// | `check_type_three` | a decoy `/T3` on the page shadowed a form's real Type 3 font; the procedure was never read and the secret stayed in the output |
/// | `codes_still_drawn` | `font-missing` on an ordinary document whose form carries its own `/Font` |
/// | the §6 read-back's `drawn_codes` | the same, refusing a document the redaction had handled |
/// | `cut_fonts` | a form-local font was never narrowed, so its `/ToUnicode` kept mapping the removed characters |
/// | `mapped_codes` | the read-back could not see that, for the same reason |
///
/// Four rounds of review found four of them one at a time. The fifth was found by looking for
/// the shape rather than the symptom, which is the argument for making it a type: a name that
/// does not carry its scope is the same shape as a `/Name` without its slash and a handle
/// without its document, and this repository has a rule for each of those because each was
/// found the same way.
///
/// So resolving against the wrong scope now has to be **written down**: the only ways to get one
/// of these are from a glyph, which knows where it was read, and [`Self::on_page`], which says
/// what it is doing.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ScopedFont {
    /// The Form XObject whose stream named it, or `None` for the page's own content.
    ///
    /// **Not the resolving scope by itself.** A form declaring no `/Resources` inherits the
    /// enclosing ones, so resolution is own-then-enclosing — the rule `Resources::within`
    /// applies, and the caller doing anything else is the bypass this type exists to prevent.
    drawn_in: Option<u64>,
    /// The name as the content stream spells it, without the slash.
    name: Vec<u8>,
}

impl ScopedFont {
    /// A name read in `drawn_in`'s stream.
    #[must_use]
    pub fn new(drawn_in: Option<u64>, name: Vec<u8>) -> Self {
        Self { drawn_in, name }
    }

    /// A name read in the page's own content stream, or otherwise known to be a page resource.
    ///
    /// Spelled out rather than defaulted, so a caller reaching for the page's scope when it
    /// holds a glyph's has to say so where a reader can see it.
    #[must_use]
    pub fn on_page(name: Vec<u8>) -> Self {
        Self {
            drawn_in: None,
            name,
        }
    }

    /// The form whose stream named it, or `None` for the page's own content.
    #[must_use]
    pub const fn drawn_in(&self) -> Option<u64> {
        self.drawn_in
    }

    /// The name as the content stream spells it.
    #[must_use]
    pub fn name(&self) -> &[u8] {
        &self.name
    }
}

/// Where a glyph's code sits in the stream that drew it.
///
/// Enough to rewrite the operation that drew it, and no more. The span is the **operation's**,
/// not the string's, because a cut inside a `Tj` becomes a `TJ` -- the operator changes, so the
/// whole operation is what gets replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlyphSource {
    /// The font resource name in force — what `Tf` selected.
    ///
    /// # Why the glyph carries it
    ///
    /// Font surgery removes the entries for codes the document **no longer draws**, and that
    /// is a fact per font and per code. Without both on the glyph, "which codes does this font
    /// still draw" has to be re-derived by a second walk that tracks the text state again —
    /// two readings of one document that are supposed to agree.
    ///
    /// A resource **name** rather than an object identity, because this module resolves
    /// nothing: the name is what the content stream says, and the engine side maps it. It
    /// carries the stream that named it — see [`ScopedFont`] for the five times that mattered.
    pub font: ScopedFont,
    /// The character code this glyph was drawn from.
    pub code: u32,
    /// The form this glyph was drawn from, by object identity, or `None` for the caller's own
    /// stream. A span means nothing without knowing which stream it indexes.
    pub form: Option<u64>,
    /// The drawing operation's byte span in that stream.
    pub operation: Span,
    /// Which operand held the string: `0` for `Tj`, `'` and `"`, or the index within a `TJ`
    /// array.
    pub operand: usize,
    /// This glyph's index **in codes** within that string, not in bytes.
    pub code_index: usize,
    /// How many bytes **the font** uses per code.
    ///
    /// # Not the length of this glyph's own chunk
    ///
    /// It was, and that was a defect a review caught: `bytes.chunks(n)` yields a short final
    /// chunk, so the last glyph of an odd-length two-byte string had a length of 1. Removal
    /// took the framing width from the glyph being cut, re-chunked the whole string into
    /// single bytes, and removed a **different glyph** -- measured on `(ABCDE)` at two bytes
    /// per code: asked to remove `0x45`, it left `0x45` on the page and deleted `0x43`, and
    /// returned `Ok`. A redaction that does not redact, reported as success.
    ///
    /// Inferring a framing parameter from one element of the thing being framed is the bug,
    /// not the off-by-one. This is the font's width, constant for every code in the string.
    pub bytes_per_code: u8,
}

impl Glyph {
    /// The box a region test must use: the advance box **unioned with the scaled `/FontBBox`**.
    ///
    /// See the module header for the measurement this exists for. A font that declares no
    /// `/FontBBox` gets the advance box alone, which is the best available answer and is
    /// recorded as such rather than silently treated as complete.
    #[must_use]
    pub fn conservative_box(&self) -> Rect {
        // TEXT SPACE THROUGH THE TEXT MATRIX, glyph space through the glyph matrix. See
        // `text_to_page` for what conflating the two measured.
        let advance = Rect {
            left: 0.0,
            bottom: 0.0,
            right: self.advance_with_horizontal_scale(),
            top: self.font_size,
        }
        .transformed(&self.text_to_page);
        match self.font_bbox {
            Some(bbox) => advance.union(&bbox.transformed(&self.to_page)),
            None => advance,
        }
    }

    /// The advance with `Tz` applied, which is the width it occupies on the page.
    ///
    /// `advance` is recorded without `Tz` because that is the quantity a `TJ` adjustment is
    /// expressed against. `Tz` belongs in the box, and nowhere else.
    fn advance_with_horizontal_scale(&self) -> f64 {
        if self.font_size == 0.0 {
            return self.advance;
        }
        self.advance * (self.scaled_font_size / self.font_size)
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
            "the font's CMap declares 'WMode 1', and burrow places vertical runs \
             nowhere rather than somewhere wrong",
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

impl Matrix {
    /// Whether every entry is finite.
    ///
    /// Checked after each composition, not only on the operands: two finite `cm` of `1e300`
    /// compose to an infinity, and `inf * 0` in [`Matrix::then`] is a NaN. `Rect::transformed`
    /// folds NaN corners with `min`/`max`, which leaves its own `+inf`/`-inf` initialisers in
    /// place and returns an inverted rectangle -- one that intersects nothing, so the glyph is
    /// one a redaction skips.
    #[must_use]
    pub fn is_finite(&self) -> bool {
        [self.a, self.b, self.c, self.d, self.e, self.f]
            .iter()
            .all(|value| value.is_finite())
    }
}

impl Rect {
    /// Whether every edge is finite.
    #[must_use]
    pub fn is_finite(&self) -> bool {
        [self.left, self.bottom, self.right, self.top]
            .iter()
            .all(|value| value.is_finite())
    }
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

/// The writing mode a font's encoding implies, refusing the encodings that imply nothing.
///
/// # Errors
///
/// [`Refusal::UnreadableCMap`] for a CMap stream the resolver could not read;
/// [`Refusal::SimpleFontWithMultiByteCodes`] for a simple font claiming multi-byte codes, which
/// is incoherent -- `WMode` lives on a CMap, and a font with no CMap cannot have multi-byte
/// codes; and whatever [`writing_mode_of`] refuses.
pub fn writing_mode_for(encoding: &Encoding, bytes_per_code: u8) -> Result<WritingMode> {
    match encoding {
        Encoding::Simple => {
            if bytes_per_code == 1 {
                Ok(WritingMode::Horizontal)
            } else {
                // NOT A PEDANTRY. `Encoding::Simple` is the one arm that concludes "horizontal"
                // without reading anything, so it is the one a resolver could misuse to skip
                // the question for a CID font. A simple font's codes are bytes; if they are not,
                // the resolver has classified the font wrongly and its answer is worth nothing.
                Refusal::SimpleFontWithMultiByteCodes.refuse(
                    "a font encoded as a simple font but claiming codes wider than one byte",
                )
            }
        }
        Encoding::Predefined(name) => writing_mode_of(&CMap::Predefined(name)),
        Encoding::Embedded {
            dictionary_wmode,
            program,
        } => writing_mode_of(&CMap::Embedded {
            dictionary_wmode: *dictionary_wmode,
            program,
        }),
        Encoding::UnreadableCMap => Refusal::UnreadableCMap.refuse(
            "an embedded CMap stream burrow could not read, so its writing mode is unknown",
        ),
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
                let mode = WritingMode::from_wmode(value)?;
                // A SECOND, DIFFERENT `WMode` IS A DISAGREEMENT, and it is refused for the
                // same reason the dictionary-versus-program one is. PostScript `def` semantics
                // arguably make the last one win, and believing that is how nine appended
                // bytes -- `/WMode 0 def` after a `/WMode 1 def` -- walked a vertical CMap as
                // horizontal. The module's rule is that uncertainty removes more, not less.
                if let Some(first) = declared
                    && first != mode
                {
                    return Refusal::UndeterminedWritingMode
                        .refuse("a CMap program declaring 'WMode' twice, with different values");
                }
                declared = Some(mode);
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
                let mode = predefined_writing_mode(&name)?;
                // THE SAME CONTRADICTION AS THE DICTIONARY-VERSUS-PROGRAM ONE, and it used to
                // be resolved silently toward Horizontal, which is the direction that misses
                // text. PDF 32000-1 §9.7.5.3 requires a `usecmap`'d CMap to share the writing
                // mode, so a disagreement is exactly as undecidable here as it is there.
                if inherited.is_some_and(|first| first != mode) {
                    return Refusal::UndeterminedWritingMode
                        .refuse("two 'usecmap' references with different writing modes");
                }
                inherited = Some(mode);
            }
            _ => previous_name = None,
        }
    }
    if wanted_number {
        return Refusal::UndeterminedWritingMode
            .refuse("a 'WMode' at the end of a CMap program, with no value after it");
    }
    match (declared, inherited) {
        (Some(a), Some(b)) if a != b => Refusal::UndeterminedWritingMode
            .refuse("a CMap whose own 'WMode' contradicts the one it inherits by 'usecmap'"),
        (Some(mode), _) | (None, Some(mode)) => Ok(Some(mode)),
        (None, None) => Ok(None),
    }
}

// ---- removing glyphs without moving the ones that stay --------------------------------------

/// Whether a Type 3 glyph procedure draws text this walk cannot see.
///
/// # Channel 8 fails closed until the walk descends into `/CharProcs`
///
/// A Type 3 glyph procedure is a content stream, and it may show text of its own. The geometry
/// walk does not descend into one: [`Resources`] resolves forms and glyph metrics and has no
/// hook for a procedure's content. So text drawn inside a procedure is **not found**, and a
/// redaction that removed the page's Type 3 glyphs would leave that text in the font — spike
/// 0006's channel 8, still open.
///
/// Leaving it silent is the one outcome ADR 0029 §8 forbids: an `Ok` over text nothing
/// observed. So the procedure's stream is scanned for the text-showing operators, and a
/// procedure that has any is refused rather than removed.
///
/// # A scan, not a parse, and the direction of its error
///
/// This tokenises the procedure and looks for `Tj`, `TJ`, `'` and `"` as **operators** — not a
/// byte search, which would match a `Tj` inside a string or a comment and refuse procedures
/// that draw nothing. It does not attempt to decide whether the text is inside the region,
/// because that is the walk this function exists to stand in for. A procedure that shows any
/// text refuses the whole removal.
///
/// That over-refuses: a Type 3 glyph whose procedure draws text outside the region is refused
/// along with one whose procedure draws the secret. Over-refusing is the direction that does
/// not leak, and it is temporary — see #131 and the residue note in ADR 0029.
///
/// # Errors
///
/// [`Refusal::TypeThreeProcedureShowsText`] if the procedure shows text. Whatever
/// [`super::ops::operations`] refuses, since a procedure burrow cannot tokenise is one whose
/// contents it cannot rule on. [`Error::LimitExceeded`] once `watch`'s deadline has passed: a
/// Type 3 font's `/CharProcs` may hold thousands of procedures, and the caller scans them all.
pub fn check_type_three_procedure(procedure: &[u8], watch: &Watch<'_>) -> Result<()> {
    let operations = super::ops::operations(procedure)?;
    // READ ONCE THE PROCEDURE IS LEXED, and per operation after, as every walk here is (#175).
    watch.now()?;
    for operation in operations {
        watch.tick()?;
        // `Do` COUNTS, and it did not. A procedure that shows no text of its own but draws a
        // Form XObject shows the form's text, and this walk enters neither.
        //
        // Measured by a security review: a page drawing one Type 3 glyph whose procedure is
        // `/Sec Do` redacted to `Ok`, the page's `Tj` was removed so the output rendered
        // **nothing** — zero dark pixels against 660 in the input, and PDFium extracted nothing
        // — and the emitted file still contained
        // `BT /Helv 20 Tf 0 0 Td (BURROW-SEC164-TYPE3DO) Tj ET` in full, recoverable with
        // `qpdf --qdf` by anyone holding it.
        //
        // That is "covered, not gone": ADR 0029 §8's forbidden outcome, and the same shape as
        // `19-covered-by-a-rectangle.pdf`. Refusing a procedure that draws anything is the
        // conservative reading of the rule already here, not a new one.
        if matches!(
            operation.operator.as_slice(),
            b"Tj" | b"TJ" | b"\'" | b"\"" | b"Do"
        ) {
            return Refusal::TypeThreeProcedureShowsText.refuse(
                "a Type 3 glyph procedure that draws content of its own, which burrow's walk \
                 does not yet reach",
            );
        }
    }
    Ok(())
}

/// How many places in the document draw a given Form XObject.
///
/// # Why this is a seam and not a function here
///
/// Counting uses means walking every page's resource graph, which is a qpdf-side question:
/// object identity, `/Annots`, `/AP`, nested forms, patterns and `/CharProcs`. None of that is
/// content-stream syntax, so none of it belongs in this module. What belongs here is the
/// **rule** — and the rule is what would otherwise be written by whoever happens to implement
/// the walk.
pub trait FormUses {
    /// How many places draw the form with this object identity. `1` means "only here".
    ///
    /// Counted by **object identity**, never by resource name: two pages may both call a form
    /// `/Fm0` and mean different objects, and one page may reach the same object under two
    /// names. `core/CLAUDE.md` has the rule and `tools/check-handle-identity.py` enforces it.
    ///
    /// # Errors
    ///
    /// Whatever walking the document failed with.
    fn uses(&self, form: u64) -> Result<usize>;
}

/// Refuse if the region reaches glyphs inside a Form XObject that is drawn more than once.
///
/// # The scoping is the rule, not a detail of it
///
/// Editing a shared form in place removes its text **everywhere it is drawn**: a page nobody
/// asked about silently loses content, while the page the user did select looks correctly
/// redacted. ADR 0029 §6's read-back cannot catch that, because it asks about the page it was
/// given and that page is clean.
///
/// But refusing every page that merely *contains* a shared form would refuse a large share of
/// real documents. A letterhead, a header, a footer, a watermark and a logo are all commonly
/// one form drawn on every page, and none of them is what the user selected. So the test is
/// over the glyphs **being removed** — a form is only in question when a glyph the operation is
/// about to cut came out of it.
///
/// # Errors
///
/// [`Refusal::SharedFormWouldChangeElsewhere`] naming nothing about the document beyond the
/// shape, per §7. Whatever [`FormUses::uses`] failed with.
pub fn check_form_sharing(remove: &[Glyph], uses: &dyn FormUses) -> Result<()> {
    let mut asked: Vec<u64> = remove
        .iter()
        .filter_map(|glyph| glyph.source.form)
        .collect();
    asked.sort_unstable();
    asked.dedup();
    for form in asked {
        if uses.uses(form)? > 1 {
            return Refusal::SharedFormWouldChangeElsewhere.refuse(
                "this page draws the selected text from a template used elsewhere in the \
                 document, and removing it here would remove it there too",
            );
        }
    }
    Ok(())
}

/// The marked-content keys that carry a span's text as a string beside its glyphs.
///
/// Three, not two. `/ActualText` (§14.9.4) replaces the span's text and `/Alt` (§14.9.3)
/// describes it; **`/E` (§14.9.5) is expansion text** — what an abbreviation stands for — and
/// assistive technology reads it in place of the glyphs just as it reads the other two. It was
/// missing from this list, so `/Span << /E (SECRET) /MCID 0 >>` redacted `Ok` with the string
/// intact. Found by a security review; the gap predates the rewriter, but the claim that this
/// channel is handled did not, and a third of the channel was not.
///
/// Both halves read this one list, so a key added here is detected and removed together.
const TEXT_CARRYING_KEYS: [&[u8]; 3] = [b"ActualText", b"Alt", b"E"];

// `check_marked_content` IS GONE. It became `carried_text_edits(..).map(|_| ())` the moment the
// rewriter landed -- same walk, same early return, and its one remaining refusal (`Unknown`) is
// raised by the rewriter too. Keeping it cost a second `find_form` per form in the caller, which
// is the loop whose own comment records 22.5 s against a 100 ms deadline.

/// Remove `/ActualText` and `/Alt` from every marked-content span covering a removal here.
///
/// # Dropped, not narrowed, and the measurement is why
///
/// `/ActualText` replaces a span's text for anything that reads the document. The obvious
/// alternative — shorten it to match the glyphs that remain — was rejected on evidence rather
/// than taste. One page, one sixteen-character `/ActualText`, varying how many of the span's
/// glyphs are drawn:
///
/// | glyphs drawn | what PDFium extracts |
/// |---|---|
/// | 16 of 16 | the whole string |
/// | 12 | the whole string |
/// | 8 | the whole string |
/// | 4 | the whole string |
/// | 0 | nothing |
///
/// **Removing part of a span reduces what a reader shows by nothing.** So narrowing is not a
/// tidier version of dropping; it is the only operation that would reduce exposure, and
/// `/ActualText` is a *replacement* with no character-to-glyph correspondence to narrow along.
/// There is nothing to compute it from, and inventing it puts words a screen reader will speak
/// into a document that does not contain them.
///
/// Refusing when the span is only partly removed was the third option, and it refuses the
/// ordinary case: a paragraph containing one redacted name **is** a partly-removed span.
///
/// Dropping is also required when the span goes wholly. A reader then shows nothing, and the
/// string is still in the file — `qpdf --qdf` returns it.
///
/// # What it costs
///
/// A screen-reader user loses the alternative text for content that **remains**, because a span
/// usually covers more than the region. That is disclosed rather than absorbed; see ADR 0029.
///
/// `/Alt` goes with it. It is a description rather than a replacement, but a description of
/// content that has been partly removed is false, and false alternative text is worse than
/// absent alternative text.
///
/// # Errors
///
/// Whatever reading the operations failed with; [`Refusal::MarkedContentNamedPropertiesCarryText`]
/// for a covering span whose property list is named through `/Properties` and carries text —
/// that list lives in a resource dictionary, not in this stream, and is not rewritten here; and
/// [`Refusal::MarkedContentPropertiesUnresolved`] for a named list `properties` cannot resolve.
/// Neither may be silently left alone.
pub fn carried_text_edits(
    content: &[u8],
    remove: &[Glyph],
    stream: Option<u64>,
    draws: &FormsReached<'_>,
    properties: &NamedProperties,
    watch: &Watch<'_>,
) -> Result<Vec<(Span, Vec<u8>)>> {
    let mine: BTreeSet<Span> = remove
        .iter()
        .filter(|glyph| glyph.source.form == stream)
        .map(|glyph| glyph.source.operation)
        .collect();
    if mine.is_empty() && draws.is_empty() {
        return Ok(Vec::new());
    }
    let mut edits = Vec::new();
    for covering in carrying_spans_over_removals(content, &mine, draws, properties, watch)? {
        match covering.carried {
            Carried::Text => {
                let (from, to) = covering.properties;
                let original = content.get(from..to).ok_or_else(|| {
                    // probe-allowed: a burrow invariant, not a judgement about the file
                    Error::Internal(
                        "pdf geometry: a property list span outside the stream it came from"
                            .to_owned(),
                    )
                })?;
                edits.push((covering.properties, without_carried_keys(original)?));
            }
            // NOT SILENTLY LEFT ALONE. A property list this could not read may carry the text,
            // so the operation refuses exactly as it did before the rewriter existed.
            Carried::Unknown => {
                return Refusal::MarkedContentPropertiesUnresolved.refuse(
                    "the selected text is inside a marked-content span whose properties burrow \
                     could not read, so it cannot tell whether they carry their own copy of the \
                     text",
                );
            }
            // READ, AND IT CARRIES THE TEXT -- IN A RESOURCE, NOT IN THIS STREAM. See the
            // variant: the dictionary is shared by construction, and dropping the key there
            // edits spans and pages nobody asked about.
            Carried::NamedText => {
                return Refusal::MarkedContentNamedPropertiesCarryText.refuse(
                    "the selected text is inside a marked-content span whose alternative text \
                     is kept in the page's shared resources rather than beside the text, and \
                     removing it there would remove it from other text too",
                );
            }
            // A STRING UNDER A KEY THE REWRITER DOES NOT TOUCH, so there is nothing to narrow
            // and leaving it emits it. Refused by name rather than leaked.
            Carried::OpaqueString => {
                return Refusal::MarkedContentCarriesOpaqueString.refuse(
                    "the selected text is inside a marked-content span whose properties name \
                     one of the entries that carry a span's text, in a position burrow cannot \
                     remove it from, so it cannot tell whether that text repeats the glyphs",
                );
            }
            Carried::Nothing => {}
        }
    }
    Ok(edits)
}

/// The same property dictionary with every [`TEXT_CARRYING_KEYS`] entry removed.
///
/// Rebuilt from the parsed operand rather than spliced out of the bytes: a key's value may be a
/// string containing `>>`, and cutting on a byte pattern would end the dictionary early. The
/// lexer already knows where each item begins and ends.
fn without_carried_keys(dictionary: &[u8]) -> Result<Vec<u8>> {
    let read = super::ops::operations(&[dictionary, b" BDC"].concat())?;
    let Some(operand @ Operand::Dict { .. }) = read.first().and_then(|op| op.operands.last())
    else {
        // probe-allowed: the caller only offers spans it read as a dictionary.
        return Err(Error::Internal(
            "pdf geometry: a property list that does not re-read as a dictionary".to_owned(),
        ));
    };
    if is_not_key_value_pairs(operand) {
        return Refusal::MarkedContentPropertyListMalformed.refuse(
            "a marked-content property list that is not a sequence of key/value pairs, which \
             burrow will not rebuild because doing so would drop or re-pair its items and emit a \
             dictionary the document did not have",
        );
    }
    let rebuilt = rebuilt_without_carried(dictionary, operand)?;

    // AND THE REWRITE IS CHECKED AGAINST THE DETECTOR THAT DECIDED IT, which is the part that
    // matters more than the recursion below.
    //
    // `holds_text_key` reads `/ActualText` at **any** depth — its own comment says so — and this
    // rebuilt only the top level. A code review measured the consequence: for
    // `<< /A << /ActualText (secret) >> /MCID 0 >> BDC` the module classified the span as
    // carrying text, produced an edit whose replacement was **byte-identical to the input**, and
    // -- because the refusal was deleted in the same commit -- emitted the document. Detector
    // wider than rewriter, with nothing left to catch the difference: a safe-to-unsafe change on
    // the one channel the commit is about.
    //
    // Recursing fixes that shape. Asking the detector again fixes the class: any future
    // divergence between "what counts as carrying text" and "what the rewrite removes" fails
    // here rather than shipping. ADR 0022's argument, one level down.
    let verify = super::ops::operations(&[rebuilt.as_slice(), b" BDC"].concat())?;
    let still_carries = verify
        .first()
        .and_then(|op| op.operands.last())
        .is_some_and(holds_text_key);
    let still_names_one = verify
        .first()
        .and_then(|op| op.operands.last())
        .is_some_and(names_a_text_key);
    if still_names_one {
        // A LIST THAT HELD A HANDLED KEY **AND** NAMED ONE SOMEWHERE ELSE. The
        // `Carried::OpaqueString` arm above never sees it, because `holds_text_key` answered
        // first and classified it as rewritable; without this the rewrite would strip the key it
        // knows and emit the one it does not. Decided on the rewritten bytes, so it cannot be
        // wrong about what actually survived.
        return Refusal::MarkedContentCarriesOpaqueString.refuse(
            "the selected text is inside a marked-content span whose properties still name one \
             of the entries that carry a span's text after burrow removed the ones it knows how \
             to remove, so it cannot tell whether that text repeats the glyphs",
        );
    }
    if still_carries {
        // probe-allowed: burrow disagreeing with itself about bytes it just wrote.
        return Err(Error::Internal(
            "pdf geometry: a rewritten property list that still carries its own copy of the \
             text, so the detector and the rewriter disagree"
                .to_owned(),
        ));
    }
    Ok(rebuilt)
}

/// One operand rebuilt with every [`TEXT_CARRYING_KEYS`] entry removed, at any depth.
///
/// Dictionaries and arrays are rebuilt so a nested `/ActualText` is reached; everything else is
/// copied by span, which is what keeps a string containing `>>` intact.
fn rebuilt_without_carried(source: &[u8], operand: &Operand) -> Result<Vec<u8>> {
    let verbatim = |operand: &Operand| -> Result<Vec<u8>> {
        let (from, to) = operand.span();
        source.get(from..to).map(<[u8]>::to_vec).ok_or_else(|| {
            // probe-allowed: a burrow invariant, not a judgement about the file
            Error::Internal(
                "pdf geometry: an operand span outside the property list it came from".to_owned(),
            )
        })
    };
    match operand {
        Operand::Dict { items, .. } => {
            let mut out = Vec::from(b"<<".as_slice());
            // PAIRWISE by `as_chunks`: a dictionary with an odd number of items is malformed, and
            // this drops the trailing one rather than indexing past the end.
            for [key, value] in items.as_chunks::<2>().0 {
                if matches!(key, Operand::Name { value: name, .. }
                    if TEXT_CARRYING_KEYS.contains(&name.as_slice()))
                {
                    continue;
                }
                out.push(b' ');
                out.extend_from_slice(&verbatim(key)?);
                out.push(b' ');
                out.extend_from_slice(&rebuilt_without_carried(source, value)?);
            }
            out.extend_from_slice(b" >>");
            Ok(out)
        }
        Operand::Array { items, .. } => {
            let mut out = Vec::from(b"[".as_slice());
            for item in items {
                out.push(b' ');
                out.extend_from_slice(&rebuilt_without_carried(source, item)?);
            }
            out.extend_from_slice(b" ]");
            Ok(out)
        }
        other => verbatim(other),
    }
}

/// One marked-content span that is open over a removal in this stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CoveringSpan {
    /// The `BDC`'s property-list operand, for rewriting it in place.
    pub(crate) properties: Span,
    /// What reading that property list established.
    carried: Carried,
}

/// Every marked-content span open over a removal in this stream, outermost first.
///
/// # One walk, two callers, on purpose
///
/// [`carried_text_edits`] is the only caller now, but the split stays: finding which spans
/// cover a removal is a different question from deciding what to do about each.
/// Two walks answering "which spans cover a removal" is two walks that can disagree, and the
/// whole history of this seam is answers that were smaller than the truth — the scope that
/// stopped at the forms holding a glyph, the descent that stopped at a form with no
/// `/Resources`, the memo that kept the first path's answer. A stripper that found fewer spans
/// than the checker would leak exactly the difference, silently, in the direction where the
/// checker had already said `Ok`.
///
/// # Errors
///
/// Whatever reading the operations failed with.
fn carrying_spans_over_removals(
    content: &[u8],
    mine: &BTreeSet<Span>,
    draws: &FormsReached<'_>,
    properties: &NamedProperties,
    watch: &Watch<'_>,
) -> Result<Vec<CoveringSpan>> {
    use super::ops::operations;

    // A STACK, NOT A FLAG, AND EVERY SPAN IS ON IT. Marked content nests, so an inner
    // `/P << /MCID 3 >> BDC` must not clear an outer `/Span << /ActualText … >> BDC` when its
    // `EMC` arrives. Pushing only the carrying spans would do exactly that: the inner span's
    // `EMC` would pop the outer span's entry. `BMC` takes no property list and so carries
    // nothing, but it still opens a span and still has to be on the stack.
    let mut open: Vec<CoveringSpan> = Vec::new();
    let mut found: Vec<CoveringSpan> = Vec::new();
    // A CURSOR INTO THE STACK, NOT A MEMBERSHIP TEST. `recorded` is how much of `open`'s prefix
    // has already been emitted, so each span is recorded once, when the first removal passes
    // under it.
    //
    // # Two measurements, and why the first one was not enough
    //
    // This began as `!found.contains(covering)` over a `Vec`. Before the rewriter, the walk
    // returned at the FIRST covering span; collecting them all made the scan quadratic. A
    // security review measured 250,000 nested `/S << /ActualText (x) >> BDC` -- 19,776 bytes
    // Flate-compressed -- at 18.34 s against a `max_duration_ms` of 100. A `BTreeSet` took that
    // to 0.59 s and looked like the fix.
    //
    // It was not. **The set deduplicated the push, not the scan.** The loop below still ran once
    // per open span per removal, so the cost is `open x removals`, and the 250,000-span
    // measurement varied only one of those two factors -- it held removals at one. The next
    // review varied both and the quadratic was still there, worse: 60,000 spans over 60,000
    // removals, an 11,775-byte gzip, took **109 s** and 417 MB. 1,090x the deadline.
    //
    // A stack is ordered, so a cursor answers "have I recorded this one" in O(1) without a
    // lookup, and the amortised cost becomes pushes + pops + removals. Measured on the same
    // fixtures: 60k x 60k 109.05 s -> 0.23 s, 16k x 16k 7.00 s -> 0.59 s.
    //
    // It also removes a defence nothing tested. A mutation making the set inert survived the
    // suite: the consequence was duplicate edits for one span and a `Malformed` from
    // `Contents::apply` -- failing closed, but anonymously, blaming the file. With a cursor a
    // duplicate cannot be constructed, so there is no inert defence left to test.
    let mut recorded: usize = 0;
    let operations = operations(content)?;
    // AND ONCE AFTER THE LEX, which is the one step here the watch cannot interrupt: without
    // this, a lex that ran past the deadline was followed by up to 255 more operations unread.
    watch.now()?;
    for operation in operations {
        // THE DEADLINE, per operation (#175): 60,000 spans over 60,000 removals was a single
        // uncooperative step here.
        watch.tick()?;
        match operation.operator.as_slice() {
            b"BDC" => open.push(CoveringSpan {
                properties: operation
                    .operands
                    .last()
                    .map_or(operation.span, super::ops::Operand::span),
                carried: carried_by(&operation, properties),
            }),
            b"BMC" => open.push(CoveringSpan {
                properties: operation.span,
                carried: Carried::Nothing,
            }),
            // AN UNMATCHED `EMC` IS NOT A REFUSAL HERE. The walk that produced these glyphs has
            // already accepted the stream, and this check exists to answer one question about
            // it; inventing a second opinion on well-formedness would refuse documents over a
            // rule nothing else in the walk applies.
            b"EMC" => {
                open.pop();
                // THE CURSOR FOLLOWS THE STACK DOWN. A span that was recorded and has now closed
                // must not hold the cursor above a span that reopens at the same depth.
                recorded = recorded.min(open.len());
            }
            _ => {}
        }
        let removes_here = mine.contains(&operation.span)
            || (operation.operator == b"Do" && draws.reached(&operation));
        if !removes_here {
            continue;
        }
        for covering in open.get(recorded..).unwrap_or_default() {
            if covering.carried != Carried::Nothing {
                found.push(*covering);
            }
        }
        recorded = open.len();
    }
    Ok(found)
}

/// Which `Do` operations in a stream draw a form the removal reaches.
///
/// [`carried_text_edits`] needs this because marked content descends through `Do` and its own
/// walk does not: a span opened in one stream covers glyphs drawn from another. This module
/// resolves nothing, so the answer is supplied by the caller — or declared unavailable, which is
/// a different thing and says so.
#[derive(Debug, Clone, Copy)]
pub enum FormsReached<'a> {
    /// The resource names in this stream whose form the removal reaches, resolved by the caller.
    ///
    /// An empty set means "this stream draws no form the removal touches", not "unknown".
    Named(&'a BTreeSet<Vec<u8>>),
    /// Every `Do` in this stream must be treated as drawing one.
    ///
    /// **For a form's own stream**, where resolving a nested `Do` would need that form's
    /// `/Resources` and this module has none.
    ///
    /// This used to add that nested forms were refused anyway by `form-vanished`, and that this
    /// did not lean on it — a defect is not a control, and relying on one for a leak boundary is
    /// how it becomes load-bearing before anybody notices it was a defect. #164 fixed that
    /// lookup, so the refusal is gone and only the argument remains: the conservative answer
    /// here is correct on its own, which is why removing the thing it did not depend on changed
    /// nothing.
    Unresolved,
}

impl FormsReached<'_> {
    /// Whether this stream draws nothing the removal reaches, so a stream with no removed glyphs
    /// of its own has nothing to answer for.
    ///
    /// [`Self::Unresolved`] is never empty: not knowing is not the same as knowing there is none.
    fn is_empty(&self) -> bool {
        match self {
            Self::Named(names) => names.is_empty(),
            Self::Unresolved => false,
        }
    }

    /// Whether this `Do` draws a form the removal reaches.
    fn reached(&self, operation: &Operation) -> bool {
        match self {
            Self::Named(names) => operation.operands.iter().any(|operand| match operand {
                Operand::Name { value, .. } => names.contains(value),
                _ => false,
            }),
            Self::Unresolved => true,
        }
    }
}

/// The property lists a stream's `BDC` operands may name, as the stream's own scope defines them.
///
/// # A name is scoped, and this is where the scope is decided
///
/// `/P /MC0 BDC` looks `MC0` up in the `/Properties` of **whatever drew the stream** — the page,
/// or a Form XObject's own `/Resources`, or, for a form declaring none, whatever encloses it. The
/// same resolution fonts needed, and five consumers got that wrong for fonts before
/// [`ScopedFont`] made the scope part of the type. This module holds no document, so the caller
/// resolves against the right scope and hands over the answer as data; the caller is where the
/// scope is known.
///
/// **Several lists per name, not one.** A form declaring no `/Resources` inherits from whatever
/// encloses it, and a form reached by two routes has two enclosures. Every candidate is kept and
/// the most cautious reading wins — the union-over-paths rule the `Do` scope learnt from a leak.
///
/// # Empty is the cautious answer, by construction
///
/// A name this does not hold resolves to nothing, and nothing is
/// [`Refusal::MarkedContentPropertiesUnresolved`]. So the `Default` value refuses every named
/// span rather than passing it: a caller that forgot to resolve fails closed, which is the
/// answer the `within` precedent wanted when it gave [`Resources`] no default method.
///
/// # A verdict per name, not the bytes
///
/// This held every candidate list as bytes and classified them at every `BDC`. A security review
/// measured both halves of that: 4,000 names pointing at one 200 kB list, inherited by five
/// resource-less forms, peaked at **5.4 GB** from a 252 kB file, since each form's scope cloned
/// the bytes; and 40,000 `BDC`s naming a 200 kB list took **113 s**, re-lexing it every time. So a
/// list is read once, into a [`PropertyList`], and a name keeps only the most cautious verdict
/// among its candidates.
#[derive(Debug, Clone, Default)]
pub struct NamedProperties {
    /// Name, without the `/`, to what the most cautious of its candidates carries.
    lists: BTreeMap<Vec<u8>, Carried>,
}

/// One property list, read once.
///
/// Opaque on purpose: what it carries is this module's judgement, and a caller that could build
/// one from a conclusion would be a second classifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropertyList(Carried);

impl PropertyList {
    /// Classify `list`, a dictionary written as PDF syntax.
    #[must_use]
    pub fn read(list: &[u8]) -> Self {
        Self(named_list_carries(list))
    }
}

impl NamedProperties {
    /// Record that `name` (without the `/`) may resolve to `list`.
    ///
    /// **The most cautious candidate wins**: a list that carries text over one that could not be
    /// read, over one that carries nothing. A list that was read and found to carry text is the
    /// more specific true statement, and either refuses.
    pub fn insert(&mut self, name: Vec<u8>, list: PropertyList) {
        let verdict = self.lists.entry(name).or_insert(list.0);
        if caution(list.0) > caution(*verdict) {
            *verdict = list.0;
        }
    }

    /// Every candidate from `other`, for a stream that resolves through more than one scope.
    pub fn extend(&mut self, other: &Self) {
        for (name, verdict) in &other.lists {
            self.insert(name.clone(), PropertyList(*verdict));
        }
    }

    /// What a `BDC` naming `name` carries.
    fn carried(&self, name: &[u8]) -> Carried {
        self.lists.get(name).copied().unwrap_or(Carried::Unknown)
    }
}

/// How cautious a verdict is, for choosing among a name's candidates.
const fn caution(carried: Carried) -> u8 {
    match carried {
        Carried::Nothing => 0,
        Carried::Unknown => 1,
        // `Text` and `OpaqueString` never come from a named list; ranked with `NamedText` so a
        // stray one fails closed rather than passing.
        Carried::Text | Carried::OpaqueString | Carried::NamedText => 2,
    }
}

/// What one resolved property list carries.
///
/// The **same detectors** an inline list is read with, on the same parse — a second classifier
/// for named lists would be a second answer to "does this carry text", and the rewriter's history
/// is two answers disagreeing by exactly the leak.
fn named_list_carries(list: &[u8]) -> Carried {
    let Ok(read) = super::ops::operations(&[list, b" BDC"].concat()) else {
        return Carried::Unknown;
    };
    let Some(dictionary @ Operand::Dict { .. }) = read
        .first()
        .filter(|_| read.len() == 1)
        .and_then(|operation| operation.operands.last())
    else {
        return Carried::Unknown;
    };
    // A KEY NAMED ANYWHERE, not only a key in key position. Nothing here is rewritten, so the
    // distinction `holds_text_key` draws for the rewriter does not apply: either way the text is
    // in a resource this operation will not edit.
    if holds_text_key(dictionary) || names_a_text_key(dictionary) {
        return Carried::NamedText;
    }
    // AN INDIRECT REFERENCE IS A DOOR THIS DID NOT OPEN. `/K 5 0 R` points at an object whose
    // bytes are not in the list, and that object could be a dictionary holding `/ActualText`.
    // The lexer reads `5 0 R` as two numbers and the keyword `R`; the keyword is the witness.
    if holds_reference(dictionary) {
        return Carried::Unknown;
    }
    Carried::Nothing
}

/// Whether `operand` holds an indirect reference anywhere.
fn holds_reference(operand: &Operand) -> bool {
    match operand {
        Operand::Keyword { value, .. } => value.as_slice() == b"R",
        Operand::Dict { items, .. } | Operand::Array { items, .. } => {
            items.iter().any(holds_reference)
        }
        _ => false,
    }
}

/// What an open marked-content span was found to carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Carried {
    /// Its property list was read and holds neither [`TEXT_CARRYING_KEYS`].
    Nothing,
    /// Its property list was read and holds one of them.
    Text,
    /// Its property list is named, was resolved, and carries one of them — in a resource.
    ///
    /// See [`Refusal::MarkedContentNamedPropertiesCarryText`]. Separate from [`Self::Text`]
    /// because the text is not in the stream, so there is nothing here to rewrite.
    NamedText,
    /// Its property list could not be read: an unresolved name, or one this could not follow.
    Unknown,
    /// Its property list names one of [`TEXT_CARRYING_KEYS`] somewhere it is not a key.
    ///
    /// See [`Refusal::MarkedContentCarriesOpaqueString`]. Separate from [`Self::Text`] because
    /// there is nothing to rewrite: the refusal is the whole outcome.
    OpaqueString,
}

/// Whether a `BDC` operation's property list carries the span's text.
fn carried_by(operation: &Operation, properties: &NamedProperties) -> Carried {
    match operation.operands.last() {
        // AN INLINE DICTIONARY, read at any depth: a `/ActualText` under a nested key is still
        // a copy of the text. The whole dictionary is handed over rather than its items one by
        // one, because `holds_text_key` now asks whether a name is a **key**, and an item seen
        // alone has lost the position that answers that. Passing the items individually is how
        // this read `/ActualText` in value position as though it were a key.
        Some(dictionary @ Operand::Dict { .. }) => {
            if holds_text_key(dictionary) {
                Carried::Text
            } else if names_a_text_key(dictionary) {
                Carried::OpaqueString
            } else {
                Carried::Nothing
            }
        }
        // A NAME, resolved through the `/Properties` of the scope that drew this stream -- which
        // the caller supplies, because this module holds no document. A name the scope does not
        // define is reported as unread rather than as read-and-found.
        Some(Operand::Name { value, .. }) => properties.carried(value),
        // Anything else is not a property list this can read, which is the same claim.
        _ => Carried::Unknown,
    }
}

/// Whether this operand is, or contains, a dictionary that is not key/value pairs.
///
/// See [`Refusal::MarkedContentPropertyListMalformed`]. Asked before the rebuild rather than
/// during it, so the refusal names the document's shape rather than a step burrow got part-way
/// through.
///
/// # Two conditions, because the first one alone missed the case it was written for
///
/// The odd-length test is the obvious one and it does not catch
/// `/Span << /MCID 0 /ActualText 4 0 R >>`, which is the shape a security review measured
/// rebuilding into `<< /MCID 0 0 R >>`. This module's lexer has no indirect references — there
/// is no such thing inside a content stream — so `4 0 R` is three operands, the dictionary has
/// six items, and it is perfectly even. What is wrong with it is that the fifth item, in key
/// position, is the *number* `0`.
///
/// So both: an odd count, and a non-name where a key belongs. Each catches a shape the other
/// does not, and together they are the condition `as_chunks::<2>()` silently assumes.
fn is_not_key_value_pairs(operand: &Operand) -> bool {
    match operand {
        Operand::Dict { items, .. } => {
            items.len() % 2 != 0
                || items
                    .iter()
                    .step_by(2)
                    .any(|key| !matches!(key, Operand::Name { .. }))
                || items.iter().any(is_not_key_value_pairs)
        }
        Operand::Array { items, .. } => items.iter().any(is_not_key_value_pairs),
        _ => false,
    }
}

/// Whether this operand *names* one of [`TEXT_CARRYING_KEYS`] anywhere, key position or not.
///
/// Exactly the reach [`holds_text_key`] used to have, kept as its own function because the
/// difference between the two is the thing that needs a decision rather than a silent drop.
///
/// # Why this is the discriminator, and "holds any string" is not
///
/// The first attempt refused a covering span whose property list held **any** string, on the
/// argument that burrow cannot relate it to the glyphs. That is true and it refuses far too much:
/// `/Span << /MCID 0 /Lang (en-US) >>` is ordinary tagged output and repeats no text. It failed
/// `the_rewritten_property_list_keeps_every_other_key`, which is that probe earning its place.
///
/// The residue worth refusing is narrower and needs no allowlist to describe: a list where the
/// producer wrote one of the three accessibility keys in a position the rewriter does not remove
/// from, such as `/K [ /ActualText (secret) ]`. The key is *named*, so the string beside it is
/// plausibly that channel; nothing is removed, so leaving it emits it. Refusing exactly the gap
/// between the wide reading and the narrow one is what keeps the narrowing from being a leak.
fn names_a_text_key(operand: &Operand) -> bool {
    match operand {
        Operand::Name { value, .. } => TEXT_CARRYING_KEYS.contains(&value.as_slice()),
        Operand::Dict { items, .. } | Operand::Array { items, .. } => {
            items.iter().any(names_a_text_key)
        }
        _ => false,
    }
}

/// Whether this operand holds one of [`TEXT_CARRYING_KEYS`] **as a key**, at any depth.
///
/// # A key, not a name anywhere
///
/// This matched any `Operand::Name` equal to one of the keys, wherever it appeared — including as
/// a *value* and as a bare item inside an array. The comment defending that said treating a value
/// as a key "refuses too much rather than too little", which was sound while the outcome was a
/// refusal and stopped being sound when the outcome became a rewrite: `/K [ /ActualText (x) ]`
/// was reported as carrying text, and there is no key there to remove, so the rewrite was a
/// no-op that the self-check below then had to catch as an internal error. Burrow blaming itself
/// for a document it should simply handle.
///
/// The scope it leaves out is the honest one. `/ActualText`, `/Alt` and `/E` are read by
/// assistive technology **because they are those keys**; a string sitting under some other key is
/// not that channel, and could not be — every operand in a content stream may be a string.
///
/// Dictionaries and arrays are still descended, so a carrying dictionary nested inside either is
/// found. What changed is that the detector now matches exactly what [`rebuilt_without_carried`]
/// removes, which is what keeps the two from disagreeing.
fn holds_text_key(operand: &Operand) -> bool {
    match operand {
        Operand::Dict { items, .. } => items.as_chunks::<2>().0.iter().any(|[key, value]| {
            matches!(key, Operand::Name { value: name, .. }
                if TEXT_CARRYING_KEYS.contains(&name.as_slice()))
                || holds_text_key(value)
        }),
        Operand::Array { items, .. } => items.iter().any(holds_text_key),
        _ => false,
    }
}

/// Remove `remove` from `content`, leaving every other glyph exactly where it was.
///
/// # The whole point is that nothing reflows
///
/// Cutting a glyph out of a run shortens the run, and every glyph after the cut slides left by
/// the removed advance. The page still *reads* plausibly, which is the worst kind of wrong:
/// nothing about the output announces it, and on a form, a table or a signature block the
/// remaining text can end up appearing to say something it did not. A redaction that reflows
/// the line has altered the evidence rather than removed a secret from it.
///
/// So each removed glyph becomes a positioning adjustment equal to **the displacement it
/// caused** -- [`Glyph::displacement`], recorded by the same expression that moved the pen,
/// including `Tc`, `Tz`, and `Tw` where and only where the code was a single-byte 32. Nothing
/// after the cut moves, and `tests/glyph_geometry.rs` holds that to PDFium's own char origins
/// rather than to this module's arithmetic.
///
/// # What comes out
///
/// Every affected drawing operation becomes a `TJ`: kept runs as hex strings, removed glyphs as
/// array numbers, and the producer's own kerns carried through **in place**. A `Tj` with a cut
/// inside it is therefore replaced by a `TJ`, which is why [`GlyphSource`] carries the
/// *operation's* span rather than the string's -- the operator itself changes.
///
/// Hex strings rather than literals: `(`, `)` and a backslash inside a re-encoded run would need
/// escaping, and an escaping bug here silently corrupts the text that stays.
///
/// # Errors
///
/// [`Refusal::GlyphFromAnotherStream`] if any glyph came from a Form XObject -- its span indexes
/// that form's bytes, not these, so applying it here would cut the wrong text out of the page.
/// Following into forms is the next step; refusing is what stops it being silently skipped.
///
/// [`Refusal::AdjustmentNotExpressible`] if a displacement cannot be written as an adjustment,
/// which is `Tz 0` or a zero font size making the conversion a division by zero.
///
/// Whatever [`super::ops::operations`] and [`super::strings::decode_string`] refuse.
pub fn remove_glyphs(
    content: &[u8],
    stream: Option<u64>,
    remove: &[Glyph],
    watch: &Watch<'_>,
) -> Result<Vec<u8>> {
    // EVERY GLYPH MUST BELONG TO THE STREAM BEING EDITED. A span from another stream indexes
    // different bytes, so applying it here cuts whatever happens to sit at those offsets --
    // a cut in the wrong place, reported as success. Editing a form is done by calling this
    // again with that form's own content and its identity.
    if remove.iter().any(|glyph| glyph.source.form != stream) {
        return Refusal::GlyphFromAnotherStream
            .refuse("a glyph drawn in a different stream, whose span does not index this one");
    }
    if remove.is_empty() {
        return Ok(content.to_vec());
    }

    // THE SPLICE IS #128'S, not a second implementation of one. A single-element `Contents` is
    // the degenerate case of the page-content span map, and reusing it keeps the offset
    // arithmetic in one place rather than in two that can disagree.
    let mut applied = remove_glyphs_across(
        &super::contents::Contents::concatenate(&[content])?,
        stream,
        remove,
        watch,
    )?;
    applied.pop().filter(|_| applied.is_empty()).map_or_else(
        || {
            // probe-allowed: a burrow invariant, not a judgement about the file
            Err(Error::Internal(
                "pdf geometry: the splice returned something other than one stream".to_owned(),
            ))
        },
        Ok,
    )
}

/// Remove `remove` from a page's `/Contents`, returning **one buffer per element**.
///
/// # Why this exists beside [`remove_glyphs`]
///
/// A page's `/Contents` may be an array, and PDF 32000-1 §7.8.2 makes the array **one lexical
/// stream** — so the tokenising happens on the concatenation and the writing happens per
/// element. [`remove_glyphs`] is this function over a one-element array, which is what keeps
/// the edit-building in one place: two implementations of "which operations does this glyph
/// belong to" is how they stop agreeing, and the agreement is what stops a cut landing at the
/// wrong offset.
///
/// # Errors
///
/// [`Refusal::GlyphFromAnotherStream`] if any glyph was drawn in a different stream, and
/// [`Refusal::GlyphWithoutItsOperation`] if one is attributed to an operation the content does
/// not contain. Whatever tokenising or splicing refused.
pub fn remove_glyphs_across(
    contents: &super::contents::Contents,
    stream: Option<u64>,
    remove: &[Glyph],
    watch: &Watch<'_>,
) -> Result<Vec<Vec<u8>>> {
    contents.apply(&glyph_edits(contents.bytes(), stream, remove, watch)?)
}

/// As [`remove_glyphs_across`], and also drop the text any covering span carries.
///
/// Returns the rewritten elements **and how many property lists were stripped**, because the
/// caller needs that number for the disclosure and the only other way to get it is to run the
/// covering-span walk a second time. It did: `rewrite` called [`carried_text_edits`] for its
/// `.len()` and then called this, which calls it again — doubling the worst case of the walk
/// this module measured at 18.3s over 250,000 spans, and giving two answers to the question
/// this module's own doc says must have one.
///
/// Both edit sets are computed against the **original** bytes and applied in one pass, because
/// each is a span into those bytes: stripping first would move every glyph span, and stripping
/// afterwards would be looking for glyphs that are no longer there to say which spans covered
/// them. `Contents::apply` owns the offset arithmetic and wants them ascending, so they are
/// merged and sorted rather than applied twice.
///
/// # Errors
///
/// As [`remove_glyphs_across`] and [`carried_text_edits`].
pub fn remove_glyphs_and_carried_text(
    contents: &super::contents::Contents,
    stream: Option<u64>,
    remove: &[Glyph],
    draws: &FormsReached<'_>,
    properties: &NamedProperties,
    watch: &Watch<'_>,
) -> Result<(Vec<Vec<u8>>, usize)> {
    let content = contents.bytes();
    let mut edits = glyph_edits(content, stream, remove, watch)?;
    let carried = carried_text_edits(content, remove, stream, draws, properties, watch)?;
    let dropped = carried.len();
    for (span, replacement) in carried {
        // A PROPERTY LIST SPLIT ACROSS TWO `/Contents` ELEMENTS IS REFUSED BY NAME.
        //
        // §7.8.2 puts the divisions between lexical tokens, so a `BDC`'s dictionary may legally
        // begin in one element and end in the next. `Contents::apply` already refuses to
        // *replace* across a boundary -- deleting across one is defined, replacing is not -- but
        // it refuses with a `Malformed` about content-stream edits, which names no rule and
        // reads as a complaint about the file. A security review measured that: the message
        // blames the user's syntax for a legal document, and `redaction_corpus.rs` would panic
        // on it ("refused without naming a rule") rather than record it.
        //
        // Failing closed is right; failing closed anonymously is not.
        let (from, to) = span;
        if contents.locate(from).map(|(element, _)| element)
            != contents
                .locate(to.saturating_sub(1))
                .map(|(element, _)| element)
        {
            return Refusal::MarkedContentSplitAcrossElements.refuse(
                "a marked-content property list that begins in one /Contents element and ends \
                 in the next, which burrow will not rewrite in place",
            );
        }
        edits.push(super::contents::Edit { span, replacement });
    }
    edits.sort_unstable_by_key(|edit| edit.span);
    Ok((contents.apply(&edits)?, dropped))
}

/// The edits that remove `remove`'s glyphs from `content`, in ascending order.
fn glyph_edits(
    content: &[u8],
    stream: Option<u64>,
    remove: &[Glyph],
    watch: &Watch<'_>,
) -> Result<Vec<super::contents::Edit>> {
    if remove.iter().any(|glyph| glyph.source.form != stream) {
        return Refusal::GlyphFromAnotherStream
            .refuse("a glyph drawn in a different stream, whose span does not index this one");
    }
    let operations = super::ops::operations(content)?;
    // AND ONCE AFTER THE LEX, which is the one step here the watch cannot interrupt: without
    // this, a lex that ran past the deadline was followed by up to 255 more operations unread.
    watch.now()?;
    // GROUPED ONCE, NOT RESCANNED PER OPERATION. This filtered the whole `remove` slice inside
    // the loop, so the cost was operations x removals. A security review found the covering-span
    // walk quadratic in the same two factors and a cursor fixed that one; measuring the result
    // showed 60,000 spans over 60,000 removals still took 13.8 s, which is this loop -- the walk
    // above it was no longer the slowest thing, only the first thing found. 3.6e9 comparisons
    // over an 11,842-byte file.
    //
    // Worth stating because the first fix looked complete and was not: one measurement that
    // varies one factor cannot tell a fixed quadratic from a moved one.
    let mut by_operation: BTreeMap<Span, Vec<&Glyph>> = BTreeMap::new();
    for glyph in remove {
        by_operation
            .entry(glyph.source.operation)
            .or_default()
            .push(glyph);
    }
    let mut edits: Vec<super::contents::Edit> = Vec::new();
    for operation in &operations {
        // THE DEADLINE, per operation (#175).
        watch.tick()?;
        let cuts: &[&Glyph] = by_operation
            .get(&operation.span)
            .map_or(&[], std::vec::Vec::as_slice);
        if !cuts.is_empty() {
            edits.push(super::contents::Edit {
                span: operation.span,
                replacement: rewrite_without(content, operation, cuts)?,
            });
        }
    }
    if edits.len() < count_distinct_operations(remove) {
        return Refusal::GlyphWithoutItsOperation
            .refuse("a glyph attributed to an operation this stream does not contain");
    }
    Ok(edits)
}

/// How many distinct operations the cuts name, so an unmatched one is caught rather than ignored.
fn count_distinct_operations(remove: &[Glyph]) -> usize {
    let mut spans: Vec<Span> = remove.iter().map(|glyph| glyph.source.operation).collect();
    spans.sort_unstable();
    spans.dedup();
    spans.len()
}

/// One drawing operation, rewritten as a `TJ` with its cuts turned into adjustments.
fn rewrite_without(content: &[u8], operation: &Operation, cuts: &[&Glyph]) -> Result<Vec<u8>> {
    // The operation's items, in the order they were written. A `TJ` has its array; the other
    // three show operators have exactly one string, at the operand index `GlyphSource` records.
    let items: Vec<(usize, &Operand)> = match operation.operator.as_slice() {
        b"TJ" => match operation.operands.first() {
            Some(Operand::Array { items, .. }) => items.iter().enumerate().collect(),
            _ => {
                return Refusal::ShowArrayOperandNotAnArray
                    .refuse("a 'TJ' whose operand is not an array");
            }
        },
        b"Tj" | b"'" => vec![(0, show_operand(operation, 0)?)],
        b"\"" => vec![(2, show_operand(operation, 2)?)],
        _ => {
            return Refusal::GlyphOnANonShowingOperation
                .refuse("a glyph attributed to an operation that does not show text");
        }
    };

    // `'` AND `\"` ARE NOT `Tj`. `'` is `T*` then `Tj`; `\"` is `aw Tw`, `ac Tc`, `T*`, then
    // `Tj`. Rewriting either as a bare `TJ` drops the line move and, for `\"`, the two spacing
    // operands -- so everything after the cut moves down a line and every later glyph in the
    // stream gets the wrong word and character spacing.
    //
    // Measured against PDFium by a review: cutting one glyph from `(AB) ' (CD) '` moved every
    // kept glyph **+14 pt in y**, one whole line. The committed differential fixtures could not
    // see it because all three used `Tj` or `TJ` only, and `'`/`\"` are ordinary output from
    // the dvips family. The prefix below restores what the operator did besides showing text.
    let mut out = Vec::new();
    match operation.operator.as_slice() {
        b"'" => out.extend_from_slice(b"T* "),
        b"\"" => {
            let word = number_operand(&operation.operands, 0)?;
            let character = number_operand(&operation.operands, 1)?;
            out.extend_from_slice(format!("{word} Tw {character} Tc T* ").as_bytes());
        }
        _ => {}
    }
    out.push(b'[');
    for (at, item) in items {
        match item {
            // THE PRODUCER'S OWN KERNS, CARRIED THROUGH IN PLACE. They sit between glyphs the
            // cut does not touch, and dropping or reordering one moves everything after it.
            Operand::Number { value, .. } => {
                out.extend_from_slice(format!("{value} ").as_bytes());
            }
            Operand::Str { span } => {
                let raw = content.get(span.0..span.1).ok_or_else(|| {
                    // probe-allowed: reports a bug in this code, not a judgement about a file
                    Error::Internal("pdf geometry: a string's span left its stream".to_owned())
                })?;
                let bytes = super::strings::decode_string(raw)?;
                emit_run(&mut out, &bytes, cuts, at)?;
            }
            _ => {
                return Refusal::ShowArrayItemNotShowable.refuse(
                    "a 'TJ' array holding something that is neither a string nor a number",
                );
            }
        }
    }
    out.extend_from_slice(b"] TJ");
    Ok(out)
}

/// The one string operand of a `Tj`, `'` or `"`.
fn show_operand(operation: &Operation, at: usize) -> Result<&Operand> {
    match operation.operands.get(at) {
        Some(item @ Operand::Str { .. }) => Ok(item),
        _ => Refusal::ShowOperandNotAString
            .refuse("a text-showing operator whose operand is not a string"),
    }
}

/// One string, emitted as kept runs and adjustments for the codes cut out of it.
fn emit_run(out: &mut Vec<u8>, bytes: &[u8], cuts: &[&Glyph], operand: usize) -> Result<()> {
    let mut here: Vec<&Glyph> = cuts
        .iter()
        .copied()
        .filter(|glyph| glyph.source.operand == operand)
        .collect();
    here.sort_by_key(|glyph| glyph.source.code_index);

    let Some(first) = here.first() else {
        emit_hex(out, bytes);
        return Ok(());
    };
    // THE FONT'S WIDTH, and every cut in this string must agree on it. Taking it from one
    // glyph's own chunk was a defect: a short final chunk gave a width of 1 and re-chunked the
    // whole string, so the removal cut a different glyph and returned `Ok`. See
    // `GlyphSource::bytes_per_code`.
    let width = usize::from(first.source.bytes_per_code);
    if width == 0 {
        return Refusal::ZeroBytesPerCode.refuse("a font claiming zero bytes per code");
    }
    if here
        .iter()
        .any(|glyph| glyph.source.bytes_per_code != first.source.bytes_per_code)
    {
        return Refusal::MixedCodeWidths.refuse(
            "glyphs cut from one string disagreeing on the font's code width, so the string \
             cannot be framed",
        );
    }
    if !bytes.len().is_multiple_of(width) {
        return Refusal::StringNotWholeCodes.refuse(
            "a string being edited whose length is not a multiple of the font's code width",
        );
    }

    let mut kept: Vec<u8> = Vec::new();
    let mut cut = here.iter().peekable();
    for (code, chunk) in bytes.chunks(width).enumerate() {
        let cut_here = cut
            .peek()
            .is_some_and(|glyph| glyph.source.code_index == code);
        if cut_here {
            if let Some(glyph) = cut.next() {
                if !kept.is_empty() {
                    emit_hex(out, &kept);
                    kept.clear();
                }
                out.extend_from_slice(format!("{} ", adjustment_for(glyph)?).as_bytes());
            }
        } else {
            kept.extend_from_slice(chunk);
        }
    }
    if !kept.is_empty() {
        emit_hex(out, &kept);
    }
    // A CUT THAT NAMED A CODE THIS STRING DOES NOT HAVE is a glyph attributed to the wrong
    // operand, and silently dropping it would leave the secret on the page while every other
    // check reported success.
    if cut.next().is_some() {
        return Refusal::GlyphWithoutItsOperation
            .refuse("a glyph whose code index is past the end of the string it names");
    }
    Ok(())
}

/// The `TJ` number that reproduces a removed glyph's displacement.
///
/// A number `n` in a `TJ` array displaces by `-n/1000 x Tfs x Tz/100`, so reproducing a
/// displacement `d` needs `n = -d x 1000 / (Tfs x Tz/100)`. The denominator is recovered from
/// the glyph rather than from the graphics state, which no longer exists by the time a removal
/// runs -- and it is the same `Tz`-scaled font size the displacement was computed against, so
/// the two cancel exactly rather than approximately.
fn adjustment_for(glyph: &Glyph) -> Result<f64> {
    let scaled_size = glyph.scaled_font_size;
    if !scaled_size.is_finite() || scaled_size.abs() < f64::EPSILON {
        return Refusal::AdjustmentNotExpressible.refuse(
            "a glyph displaced under a zero font size or a zero 'Tz', so no adjustment \
             reproduces it",
        );
    }
    let number = -glyph.displacement * 1000.0 / scaled_size;
    if number.is_finite() {
        Ok(number)
    } else {
        Refusal::AdjustmentNotExpressible
            .refuse("a displacement with no finite 'TJ' adjustment that reproduces it")
    }
}

/// A run of code bytes as a hex string, which needs no escaping.
fn emit_hex(out: &mut Vec<u8>, bytes: &[u8]) {
    out.push(b'<');
    for byte in bytes {
        out.extend_from_slice(format!("{byte:02x}").as_bytes());
    }
    out.extend_from_slice(b"> ");
}

// ---- the content-stream walk ---------------------------------------------------------------

/// How deep Form XObjects may nest before the walk refuses.
///
/// A form may draw another form, and nothing in a file bounds that. Matching
/// [`MAX_NESTING`](super::ops) would be arbitrary here — that one is about brackets — so this is
/// its own number: deeper than any producer nests (a form inside a form inside a stamp is three)
/// and shallow enough that the work is bounded well before the stack is.
pub const MAX_FORM_DEPTH: usize = 16;

/// How many Form XObject draws one walk will follow in total.
///
/// # Depth is not the bound it looks like
///
/// [`MAX_FORM_DEPTH`] stops recursion and the open-form set stops cycles, and between them they
/// look like a bound. They are not: a form may draw the *next* form `B` times without ever
/// recursing into itself, so the work is `B^16`. Measured in release, from **405 bytes** of
/// content spread over sixteen form objects -- `B = 3` produced 14,348,907 glyphs in 9.7 s at
/// 1,644 MiB, and `B = 4` extrapolates to a billion glyphs and something like 128 GB.
///
/// So the total number of draws is counted across the whole walk, not just the open stack.
pub const MAX_FORM_DRAWS: usize = 4096;

/// How many glyphs one walk will place.
///
/// A page has a knowable maximum. This is far above any real one and far below where `Vec`
/// growth is the problem: `size_of::<Glyph>()` is 120 bytes, so this bounds `out` at ~24 MiB.
pub const MAX_GLYPHS: usize = 200_000;

/// How many operands each operator this walk models takes, or `None` for one it ignores.
///
/// # Why a count mismatch is refused rather than trimmed
///
/// `ops::operations` gathers **every** pending operand into the operation. A renderer keeps a
/// small parameter buffer and takes the **last** N. Reading the **first** N, as this walk did,
/// is a leak with a six-byte exploit: measured, `0 0 0 0 0 0 1 0 0 1 100 700 Tm (SECRET) Tj`
/// puts the glyph at `(100, 700)` in PDFium and at `(0, 0)` here, so a region drawn over the
/// visible word met no glyph box at all and the redaction removed nothing.
///
/// Taking the last N instead would match PDFium. It is not what this does, because matching one
/// renderer's recovery from a malformed operand stack is a guess about every other renderer's,
/// and ADR 0029 licenses refusing where guessing would place glyphs. A padded operand run is
/// malformed; the walk says so.
const fn arity(operator: &[u8]) -> Option<usize> {
    Some(match operator {
        b"q" | b"Q" | b"BT" | b"ET" | b"T*" => 0,
        b"Tc" | b"Tw" | b"Tz" | b"TL" | b"Ts" | b"Do" | b"Tj" | b"TJ" | b"'" | b"BMC" | b"MP" => 1,
        // THE MARKED-CONTENT OPERATORS, which had no count. A reader takes a `BDC`'s tag and
        // property list from its LAST TWO operands; this walk read the tag from the FIRST, so
        // `/Pad /OC /OC1 BDC` hid a layer from the `/OC` mark refusal -- measured by the #166
        // security review, `Ok` over content PDFium and poppler hide. A padded run is refused.
        b"Tf" | b"Td" | b"TD" | b"BDC" | b"DP" => 2,
        b"\"" => 3,
        b"cm" | b"Tm" => 6,
        _ => return None,
    })
}

/// How many operations a walk goes between readings of the clock.
///
/// Reading it per operation would cost a clock call per `q`; reading it per stream is what
/// #175 measured failing. The stride bounds how many operations pass unread, **not** what one
/// operation costs: a single `Tj` of 199,000 glyphs took 39 ms to place, measured by #175's code
/// review. What bounds that is [`MAX_GLYPHS`] per walk and the lexer's operand ceilings, not this.
pub const WATCH_EVERY: u32 = 256;

/// The operation's deadline, carried INTO the walk so it is consulted while the work happens.
///
/// # Why this exists (#175)
///
/// Every `max_duration_ms` checkpoint used to sit between engine calls in the redaction steps, and
/// none inside this module, so one call here was a single uncooperative step of any length.
/// Measured before this type existed, against a budget of 100 ms: a page drawing one form 4,000
/// times, each form holding 100,000 operations, **17.9 s** from a 233 KB file. The form is
/// re-walked at every `Do`, so the cost is draws x form size and nothing noticed it growing.
///
/// Now every walk reads the clock once its stream is lexed, and every [`WATCH_EVERY`] operations
/// after that. A form is walked at every `Do`, so each draw is read at least once. What remains
/// uncooperative is decoding and lexing ONE stream, linear in its decoded size, which qpdf's
/// per-filter memory ceiling (256 MiB) bounds; ADR 0029's #175 amendment has the number.
///
/// Expiry is [`Error::LimitExceeded`] naming `max_duration_ms`, the same error every other
/// checkpoint raises: it is the caller's ceiling, not a property of the document, so it is not a
/// [`Refusal`].
pub struct Watch<'a> {
    deadline: Deadline,
    clock: &'a dyn Clock,
    ticks: Cell<u32>,
}

impl<'a> Watch<'a> {
    /// A watch on `deadline`, read against `clock` -- the operation's own, never a second one.
    #[must_use]
    pub const fn new(deadline: Deadline, clock: &'a dyn Clock) -> Self {
        Self {
            deadline,
            clock,
            ticks: Cell::new(0),
        }
    }

    /// Counts one unit of work, reading the clock every [`WATCH_EVERY`] of them.
    ///
    /// # Errors
    ///
    /// [`Error::LimitExceeded`] once the deadline has passed.
    pub fn tick(&self) -> Result<()> {
        let ticks = self.ticks.get().wrapping_add(1);
        self.ticks.set(ticks);
        if ticks.is_multiple_of(WATCH_EVERY) {
            self.now()
        } else {
            Ok(())
        }
    }

    /// Reads the clock now, whatever the count.
    ///
    /// # Errors
    ///
    /// [`Error::LimitExceeded`] once the deadline has passed.
    pub fn now(&self) -> Result<()> {
        self.deadline.checkpoint(self.clock)
    }
}

impl std::fmt::Debug for Watch<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Watch")
            .field("deadline", &self.deadline)
            .field("ticks", &self.ticks.get())
            .finish_non_exhaustive()
    }
}

/// What one walk has spent, so a page cannot buy unbounded work with a few hundred bytes.
#[derive(Debug)]
struct Budget<'w> {
    /// The operation's deadline, ticked per operation. See [`Watch`].
    watch: &'w Watch<'w>,
    /// The forms currently open, by object identity -- the cycle check.
    open_forms: Vec<u64>,
    /// Every `Do` on a form followed so far, cycles and siblings alike.
    forms_drawn: usize,
    /// The form whose content stream is being walked, or `None` for the caller's own.
    ///
    /// Carried so a `GlyphSource` span is never separated from the stream it indexes. A span
    /// alone would be read against the page and silently name the wrong bytes.
    in_form: Option<u64>,
}

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
#[derive(Debug, Clone, PartialEq)]
pub struct GlyphMetrics {
    /// The advance, in glyph-space units before the font matrix.
    pub width: f64,
    /// How many bytes of the string this code took — 1 for a simple font, 2 for Identity-H.
    ///
    /// Carried because word spacing depends on it; see [`takes_word_spacing`].
    pub bytes_per_code: u8,
    /// The font's `/FontBBox`, in glyph space.
    pub font_bbox: Option<Rect>,
    /// Glyph space to text space -- `/FontMatrix` for a Type 3, 0.001 otherwise.
    pub font_matrix: Matrix,
    /// How the font's codes are encoded, from which the walk derives the writing mode.
    ///
    /// # Why this is the encoding and not a `WritingMode`
    ///
    /// It used to be a `WritingMode`, supplied by whoever implements [`Resources`]. That made
    /// the whole `WMode` rule true at unit level and **nothing at all at document level**: a
    /// resolver that simply never read the CMap would return `Horizontal`, silently, with no
    /// refusal and no compile error. The dangerous answer was the one you got by doing nothing.
    ///
    /// So the seam carries what the *file* says and the walk draws the conclusion, through
    /// [`writing_mode_of`]. A resolver can no longer omit the question; it can only answer it,
    /// name a predefined CMap, hand over the program, or say it could not read the stream --
    /// and that last one is [`Encoding::UnreadableCMap`], which is refused.
    pub encoding: Encoding,
}

/// How a font's codes are encoded, as the resolver found it in the file.
///
/// Every arm is a statement a resolver has to make deliberately. There is no arm meaning
/// "I did not look", which is the point: see [`GlyphMetrics::encoding`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Encoding {
    /// A simple font -- Type 1, TrueType, Type 3 -- whose `/Encoding` is a name or a
    /// difference list rather than a CMap.
    ///
    /// Horizontal by construction: `WMode` is a property of a CMap, and a simple font has
    /// none. Single-byte by construction too, which the walk checks rather than assumes.
    Simple,
    /// A predefined CMap, by registry name. The document carries no program.
    Predefined(Vec<u8>),
    /// An embedded CMap stream the resolver read: its dictionary's `/WMode` and its program.
    Embedded {
        /// `/WMode` on the CMap stream dictionary, if the file states one.
        dictionary_wmode: Option<i64>,
        /// The CMap program, decoded.
        program: Vec<u8>,
    },
    /// An embedded CMap stream the resolver could **not** read.
    ///
    /// A filter it does not implement, a decode that failed, a stream that is not there. The
    /// walk refuses it: a CMap nobody read is a writing mode nobody knows, and guessing
    /// horizontal is the guess that boxes vertical text in the wrong place and misses it.
    UnreadableCMap,
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

    /// The resources the Form XObject named `name` draws against, or `None` when it declares
    /// none and inherits the enclosing ones.
    ///
    /// # This existed as a comment before it existed as a method, and the gap was a leak
    ///
    /// [`Form`]'s own rustdoc says *"each form carries its own `/Resources`"*, `prune` walks
    /// them and `sharing` walks them — and this walk did not. `draw_form` recursed with the
    /// **caller's** resolver, so a `Tf /F1` inside a form resolved `/F1` against the **page's**
    /// `/Font` dictionary.
    ///
    /// Measured, with a page `/F1` of zero widths and a form-local `/F1` of real ones: PDFium
    /// renders twelve characters spanning x=73 to x=233; burrow placed all twelve in a
    /// zero-width box at x=72. A region over the rendered text reached nothing, the redaction
    /// returned `Ok`, both verification passes agreed — and **seven characters were still
    /// drawn inside the rectangle the user selected**. The font surgery then narrowed the page
    /// font, whose codes had nothing to do with the removal.
    ///
    /// **`redact_verify` cannot catch this**, because both passes call the same walk and
    /// misplace the glyphs identically. It is not the `#111` placement residue ADR 0029 §6
    /// discloses — that residue is imprecision, and this was a resolution defect PDFium
    /// disagrees with outright.
    ///
    /// # Errors
    ///
    /// Whatever resolving the form or its resources failed with.
    fn within(&self, name: &[u8]) -> Result<Option<Box<dyn Resources + '_>>>;
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
pub fn glyphs_in(
    content: &[u8],
    resources: &dyn Resources,
    watch: &Watch<'_>,
) -> Result<Vec<Glyph>> {
    let mut out = Vec::new();
    let mut budget = Budget {
        watch,
        open_forms: Vec::new(),
        forms_drawn: 0,
        in_form: None,
    };
    walk(
        content,
        resources,
        GraphicsState {
            ctm: Matrix::IDENTITY,
            text: TextState::default(),
            font: None,
        },
        &mut budget,
        &mut out,
    )?;
    Ok(out)
}

/// One numeric operand, refused rather than defaulted.
///
/// # No file bytes in the message
///
/// An earlier draft named the operator with `String::from_utf8_lossy`, which is up to 255
/// attacker-chosen bytes in an error string. `core/CLAUDE.md`: errors describe the failure, not
/// the input. The position is a `usize` this code chose; nothing here comes from the file.
fn number_operand(operands: &[Operand], at: usize) -> Result<f64> {
    let Some(value) = operands.get(at).and_then(Operand::as_number) else {
        return Refusal::NumericOperandNotANumber
            .refuse("an operator whose numeric operand is missing or is not a number");
    };
    // NON-FINITE FAILS CLOSED. Two `cm` of 1e300 compose to an infinity, `inf * 0` in
    // `Matrix::then` is a NaN, and `Rect::transformed` folds NaN corners with `min`/`max` --
    // which leaves its `+inf/-inf` initialisers untouched and returns an inverted rectangle
    // that intersects nothing. A box that intersects nothing is a glyph a redaction skips.
    if !value.is_finite() {
        return Refusal::NonFiniteGeometry
            .refuse("an operand that is not a finite number, so no box derived from it is one");
    }
    Ok(value)
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
    budget: &mut Budget<'_>,
    out: &mut Vec<Glyph>,
) -> Result<()> {
    let operations = super::ops::operations(content)?;
    // AND ONCE AFTER THE LEX, which is the one step here the watch cannot interrupt: without
    // this, a lex that ran past the deadline was followed by up to 255 more operations unread.
    budget.watch.now()?;

    let mut state = initial;
    let mut stack: Vec<GraphicsState> = Vec::new();
    // `Some` between `BT` and `ET`. The matrices live here rather than in `GraphicsState`
    // because `q`/`Q` do NOT save them -- they are reset by `BT` and nothing else touches them.
    let mut position: Option<TextPosition> = None;

    for operation in &operations {
        // THE DEADLINE, INSIDE THE WALK (#175). Before it, a form drawn 4,000 times walked for
        // 17.9 s against a 100 ms budget, because nothing here could see the clock.
        budget.watch.tick()?;
        // FALLIBLE, and the reason is `Tz`. An operand that is missing or is not a number used
        // to become `0.0`, so `/Bogus Tz` set the horizontal scale to zero and collapsed every
        // glyph box on the page to a point -- a box that intersects almost nothing, so a
        // redaction over it removes almost nothing. The operand-shape rules already refuse a
        // `Do` or a `TJ` of the wrong shape; a number is no different.
        let number = |at: usize| number_operand(&operation.operands, at);
        // THE COUNT, BEFORE ANYTHING READS AN OPERAND. See `arity`.
        if let Some(expected) = arity(&operation.operator)
            && operation.operands.len() != expected
        {
            return Refusal::OperandCountMismatch
                .refuse("an operator given more or fewer operands than it takes");
        }
        match operation.operator.as_slice() {
            // A PATTERN CAN DRAW TEXT THIS WALK DOES NOT REACH. Measured: a page whose only
            // text lives in a tiling pattern walks to `Ok(0)` while PDFium inks 740 pixels of
            // it -- and `FPDFText_*` reports zero characters too, so the §6 read-back is blind
            // to it as well. An `Ok` over text nothing observed is the one outcome this module
            // exists to prevent, so the pattern is refused until it is walked.
            b"scn" | b"SCN" if matches!(operation.operands.last(), Some(Operand::Name { .. })) => {
                return Refusal::PatternMayDrawText
                    .refuse("a pattern fill, whose own content stream burrow does not yet walk");
            }
            // AN OPTIONAL-CONTENT MARK, whatever it names. See the variant for the untyped
            // membership dictionary a type-keyed signal walked past.
            b"BDC"
                if matches!(operation.operands.first(),
                    Some(Operand::Name { value, .. }) if value.as_slice() == b"OC") =>
            {
                return Refusal::OptionalContentMarked.refuse(
                    "this page marks some of its content as belonging to a layer (optional \
                     content), which can hide it from view",
                );
            }
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
                    a: number(0)?,
                    b: number(1)?,
                    c: number(2)?,
                    d: number(3)?,
                    e: number(4)?,
                    f: number(5)?,
                };
                state.ctm = m.then(&state.ctm);
                if !state.ctm.is_finite() {
                    return Refusal::NonFiniteGeometry
                        .refuse("a transform that composes to a value that is not finite");
                }
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
            b"Tc" => state.text.char_spacing = number(0)?,
            b"Tw" => state.text.word_spacing = number(0)?,
            b"Tz" => state.text.horizontal_scale = number(0)?,
            b"TL" => state.text.leading = number(0)?,
            b"Ts" => state.text.rise = number(0)?,
            b"Tf" => {
                state.text.font_size = number(1)?;
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
                draw_form(value, resources, &state, budget, out)?;
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
                text_operator(
                    content, operation, &mut state, place, resources, budget, out,
                )?;
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
    budget: &mut Budget<'_>,
    out: &mut Vec<Glyph>,
) -> Result<()> {
    let Some(form) = resources.form(name)? else {
        // An image, or anything else that is not a form. It draws no glyphs.
        return Ok(());
    };

    // A CYCLE IS A REFUSAL, not a stop. A form that draws itself has no finite glyph list, and
    // returning the glyphs found before the loop was noticed would report a page as containing
    // less than it does. Keyed on object identity, not on the name -- see `Form::id`.
    // THE TOTAL, not the depth. See `MAX_FORM_DRAWS`.
    budget.forms_drawn += 1;
    if budget.forms_drawn > MAX_FORM_DRAWS {
        return Refusal::TooManyFormDraws
            .refuse("more Form XObject draws on one page than burrow will follow");
    }
    if budget.open_forms.contains(&form.id) {
        return Refusal::FormCycle
            .refuse("a Form XObject draws itself, directly or through another form");
    }
    if budget.open_forms.len() >= MAX_FORM_DEPTH {
        return Refusal::FormDepth.refuse("Form XObjects nested deeper than burrow will walk");
    }

    budget.open_forms.push(form.id);
    // AND WHICH STREAM THE SPANS BELOW WILL INDEX. Restored on the way out, including on the
    // error path, so a refusal deep in a form cannot leave the caller recording page spans
    // against a form's bytes.
    let enclosing = budget.in_form.replace(form.id);
    // THE FORM'S MATRIX COMPOSES WITH THE CTM AT THE `Do`, in that order. The other order puts
    // the form's own transform outside the page's, which is plausible and wrong.
    // THE FORM'S OWN RESOURCES, falling back to the enclosing ones where it declares none.
    // Recursing with the caller's was a leak; see `Resources::within` for the measurement.
    let scoped = resources.within(name)?;
    let inner: &dyn Resources = scoped.as_deref().unwrap_or(resources);
    let result = walk(
        &form.content,
        inner,
        GraphicsState {
            ctm: form.matrix.then(&state.ctm),
            ..state.clone()
        },
        budget,
        out,
    );
    budget.in_form = enclosing;
    budget.open_forms.pop();
    result
}

/// One text-placing or text-showing operator.
fn text_operator(
    content: &[u8],
    operation: &Operation,
    state: &mut GraphicsState,
    place: &mut TextPosition,
    resources: &dyn Resources,
    budget: &Budget<'_>,
    out: &mut Vec<Glyph>,
) -> Result<()> {
    // Fallible for the same reason as the one in `walk`: see the comment there.
    let number = |at: usize| number_operand(&operation.operands, at);
    match operation.operator.as_slice() {
        b"Tm" => {
            let m = Matrix {
                a: number(0)?,
                b: number(1)?,
                c: number(2)?,
                d: number(3)?,
                e: number(4)?,
                f: number(5)?,
            };
            place.text = m;
            place.line = m;
        }
        b"Td" => place.next_line_at(number(0)?, number(1)?),
        b"TD" => {
            // `TD` SETS THE LEADING TOO, to the NEGATIVE of its second operand. A walk that
            // treated it as `Td` would leave `TL` at whatever it was, so every later `T*` on the
            // page moves by the wrong amount.
            state.text.leading = -number(1)?;
            place.next_line_at(number(0)?, number(1)?);
        }
        b"T*" => place.next_line_at(0.0, -state.text.leading),
        b"'" => {
            place.next_line_at(0.0, -state.text.leading);
            show(
                &Shown {
                    content,
                    at: (operation.span, 0),
                    form: budget.in_form,
                },
                operation.operands.first(),
                state,
                place,
                resources,
                out,
            )?;
        }
        b"\"" => {
            // `aw ac string "` sets word spacing, then character spacing, then does `'`.
            state.text.word_spacing = number(0)?;
            state.text.char_spacing = number(1)?;
            place.next_line_at(0.0, -state.text.leading);
            show(
                &Shown {
                    content,
                    at: (operation.span, 2),
                    form: budget.in_form,
                },
                operation.operands.get(2),
                state,
                place,
                resources,
                out,
            )?;
        }
        b"Tj" => show(
            &Shown {
                content,
                at: (operation.span, 0),
                form: budget.in_form,
            },
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
            for (operand, item) in items.iter().enumerate() {
                match item {
                    Operand::Str { .. } => {
                        show(
                            &Shown {
                                content,
                                at: (operation.span, operand),
                                form: budget.in_form,
                            },
                            Some(item),
                            state,
                            place,
                            resources,
                            out,
                        )?;
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
/// Where a shown string sits, and in which stream -- the provenance half of `show`'s arguments.
struct Shown<'a> {
    /// The stream the span indexes.
    content: &'a [u8],
    /// The drawing operation's span, and which operand of it holds the string.
    at: (Span, usize),
    /// Which form's stream that is, or `None` for the caller's own.
    form: Option<u64>,
}

fn show(
    shown: &Shown<'_>,
    operand: Option<&Operand>,
    state: &GraphicsState,
    place: &mut TextPosition,
    resources: &dyn Resources,
    out: &mut Vec<Glyph>,
) -> Result<()> {
    let (content, at) = (shown.content, shown.at);
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
        // probe-allowed: reports a bug in this code, not a judgement about a file
        Error::Internal("pdf geometry: a string's span left its content stream".to_owned())
    })?;
    let bytes = super::strings::decode_string(raw)?;
    let per_code = resources.bytes_per_code(&font)?;
    if per_code == 0 {
        return Refusal::ZeroBytesPerCode.refuse("a font claiming zero bytes per code");
    }
    // A STRING THAT DOES NOT DIVIDE INTO WHOLE CODES has no unambiguous reading. `chunks`
    // would yield a short final chunk and the walk would report a glyph for a code the font
    // does not have, which a removal then cannot place back. Refused here rather than papered
    // over, because a phantom glyph is a wrong answer about what the page draws.
    if bytes.len() % usize::from(per_code) != 0 {
        return Refusal::StringNotWholeCodes.refuse(
            "a shown string whose length is not a multiple of the font's bytes per code, so \
             which codes it holds is not derivable",
        );
    }

    for (index, chunk) in bytes.chunks(usize::from(per_code)).enumerate() {
        let mut code = 0_u32;
        for byte in chunk {
            code = (code << 8) | u32::from(*byte);
        }
        let metrics = resources.glyph(&font, code)?;
        // THE WALK DRAWS THE CONCLUSION, from what the file says. See `GlyphMetrics::encoding`.
        check_writing_mode(writing_mode_for(&metrics.encoding, per_code)?)?;
        // THE METRICS COME OUT OF THE FILE TOO. Checking the content stream's operands and the
        // composed CTM left this open: a `/W` entry of `f64::MAX` against a `/FontMatrix` of
        // 1e297 multiplies to an infinity, and the box built from it was
        // `left: -inf, right: inf` -- which contains everything, and whose union with anything
        // is still infinite. Found by `pdfsyntax_geometry` on its second run, in seconds.
        //
        // A box that spans the page is not the leak direction the way an inverted one is, but
        // it makes every region test true, which is a redaction that removes the whole page and
        // reports success. Neither answer is one to guess at.
        if !metrics.width.is_finite()
            || !metrics.font_matrix.is_finite()
            || metrics.font_bbox.is_some_and(|bbox| !bbox.is_finite())
        {
            return Refusal::NonFiniteGeometry
                .refuse("a font declaring a width, matrix or bounding box that is not finite");
        }

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
        if !to_page.is_finite() {
            return Refusal::NonFiniteGeometry
                .refuse("a glyph transform that composes to a value that is not finite");
        }
        // TEXT SPACE -> PAGE SPACE. The same tail as `to_page` without the font matrix and the
        // font size, because the advance and the font size are already text-space quantities.
        let text_to_page = Matrix::translate(0.0, state.text.rise)
            .then(&place.text)
            .then(&state.ctm);
        // NO FINITENESS CHECK OF ITS OWN, and its absence is measured. `to_page` is
        // `(font matrix x scale) x text_to_page`, so any non-finite entry here reaches `to_page`
        // through an added translation or `0 x inf = NaN`, and the check above refuses first. The
        // guard that stood here could not fire; the #177 reachability gate, which requires a
        // witness for every raise site, is what showed it.
        let origin = place.text.then(&state.ctm).apply(0.0, state.text.rise);

        let width = metrics.width * metrics.font_matrix.a;
        if out.len() >= MAX_GLYPHS {
            return Refusal::TooManyGlyphs.refuse("more glyphs on one page than burrow will place");
        }
        // THE DISPLACEMENT, computed once, here, and both used to move the pen and recorded on
        // the glyph. Word spacing applies ONLY to single-byte code 32; see `takes_word_spacing`.
        let word = if takes_word_spacing(code, metrics.bytes_per_code) {
            state.text.word_spacing
        } else {
            0.0
        };
        let displacement = (width * state.text.font_size + state.text.char_spacing + word) * scale;

        let glyph = Glyph {
            origin,
            to_page,
            text_to_page,
            advance: width * state.text.font_size,
            font_bbox: metrics.font_bbox,
            font_size: state.text.font_size,
            scaled_font_size: state.text.font_size * scale,
            displacement,
            source: GlyphSource {
                font: ScopedFont::new(shown.form, font.clone()),
                code,
                form: shown.form,
                operation: at.0,
                operand: at.1,
                code_index: index,
                bytes_per_code: per_code,
            },
        };

        // THE DERIVED VALUES, checked before the glyph is accepted, because each input above
        // can be finite while the product is not: `f64::MAX` times a `/FontMatrix` of 1e297 is
        // an infinity, and so is a finite `/FontBBox` through a large enough transform. This is
        // the one check that sees a value no single input predicts, and the fuzzer found it in
        // seconds on the target's second run.
        //
        // The failing direction is the mirror of an inverted box: an infinite one INTERSECTS
        // EVERYTHING, so a redaction built on it removes the whole page and reports success.
        // A guess in either direction is wrong, so neither is made.
        if !glyph.conservative_box().is_finite()
            || !glyph.origin.0.is_finite()
            || !glyph.origin.1.is_finite()
        {
            return Refusal::NonFiniteGeometry
                .refuse("a glyph whose box is not finite, so no region test over it is one");
        }
        out.push(glyph);
        // THE SAME VALUE that was recorded, not a second copy of the expression.
        place.text = Matrix::translate(displacement, 0.0).then(&place.text);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use burrow_types::{Deadline, Error, Limits, ManualClock, Result};

    use std::collections::{BTreeMap, BTreeSet};

    use super::{
        CMap, Encoding, Form, FormUses, FormsReached, Glyph, GlyphMetrics, MAX_FORM_DEPTH,
        MAX_GLYPHS, Matrix, NamedProperties, Operand, PropertyList, RAISED, Rect, Refusal,
        Resources, TextPosition, TextState, WATCH_EVERY, Watch, WritingMode, carried_text_edits,
        check_form_sharing, check_type_three_procedure, check_writing_mode, glyphs_in,
        remove_glyphs, remove_glyphs_and_carried_text, takes_word_spacing, writing_mode_of,
    };

    /// A watch that never expires: a STOPPED clock, so the walk's checkpoints are inert.
    ///
    /// For tests of what the walk computes. The deadline itself is tested with a clock that moves;
    /// a stopped one here is deliberate and named so, because a stopped clock in a production path is
    /// exactly how `max_duration_ms` stopped existing once before.
    fn unwatched() -> Watch<'static> {
        static STOPPED: ManualClock = ManualClock::new(0);
        Watch::new(Deadline::start(&STOPPED, &Limits::DEFAULT), &STOPPED)
    }

    /// A resources table with one font of known width and whatever forms a test names.
    ///
    /// Widths are 500/1000 so an advance is half the font size -- a number a reader can check in
    /// their head, which is what makes a failure message useful rather than a pair of decimals.
    struct Fake {
        forms: Vec<(Vec<u8>, Form)>,
        encoding: Encoding,
        bytes_per_code: u8,
        width: f64,
        font_matrix_scale: f64,
    }

    impl Fake {
        fn new() -> Self {
            Self {
                forms: Vec::new(),
                encoding: Encoding::Simple,
                bytes_per_code: 1,
                width: 500.0,
                font_matrix_scale: 0.001,
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
        fn within(&self, _name: &[u8]) -> Result<Option<Box<dyn Resources + '_>>> {
            // ONE FLAT RESOURCE SET; see the fakes in `tests/glyph_geometry.rs` for why this
            // is stated rather than defaulted.
            Ok(None)
        }

        fn form(&self, name: &[u8]) -> Result<Option<Form>> {
            Ok(self
                .forms
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, f)| f.clone()))
        }

        fn glyph(&self, _name: &[u8], _code: u32) -> Result<GlyphMetrics> {
            Ok(GlyphMetrics {
                width: self.width,
                bytes_per_code: self.bytes_per_code,
                font_bbox: Some(Rect {
                    left: 0.0,
                    bottom: 0.0,
                    right: 1000.0,
                    top: 1000.0,
                }),
                font_matrix: Matrix::scale(self.font_matrix_scale, self.font_matrix_scale),
                encoding: self.encoding.clone(),
            })
        }

        fn bytes_per_code(&self, _name: &[u8]) -> Result<u8> {
            Ok(self.bytes_per_code)
        }
    }

    fn placed(content: &str) -> Vec<Glyph> {
        glyphs_in(content.as_bytes(), &Fake::new(), &unwatched()).expect("the fixture should walk")
    }

    // ---- the refusal names themselves are checked ------------------------------------------

    #[test]
    fn every_rule_name_is_distinct() {
        // Two variants sharing a name makes `caught` accept the wrong one, which is exactly the
        // looseness this enum exists to remove.
        let mut names: Vec<&str> = Refusal::ALL.iter().map(|rule| rule.rule()).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "two refusals share a rule name");

        // `ALL` IS HAND-MAINTAINED, and its doc used to claim it could not drift. It could: a
        // variant added to the enum and to `rule()` -- both of which the compiler forces -- but
        // not to `ALL` was never raised, never tested, never checked for a name collision, and
        // silent in every probe. So the expectation is derived from the enum's own source
        // rather than from the list that is the thing at risk.
        let source = include_str!("geometry.rs");
        let body = source
            .split_once("pub enum Refusal {")
            .and_then(|(_, rest)| rest.split_once("\n}\n"))
            .expect("the enum's source brackets the variants")
            .0;
        let in_enum = body
            .lines()
            .map(str::trim)
            .filter(|line| {
                line.ends_with(',')
                    && !line.starts_with("//")
                    && line.chars().next().is_some_and(char::is_uppercase)
            })
            .count();
        assert_eq!(
            total, in_enum,
            "`Refusal::ALL` lists {total} of the enum's {in_enum} variants"
        );
        assert_eq!(
            total, 38,
            "a refusal was added or removed without updating the probes"
        );
    }

    #[test]
    fn a_rule_catches_its_own_refusal_and_rejects_every_other() {
        // THE PER-RULE PROBE. A `caught` that matched everything would make every refusal test
        // in this module vacuous, and a `caught` that matched nothing would make them all fail
        // for the wrong reason. Both directions, for every ordered pair of rules.
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
    fn caught_anchors_on_the_message_rather_than_searching_it() {
        // `contains` accepted an error this module never built. `caught` is the oracle every
        // refusal test here rests on, so a tag appearing anywhere must not satisfy it.
        // probe-allowed: the whole point of this test is an error this module did not build
        let foreign = Error::Unsupported("qpdf: /Annot [form-cycle] in the document".to_owned());
        assert!(
            !Refusal::FormCycle.caught(&foreign),
            "caught accepted a refusal this module did not build"
        );
        let ours = Refusal::FormCycle
            .refuse::<()>("a detail")
            .expect_err("refuses");
        assert!(Refusal::FormCycle.caught(&ours));
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
            exempt.lines().count() <= 40,
            "the exempt window has grown past the two functions it is for: {} lines",
            exempt.lines().count()
        );
        // WHAT WAS EXAMINED, as a number with an expectation beside it. Without this the probe
        // could scan two lines and report the same silence it reports over a clean file -- and
        // "4 of 15" reads exactly like success.
        let mut allowlisted = 0_usize;
        let examined = before.lines().count() + after.lines().count();
        assert_eq!(
            examined + exempt.lines().count(),
            // `+ 2` because `split_once` cuts mid-line at each of the two markers, so the two
            // boundary lines are each counted in both of the halves they straddle.
            source.lines().count() + 2,
            "scanned {examined} lines of {}; the split lost some",
            source.lines().count()
        );
        for (half, label) in [(before, "before the exempt window"), (after, "after it")] {
            // COMMENTS ARE NOT CONSTRUCTION SITES. The doc on `Refusal` quotes the loose shape
            // in order to argue against it, and a probe that could not tell the two apart would
            // have to be weakened somewhere instead.
            // A VISIBLE, GREPPABLE ALLOWLIST. Two lines in this file build an `Error`
            // outside the refusal vocabulary on purpose -- the `Internal` in `show`, and the
            // foreign error the `caught` test needs -- and each says so on its own line, where
            // a reviewer reading the diff sees it. An allowlist matched on message text would
            // have silently widened the day someone reworded the message.
            // The marker sits on the line ABOVE the construction, where a Rust comment
            // belongs, so the filter looks back one line rather than demanding it trail.
            let marker = concat!("// probe-", "allowed:");
            let lines: Vec<&str> = half.lines().collect();
            allowlisted += lines.iter().filter(|l| l.contains(marker)).count();
            for (at, line) in lines.iter().enumerate() {
                if line.trim_start().starts_with("//")
                    || at
                        .checked_sub(1)
                        .and_then(|prior| lines.get(prior))
                        .is_some_and(|prior| prior.contains(marker))
                {
                    continue;
                }
                // `Error::Internal` IS ON THE LIST TOO. It is not a refusal -- it reports a
                // bug in this code rather than a judgement about a file -- but leaving it off
                // meant the probe could not tell the one deliberate use from a new one, and a
                // reviewer used exactly that to swap a deleted refusal for an `Internal` with
                // both probes green. One allowlisted site, named, and everything else refused.
                for bare in [
                    concat!("Error::", "Malformed("),
                    concat!("Error::", "Unsupported("),
                    concat!("Error::", "Internal("),
                ] {
                    assert!(
                        !line.contains(bare),
                        "a bare `{bare}` in {label}: build it through `Refusal` so a test can \
                         name it -- {line}"
                    );
                }
            }
        }
        // AND HOW MANY WERE EXEMPTED, with an expectation beside it. An allowlist that grows
        // unnoticed is how a structural probe stops being one.
        assert_eq!(
            allowlisted, 8,
            "the allowlist holds {allowlisted} lines; it is for exactly eight -- the two \
             `Internal`s reporting a span that left its stream, the splice invariant, the \
             foreign error the `caught` test builds, and the two the `/ActualText` rewriter \
             adds: a property-list span outside its stream, and a property list that does not \
             re-read as a dictionary, an operand span outside the property list it came from, \
             and a rewritten property list that still carries text. All four of the rewriter's \
             are burrow disagreeing with itself about bytes it just parsed, which is what \
             `Internal` is for"
        );
    }

    /// Drive one witness for `rule` and return the production line that raised it.
    ///
    /// # What replaced a text search, and what this measures
    ///
    /// The gate this replaces checked that `Refusal::X.refuse(` appeared in the production half
    /// and `Refusal::X)` in the test half. A code review planted `Refusal::NeverReachable`:
    /// added to the enum and `ALL`, raised only inside `if never && !never`, and "tested" by
    /// naming it in an array. **557 passed, 0 failed** (#177). The gate's name claimed
    /// reachability and the check was a grep.
    ///
    /// Now each raise **site** has a witness: real input driven through this module's code. A
    /// site counts only if the **last** refusal `refuse` recorded during the witness is this
    /// rule, raised from that site, above `mod tests`. It has to be the last one, because an
    /// earlier production raise, discarded, would otherwise vouch for an error the test built.
    ///
    /// **What it does not measure, stated because the first version of this comment claimed
    /// it did:** that the site is reachable **from the operation**. Some witnesses call a
    /// checker directly (`check_type_three_procedure`, `check_form_sharing`, the marked-content
    /// entry points, `writing_mode_of`) or forge a `Glyph`'s source. For those, this proves the
    /// site can be raised by this module's code on some input; **the wiring from the operation
    /// is the integration suite's** (`tests/redaction_defences.rs`, named for the mutations it
    /// kills). A code review planted a rule raised only by a `pub fn` production never calls,
    /// and this gate passed it -- correctly, by what it measures.
    ///
    /// # Errors
    ///
    /// A sentence naming the rule and what was wrong, for the gate and its own probes to assert on.
    fn prove(rule: Refusal, witness: Witness) -> core::result::Result<u32, String> {
        let tests_start = tests_start()?;
        RAISED.with(|raised| raised.borrow_mut().clear());
        let outcome = witness();
        let raised = RAISED.with(|raised| raised.borrow().clone());
        let error = match outcome {
            Ok(()) => return Err(format!("`{}`: its witness did not refuse", rule.rule())),
            Err(error) => error,
        };
        if !rule.caught(&error) {
            return Err(format!(
                "`{}`: its witness refused by another rule: {error:?}",
                rule.rule()
            ));
        }
        // THE FILE AS WELL AS THE LINE: a raise pulled in by `include!` would report its own
        // line numbers, which would read as production here.
        match raised.last() {
            Some((what, file, line))
                if *what == rule
                    && file.ends_with("pdfsyntax/geometry.rs")
                    && *line < tests_start =>
            {
                Ok(*line)
            }
            _ => Err(format!(
                "`{}` was not raised by production code as the last refusal: recorded \
                 {raised:?}, and the tests begin at line {tests_start}",
                rule.rule()
            )),
        }
    }

    /// The line `mod tests {` is on, which divides production from tests.
    fn tests_start() -> core::result::Result<u32, String> {
        include_str!("geometry.rs")
            .lines()
            .position(|line| line.trim() == "mod tests {")
            .and_then(|at| u32::try_from(at + 1).ok())
            .ok_or_else(|| "the test module marker is missing".to_owned())
    }

    /// Every raise site in `source`'s production half: the line of each `.refuse(`, and the
    /// rule it raises.
    ///
    /// # Errors
    ///
    /// A site whose receiver is not a literal `Refusal::Name`. A helper taking the rule as a
    /// parameter would record the helper's line for every rule it raised, so every witness
    /// through it would pass the gate on the helper's behalf. Refused rather than allowed, so the
    /// per-site measurement cannot be diluted by a refactor.
    fn raise_sites(source: &str) -> core::result::Result<BTreeMap<u32, String>, String> {
        let lines: Vec<&str> = source.lines().collect();
        let end = lines
            .iter()
            .position(|line| line.trim() == "mod tests {")
            .ok_or("the test module marker is missing")?;
        let receiver = |text: &str| -> Option<String> {
            let name = text
                .trim_end()
                .rsplit_once("Refusal::")
                .map(|(_, name)| name)?;
            (!name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric()))
                .then(|| name.to_owned())
        };
        let mut sites = BTreeMap::new();
        for (at, line) in lines.iter().enumerate().take(end) {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") || trimmed.contains("fn refuse<") {
                continue;
            }
            let Some((before, _)) = line.split_once(".refuse(") else {
                continue;
            };
            let named = if before.trim().is_empty() {
                // RUSTFMT WRAPPED IT: the receiver ends the previous line.
                at.checked_sub(1)
                    .and_then(|prior| lines.get(prior))
                    .and_then(|prior| receiver(prior))
            } else {
                receiver(before)
            };
            let name = named.ok_or_else(|| {
                format!(
                    "line {}: a raise whose receiver is not a named rule -- {}",
                    at + 1,
                    line.trim()
                )
            })?;
            let number = u32::try_from(at + 1).map_err(|_| "a line past u32".to_owned())?;
            sites.insert(number, name);
        }
        Ok(sites)
    }

    /// How a witness reaches its raise site, so the count says what kind of reach it is.
    ///
    /// # Three kinds, because a security review measured the difference
    ///
    /// The first count said "60 of 60 raise sites" over witnesses of very different strength. A
    /// review showed that `GlyphFromAnotherStream` was reported reachable through
    /// `remove_glyphs`, which **no production code calls**, while production filters the glyphs
    /// it passes so that the other site cannot fire either. Several witnesses forge a
    /// `Glyph`'s source into a state no walk produces. Those are invariant checks, and a
    /// refusal signal built on them (#125) would count as refusable a shape no file triggers.
    /// So each witness says which kind it is, and the gate reports all three.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Reach {
        /// Content bytes walked by `glyphs_in`, against a resolver answering as the
        /// [`super::Resources`] trait allows: what a document drives.
        Content,
        /// A public function the redaction steps call, named, with inputs a walk produced. The
        /// gate checks the name is called in `redact/steps.rs`.
        Entry(&'static str),
        /// Forged state no walk produces, or a function no production code calls: an
        /// invariant check, not a refusal a file can trigger.
        Invariant,
    }

    /// Real input, driven through real code, that must refuse.
    type Witness = fn() -> Result<()>;

    /// A walk of `content` against the default resources, as a witness's outcome.
    fn walk(content: &str) -> Result<()> {
        glyphs_in(content.as_bytes(), &Fake::new(), &unwatched()).map(drop)
    }

    /// A walk against `resources`, as a witness's outcome.
    fn walk_with(content: &str, resources: &Fake) -> Result<()> {
        glyphs_in(content.as_bytes(), resources, &unwatched()).map(drop)
    }

    /// A two-byte font encoded by `encoding`, walked.
    ///
    /// Through `glyphs_in` and so through `writing_mode_for`, which is how production reaches
    /// the writing-mode rules. The witnesses called `writing_mode_of` directly, and a review
    /// planted `writing_mode_for` swallowing its refusal as horizontal: the gate and the whole
    /// suite stayed green, 733 passed.
    fn walk_encoded(encoding: Encoding) -> Result<()> {
        let mut resources = Fake::new();
        resources.bytes_per_code = 2;
        resources.encoding = encoding;
        walk_with("/F1 10 Tf BT 0 0 Td (AB) Tj ET", &resources)
    }

    /// An embedded CMap, walked.
    fn walk_cmap(dictionary_wmode: Option<i64>, program: &[u8]) -> Result<()> {
        walk_encoded(Encoding::Embedded {
            dictionary_wmode,
            program: program.to_vec(),
        })
    }

    /// Remove `remove` from `content` through the combined pass, which production calls.
    fn cut(content: &[u8], remove: &[Glyph]) -> Result<()> {
        let contents = super::super::contents::Contents::concatenate(&[content])?;
        remove_glyphs_and_carried_text(
            &contents,
            None,
            remove,
            &FormsReached::Named(&BTreeSet::new()),
            &NamedProperties::default(),
            &unwatched(),
        )
        .map(drop)
    }

    /// The glyph `walk_content` places first, altered by `forge`, cut from `edit_content`.
    ///
    /// The removal path's own checks are reached only by a glyph that disagrees with the bytes it
    /// is cut from, which no walk of those bytes produces -- so these are [`Reach::Invariant`].
    fn cut_forged(walk_content: &str, edit_content: &str, forge: fn(&mut Glyph)) -> Result<()> {
        let (mut glyphs, _) = walked(walk_content);
        let mut glyph = glyphs.remove(0);
        forge(&mut glyph);
        cut(edit_content.as_bytes(), &[glyph])
    }

    /// A marked-content check over `content`'s own glyphs, as production's rewrite asks it.
    fn marked(content: &[u8], properties: &NamedProperties) -> Result<()> {
        let glyphs = glyphs_in(content, &Fake::new(), &unwatched())?;
        carried_text_edits(
            content,
            &glyphs,
            None,
            &FormsReached::Named(&BTreeSet::new()),
            properties,
            &unwatched(),
        )
        .map(drop)
    }

    /// The first witness for `rule`, for the probes that need one.
    fn witness(rule: Refusal) -> Witness {
        witnesses(rule)
            .first()
            .map_or(|| Ok(()), |(_, witness)| *witness)
    }

    /// Every witness for `rule`, one per raise site, each with how it reaches the site.
    ///
    /// **Exhaustive by construction**: a variant added without an arm does not compile, and a
    /// site added without a witness fails the gate.
    #[allow(
        clippy::too_many_lines,
        reason = "one arm per rule and one entry per raise site; the length is the module's"
    )]
    fn witnesses(rule: Refusal) -> Vec<(Reach, Witness)> {
        use Reach::{Content, Entry, Invariant};
        match rule {
            Refusal::UnmatchedRestore => {
                vec![(Content, || walk("Q /F1 10 Tf BT 0 0 Td (A) Tj ET"))]
            }
            Refusal::UnbalancedSave => vec![(Content, || walk("q /F1 10 Tf BT 0 0 Td (A) Tj ET"))],
            Refusal::NestedTextObject => {
                vec![(Content, || walk("/F1 10 Tf BT BT 0 0 Td (A) Tj ET ET"))]
            }
            Refusal::UnterminatedTextObject => {
                vec![(Content, || walk("/F1 10 Tf BT 0 0 Td (A) Tj"))]
            }
            Refusal::UnmatchedEndText => vec![(Content, || walk("ET"))],
            Refusal::TextOutsideTextObject => vec![(Content, || walk("/F1 10 Tf 0 0 Td (A) Tj"))],
            Refusal::FormOperandNotAName => vec![(Content, || walk("5 Do"))],
            Refusal::ShowArrayOperandNotAnArray => vec![
                (Content, || walk("/F1 10 Tf BT (A) TJ ET")),
                (Invariant, || {
                    cut_forged("/F1 10 Tf BT (A) Tj ET", "/F1 10 Tf BT (A) TJ ET", |_| {})
                }),
            ],
            Refusal::ShowArrayItemNotShowable => vec![
                (Content, || walk("/F1 10 Tf BT [/Name] TJ ET")),
                (Invariant, || {
                    cut_forged(
                        "/F1 10 Tf BT [(A) 00] TJ ET",
                        "/F1 10 Tf BT [(A) /N] TJ ET",
                        |_| {},
                    )
                }),
            ],
            Refusal::ShowOperandNotAString => vec![
                (Content, || walk("/F1 10 Tf BT /Name Tj ET")),
                (Invariant, || {
                    cut_forged("/F1 10 Tf BT (A) Tj ET", "/F1 10 Tf BT /NN Tj ET", |_| {})
                }),
            ],
            Refusal::NoFontSelected => vec![(Content, || walk("BT 0 0 Td (A) Tj ET"))],
            Refusal::ZeroBytesPerCode => vec![
                (Content, || {
                    let mut resources = Fake::new();
                    resources.bytes_per_code = 0;
                    walk_with("/F1 10 Tf BT 0 0 Td (A) Tj ET", &resources)
                }),
                (Invariant, || {
                    cut_forged(
                        "/F1 10 Tf BT (A) Tj ET",
                        "/F1 10 Tf BT (A) Tj ET",
                        |glyph| {
                            glyph.source.bytes_per_code = 0;
                        },
                    )
                }),
            ],
            Refusal::NumericOperandNotANumber => {
                vec![(Content, || walk("/F1 10 Tf /Bogus Tz BT 0 0 Td (A) Tj ET"))]
            }
            Refusal::OperandCountMismatch => vec![(Content, || {
                walk("/F1 12 Tf BT 0 0 0 0 0 0 1 0 0 1 100 700 Tm (A) Tj ET")
            })],
            Refusal::NonFiniteGeometry => vec![
                // A width and a font matrix, each finite, whose advance is not.
                (Content, || {
                    let mut wild = Fake::new();
                    wild.width = f64::MAX;
                    wild.font_matrix_scale = 1e297;
                    walk_with("/F1 10 Tf BT 0 0 Td (A) Tj ET", &wild)
                }),
                // AN INFINITE OPERAND, handed over directly. Content cannot produce one -- the
                // lexer refuses a number too large to represent -- so this check is defence in
                // depth against the lexer changing, and says so.
                (Invariant, || {
                    super::number_operand(
                        &[Operand::Number {
                            span: (0, 0),
                            value: f64::INFINITY,
                        }],
                        0,
                    )
                    .map(drop)
                }),
                // Two finite transforms whose product is not.
                (Content, || {
                    let big = format!("1{}", "0".repeat(200));
                    walk(&format!(
                        "{big} 0 0 {big} 0 0 cm {big} 0 0 {big} 0 0 cm /F1 10 Tf BT (A) Tj ET"
                    ))
                }),
                // A font whose width is not finite.
                (Content, || {
                    let mut resources = Fake::new();
                    resources.width = f64::INFINITY;
                    walk_with("/F1 10 Tf BT 0 0 Td (A) Tj ET", &resources)
                }),
                // A glyph transform that composes past finite.
                (Content, || {
                    let mut resources = Fake::new();
                    resources.font_matrix_scale = 1e200;
                    walk_with(
                        &format!("/F1 1{} Tf BT 0 0 Td (A) Tj ET", "0".repeat(200)),
                        &resources,
                    )
                }),
            ],
            Refusal::TooManyGlyphs => vec![(Content, || {
                walk(&format!(
                    "/F1 1 Tf BT ({}) Tj ET",
                    "A".repeat(MAX_GLYPHS + 1)
                ))
            })],
            Refusal::TooManyFormDraws => vec![(Content, || {
                let mut resources = Fake::new();
                for level in 0..6_u64 {
                    let next = format!("/Fm{} Do ", level + 1).repeat(5);
                    resources = resources.with_form(
                        format!("Fm{level}").as_bytes(),
                        200 + level,
                        Matrix::IDENTITY,
                        &next,
                    );
                }
                resources = resources.with_form(
                    b"Fm6",
                    999,
                    Matrix::IDENTITY,
                    "/F1 10 Tf BT 0 0 Td (A) Tj ET",
                );
                walk_with("/Fm0 Do", &resources)
            })],
            Refusal::PatternMayDrawText => {
                vec![(Content, || walk("/Pattern cs /P1 scn 0 0 600 300 re f"))]
            }
            Refusal::UnreadableCMap => vec![(Content, || walk_encoded(Encoding::UnreadableCMap))],
            Refusal::SimpleFontWithMultiByteCodes => {
                vec![(Content, || walk_encoded(Encoding::Simple))]
            }
            Refusal::GlyphFromAnotherStream => vec![
                // `remove_glyphs` has NO production caller, and production filters the glyphs it
                // hands the combined pass to the stream being edited. Both sites are invariants.
                (Invariant, || {
                    let resources = Fake::new().with_form(
                        b"Fm0",
                        7,
                        Matrix::IDENTITY,
                        "/F1 10 Tf BT 0 0 Td (ABC) Tj ET",
                    );
                    let glyphs = glyphs_in(b"/Fm0 Do", &resources, &unwatched())?;
                    remove_glyphs(
                        b"/Fm0 Do",
                        None,
                        glyphs.get(1..2).unwrap_or_default(),
                        &unwatched(),
                    )
                    .map(drop)
                }),
                (Invariant, || {
                    let (glyphs, _) = form_glyphs(b"/Fm0 Do", "/F1 10 Tf BT 0 0 Td (ABC) Tj ET");
                    cut(b"/Fm0 Do", &glyphs[1..2])
                }),
            ],
            Refusal::GlyphWithoutItsOperation => vec![
                (Invariant, || {
                    let (glyphs, content) = walked("/F1 10 Tf BT 0 0 Td (ABC) Tj ET");
                    let mut stray = glyphs[1].clone();
                    stray.source.operation = (9_999, 10_000);
                    cut(&content, &[stray])
                }),
                (Invariant, || {
                    cut_forged(
                        "/F1 10 Tf BT (A) Tj ET",
                        "/F1 10 Tf BT (A) Tj ET",
                        |glyph| {
                            glyph.source.code_index = 99;
                        },
                    )
                }),
            ],
            Refusal::GlyphOnANonShowingOperation => vec![(Invariant, || {
                let content = b"/F1 10 Tf 1 0 0 1 0 0 cm BT 0 0 Td (ABC) Tj ET";
                let glyphs = glyphs_in(content, &Fake::new(), &unwatched())?;
                let operations = super::super::ops::operations(content)?;
                let mut stray = glyphs[1].clone();
                stray.source.operation = operations
                    .iter()
                    .find(|operation| operation.operator == b"cm")
                    .map(|operation| operation.span)
                    .unwrap_or_default();
                cut(content, &[stray])
            })],
            Refusal::AdjustmentNotExpressible => vec![
                // A document can say `0 Tz`, and a region can reach a glyph under it.
                (Entry("remove_glyphs_and_carried_text"), || {
                    let (glyphs, content) = walked("/F1 10 Tf 0 Tz BT 0 0 Td (ABC) Tj ET");
                    cut(&content, &glyphs[1..2])
                }),
                (Invariant, || {
                    cut_forged(
                        "/F1 10 Tf BT (A) Tj ET",
                        "/F1 10 Tf BT (A) Tj ET",
                        |glyph| {
                            glyph.displacement = f64::MAX;
                        },
                    )
                }),
            ],
            Refusal::SharedFormWouldChangeElsewhere => {
                vec![(Entry("check_form_sharing"), || {
                    let (glyphs, _) = form_glyphs(b"/Fm0 Do", "/F1 10 Tf BT 0 0 Td (ABC) Tj ET");
                    check_form_sharing(&glyphs[1..2], &Uses(vec![(7, 4)]))
                })]
            }
            Refusal::TypeThreeProcedureShowsText => {
                vec![(Entry("check_type_three_procedure"), || {
                    check_type_three_procedure(
                        b"500 0 d0\nBT /F2 1 Tf (SECRET) Tj ET\n",
                        &unwatched(),
                    )
                })]
            }
            Refusal::StringNotWholeCodes => vec![
                (Content, || {
                    walk_encoded(Encoding::Predefined(b"Identity-H".to_vec())).and_then(|()| {
                        let mut wide = Fake::new();
                        wide.bytes_per_code = 2;
                        wide.encoding = Encoding::Predefined(b"Identity-H".to_vec());
                        walk_with("/F1 10 Tf BT 0 0 Td (ABCDE) Tj ET", &wide)
                    })
                }),
                (Invariant, || {
                    cut_forged(
                        "/F1 10 Tf BT (ABC) Tj ET",
                        "/F1 10 Tf BT (ABC) Tj ET",
                        |glyph| {
                            glyph.source.bytes_per_code = 2;
                        },
                    )
                }),
            ],
            Refusal::MixedCodeWidths => vec![(Invariant, || {
                let mut wide = Fake::new();
                wide.bytes_per_code = 2;
                wide.encoding = Encoding::Predefined(b"Identity-H".to_vec());
                let content = b"/F1 10 Tf BT 0 0 Td (ABCD) Tj ET";
                let mut glyphs = glyphs_in(content, &wide, &unwatched())?;
                glyphs[1].source.bytes_per_code = 1;
                cut(content, &glyphs)
            })],
            Refusal::FormCycle => vec![(Content, || {
                let resources = Fake::new().with_form(b"Fm0", 7, Matrix::IDENTITY, "/Fm0 Do");
                walk_with("/Fm0 Do", &resources)
            })],
            Refusal::FormDepth => vec![(Content, || {
                let mut resources = Fake::new();
                for level in 0..=MAX_FORM_DEPTH {
                    let next = format!("/Fm{} Do", level + 1);
                    resources = resources.with_form(
                        format!("Fm{level}").as_bytes(),
                        100 + u64::try_from(level).unwrap_or(0),
                        Matrix::IDENTITY,
                        &next,
                    );
                }
                walk_with("/Fm0 Do", &resources)
            })],
            Refusal::VerticalWriting => vec![(Content, || {
                walk_cmap(None, b"/CMapName /Identity-H def /WMode 1 def")
            })],
            Refusal::UndeterminedWritingMode => vec![
                // A predefined name with neither suffix.
                (Content, || {
                    walk_encoded(Encoding::Predefined(b"SomethingElse".to_vec()))
                }),
                // `WMode 2`, from the stream dictionary.
                (Content, || walk_cmap(Some(2), b"")),
                // The dictionary and the program disagree.
                (Content, || walk_cmap(Some(0), b"/WMode 1 def")),
                // A `WMode` that is a number and not an integer.
                (Content, || walk_cmap(None, b"/WMode 1.5 def")),
                // Declared twice, differently.
                (Content, || walk_cmap(None, b"/WMode 0 def /WMode 1 def")),
                // A `WMode` followed by something that is not a number.
                (Content, || walk_cmap(None, b"/WMode /X def")),
                // A `usecmap` with nothing named before it.
                (Content, || walk_cmap(None, b"usecmap")),
                // Two `usecmap`s that disagree.
                (Content, || {
                    walk_cmap(None, b"/Identity-H usecmap /Identity-V usecmap")
                }),
                // A trailing `WMode` with no value: the site a review found no test reached.
                (Content, || walk_cmap(None, b"/WMode")),
                // Its own `WMode` contradicting what it inherits.
                (Content, || {
                    walk_cmap(None, b"/Identity-V usecmap /WMode 0 def")
                }),
            ],
            Refusal::MarkedContentPropertiesUnresolved => {
                vec![(Entry("carried_text_edits"), || {
                    marked(
                        b"/Span /MC0 BDC BT /F1 12 Tf (AB) Tj ET EMC",
                        &NamedProperties::default(),
                    )
                })]
            }
            Refusal::MarkedContentNamedPropertiesCarryText => {
                vec![(Entry("carried_text_edits"), || {
                    let mut properties = NamedProperties::default();
                    properties.insert(
                        b"MC0".to_vec(),
                        PropertyList::read(b"<< /ActualText (secret) >>"),
                    );
                    marked(b"/Span /MC0 BDC BT /F1 12 Tf (AB) Tj ET EMC", &properties)
                })]
            }
            Refusal::MarkedContentSplitAcrossElements => {
                vec![(Entry("remove_glyphs_and_carried_text"), || {
                    let contents = super::super::contents::Contents::concatenate(&[
                        b"/Span << /ActualText (secret)".as_slice(),
                        b" >> BDC BT /F1 12 Tf (AB) Tj ET EMC".as_slice(),
                    ])?;
                    let glyphs = glyphs_in(contents.bytes(), &Fake::new(), &unwatched())?;
                    remove_glyphs_and_carried_text(
                        &contents,
                        None,
                        &glyphs,
                        &FormsReached::Named(&BTreeSet::new()),
                        &NamedProperties::default(),
                        &unwatched(),
                    )
                    .map(drop)
                })]
            }
            Refusal::MarkedContentCarriesOpaqueString => vec![
                (Entry("carried_text_edits"), || {
                    marked(
                        b"/Span << /MCID 0 /K [ /ActualText (secret) ] >> BDC BT /F1 12 Tf (x) Tj ET EMC",
                        &NamedProperties::default(),
                    )
                }),
                // A list holding a handled key AND naming one elsewhere, decided after the rewrite.
                (Entry("carried_text_edits"), || {
                    marked(
                        b"/Span << /ActualText (x) /K [ /Alt (y) ] >> BDC BT /F1 12 Tf (x) Tj ET EMC",
                        &NamedProperties::default(),
                    )
                }),
            ],
            Refusal::MarkedContentPropertyListMalformed => {
                vec![(Entry("carried_text_edits"), || {
                    marked(
                        b"/Span << /MCID 0 /ActualText 4 0 R >> BDC BT /F1 12 Tf (x) Tj ET EMC",
                        &NamedProperties::default(),
                    )
                })]
            }
            Refusal::OptionalContentMarked => {
                vec![(Content, || walk("/OC /OC1 BDC BT /F1 12 Tf (AB) Tj ET EMC"))]
            }
        }
    }

    #[test]
    fn every_raise_site_is_raised_by_production_code_when_its_witness_runs() {
        // PER SITE, NOT PER RULE, AND EACH SITE SAYS HOW IT IS REACHED. See `Reach`. The first
        // version proved one site per rule -- 38 of 61 -- and printed "38 of 38 rules"; the
        // second proved every site and printed "60 of 60", which a security review showed
        // counted invariant checks as refusals a file can trigger. The expectation comes from
        // the source, and the report says which kind each site is.
        let sites = raise_sites(include_str!("geometry.rs")).expect("the sites read");
        let steps = include_str!("../redact/steps.rs");
        let mut covered: BTreeMap<u32, (String, Reach)> = BTreeMap::new();
        let mut failures = Vec::new();
        for rule in Refusal::ALL {
            for (reach, witness) in witnesses(*rule) {
                // AN ENTRY IS ONLY AN ENTRY IF PRODUCTION CALLS IT, outside a comment.
                if let Reach::Entry(name) = reach {
                    let call = format!("{name}(");
                    if !steps
                        .lines()
                        .any(|line| !line.trim_start().starts_with("//") && line.contains(&call))
                    {
                        failures.push(format!(
                            "`{}` claims entry `{name}`, which redact/steps.rs never calls",
                            rule.rule()
                        ));
                    }
                }
                match prove(*rule, witness) {
                    Ok(line) => {
                        covered.insert(line, (format!("{rule:?}"), reach));
                    }
                    Err(why) => failures.push(why),
                }
            }
        }
        let uncovered: Vec<String> = sites
            .iter()
            .filter(|(line, _)| !covered.contains_key(line))
            .map(|(line, rule)| format!("line {line} ({rule})"))
            .collect();
        let misnamed: Vec<String> = covered
            .iter()
            .filter(|(line, (rule, _))| sites.get(line) != Some(rule))
            .map(|(line, (rule, _))| {
                format!(
                    "line {line}: proven as {rule}, the source says {:?}",
                    sites.get(line)
                )
            })
            .collect();
        let count = |kind: fn(&Reach) -> bool| covered.values().filter(|(_, r)| kind(r)).count();
        let invariant: Vec<String> = covered
            .iter()
            .filter(|(_, (_, reach))| *reach == Reach::Invariant)
            .map(|(line, (rule, _))| format!("{line} {rule}"))
            .collect();
        eprintln!(
            "  refusal reachability: {} of {} raise sites raised by production code -- {} from \
             content, {} through an entry the redaction steps call, {} invariant-only: {}",
            covered.len(),
            sites.len(),
            count(|reach| *reach == Reach::Content),
            count(|reach| matches!(reach, Reach::Entry(_))),
            invariant.len(),
            invariant.join(", ")
        );
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        assert!(misnamed.is_empty(), "{}", misnamed.join("\n"));
        assert!(
            uncovered.is_empty(),
            "{} raise site(s) no witness reaches:\n  {}",
            uncovered.len(),
            uncovered.join("\n  ")
        );
        // A SITE SCAN THAT FOUND ALMOST NOTHING AGREES WITH EVERYTHING.
        assert!(
            sites.len() >= Refusal::ALL.len(),
            "only {} raise sites found",
            sites.len()
        );
    }

    #[test]
    fn nothing_above_the_tests_is_compiled_only_for_them_except_the_recorder() {
        // A RAISE INSIDE `#[cfg(test)]` IN THE PRODUCTION HALF records a production line and
        // exists in no build a document reaches. A security review planted one, and one behind
        // `cfg!(test)`, and both passed the gate. The recorder's four lines are the only
        // test-only code up there, and the count is gated, not just printed.
        let source = include_str!("geometry.rs");
        let end = source
            .lines()
            .position(|line| line.trim() == "mod tests {")
            .expect("the test module marker");
        let test_only: Vec<(usize, &str)> = source
            .lines()
            .enumerate()
            .take(end)
            .filter(|(_, line)| !line.trim_start().starts_with("//"))
            .filter(|(_, line)| {
                ["cfg(test)", "cfg!(test)", "cfg_attr(test"]
                    .iter()
                    .any(|needle| line.contains(needle))
            })
            .map(|(at, line)| (at + 1, line.trim()))
            .collect();
        assert_eq!(
            test_only.len(),
            5,
            "test-only code above `mod tests` is the recorder's four lines -- the `track_caller` \
             attribute, the two statements in `refuse`, the thread-local -- and the attribute on \
             `mod tests` itself, and nothing else: {test_only:?}"
        );
    }

    #[test]
    fn the_reachability_gate_rejects_a_rule_raised_from_the_test_module() {
        // THE PROBES ON THE GATE. The #177 plant, a rule "tested" without production code
        // raising it, has two shapes, and the gate must reject both by name: a witness that
        // raises the rule itself, and a witness that does not refuse at all.
        let planted = prove(Refusal::FormCycle, || {
            Refusal::FormCycle.refuse::<()>("raised here, in the test module")
        });
        assert!(
            planted
                .as_ref()
                .is_err_and(|why| why.contains("was not raised by production code")),
            "a rule raised from the test module passed the gate: {planted:?}"
        );
        let silent = prove(Refusal::FormCycle, || walk("/F1 10 Tf BT 0 0 Td (A) Tj ET"));
        assert!(
            silent
                .as_ref()
                .is_err_and(|why| why.contains("did not refuse")),
            "a witness that refused nothing passed the gate: {silent:?}"
        );
        let other = prove(Refusal::FormCycle, || walk("ET"));
        assert!(
            other
                .as_ref()
                .is_err_and(|why| why.contains("refused by another rule")),
            "a witness refusing by the wrong rule passed the gate: {other:?}"
        );
        // A PRODUCTION RAISE, DISCARDED, MUST NOT VOUCH FOR ONE THE TEST BUILT. The gate reads
        // the LAST refusal; a review planted exactly this against a gate that read any.
        let laundered = prove(Refusal::FormCycle, || {
            let _discarded = witness(Refusal::FormCycle)();
            Refusal::FormCycle.refuse::<()>("built in the test module")
        });
        assert!(
            laundered
                .as_ref()
                .is_err_and(|why| why.contains("as the last refusal")),
            "a discarded production raise vouched for a test-built one: {laundered:?}"
        );
        // THE NEAR-MISS: a real witness, driven through real code, passes.
        assert!(prove(Refusal::FormCycle, witness(Refusal::FormCycle)).is_ok());
    }

    #[test]
    fn the_site_scan_refuses_a_raise_through_a_helper() {
        // `fn refuse_as(rule: Refusal) { rule.refuse(..) }` would record the helper's line for
        // every rule it raised, and every witness through it would pass on the helper's behalf.
        let helper = "fn f() {\n    rule.refuse(\"x\")\n}\nmod tests {\n}\n";
        assert!(
            raise_sites(helper).is_err_and(|why| why.contains("not a named rule")),
            "a raise through a helper was accepted as a site"
        );
        // THE NEAR-MISSES: a named receiver on one line, and one rustfmt wrapped.
        let named = "fn f() {\n    Refusal::FormCycle.refuse(\"x\")\n    Refusal::FormDepth\n        .refuse(\"y\")\n}\nmod tests {\n}\n";
        let sites = raise_sites(named).expect("named receivers are sites");
        assert_eq!(
            sites.into_iter().collect::<Vec<_>>(),
            vec![(2, "FormCycle".to_owned()), (4, "FormDepth".to_owned())]
        );
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
        assert_refused(
            glyphs_in(content.as_bytes(), &Fake::new(), &unwatched()),
            rule,
        );
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
        assert_refused(
            glyphs_in(b"/Fm0 Do", &resources, &unwatched()),
            Refusal::FormCycle,
        );
    }

    #[test]
    fn a_cycle_through_two_forms_is_refused_by_identity_and_not_by_name() {
        // KEYED ON THE OBJECT, not the name. The two forms name each other through DIFFERENT
        // resource names, so a walk remembering names would have to see `/FmA` twice to notice
        // -- and would also refuse a legitimate document that reuses one name inside a form.
        let resources = Fake::new()
            .with_form(b"FmA", 11, Matrix::IDENTITY, "/FmB Do")
            .with_form(b"FmB", 12, Matrix::IDENTITY, "/FmA Do");
        assert_refused(
            glyphs_in(b"/FmA Do", &resources, &unwatched()),
            Refusal::FormCycle,
        );
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
        assert_refused(
            glyphs_in(b"/Fm0 Do", &resources, &unwatched()),
            Refusal::FormDepth,
        );
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
        let glyphs =
            glyphs_in(b"q 1 0 0 1 100 0 cm /Fm0 Do Q", &resources, &unwatched()).expect("walks");
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
        let glyphs = glyphs_in(b"/F1 10 Tf /Fm0 Do", &resources, &unwatched()).expect("walks");
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
        // FOUND BY the text-search gate that preceded
        // `every_refusal_is_raised_by_production_code_when_its_witness_runs` (#177), which
        // reported five rules nothing asserted. They are the ones a malformed file reaches first, so they were exactly the
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
    fn a_numeric_operand_that_is_not_a_number_is_refused() {
        // THE DANGEROUS DEFAULT. `/Bogus Tz` used to set the horizontal scale to 0.0, which
        // collapses every glyph box on the page to a point -- a box that intersects almost
        // nothing, so a redaction over it removes almost nothing. `TextState::default` sets
        // the scale to 100 for this exact reason; silently reintroducing 0 from a malformed
        // operand undid it. Found by review.
        refusing(
            "/F1 10 Tf /Bogus Tz BT 0 0 Td (A) Tj ET",
            Refusal::NumericOperandNotANumber,
        );
        // A name where a number belongs, AT THE RIGHT COUNT -- the arity check passes this,
        // so the operand check is what has to catch it. The first draft used `BT Tm` and
        // `1 0 0 1 0 cm`, which are count mismatches; the rule-naming assertion said so.
        refusing(
            "/F1 10 Tf BT 0 /Bogus Td (A) Tj ET",
            Refusal::NumericOperandNotANumber,
        );
        // THE NEAR-MISS: the same operators with proper numbers are not refused.
        assert_eq!(placed("/F1 10 Tf 50 Tz BT 0 0 Td (A) Tj ET").len(), 1);
    }

    #[test]
    fn a_padded_operand_run_is_refused_rather_than_read_from_the_front() {
        // THE MEASURED LEAK. `ops::operations` gathers every pending operand; a renderer keeps
        // a small buffer and takes the LAST six. Reading the first six put this glyph at
        // (0, 0) while PDFium put it at (100, 700) -- so a region over the visible word met no
        // box, and the redaction removed nothing. Found by review, with both numbers measured.
        refusing(
            "/F1 12 Tf BT 0 0 0 0 0 0 1 0 0 1 100 700 Tm (A) Tj ET",
            Refusal::OperandCountMismatch,
        );
        refusing("1 0 0 1 100 700 0 cm", Refusal::OperandCountMismatch);
        refusing("/F1 12 Tf 9 9 /Other /F1 Tf", Refusal::OperandCountMismatch);
        // THE NEAR-MISS: the honest page, which must still walk.
        let glyphs = placed("/F1 12 Tf BT 1 0 0 1 100 700 Tm (A) Tj ET");
        assert_eq!(glyphs.len(), 1);
        assert!((glyphs[0].origin.0 - 100.0).abs() < 1e-9);
    }

    #[test]
    fn a_non_finite_operand_is_refused_rather_than_boxed() {
        // An infinity composes to a NaN, and `Rect::transformed` folding NaN corners with
        // min/max leaves its own +inf/-inf initialisers -- an inverted rectangle that
        // intersects nothing. A box that intersects nothing is a glyph a redaction skips.
        // Each `cm` on its own is finite -- 1e300 is a perfectly good f64. It is the
        // COMPOSITION that overflows, which is why the check cannot live on the operands only.
        // AND THROUGH THE FONT METRICS, which come out of the file just as the operands do.
        // This is the fuzzer's own finding, reproduced: a `/W` of `f64::MAX` against a large
        // `/FontMatrix` composes to an infinity, and the box built from it was
        // `left: -inf, right: inf` -- a box that makes every region test true, so the
        // redaction removes the whole page and reports success.
        let mut wild = Fake::new();
        wild.width = f64::MAX;
        wild.font_matrix_scale = 1e297;
        assert_refused(
            glyphs_in(b"/F1 10 Tf BT 0 0 Td (A) Tj ET", &wild, &unwatched()),
            Refusal::NonFiniteGeometry,
        );
        // THE NEAR-MISS: large but finite metrics still walk.
        let mut large = Fake::new();
        large.width = 1e6;
        assert_eq!(
            glyphs_in(b"/F1 10 Tf BT 0 0 Td (A) Tj ET", &large, &unwatched())
                .expect("finite metrics walk")
                .len(),
            1
        );

        let huge = "9".repeat(300);
        let one = format!("{huge} 0 0 {huge} 0 0 cm");
        assert_eq!(placed(&one).len(), 0, "one `cm` of 1e300 is still finite");
        refusing(&format!("{one} {one}"), Refusal::NonFiniteGeometry);
    }

    #[test]
    fn a_page_that_buys_unbounded_work_with_a_few_hundred_bytes_is_refused() {
        // DEPTH IS NOT THE BOUND. Sixteen forms, each drawing the next three times, is 3^16
        // glyphs from 405 bytes -- 9.7 s and 1,644 MiB measured in release before this cap.
        // The open-form set sees no cycle and the depth cap sees no over-deep nest.
        // SIX levels, each drawing the next FIVE times: 5^6 = 15,625 draws at a depth of six.
        // Shallower than MAX_FORM_DEPTH and acyclic, so neither existing bound sees it -- which
        // is the finding. The first draft used sixteen levels and was refused by the depth cap
        // instead; the rule-naming assertion is what said so.
        const LEVELS: u64 = 6;
        let mut resources = Fake::new();
        for level in 0..LEVELS {
            let next = format!("/Fm{} Do ", level + 1).repeat(5);
            resources = resources.with_form(
                format!("Fm{level}").as_bytes(),
                200 + level,
                Matrix::IDENTITY,
                &next,
            );
        }
        resources = resources.with_form(
            format!("Fm{LEVELS}").as_bytes(),
            999,
            Matrix::IDENTITY,
            "/F1 10 Tf BT 0 0 Td (A) Tj ET",
        );
        assert_refused(
            glyphs_in(b"/Fm0 Do", &resources, &unwatched()),
            Refusal::TooManyFormDraws,
        );
    }

    #[test]
    fn more_glyphs_than_burrow_will_place_is_a_refusal() {
        // The other half of the same bound: `out` had no ceiling, so a page that stayed inside
        // the draw budget could still ask for an unbounded `Vec<Glyph>`.
        let mut body = String::from("/F1 1 Tf BT ");
        // One `Tj` of many bytes is far cheaper to build than many operations.
        body.push_str(&format!("({}) Tj ET", "A".repeat(MAX_GLYPHS + 1)));
        assert_refused(
            glyphs_in(body.as_bytes(), &Fake::new(), &unwatched()),
            Refusal::TooManyGlyphs,
        );
    }

    #[test]
    fn a_pattern_fill_is_refused_because_the_walk_does_not_reach_its_text() {
        // MEASURED: a page whose only text lives in a tiling pattern walked to `Ok(0)` while
        // PDFium inked 740 pixels of it -- and `FPDFText_*` reported zero characters, so the
        // ADR 0029 §6 read-back was blind to it too. An `Ok` over text nothing observed is
        // exactly what this module exists to prevent.
        refusing(
            "/Pattern cs /P1 scn 0 0 600 300 re f",
            Refusal::PatternMayDrawText,
        );
        refusing(
            "/Pattern CS /P1 SCN 0 0 600 300 re S",
            Refusal::PatternMayDrawText,
        );
        // THE NEAR-MISS: a plain colour fill names no pattern and is not refused.
        assert_eq!(placed("0 0 1 rg 0 0 600 300 re f").len(), 0);
    }

    #[test]
    fn a_font_claiming_zero_bytes_per_code_is_refused() {
        // Not a division by zero but a non-terminating chunk: `chunks(0)` panics, and a walk
        // that clamped it to 1 would decode a two-byte font's codes as bytes and place every
        // glyph on the page somewhere else.
        let mut resources = Fake::new();
        resources.bytes_per_code = 0;
        assert_refused(
            glyphs_in(b"/F1 10 Tf BT 0 0 Td (A) Tj ET", &resources, &unwatched()),
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
        // NINE APPENDED BYTES used to turn a vertical CMap horizontal, because the last `def`
        // won. Found by review; the rule was already stated in three places and false here.
        assert_refused_mode(
            writing_mode_of(&CMap::Embedded {
                dictionary_wmode: None,
                program: b"/WMode 1 def /CMapName /X def /WMode 0 def",
            }),
            Refusal::UndeterminedWritingMode,
        );
        // AND THE `usecmap` HALF, which used to resolve silently toward Horizontal -- the
        // direction that misses text. Found by review.
        assert_refused_mode(
            writing_mode_of(&CMap::Embedded {
                dictionary_wmode: None,
                program: b"/WMode 0 def /UniJIS-UCS2-V usecmap",
            }),
            Refusal::UndeterminedWritingMode,
        );
        assert_refused_mode(
            writing_mode_of(&CMap::Embedded {
                dictionary_wmode: None,
                program: b"/UniJIS-UCS2-V usecmap /Plain-H usecmap",
            }),
            Refusal::UndeterminedWritingMode,
        );
        // A repeat that AGREES is not a disagreement, and must not be refused.
        assert_eq!(
            writing_mode_of(&CMap::Embedded {
                dictionary_wmode: Some(1),
                program: b"/WMode 1 def /WMode 1 def",
            })
            .expect("derives"),
            WritingMode::Vertical
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
    fn a_cmap_the_resolver_could_not_read_is_refused() {
        // THE DOCUMENT-LEVEL EVASION, closed at the seam. Before this, `GlyphMetrics` carried a
        // `WritingMode` the resolver supplied -- so a resolver that could not decode a CMap
        // stream, or never tried, returned `Horizontal` and the whole `WMode` rule was true at
        // unit level and vacuous at document level. There is now no way to say "horizontal" by
        // omission: the only arms are the ones that answer the question.
        let mut resources = Fake::new();
        resources.encoding = Encoding::UnreadableCMap;
        resources.bytes_per_code = 2;
        assert_refused(
            glyphs_in(b"/F1 10 Tf BT 0 0 Td (AB) Tj ET", &resources, &unwatched()),
            Refusal::UnreadableCMap,
        );
    }

    #[test]
    fn a_simple_font_claiming_wide_codes_is_refused() {
        // `Encoding::Simple` is the one arm that concludes "horizontal" without reading
        // anything, so it is the one a resolver could misuse to skip the question for a CID
        // font. A simple font's codes are bytes; if they are not, the classification is wrong
        // and so is the conclusion drawn from it.
        let mut resources = Fake::new();
        resources.bytes_per_code = 2;
        assert_refused(
            glyphs_in(b"/F1 10 Tf BT 0 0 Td (AB) Tj ET", &resources, &unwatched()),
            Refusal::SimpleFontWithMultiByteCodes,
        );
    }

    #[test]
    fn an_embedded_cmap_is_read_rather_than_trusted() {
        // The near-miss pair at the seam, not only at `writing_mode_of`: the SAME resolver
        // shape, differing only in the program's `WMode` digit, must walk one and refuse the
        // other. The name says `Identity-H` in both.
        let mut vertical = Fake::new();
        vertical.bytes_per_code = 2;
        vertical.encoding = Encoding::Embedded {
            dictionary_wmode: None,
            program: b"/CMapName /Identity-H def /WMode 1 def".to_vec(),
        };
        assert_refused(
            glyphs_in(b"/F1 10 Tf BT 0 0 Td (AB) Tj ET", &vertical, &unwatched()),
            Refusal::VerticalWriting,
        );

        let mut horizontal = Fake::new();
        horizontal.bytes_per_code = 2;
        horizontal.encoding = Encoding::Embedded {
            dictionary_wmode: None,
            program: b"/CMapName /Identity-H def /WMode 0 def".to_vec(),
        };
        assert_eq!(
            glyphs_in(b"/F1 10 Tf BT 0 0 Td (AB) Tj ET", &horizontal, &unwatched())
                .expect("the twin must walk")
                .len(),
            1
        );
    }

    // ---- removing glyphs without moving the ones that stay ---------------------------------

    /// Walk a fixture and hand back both the glyphs and the content, for a removal test.
    fn walked(content: &str) -> (Vec<Glyph>, Vec<u8>) {
        let bytes = content.as_bytes().to_vec();
        (
            glyphs_in(&bytes, &Fake::new(), &unwatched()).expect("the fixture should walk"),
            bytes,
        )
    }

    #[test]
    fn an_odd_length_composite_string_is_refused_rather_than_reframed() {
        // THE DEFECT A REVIEW CAUGHT, as a fixture. `(ABCDE)` at two bytes per code is five
        // bytes: `chunks(2)` yields a short final chunk, so the last glyph's own length is 1.
        // Removal took the framing width from the glyph being cut, re-chunked the whole string
        // into single bytes, and cut a DIFFERENT glyph -- measured: asked to remove `0x45`, it
        // left `0x45` on the page, deleted `0x43`, and returned `Ok`.
        //
        // The walk now refuses the string outright, because a phantom glyph for a partial code
        // is a wrong answer about what the page draws before removal is even reached.
        let mut wide = Fake::new();
        wide.bytes_per_code = 2;
        wide.encoding = Encoding::Predefined(b"Identity-H".to_vec());
        assert_refused(
            glyphs_in(b"/F1 10 Tf BT 0 0 Td (ABCDE) Tj ET", &wide, &unwatched()),
            Refusal::StringNotWholeCodes,
        );
    }

    #[test]
    fn a_whole_composite_string_is_cut_at_the_right_code() {
        // THE NEAR-MISS, and the positive half of the same finding: an even-length string
        // frames correctly and the code that comes out is the one that was asked for.
        let mut wide = Fake::new();
        wide.bytes_per_code = 2;
        wide.encoding = Encoding::Predefined(b"Identity-H".to_vec());
        let content = b"/F1 10 Tf BT 0 0 Td (ABCDEF) Tj ET";
        let glyphs = glyphs_in(content, &wide, &unwatched()).expect("walks");
        assert_eq!(glyphs.len(), 3, "three two-byte codes");
        assert!(
            glyphs.iter().all(|g| g.source.bytes_per_code == 2),
            "every glyph carries the FONT's width, not its own chunk's"
        );
        let out = remove_glyphs(content, None, &glyphs[2..3], &unwatched()).expect("removes");
        let text = String::from_utf8_lossy(&out).into_owned();
        assert!(
            text.contains("<41424344>") && !text.contains("4546"),
            "the third code (0x4546) is the one removed: {text}"
        );
    }

    #[test]
    fn glyphs_disagreeing_on_the_code_width_are_refused() {
        // Nothing the walk produces can disagree today -- the width comes from one font per
        // string. The rule exists because `emit_run` frames a string from it, and a framing
        // parameter that could differ between elements is the shape of the defect above.
        let mut wide = Fake::new();
        wide.bytes_per_code = 2;
        wide.encoding = Encoding::Predefined(b"Identity-H".to_vec());
        let content = b"/F1 10 Tf BT 0 0 Td (ABCD) Tj ET";
        let mut glyphs = glyphs_in(content, &wide, &unwatched()).expect("walks");
        glyphs[1].source.bytes_per_code = 1;
        assert_refused_bytes(
            remove_glyphs(content, None, &glyphs, &unwatched()),
            Refusal::MixedCodeWidths,
        );
    }

    #[test]
    fn a_removed_glyph_leaves_an_adjustment_equal_to_what_it_displaced() {
        // Width 500/1000 at 10pt is an advance of 5, and `Tz` is 100, so the adjustment that
        // reproduces it is -500 thousandths. A reader can check that in their head, which is
        // what makes the failure message useful rather than a pair of decimals.
        let (glyphs, content) = walked("/F1 10 Tf BT 0 0 Td (ABC) Tj ET");
        assert_eq!(glyphs.len(), 3);
        let out = remove_glyphs(&content, None, &glyphs[1..2], &unwatched()).expect("removes");
        let text = String::from_utf8_lossy(&out).into_owned();
        assert!(text.contains("-500"), "no adjustment in the output: {text}");
        // `A` and `C` stay, as hex, and `B` does not.
        assert!(text.contains("<41>") && text.contains("<43>"), "{text}");
        assert!(
            !text.contains("42"),
            "the removed code is still there: {text}"
        );
    }

    #[test]
    fn word_spacing_rides_with_a_removed_space_and_only_a_single_byte_one() {
        // THE CLASSIC MISS, on the removal side. A removed space in a simple font displaced by
        // its width PLUS `Tw`, so the adjustment has to put both back. Getting it wrong shifts
        // everything after the cut by the word spacing -- small enough to look like rounding.
        let (glyphs, content) = walked("/F1 10 Tf 4 Tw BT 0 0 Td (A B) Tj ET");
        assert_eq!(glyphs.len(), 3);
        let space = &glyphs[1];
        assert!(
            (space.displacement - 9.0).abs() < 1e-9,
            "a space at width 5 with `4 Tw` displaces 9, not {}",
            space.displacement
        );
        let out = remove_glyphs(&content, None, &glyphs[1..2], &unwatched()).expect("removes");
        assert!(
            String::from_utf8_lossy(&out).contains("-900"),
            "the adjustment dropped the word spacing: {}",
            String::from_utf8_lossy(&out)
        );
    }

    #[test]
    fn a_producers_own_kern_is_carried_through_in_place() {
        // The kern sits between glyphs the cut does not touch. Dropping it, or moving it,
        // shifts everything after by a point or two.
        let (glyphs, content) = walked("/F1 10 Tf BT 0 0 Td [(AB) -25 (CD)] TJ ET");
        assert_eq!(glyphs.len(), 4);
        let out = remove_glyphs(&content, None, &glyphs[3..4], &unwatched()).expect("removes");
        let text = String::from_utf8_lossy(&out).into_owned();
        assert!(text.contains("-25"), "the kern was dropped: {text}");
        assert!(
            text.contains("<4142>"),
            "the untouched run was rewritten: {text}"
        );
    }

    #[test]
    fn removing_nothing_returns_the_stream_unchanged() {
        // THE NON-VACUITY CONTROL'S PARTNER. A removal that rewrote a stream it was asked not
        // to touch would fail the byte-identity half of every test above for the wrong reason.
        let (_, content) = walked("/F1 10 Tf BT 0 0 Td (ABC) Tj ET");
        assert_eq!(
            remove_glyphs(&content, None, &[], &unwatched()).expect("removes"),
            content
        );
    }

    #[test]
    fn a_glyph_attributed_to_the_wrong_operation_is_refused_by_its_own_name() {
        // FOUR CONDITIONS SHARED ONE NAME, which a review flagged: only one of them was about
        // another stream, §7's disclosure keys on the name, and every `assert_refused_bytes`
        // in this suite keys on it too -- so a test could assert the right refusal for the
        // wrong reason, and the user would be told the wrong thing.
        let (glyphs, content) = walked("/F1 10 Tf BT 0 0 Td (ABC) Tj ET");
        let mut stray = glyphs[1].clone();
        stray.source.operation = (9_999, 10_000);
        assert_refused_bytes(
            remove_glyphs(&content, None, &[stray], &unwatched()),
            Refusal::GlyphWithoutItsOperation,
        );
    }

    #[test]
    fn a_glyph_attributed_to_an_operation_that_shows_no_text_is_refused_by_its_own_name() {
        let content = b"/F1 10 Tf 1 0 0 1 0 0 cm BT 0 0 Td (ABC) Tj ET";
        let glyphs = glyphs_in(content, &Fake::new(), &unwatched()).expect("walks");
        let operations = super::super::ops::operations(content).expect("tokenises");
        let cm = operations
            .iter()
            .find(|operation| operation.operator == b"cm")
            .expect("the fixture has a `cm`");
        let mut stray = glyphs[1].clone();
        stray.source.operation = cm.span;
        assert_refused_bytes(
            remove_glyphs(content, None, &[stray], &unwatched()),
            Refusal::GlyphOnANonShowingOperation,
        );
    }

    #[test]
    fn a_glyph_from_a_form_is_refused_rather_than_cut_out_of_the_page() {
        // ITS SPAN INDEXES THE FORM'S BYTES. Applying it to the page would delete whatever
        // happened to sit at those offsets -- a cut in the wrong place, reported as success.
        let resources = Fake::new().with_form(
            b"Fm0",
            7,
            Matrix::IDENTITY,
            "/F1 10 Tf BT 0 0 Td (ABC) Tj ET",
        );
        let page = b"/Fm0 Do";
        let glyphs = glyphs_in(page, &resources, &unwatched()).expect("walks");
        assert_eq!(glyphs.len(), 3);
        assert_eq!(glyphs[0].source.form, Some(7));
        assert_refused_bytes(
            remove_glyphs(page, None, &glyphs[1..2], &unwatched()),
            Refusal::GlyphFromAnotherStream,
        );
    }

    #[test]
    fn a_displacement_with_no_expressible_adjustment_is_refused() {
        // `0 Tz` scales the font size to zero, so the conversion to thousandths of text space
        // is a division by zero. Emitting an infinity would put a `TJ` number in the stream
        // that no renderer can act on.
        let (glyphs, content) = walked("/F1 10 Tf 0 Tz BT 0 0 Td (ABC) Tj ET");
        assert_refused_bytes(
            remove_glyphs(&content, None, &glyphs[1..2], &unwatched()),
            Refusal::AdjustmentNotExpressible,
        );
    }

    /// A `FormUses` that answers from a table, standing in for the qpdf-side resource walk.
    struct Uses(Vec<(u64, usize)>);

    impl FormUses for Uses {
        fn uses(&self, form: u64) -> Result<usize> {
            Ok(self
                .0
                .iter()
                .find(|(id, _)| *id == form)
                .map_or(1, |(_, count)| *count))
        }
    }

    /// Walk a page that draws one form, and hand back the glyphs it drew.
    fn form_glyphs(page: &[u8], body: &str) -> (Vec<Glyph>, Vec<u8>) {
        let resources = Fake::new().with_form(b"Fm0", 7, Matrix::IDENTITY, body);
        (
            glyphs_in(page, &resources, &unwatched()).expect("walks"),
            body.as_bytes().to_vec(),
        )
    }

    #[test]
    fn a_shared_form_the_region_reaches_is_refused() {
        // EDITING IT IN PLACE WOULD REMOVE THE TEXT EVERYWHERE IT IS DRAWN -- a page nobody
        // asked about loses content, while the page the user selected looks correctly
        // redacted. Section 6's read-back cannot catch that: it asks about the page it was
        // given, and that page is clean.
        let (glyphs, _) = form_glyphs(b"/Fm0 Do", "/F1 10 Tf BT 0 0 Td (ABC) Tj ET");
        assert_eq!(glyphs[0].source.form, Some(7));
        let shared = Uses(vec![(7, 4)]);
        assert_refused_unit(
            check_form_sharing(&glyphs[1..2], &shared),
            Refusal::SharedFormWouldChangeElsewhere,
        );
    }

    #[test]
    fn a_letterhead_the_region_avoids_is_not_refused() {
        // THE SCOPING, and it is the decision rather than a detail of it. A header, a footer,
        // a watermark and a logo are all commonly one form drawn on every page. Refusing any
        // page that merely CONTAINS one would refuse a large share of real documents, and a
        // redaction nobody can run leaks nothing only because it never runs.
        let resources = Fake::new().with_form(
            b"Fm0",
            7,
            Matrix::IDENTITY,
            "/F1 10 Tf BT 0 0 Td (LETTERHEAD) Tj ET",
        );
        let page = b"/Fm0 Do /F1 10 Tf BT 0 0 Td (BODY) Tj ET";
        let glyphs = glyphs_in(page, &resources, &unwatched()).expect("walks");

        // The region reaches the page's own text, not the letterhead's.
        let body: Vec<Glyph> = glyphs
            .iter()
            .filter(|glyph| glyph.source.form.is_none())
            .cloned()
            .collect();
        assert!(
            !body.is_empty(),
            "the fixture must draw text outside the form"
        );
        assert!(
            glyphs.iter().any(|glyph| glyph.source.form == Some(7)),
            "and inside it, or the shared form is not present and this asks nothing"
        );

        let shared = Uses(vec![(7, 12)]);
        check_form_sharing(&body, &shared).expect("a letterhead the region avoids is not a bar");
        // And the redaction itself goes through, on the page's own stream.
        let out = remove_glyphs(page, None, &body[1..2], &unwatched()).expect("removes");
        assert!(String::from_utf8_lossy(&out).contains("TJ"));
    }

    #[test]
    fn a_form_drawn_once_is_edited_rather_than_refused() {
        // There is nowhere else for the edit to reach, so there is nothing to refuse.
        let (glyphs, content) = form_glyphs(b"/Fm0 Do", "/F1 10 Tf BT 0 0 Td (ABC) Tj ET");
        check_form_sharing(&glyphs[1..2], &Uses(vec![(7, 1)])).expect("drawn once");
        // Editing it means passing THAT form's content and identity, not the page's.
        let out = remove_glyphs(&content, Some(7), &glyphs[1..2], &unwatched()).expect("removes");
        assert!(String::from_utf8_lossy(&out).contains("-500"));
    }

    #[test]
    fn a_glyph_from_another_stream_is_still_refused() {
        // The scoping loosened WHICH stream may be edited, not whether a span has to belong to
        // the one being edited. A form's span against the page's bytes cuts in the wrong place.
        let (glyphs, _) = form_glyphs(b"/Fm0 Do", "/F1 10 Tf BT 0 0 Td (ABC) Tj ET");
        assert_refused_bytes(
            remove_glyphs(b"/Fm0 Do", None, &glyphs[1..2], &unwatched()),
            Refusal::GlyphFromAnotherStream,
        );
    }

    #[test]
    fn a_type_three_procedure_that_shows_text_is_refused() {
        // CHANNEL 8, FAILING CLOSED. The walk does not descend into `/CharProcs`, so a
        // redaction that removed this page's Type 3 glyphs would leave the procedure's own
        // text in the font. An `Ok` over text nothing observed is what §8 forbids.
        assert_refused_unit(
            check_type_three_procedure(b"500 0 d0\nBT /F2 1 Tf (SECRET) Tj ET\n", &unwatched()),
            Refusal::TypeThreeProcedureShowsText,
        );
        for shape in [
            b"500 0 d0 BT [(A)] TJ ET".as_slice(),
            b"500 0 d0 BT (A) ' ET".as_slice(),
            b"500 0 d0 BT 1 1 (A) \" ET".as_slice(),
        ] {
            assert_refused_unit(
                check_type_three_procedure(shape, &unwatched()),
                Refusal::TypeThreeProcedureShowsText,
            );
        }
    }

    #[test]
    fn an_ordinary_type_three_procedure_is_not_refused() {
        // THE NEAR-MISS, and it is the majority case. Almost every Type 3 procedure draws
        // shapes and no text; refusing those would refuse essentially every document with a
        // Type 3 font in it.
        check_type_three_procedure(b"500 0 d0\n0 0 500 500 re f\n", &unwatched())
            .expect("draws no text");
        // A SCAN, NOT A BYTE SEARCH: `Tj` inside a string is not an operator, and a byte
        // search would refuse this procedure for drawing nothing at all.
        check_type_three_procedure(b"500 0 d0\n% Tj in a comment\n0 0 1 1 re f\n", &unwatched())
            .expect("a comment is not an operator");
    }

    /// The rule-naming assertion, for a check that yields nothing.
    #[track_caller]
    fn assert_refused_unit(outcome: Result<()>, rule: Refusal) {
        match outcome {
            Err(error) => assert!(
                rule.caught(&error),
                "refused, but by a different rule: wanted `{}`, got {error:?}",
                rule.rule()
            ),
            Ok(()) => panic!("expected a refusal by `{}`, got success", rule.rule()),
        }
    }

    /// The rule-naming assertion, for a removal rather than a walk.
    #[track_caller]
    fn assert_refused_bytes(outcome: Result<Vec<u8>>, rule: Refusal) {
        match outcome {
            Err(error) => assert!(
                rule.caught(&error),
                "refused, but by a different rule: wanted `{}`, got {error:?}",
                rule.rule()
            ),
            Ok(bytes) => panic!(
                "expected a refusal by `{}`, got {} bytes",
                rule.rule(),
                bytes.len()
            ),
        }
    }

    #[test]
    fn an_optional_content_mark_is_refused_whatever_it_names() {
        refusing(
            "/OC /OC1 BDC BT /F1 12 Tf (AB) Tj ET EMC",
            Refusal::OptionalContentMarked,
        );
    }

    #[test]
    fn an_ordinary_marked_content_tag_is_not_an_optional_content_mark() {
        // The near-miss: `/P /MC0 BDC` and a tag merely spelled like one (`/OCX`) are ordinary.
        for content in [
            "/P /MC0 BDC BT /F1 12 Tf (AB) Tj ET EMC",
            "/OCX /MC0 BDC BT /F1 12 Tf (AB) Tj ET EMC",
            "/Span << /MCID 0 >> BDC BT /F1 12 Tf (AB) Tj ET EMC",
        ] {
            assert!(
                glyphs_in(content.as_bytes(), &Fake::new(), &unwatched()).is_ok(),
                "{content} must walk"
            );
        }
    }

    #[test]
    fn a_vertical_writing_mode_is_refused() {
        let mut resources = Fake::new();
        // STATED AS THE FILE WOULD STATE IT -- an embedded CMap whose program declares
        // `WMode 1` -- rather than as a `WritingMode` the fake simply asserts. The walk has to
        // read it to refuse it, which is what makes this a test of the derivation.
        resources.encoding = Encoding::Embedded {
            dictionary_wmode: None,
            program: b"/CMapName /Ordinary-H def /WMode 1 def".to_vec(),
        };
        resources.bytes_per_code = 2;
        // `(AB)` and not `(A)`: a one-byte string against a two-byte font is now refused for
        // not dividing into whole codes, which would make this a test of that rule instead.
        // The rule-naming assertion is what said so.
        assert_refused(
            glyphs_in(b"/F1 10 Tf BT 0 0 Td (AB) Tj ET", &resources, &unwatched()),
            Refusal::VerticalWriting,
        );
    }

    /// Per-rule probes for the marked-content rewriter.
    ///
    /// Each rule matches its own positive fixture and rejects a near-miss, and the near-misses
    /// are the point: a check that refuses every `BDC` would pass the positive cases while
    /// refusing every tagged document in existence, and one that refuses none would pass
    /// nothing. Both of those printed a clean corpus sweep before this existed.
    mod marked_content_probes {
        use super::{Fake, glyphs_in, unwatched};

        /// A stream that draws no form the removal reaches — the ordinary case for these
        /// probes, whose fixtures draw their glyphs inline.
        fn nothing_drawn() -> std::collections::BTreeSet<Vec<u8>> {
            std::collections::BTreeSet::new()
        }

        /// The glyphs `content` draws, all of them, from the caller's own stream.
        fn all_glyphs(content: &[u8]) -> Vec<super::super::Glyph> {
            glyphs_in(content, &Fake::new(), &unwatched()).expect("the fixture walks")
        }

        /// How many property lists the rewriter would strip from `content`.
        ///
        /// The positive shapes assert on THIS now rather than on a refusal: `/ActualText` is
        /// handled, so "does it refuse" stopped being the question and "does it find the span
        /// to strip" became it. A probe left asserting the refusal would have gone green by
        /// asserting the old behaviour of a rule that no longer has it.
        #[track_caller]
        fn strips(content: &[u8]) -> usize {
            let glyphs = all_glyphs(content);
            let edits = super::super::carried_text_edits(
                content,
                &glyphs,
                None,
                &super::super::FormsReached::Named(&nothing_drawn()),
                &super::super::NamedProperties::default(),
                &unwatched(),
            )
            .expect("the fixture is readable");
            for (_, replacement) in &edits {
                let text = String::from_utf8_lossy(replacement);
                // EVERY KEY IN THE CONSTANT, not the two that were there when this was
                // written. `/E` was added to `TEXT_CARRYING_KEYS` and not here, which left the
                // helper guarding a smaller set than the thing it guards.
                for key in super::super::TEXT_CARRYING_KEYS {
                    let key = String::from_utf8_lossy(key);
                    assert!(
                        !text.contains(key.as_ref()),
                        "a rewritten property list still carries /{key}: {text}"
                    );
                }
            }
            edits.len()
        }

        /// Assert `content`'s own glyphs are refused, **and by which rule**.
        ///
        /// By the variant rather than by the rule's string, for the reason `assert_refused`
        /// gives one level up: a rule name is a `&str` any refusal could produce, and
        /// `Refusal::caught` is the thing that distinguishes them.
        #[track_caller]
        fn assert_refused_by(content: &[u8], rule: super::Refusal) {
            let glyphs = all_glyphs(content);
            match super::super::carried_text_edits(
                content,
                &glyphs,
                None,
                &super::super::FormsReached::Named(&nothing_drawn()),
                &super::super::NamedProperties::default(),
                &unwatched(),
            ) {
                Err(error) => assert!(
                    rule.caught(&error),
                    "refused, but by a different rule: wanted `{}`, got {error:?}",
                    rule.rule()
                ),
                Ok(_) => panic!("expected a refusal by `{}`, got none", rule.rule()),
            }
        }

        /// A carried key named where the rewriter cannot remove it is refused, not emitted.
        ///
        /// This shape — a *name* in array position rather than a key — is the gap the narrowing
        /// of `holds_text_key` opened, and it reached the emitted file once. The positive probe
        /// is that exact shape.
        #[test]
        fn a_carried_key_named_outside_key_position_is_refused() {
            assert_refused_by(
                b"/Span << /MCID 0 /K [ /ActualText (secret) ] >> BDC BT /F1 12 Tf (x) Tj ET EMC",
                super::Refusal::MarkedContentCarriesOpaqueString,
            );
        }

        /// The same refusal when the list ALSO held a handled key, decided on the rewritten bytes.
        ///
        /// The `Carried::OpaqueString` arm never sees this list, because it holds `/ActualText`
        /// and so classifies as `Carried::Text`. Without the second check the rewrite would strip
        /// the key it knows and emit the string it does not.
        #[test]
        fn a_leftover_naming_beside_a_handled_key_is_refused_too() {
            assert_refused_by(
                b"/Span << /ActualText (a) /K [ /Alt (secret) ] >> BDC BT /F1 12 Tf (x) Tj ET EMC",
                super::Refusal::MarkedContentCarriesOpaqueString,
            );
        }

        /// THE NEAR-MISS: an ordinary string under an ordinary key is left alone, not refused.
        ///
        /// `/Lang` is the case that killed the first version of this rule, which refused any
        /// property list holding a string. A rule that refuses every covering span passes both
        /// probes above while making the operation useless, which is what a near-miss is for.
        #[test]
        fn an_ordinary_string_under_an_ordinary_key_is_not_refused() {
            assert_allowed(b"/Span << /MCID 0 /Lang (en-US) >> BDC BT /F1 12 Tf (x) Tj ET EMC");
        }

        /// A key with no value is refused, not rebuilt into something the document did not have.
        ///
        /// Measured end to end before this: `/ActualText 4 0 R` (an indirect reference, three
        /// items after `/MCID 0`) rebuilt as `<< /MCID 0 0 R >>`.
        #[test]
        fn an_odd_property_list_is_refused_rather_than_silently_repaired() {
            assert_refused_by(
                b"/Span << /MCID 0 /ActualText 4 0 R >> BDC BT /F1 12 Tf (x) Tj ET EMC",
                super::Refusal::MarkedContentPropertyListMalformed,
            );
        }

        /// The same, one dictionary down, where the loss was of an unrelated key.
        ///
        /// `/Span << /MCID 0 /Pad << /ActualText (X) >> /Tail >>` dropped `/Tail`.
        #[test]
        fn an_odd_nested_property_list_is_refused_too() {
            assert_refused_by(
                b"/Span << /MCID 0 /Pad << /ActualText (X) >> /Tail >> BDC BT /F1 12 Tf (x) Tj \
                  ET EMC",
                super::Refusal::MarkedContentPropertyListMalformed,
            );
        }

        /// THE NEAR-MISS: an even property list carrying the same key is rewritten, not refused.
        ///
        /// A rule that refused every nested dictionary would pass both probes above and take the
        /// nesting case — the one a previous round's leak was about — offline with it.
        #[test]
        fn an_even_property_list_with_a_nested_carrier_is_still_rewritten() {
            assert_eq!(
                strips(
                    b"/Span << /MCID 0 /Pad << /ActualText (X) >> >> BDC BT /F1 12 Tf (x) Tj \
                         ET EMC"
                ),
                1
            );
        }

        /// Assert `content`'s own glyphs are allowed through.
        #[track_caller]
        fn assert_allowed(content: &[u8]) {
            let glyphs = all_glyphs(content);
            match super::super::carried_text_edits(
                content,
                &glyphs,
                None,
                &super::super::FormsReached::Named(&nothing_drawn()),
                &super::super::NamedProperties::default(),
                &unwatched(),
            ) {
                Err(error) => panic!("expected no refusal, got {error:?}"),
                // AND NOTHING STRIPPED. "Allowed" used to mean "did not refuse"; with the
                // rewriter it has to mean "and left the document alone", or a probe for an
                // ordinary tagged span would pass while the span was being rewritten.
                Ok(edits) => assert!(
                    edits.is_empty(),
                    "expected nothing stripped, got {} edit(s)",
                    edits.len()
                ),
            }
        }

        #[test]
        fn the_rewritten_property_list_keeps_every_other_key() {
            let out = super::super::without_carried_keys(
                b"<< /Type /Span /ActualText (secret) /MCID 4 /Lang (en-GB) >>",
            )
            .expect("re-reads");
            let text = String::from_utf8_lossy(&out);
            assert!(!text.contains("ActualText"), "{text}");
            assert!(text.contains("/Type /Span"), "{text}");
            assert!(text.contains("/MCID 4"), "{text}");
            assert!(text.contains("/Lang (en-GB)"), "{text}");
        }

        #[test]
        fn a_value_containing_the_dictionary_terminator_does_not_end_it_early() {
            // WHY IT IS REBUILT FROM THE PARSE RATHER THAN SPLICED OUT OF THE BYTES. A string
            // value may contain `>>`, and cutting on that byte pattern would truncate the
            // dictionary and take every later key with it -- including `/MCID`, which is how a
            // tagged document is stitched to its structure tree.
            let out = super::super::without_carried_keys(
                b"<< /ActualText (ends with >> inside) /MCID 7 >>",
            )
            .expect("re-reads");
            let text = String::from_utf8_lossy(&out);
            assert!(!text.contains("ActualText"), "{text}");
            assert!(text.contains("/MCID 7"), "{text}");
        }

        #[test]
        fn alt_goes_with_actualtext() {
            let out = super::super::without_carried_keys(b"<< /Alt (a description) /MCID 1 >>")
                .expect("re-reads");
            let text = String::from_utf8_lossy(&out);
            assert!(!text.contains("/Alt"), "{text}");
            assert!(text.contains("/MCID 1"), "{text}");
        }

        #[test]
        fn a_property_list_carrying_nothing_comes_back_equivalent() {
            let out = super::super::without_carried_keys(b"<< /MCID 0 >>").expect("re-reads");
            assert!(String::from_utf8_lossy(&out).contains("/MCID 0"));
        }

        /// Assert a combined pass refused, by the named rule.
        #[track_caller]
        fn assert_split_refusal(
            outcome: burrow_types::Result<(Vec<Vec<u8>>, usize)>,
            rule: super::Refusal,
        ) {
            match outcome {
                Err(error) => assert!(
                    rule.caught(&error),
                    "refused by the wrong rule: wanted `{}`, got {error:?}",
                    rule.rule()
                ),
                Ok(_) => panic!("expected a refusal by `{}`", rule.rule()),
            }
        }

        #[test]
        fn a_property_list_split_across_two_contents_elements_refuses_by_name() {
            // §7.8.2 divides a `/Contents` array between lexical tokens, so a `BDC`'s dictionary
            // may legally begin in one element and end in the next. `Contents::apply` refuses to
            // replace across a boundary; this gives that outcome a rule name so the corpus sweep
            // records it rather than panicking on an unnamed refusal.
            let first = b"/Span << /ActualText (secret)".as_slice();
            let second = b" >> BDC BT /F1 12 Tf (AB) Tj ET EMC".as_slice();
            let contents = super::super::super::contents::Contents::concatenate(&[first, second])
                .expect("two elements");
            let glyphs = glyphs_in(contents.bytes(), &Fake::new(), &unwatched()).expect("walks");
            let outcome = super::super::remove_glyphs_and_carried_text(
                &contents,
                None,
                &glyphs,
                &super::super::FormsReached::Named(&nothing_drawn()),
                &super::super::NamedProperties::default(),
                &unwatched(),
            );
            // THE RULE PASSED AS AN ARGUMENT. That mattered to the text-search gate #177 replaced,
            // which scanned for `Refusal::X)`; reachability is now measured by the witness gate,
            // and the argument form stays because it names the rule in the assertion.
            assert_split_refusal(outcome, super::Refusal::MarkedContentSplitAcrossElements);
        }

        #[test]
        fn an_inner_bmc_does_not_close_the_carrying_span() {
            // THE `BMC` HALF OF "A STACK, NOT A FLAG", which the nesting probes never covered:
            // they all used an inner `BDC`. A security review deleted the `BMC` push and the
            // whole suite stayed green, while `/Span << /ActualText … >> BDC BMC EMC BT … EMC`
            // emitted the canary verbatim -- the inner `EMC` popped the carrying span.
            assert_eq!(
                strips(
                    // `/Tx BMC`, not a bare `BMC`: the marked-content operators have an operand
                    // count now, and a tagless `BMC` is refused before the stack is reached.
                    b"/Span << /ActualText (secret) >> BDC /Tx BMC EMC \
                      BT /F1 12 Tf (AB) Tj ET EMC"
                ),
                1
            );
        }

        #[test]
        fn expansion_text_is_carried_text_too() {
            // `/E` is §14.9.5's expansion text -- what an abbreviation stands for -- and a screen
            // reader reads it in place of the glyphs, exactly as it does `/ActualText`. It was
            // missing from `TEXT_CARRYING_KEYS`, so this redacted with the string intact.
            assert_eq!(
                strips(b"/Span << /E (secret) /MCID 0 >> BDC BT /F1 12 Tf (AB) Tj ET EMC"),
                1
            );
        }

        #[test]
        fn an_actualtext_nested_under_another_key_is_stripped_too() {
            // THE DETECTOR READS ANY DEPTH AND THE REWRITER ONLY READ THE TOP. A code review
            // measured the result: this shape was classified as carrying text, produced an edit
            // whose replacement was byte-identical to the input, and -- with the refusal gone --
            // shipped. Every probe here had been written at the shallow shape.
            assert_eq!(
                strips(
                    b"/Span << /A << /ActualText (secret) >> /MCID 0 >> BDC \
                      BT /F1 12 Tf (AB) Tj ET EMC"
                ),
                1
            );
        }

        #[test]
        fn an_actualtext_inside_an_array_is_stripped_too() {
            assert_eq!(
                strips(
                    b"/Span << /A [ << /ActualText (secret) >> ] /MCID 0 >> BDC \
                      BT /F1 12 Tf (AB) Tj ET EMC"
                ),
                1
            );
        }

        #[test]
        fn an_actualtext_span_around_the_removed_glyph_is_stripped() {
            assert_eq!(
                strips(b"/Span << /ActualText (secret) >> BDC BT /F1 12 Tf (AB) Tj ET EMC"),
                1
            );
        }

        #[test]
        fn an_alt_span_is_stripped_the_same_way() {
            // `/Alt` is a description rather than a replacement, and a description of content
            // that has been partly removed is false. False alternative text is worse than none.
            assert_eq!(
                strips(b"/Span << /Alt (secret) >> BDC BT /F1 12 Tf (AB) Tj ET EMC"),
                1
            );
        }

        #[test]
        fn a_named_property_list_is_refused_as_unread_rather_than_as_found() {
            // THE NEAR-MISS THAT SEPARATES THE TWO RULES. Nothing here carries text as far as
            // anyone knows; what is true is that this module cannot resolve `/MC0`. Reporting
            // it as `carries-text` would be an assertion about the user's document that no
            // measurement supports.
            assert_refused_by(
                b"/Span /MC0 BDC BT /F1 12 Tf (AB) Tj ET EMC",
                super::Refusal::MarkedContentPropertiesUnresolved,
            );
        }

        /// `names` resolving through one scope, each `(name, list)` a property list as PDF.
        fn resolving(names: &[(&str, &str)]) -> super::super::NamedProperties {
            let mut properties = super::super::NamedProperties::default();
            for (name, list) in names {
                properties.insert(
                    name.as_bytes().to_vec(),
                    super::super::PropertyList::read(list.as_bytes()),
                );
            }
            properties
        }

        /// What `content`'s own removal gets, with `properties` resolving its names.
        fn edits_with(
            content: &[u8],
            properties: &super::super::NamedProperties,
        ) -> burrow_types::Result<Vec<(super::super::Span, Vec<u8>)>> {
            super::super::carried_text_edits(
                content,
                &all_glyphs(content),
                None,
                &super::super::FormsReached::Named(&nothing_drawn()),
                properties,
                &unwatched(),
            )
        }

        #[track_caller]
        fn assert_named_refusal(
            outcome: burrow_types::Result<Vec<(super::super::Span, Vec<u8>)>>,
            rule: super::Refusal,
        ) {
            match outcome {
                Err(error) => assert!(
                    rule.caught(&error),
                    "refused, but by a different rule: wanted `{}`, got {error:?}",
                    rule.rule()
                ),
                Ok(edits) => panic!(
                    "expected a refusal by `{}`, got {} edit(s)",
                    rule.rule(),
                    edits.len()
                ),
            }
        }

        const NAMED: &[u8] = b"/Span /MC0 BDC BT /F1 12 Tf (AB) Tj ET EMC";

        #[test]
        fn a_named_list_that_resolves_to_an_ordinary_one_is_untouched() {
            // THE PROBE THAT MEASURES WHETHER #166 BOUGHT ANYTHING. Before the resolver this
            // refused as unresolved, and it is what tagged output looks like.
            let outcome = edits_with(NAMED, &resolving(&[("MC0", "<< /MCID 0 >>")]));
            assert!(
                outcome.as_ref().is_ok_and(Vec::is_empty),
                "an ordinary named list must pass with nothing stripped: {outcome:?}"
            );
        }

        #[test]
        fn a_named_list_that_carries_text_is_refused_by_name() {
            assert_named_refusal(
                edits_with(
                    NAMED,
                    &resolving(&[("MC0", "<< /MCID 0 /ActualText (secret) >>")]),
                ),
                super::Refusal::MarkedContentNamedPropertiesCarryText,
            );
        }

        #[test]
        fn a_named_list_naming_a_carried_key_outside_key_position_is_refused_too() {
            // Nothing is rewritten for a named list, so the key/non-key distinction the
            // rewriter draws does not apply: the name is there, beside a string, in a resource
            // this operation will not edit.
            assert_named_refusal(
                edits_with(NAMED, &resolving(&[("MC0", "<< /K [ /Alt (secret) ] >>")])),
                super::Refusal::MarkedContentNamedPropertiesCarryText,
            );
        }

        #[test]
        fn a_named_list_holding_a_reference_is_unresolved_rather_than_passed() {
            // `5 0 R` lexes as two numbers and the keyword `R`. The target is not in the bytes
            // and could be `<< /ActualText … >>`.
            assert_named_refusal(
                edits_with(NAMED, &resolving(&[("MC0", "<< /MCID 0 /Pad 5 0 R >>")])),
                super::Refusal::MarkedContentPropertiesUnresolved,
            );
        }

        #[test]
        fn a_name_the_scope_does_not_define_is_unresolved() {
            // Resolving `MC1` does not resolve `MC0`. A resolver that answered "nothing" for a
            // name it did not hold would pass exactly the span it never looked at.
            assert_named_refusal(
                edits_with(NAMED, &resolving(&[("MC1", "<< /MCID 0 >>")])),
                super::Refusal::MarkedContentPropertiesUnresolved,
            );
        }

        #[test]
        fn a_named_list_carrying_an_operator_is_unresolved_rather_than_read_by_its_first_half() {
            // `PropertyList::read` lexes `<list> BDC` and reads the first operation's operand.
            // Bytes holding an operator split into two operations, and the first half alone reads
            // as ordinary. qpdf's `unparse` never produces this; `PropertyList` is public, and the
            // filter that refuses it survived a mutation sweep until this probe.
            assert_named_refusal(
                edits_with(
                    NAMED,
                    &resolving(&[("MC0", "<< /MCID 0 >> BDC /S << /ActualText (x) >>")]),
                ),
                super::Refusal::MarkedContentPropertiesUnresolved,
            );
        }

        #[test]
        fn a_named_list_that_is_not_a_dictionary_is_unresolved() {
            assert_named_refusal(
                edits_with(NAMED, &resolving(&[("MC0", "(not a dictionary)")])),
                super::Refusal::MarkedContentPropertiesUnresolved,
            );
        }

        #[test]
        fn the_most_cautious_candidate_wins_when_a_name_resolves_two_ways() {
            // A form declaring no `/Resources` inherits from whatever encloses it, and a form
            // reached by two routes has two enclosures. First-wins or last-wins would let the
            // order of the walk decide whether the text is seen -- the memo defect again.
            let mut properties = resolving(&[("MC0", "<< /MCID 0 >>")]);
            properties.extend(&resolving(&[("MC0", "<< /ActualText (secret) >>")]));
            assert_named_refusal(
                edits_with(NAMED, &properties),
                super::Refusal::MarkedContentNamedPropertiesCarryText,
            );
            let mut reversed = resolving(&[("MC0", "<< /ActualText (secret) >>")]);
            reversed.extend(&resolving(&[("MC0", "<< /MCID 0 >>")]));
            assert_named_refusal(
                edits_with(NAMED, &reversed),
                super::Refusal::MarkedContentNamedPropertiesCarryText,
            );
        }

        #[test]
        fn an_unreadable_candidate_outranks_one_that_carries_nothing() {
            // A name one scope defines plainly and another defines unreadably could carry text
            // through the second. `Unknown` ranked equal to `Nothing` survived a mutation sweep.
            let mut properties = resolving(&[("MC0", "<< /MCID 0 >>")]);
            properties.extend(&resolving(&[("MC0", "<< /K 5 0 R >>")]));
            assert_named_refusal(
                edits_with(NAMED, &properties),
                super::Refusal::MarkedContentPropertiesUnresolved,
            );
        }

        #[test]
        fn named_text_outranks_an_unreadable_candidate() {
            // Both refuse; the rule is what a user reads. A list that was read and found to carry
            // text is the more specific true statement, so it names the refusal. The ranking
            // survived a mutation sweep until this probe.
            let mut properties = resolving(&[("MC0", "<< /K 5 0 R >>")]);
            properties.extend(&resolving(&[("MC0", "<< /ActualText (secret) >>")]));
            assert_named_refusal(
                edits_with(NAMED, &properties),
                super::Refusal::MarkedContentNamedPropertiesCarryText,
            );
            let mut reversed = resolving(&[("MC0", "<< /ActualText (secret) >>")]);
            reversed.extend(&resolving(&[("MC0", "<< /K 5 0 R >>")]));
            assert_named_refusal(
                edits_with(NAMED, &reversed),
                super::Refusal::MarkedContentNamedPropertiesCarryText,
            );
        }

        #[test]
        fn a_named_carrying_span_the_removal_is_not_inside_is_not_refused() {
            // SCOPED TO THE SPANS THE REMOVAL IS INSIDE, as the form rule is scoped to glyphs
            // being removed. The carrying span covers `AB`; the removal is `CD`.
            let content = b"/Span /MC0 BDC BT /F1 12 Tf (AB) Tj ET EMC BT /F1 12 Tf (CD) Tj ET";
            let outside: Vec<_> = all_glyphs(content)
                .into_iter()
                .filter(|glyph| glyph.source.code == u32::from(b'C'))
                .collect();
            assert_eq!(
                outside.len(),
                1,
                "the fixture must draw one C outside the span"
            );
            let outcome = super::super::carried_text_edits(
                content,
                &outside,
                None,
                &super::super::FormsReached::Named(&nothing_drawn()),
                &resolving(&[("MC0", "<< /ActualText (secret) >>")]),
                &unwatched(),
            );
            assert!(
                outcome.as_ref().is_ok_and(Vec::is_empty),
                "a carrying span elsewhere must not refuse the removal: {outcome:?}"
            );
        }

        #[test]
        fn an_inner_span_left_open_does_not_mask_a_named_carrying_outer_one() {
            // The `last()` mutation, for the named arm: the inner inline `/P` span is open over
            // the glyph and carries nothing.
            assert_named_refusal(
                edits_with(
                    b"/Span /MC0 BDC /P << /MCID 0 >> BDC BT /F1 12 Tf (AB) Tj ET EMC EMC",
                    &resolving(&[("MC0", "<< /ActualText (secret) >>")]),
                ),
                super::Refusal::MarkedContentNamedPropertiesCarryText,
            );
        }

        #[test]
        fn an_ordinary_tagged_span_is_untouched() {
            // THE NEAR-MISS FOR THE WHOLE CHECK. `/P << /MCID 0 >> BDC` is what every tagged
            // PDF is full of. A rule that refused this would refuse most real documents while
            // still passing all three positives above.
            assert_allowed(b"/P << /MCID 0 >> BDC BT /F1 12 Tf (AB) Tj ET EMC");
            assert_eq!(
                strips(b"/P << /MCID 0 >> BDC BT /F1 12 Tf (AB) Tj ET EMC"),
                0
            );
        }

        #[test]
        fn a_carrying_span_the_removal_does_not_reach_is_not_refused() {
            // Scope: the rule is about spans the removed glyphs are *inside*. A document may
            // carry `/ActualText` elsewhere on the page and still be redactable.
            let content = b"/Span << /ActualText (secret) >> BDC BT /F1 12 Tf (AB) Tj ET EMC \
                  BT /F1 12 Tf (CD) Tj ET";
            let glyphs = all_glyphs(content);
            // BY CODE, not by position: which glyph is "outside" is a fact about the fixture's
            // text, and selecting by an x coordinate made this test depend on the fake's
            // widths instead.
            let outside: Vec<_> = glyphs
                .iter()
                .filter(|glyph| {
                    glyph.source.code == u32::from(b'C') || glyph.source.code == u32::from(b'D')
                })
                .cloned()
                .collect();
            assert_eq!(
                outside.len(),
                2,
                "the fixture must draw exactly the two glyphs outside the span"
            );
            // NO EDITS, not merely no refusal. `carried_text_edits` returning `Ok` now says
            // only that nothing was unreadable; the near-miss claim is that nothing was
            // stripped, and asserting `is_ok()` would pass on a rewrite of a span the removal
            // never reached.
            assert!(
                super::super::carried_text_edits(
                    content,
                    &outside,
                    None,
                    &super::super::FormsReached::Named(&nothing_drawn()),
                    &super::super::NamedProperties::default(),
                    &unwatched(),
                )
                .expect("readable")
                .is_empty()
            );
        }

        #[test]
        fn a_nested_inner_span_does_not_close_the_carrying_outer_one() {
            // The stack, not a flag. The inner `EMC` must not clear the outer `/ActualText`.
            assert_eq!(
                strips(
                    b"/Span << /ActualText (secret) >> BDC /P << /MCID 0 >> BDC EMC \
                      BT /F1 12 Tf (AB) Tj ET EMC"
                ),
                1
            );
        }

        #[test]
        fn an_inner_span_left_open_does_not_mask_the_carrying_outer_one() {
            // THE PROBE THAT WAS MISSING, and a security review measured its absence: mutating
            // `open.contains(&Carried::Text)` to `open.last() == Some(&Carried::Text)` --
            // "only the innermost span counts" -- survived the entire suite. The nesting probe
            // above closes the inner span before the glyph, so `last()` is still the carrier
            // there and the two spellings cannot be told apart by it.
            //
            // `/Span << /ActualText … >> BDC /P << /MCID 0 >> BDC  BT … Tj ET  EMC EMC` is the
            // commonest shape in a tagged PDF there is, and under that mutation it leaks.
            assert_eq!(
                strips(
                    b"/Span << /ActualText (secret) >> BDC /P << /MCID 0 >> BDC \
                      BT /F1 12 Tf (AB) Tj ET EMC EMC"
                ),
                1
            );
        }

        #[test]
        fn an_inner_span_left_open_does_not_mask_an_unresolved_outer_one() {
            // The same, for the other rule, so neither can regress to `last()`.
            assert_refused_by(
                b"/Span /MC0 BDC /P << /MCID 0 >> BDC BT /F1 12 Tf (AB) Tj ET EMC EMC",
                super::Refusal::MarkedContentPropertiesUnresolved,
            );
        }

        #[test]
        fn a_do_inside_a_carrying_span_is_a_removal_site() {
            // THE CROSS-STREAM CASE, at the unit level. The glyphs are not in this stream at
            // all -- `remove` is empty for it -- and the span still has to answer for the `Do`
            // it wraps. Measured as a working leak before the refusal existed, and the span
            // has to be found by the rewriter for the same reason it had to be found by the check.
            let content = b"/Span << /ActualText (secret) >> BDC /X1 Do EMC";
            let names: std::collections::BTreeSet<Vec<u8>> = [b"X1".to_vec()].into_iter().collect();
            let edits = super::super::carried_text_edits(
                content,
                &[],
                None,
                &super::super::FormsReached::Named(&names),
                &super::super::NamedProperties::default(),
                &unwatched(),
            )
            .expect("readable");
            assert_eq!(
                edits.len(),
                1,
                "a span wrapping a reached form must be stripped"
            );
            assert!(
                !String::from_utf8_lossy(&edits[0].1).contains("ActualText"),
                "{:?}",
                String::from_utf8_lossy(&edits[0].1)
            );
        }

        #[test]
        fn a_do_drawing_a_form_the_removal_never_reaches_is_not_a_removal_site() {
            // The near-miss: the same page, with the removal touching a different form. A rule
            // that refused every `Do` under a carrying span would pass the test above and
            // refuse every tagged document that draws a logo inside a tagged span.
            let content = b"/Span << /ActualText (secret) >> BDC /X1 Do EMC /X2 Do";
            let names: std::collections::BTreeSet<Vec<u8>> = [b"X2".to_vec()].into_iter().collect();
            assert!(
                super::super::carried_text_edits(
                    content,
                    &[],
                    None,
                    &super::super::FormsReached::Named(&names),
                    &super::super::NamedProperties::default(),
                    &unwatched(),
                )
                .expect("readable")
                .is_empty()
            );
        }

        #[test]
        fn a_span_that_closed_before_the_glyph_is_untouched() {
            // The other direction of the same stack: a carrying span that is properly closed
            // must not keep refusing everything after it.
            assert_allowed(b"/Span << /ActualText (secret) >> BDC EMC BT /F1 12 Tf (AB) Tj ET");
            assert_eq!(
                strips(b"/Span << /ActualText (secret) >> BDC EMC BT /F1 12 Tf (AB) Tj ET"),
                0
            );
        }
    }

    // ---- #175: the deadline is read INSIDE the walk --------------------------------------------
    //
    // Each of the three walks reads the clock at two places: once its stream is lexed, and every
    // `WATCH_EVERY` operations after. Each test below is built so that exactly ONE of those reads
    // expires the deadline, so deleting either one, in any of the three, fails a test by name.

    /// A clock that moves one millisecond per READ, so a deadline expires after a known number of
    /// reads -- which is the question: whether the walk reads it at all.
    #[derive(Debug, Default)]
    struct Ticking(std::sync::atomic::AtomicU64);

    impl burrow_types::Clock for Ticking {
        fn now_ms(&self) -> u64 {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        }
    }

    /// The four walks that take a watch, each over `content`'s own glyphs.
    type WalkUnderWatch = fn(&[u8], &Watch<'_>) -> Result<()>;
    const WATCHED_WALKS: [(&str, WalkUnderWatch); 4] = [
        // A PROCEDURE, WITH ITS OWN STREAMS: `SHORT` shows text, which a Type 3 procedure
        // refuses at the first operation, before any per-operation read could come due.
        ("check_type_three_procedure", |content, watch| {
            check_type_three_procedure(procedure_of(content), watch)
        }),
        ("glyphs_in", |content, watch| {
            glyphs_in(content, &Fake::new(), watch).map(drop)
        }),
        ("carried_text_edits", |content, watch| {
            let glyphs = glyphs_in(content, &Fake::new(), &unwatched())?;
            carried_text_edits(
                content,
                &glyphs,
                None,
                &FormsReached::Named(&BTreeSet::new()),
                &NamedProperties::default(),
                watch,
            )
            .map(drop)
        }),
        ("remove_glyphs", |content, watch| {
            let glyphs = glyphs_in(content, &Fake::new(), &unwatched())?;
            remove_glyphs(content, None, &glyphs, watch).map(drop)
        }),
    ];

    const SHORT: &str = "/F1 10 Tf BT 0 0 Td (A) Tj ET";

    /// A Type 3 procedure standing in for `content`: as many operations, and no text.
    fn procedure_of(content: &[u8]) -> &'static [u8] {
        static LONG: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
        if content.len() > SHORT.len() {
            LONG.get_or_init(|| {
                format!("500 0 d0{}", " q Q".repeat(WATCH_EVERY as usize * 4)).into_bytes()
            })
        } else {
            b"500 0 d0 q Q"
        }
    }

    /// `SHORT`, then enough `q Q` for the per-operation read to come due four times.
    fn long() -> String {
        format!("{SHORT}{}", " q Q".repeat(WATCH_EVERY as usize * 4))
    }

    /// Runs `walk` over `content` under a budget of `ms` on a `Ticking` clock.
    fn under(walk: WalkUnderWatch, content: &str, ms: u64) -> Result<()> {
        let clock = Ticking::default();
        let deadline = Deadline::start(&clock, &Limits::with(|limits| limits.max_duration_ms = ms));
        walk(content.as_bytes(), &Watch::new(deadline, &clock))
    }

    fn assert_expired(outcome: Result<()>, what: &str) {
        match outcome {
            Err(Error::LimitExceeded { limit, .. }) => {
                assert_eq!(limit, "max_duration_ms", "{what}: the wrong limit");
            }
            other => {
                panic!("{what}: expected max_duration_ms to expire inside the walk, got {other:?}")
            }
        }
    }

    #[test]
    fn every_walk_reads_the_clock_once_its_stream_is_lexed() {
        // KILLS: deleting the read after the lex, in any of the three. A budget of 0 and a stream
        // too short for a per-operation read: the post-lex read is the only one there is.
        for (name, walk) in WATCHED_WALKS {
            assert!(
                under(walk, SHORT, 1_000).is_ok(),
                "{name}: the control must walk"
            );
            assert_expired(under(walk, SHORT, 0), name);
        }
    }

    #[test]
    fn every_walk_reads_the_clock_while_it_works() {
        // KILLS: deleting the per-operation tick, in any of the three. A budget of 1 survives the
        // post-lex read (elapsed 1) -- shown by the short stream passing -- so only a read made
        // while the operations run can expire it.
        for (name, walk) in WATCHED_WALKS {
            assert!(
                under(walk, SHORT, 1).is_ok(),
                "{name}: the post-lex read alone must not expire"
            );
            assert!(
                under(walk, &long(), 1_000).is_ok(),
                "{name}: the control must walk"
            );
            assert_expired(under(walk, &long(), 1), name);
        }
    }

    #[test]
    fn a_form_drawn_many_times_is_read_at_every_draw() {
        // THE SHAPE #175 MEASURED at 17.9 s: one form, drawn again and again, each draw a fresh
        // walk. Twenty draws of a two-operation form is 60 operations, never a per-operation read,
        // so it is each form walk's own post-lex read that expires a budget of 10.
        let resources = Fake::new().with_form(b"Fm0", 7, Matrix::IDENTITY, "q Q");
        let content = "/Fm0 Do ".repeat(20);
        let clock = Ticking::default();
        let deadline = Deadline::start(&clock, &Limits::with(|limits| limits.max_duration_ms = 10));
        assert_expired(
            glyphs_in(
                content.as_bytes(),
                &resources,
                &Watch::new(deadline, &clock),
            )
            .map(drop),
            "twenty form draws",
        );
    }
}

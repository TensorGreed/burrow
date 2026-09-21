//! A content stream as operations, each operator with the operands that belong to it.
//!
//! `pdfsyntax::lexer` — private to this module — yields a flat token stream. Nothing in it pairs `Tj` with the string before
//! it, or groups `1 0 0 1 72 720` into the six numbers `Tm` takes. `split` never needed that — it
//! asks only *which names appear* — and redaction cannot be written without it: where a glyph
//! lands on the page is a function of `Tm`, `Tz`, `Ts`, `Tc`, `Tw`, `TL` and the Form's `/Matrix`,
//! and each of those is an operand that has to be read as its operator's.
//!
//! # The operator-table argument inverts here, and [`super::names`] is not wrong
//!
//! `names` argues against an operator table:
//!
//! > an operator table is a list of operators somebody thought of, and PDF 32000-1 plus its
//! > extensions is not a closed set
//!
//! That is correct **for a filter that over-approximates**. A name collected because it merely
//! appeared keeps one resource that could have gone; there is nothing an unlisted operator can do
//! to make that answer unsafe, so having no table is strictly better than having an incomplete one.
//!
//! For a rewriter the asymmetry runs the other way: an operator it does not understand is output
//! it gets **wrong**, and wrong output in redaction is the leak. So this module does the one thing
//! `names` refuses to do — and it still holds no table of operators. It groups operands by
//! *position*, which is a property of PDF's postfix syntax rather than of any operator list: every
//! operand precedes its operator, and an operator ends the run. An operator nobody has heard of is
//! carried through with its operands intact and its bytes untouched.
//!
//! **A caller that must know what an operator MEANS keeps that table, and owns being incomplete
//! about it.** That is the honest place for it: the geometry pass (#129) knows it cannot place a
//! glyph drawn by an operator it does not model, and can refuse. This layer would only be able to
//! guess.
//!
//! # Inline images arrive as two operations, and that is deliberate
//!
//! `BI /W 1 /H 1 ID <binary> EI` lexes as `BI`, then the dictionary's tokens, then a keyword `ID`
//! whose span the lexer has already advanced **past `EI`** — it skips the binary data itself
//! rather than leaving that to a caller who might forget. So this yields `BI` with no operands and
//! then `ID` carrying the image's keys as its operands, with a span covering every byte through
//! `EI`. Nothing is hidden and no byte is unaccounted for, which is what a rewriter needs; a
//! caller that wants the pair as one unit can see it from the operators.

use burrow_types::{Error, Result};

use super::lexer::{Lexer, MAX_NESTING, Token};

/// A half-open byte range into the content stream the operation was read from.
pub type Span = (usize, usize);

/// One operand, with the bytes it occupies and the value a caller needs.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Operand {
    /// An integer or a real, already parsed. PDF has one numeric type at this level.
    Number {
        /// Where the digits are.
        span: Span,
        /// What they say.
        value: f64,
    },
    /// A literal or hex string. The bytes are **not** decoded here — call
    /// [`decode_string`](super::strings::decode_string) on the span when the value is wanted, so
    /// a caller that only needs to know a string is there allocates nothing.
    Str {
        /// Delimiters included, which is what `decode_string` expects.
        span: Span,
    },
    /// A `/Name`, `#xx` escapes decoded.
    Name {
        /// The `/` included.
        span: Span,
        /// The decoded name, without the `/`.
        value: Vec<u8>,
    },
    /// `[ … ]`. `TJ`'s operand, and the reason arrays are grouped rather than flattened.
    Array {
        /// Brackets included.
        span: Span,
        /// The elements, in order.
        items: Vec<Operand>,
    },
    /// `<< … >>`. `BDC`'s and an inline image's operand.
    Dict {
        /// Both `<<` and `>>` included.
        span: Span,
        /// Key and value alternating, flat — this module resolves no dictionary.
        items: Vec<Operand>,
    },
    /// `true`, `false`, `null`, or any other bare word in operand position.
    ///
    /// A keyword only becomes an operand when a later token proves it was not the operator — it
    /// never happens in well-formed content, and refusing it would refuse documents that open.
    Keyword {
        /// The word's bytes.
        span: Span,
        /// The word.
        value: Vec<u8>,
    },
}

impl Operand {
    /// The bytes this operand occupies.
    #[must_use]
    pub const fn span(&self) -> Span {
        match *self {
            Self::Number { span, .. }
            | Self::Str { span }
            | Self::Array { span, .. }
            | Self::Dict { span, .. } => span,
            Self::Name { span, .. } | Self::Keyword { span, .. } => span,
        }
    }

    /// The value if this is a [`Number`](Self::Number), and `None` for every other shape.
    ///
    /// Named rather than pattern-matched at each call site because the geometry pass reads six
    /// numbers off a `cm` and would otherwise say so six times.
    #[must_use]
    pub const fn as_number(&self) -> Option<f64> {
        match *self {
            Self::Number { value, .. } => Some(value),
            _ => None,
        }
    }
}

/// One operator and everything that preceded it since the last one.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Operation {
    /// The operator's own bytes, e.g. `Tj`, `TJ`, `cm`, `Do`.
    pub operator: Vec<u8>,
    /// Its operands, in the order they were written.
    pub operands: Vec<Operand>,
    /// From the first operand's first byte — or the operator's, when there are none — through the
    /// operator's last. Trivia between operands is inside this range; trivia before and after is
    /// not, so replacing the range with nothing leaves the surrounding layout alone.
    pub span: Span,
}

/// The most operands one operator may carry.
///
/// No operator in PDF 32000-1 takes more than six, and `TJ`'s array is one operand however long
/// it is. A stream that pushes more than this is either generated to exhaust memory or is not a
/// content stream; either way the answer is a refusal, not a truncated operation — a rewriter
/// handed an operation missing operands emits the wrong thing.
pub const MAX_OPERANDS: usize = 64;

/// The most operations one stream may hold.
///
/// # This number is an allocation budget, and the first version of it was not
///
/// It read *"the bound is on this module's allocation, not on the input's size"*, at 4,194,304.
/// That sentence was false as written, and the security review of #128 measured how false: a
/// 256 MiB content stream of `/a /a … op` returned **`Ok` at 7.16 GiB of resident memory**, with
/// the operation count nowhere near its cap — because the allocation is in the **operands**, and
/// nothing bounded those in aggregate. The stream deflates 258:1, so the whole thing is a ~1 MB
/// file. On the web that is the tab.
///
/// So the three caps here are chosen together, against a worst case that can be written down:
/// [`MAX_OPERATIONS`] operations each holding a `Vec` and an operator, plus
/// [`MAX_TOTAL_OPERANDS`] operands across the whole stream, each at most
/// [`MAX_OPERAND_BYTES`] of name. That is a few hundred megabytes at the absolute limit and a
/// refusal past it, which is the same shape [`super::names::MAX_NAMES`] has.
///
/// The corpus measures 670 operations in its largest **uncompressed** fixture. That is not a
/// real-world ceiling — the producer fixtures are compressed, so carving them by
/// `stream`/`endstream` yields flate bytes rather than operators — and it is recorded for what
/// it is: evidence the cap is nowhere near what ordinary documents need, not evidence about the
/// tail. A page complex enough to pass it is refused here and would have been refused on time by
/// ADR 0027's ceilings anyway.
pub const MAX_OPERATIONS: usize = 1_048_576;

/// The most operands one stream may hold **in total**, across every operation in it.
///
/// [`MAX_OPERANDS`] bounds one operator's run and bounds nothing about the stream: sixty-four
/// operands on each of a million operations is sixty-four million. This is the cap that was
/// missing, and it is the one the 7.16 GiB measurement went through.
pub const MAX_TOTAL_OPERANDS: usize = 1_048_576;

/// The longest name or bare keyword an operand may carry.
///
/// Mirrors [`super::names::MAX_NAME_LENGTH`] and for the same reason: PDF 32000-1 §7.3.5 sets an
/// implementation limit of 127 bytes, so anything past this is generated rather than written. It
/// is also what stops a single 256 MiB run of name characters being copied into a `Vec` verbatim.
pub const MAX_OPERAND_BYTES: usize = 255;

/// Every operation in `content`, in order.
///
/// # Errors
///
/// - [`Error::Malformed`] — the bytes could not be tokenised, a bracket does not close, or the
///   stream ends with operands and no operator. A partial list may not escape: a rewriter that
///   stopped early would emit a document missing everything after the point it gave up, which is
///   a valid-looking PDF with content deleted.
/// - [`Error::Unsupported`] — [`MAX_OPERANDS`], [`MAX_TOTAL_OPERANDS`], [`MAX_OPERATIONS`] or
///   [`MAX_OPERAND_BYTES`] exceeded, or nesting deeper than the lexer's `MAX_NESTING`. Each is
///   a refusal rather than a truncation.
pub fn operations(content: &[u8]) -> Result<Vec<Operation>> {
    let mut lexer = Lexer::new(content);
    let mut out: Vec<Operation> = Vec::new();
    let mut pending: Vec<Operand> = Vec::new();
    // Across the WHOLE stream, not per operation. See `MAX_TOTAL_OPERANDS`.
    let mut operands_seen = 0usize;

    while let Some(token) = lexer.next_token()? {
        let span = lexer.span();
        match token {
            Token::Keyword(word) => {
                // THE WHOLE ASSOCIATION RULE, and it is positional rather than a table: a bare
                // word ends the operand run and is the operator. `true`, `false` and `null` are
                // bare words too and would be mistaken for operators -- which is why they are
                // handled where they actually occur, inside dictionaries and arrays, by
                // `read_composite` below. At the top level of a content stream they do not
                // appear, and a document where one does gets it treated as the operator it
                // syntactically is, with its bytes carried through untouched.
                if word.len() > MAX_OPERAND_BYTES {
                    return Err(Error::Unsupported(
                        "a content stream holds an operator longer than burrow will read"
                            .to_owned(),
                    ));
                }
                // BEFORE the push, not after. Checked afterwards, the cap admitted one more
                // than it says and had already allocated the whole list to find out.
                if out.len() >= MAX_OPERATIONS {
                    return Err(Error::Unsupported(
                        "a content stream holds more operations than burrow will read".to_owned(),
                    ));
                }
                let start = pending.first().map_or(span.0, |first| first.span().0);
                out.push(Operation {
                    operator: word,
                    operands: std::mem::take(&mut pending),
                    span: (start, span.1),
                });
            }
            Token::ArrayOpen => {
                let operand = read_composite(
                    &mut lexer,
                    content,
                    span.0,
                    Bracket::Array,
                    1,
                    &mut operands_seen,
                )?;
                push_operand(&mut pending, &mut operands_seen, operand)?;
            }
            Token::DictOpen => {
                let operand = read_composite(
                    &mut lexer,
                    content,
                    span.0,
                    Bracket::Dict,
                    1,
                    &mut operands_seen,
                )?;
                push_operand(&mut pending, &mut operands_seen, operand)?;
            }
            Token::ArrayClose | Token::DictClose => {
                return Err(Error::Malformed(
                    "pdf syntax: a ']' or '>>' with nothing open".to_owned(),
                ));
            }
            Token::Brace => {
                // `{` and `}` belong to a type 4 function, not to a content stream. A rewriter
                // that carried on would be rewriting bytes whose grammar it is not reading.
                return Err(Error::Malformed(
                    "pdf syntax: a '{' or '}' in a content stream".to_owned(),
                ));
            }
            Token::Number => {
                push_operand(&mut pending, &mut operands_seen, number(content, span)?)?
            }
            Token::Str => push_operand(&mut pending, &mut operands_seen, Operand::Str { span })?,
            Token::Name(value) => push_operand(
                &mut pending,
                &mut operands_seen,
                Operand::Name { span, value },
            )?,
        }
    }

    if pending.is_empty() {
        Ok(out)
    } else {
        // OPERANDS WITH NO OPERATOR ARE A REFUSAL, not a silent drop. A stream truncated inside
        // its last operation is exactly the shape a rewriter must not treat as complete.
        Err(Error::Malformed(
            "pdf syntax: a content stream that ends with operands and no operator".to_owned(),
        ))
    }
}

/// Which bracket [`read_composite`] is inside.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Bracket {
    Array,
    Dict,
}

/// Read to the matching `]` or `>>`, the opening bracket already consumed.
fn read_composite(
    lexer: &mut Lexer<'_>,
    content: &[u8],
    start: usize,
    bracket: Bracket,
    depth: usize,
    seen: &mut usize,
) -> Result<Operand> {
    if depth > MAX_NESTING {
        return Err(Error::Unsupported(
            "a content stream nests arrays or dictionaries deeper than burrow will read".to_owned(),
        ));
    }
    let mut items: Vec<Operand> = Vec::new();
    while let Some(token) = lexer.next_token()? {
        let span = lexer.span();
        match token {
            Token::ArrayClose if bracket == Bracket::Array => {
                return Ok(Operand::Array {
                    span: (start, span.1),
                    items,
                });
            }
            Token::DictClose if bracket == Bracket::Dict => {
                return Ok(Operand::Dict {
                    span: (start, span.1),
                    items,
                });
            }
            // A `]` inside `<<` or a `>>` inside `[` is a document whose brackets cross. Reading
            // on would mean guessing which one the producer meant.
            Token::ArrayClose | Token::DictClose => {
                return Err(Error::Malformed(
                    "pdf syntax: an array and a dictionary that close in the wrong order"
                        .to_owned(),
                ));
            }
            Token::ArrayOpen => {
                let nested =
                    read_composite(lexer, content, span.0, Bracket::Array, depth + 1, seen)?;
                push_operand(&mut items, seen, nested)?;
            }
            Token::DictOpen => {
                let nested =
                    read_composite(lexer, content, span.0, Bracket::Dict, depth + 1, seen)?;
                push_operand(&mut items, seen, nested)?;
            }
            Token::Brace => {
                return Err(Error::Malformed(
                    "pdf syntax: a '{' or '}' in a content stream".to_owned(),
                ));
            }
            Token::Number => push_operand(&mut items, seen, number(content, span)?)?,
            Token::Str => push_operand(&mut items, seen, Operand::Str { span })?,
            Token::Name(value) => push_operand(&mut items, seen, Operand::Name { span, value })?,
            // Inside brackets a bare word is `true`, `false`, `null` or a `R` reference's `R` --
            // a VALUE, never an operator. This is the one place the positional rule needs the
            // context, and it gets it from the bracket rather than from a list of words.
            Token::Keyword(value) => {
                push_operand(&mut items, seen, Operand::Keyword { span, value })?
            }
        }
    }
    Err(Error::Malformed(
        "pdf syntax: an array or dictionary that is never closed".to_owned(),
    ))
}

/// Push, refusing past [`MAX_OPERANDS`], [`MAX_TOTAL_OPERANDS`] or [`MAX_OPERAND_BYTES`]
/// rather than dropping.
///
/// `seen` is the running total across the whole stream. It is threaded through rather than
/// counted from `into` because `into` is emptied at every operator, so the per-operation cap
/// says nothing about the stream — which is exactly the gap the 7.16 GiB measurement went
/// through.
fn push_operand(into: &mut Vec<Operand>, seen: &mut usize, operand: Operand) -> Result<()> {
    if into.len() >= MAX_OPERANDS {
        return Err(Error::Unsupported(
            "a content stream gives an operator more operands than burrow will read".to_owned(),
        ));
    }
    if *seen >= MAX_TOTAL_OPERANDS {
        return Err(Error::Unsupported(
            "a content stream holds more operands than burrow will read".to_owned(),
        ));
    }
    if let Operand::Name { ref value, .. } | Operand::Keyword { ref value, .. } = operand
        && value.len() > MAX_OPERAND_BYTES
    {
        return Err(Error::Unsupported(
            "a content stream holds a name longer than burrow will record".to_owned(),
        ));
    }
    *seen += 1;
    into.push(operand);
    Ok(())
}

/// Parse the number occupying `span`.
///
/// PDF's numeric grammar is looser than Rust's — `4.` and `.5` are both legal PDF and neither is
/// legal Rust — so the shapes the specification allows and `f64::from_str` does not are
/// normalised here. Anything left over is a refusal: a number a rewriter reads wrongly is a glyph
/// placed in the wrong place, and for redaction that is the whole failure.
///
/// # A number with more than one sign is refused, and that was measured rather than argued
///
/// `--5` and `+-5` do occur, and there is no agreed answer for them. This code first took the
/// **parity** of the minus signs (`--5` → `+5`); code review read the comment beside it, which
/// claimed readers "take the last one" (`--5` → `-5`), and the two had quietly disagreed since
/// the function was written.
///
/// Rather than pick, the question was put to **PDFium — the renderer burrow itself ships**. Three
/// one-page documents, identical but for `60`, `-60` and `--60` as a `Td` operand, rendered
/// through `burrow_ops::render`; the glyph's ink columns came back at 231, 111 and **171**, and
/// 171 is where it sits with *no translation at all*. `+-60` lands there too. So PDFium treats a
/// multi-sign number as neither answer: it reads it as invalid.
///
/// Every value this function could return therefore disagrees with the renderer the verification
/// step reads back through. A `Td` operand read with the wrong sign is a glyph placed somewhere a
/// redaction will not look, so the honest output is no value at all.
fn number(content: &[u8], span: Span) -> Result<Operand> {
    let raw = content.get(span.0..span.1).ok_or_else(|| {
        Error::Internal("pdf syntax: a number's span left its content stream".to_owned())
    })?;
    let text = std::str::from_utf8(raw).map_err(|_| {
        Error::Malformed("pdf syntax: a number holding bytes that are not ASCII".to_owned())
    })?;

    let unsigned = text.trim_start_matches(['+', '-']);
    let signs = text.len().saturating_sub(unsigned.len());
    if signs > 1 {
        return Err(Error::Malformed(
            "pdf syntax: an operand with more than one sign, which burrow's own renderer reads \
             as invalid and which has no value that agrees with it"
                .to_owned(),
        ));
    }
    let negative = text.starts_with('-');
    // `4.` and `.5` are both legal PDF and neither is legal Rust. The padding goes only where
    // a digit is MISSING: an unconditional trailing `0` multiplied every integer by ten, which
    // the `cm` test caught as a page translated 72 points into 720.
    let mut padded = String::with_capacity(unsigned.len() + 2);
    if unsigned.starts_with('.') {
        padded.push('0');
    }
    padded.push_str(unsigned);
    if padded.ends_with('.') {
        padded.push('0');
    }
    // NO INTERPOLATION. `core/CLAUDE.md`: "Never log or embed file content in an error". The
    // span is restricted to `+-.0123456789`, so at most it is a coordinate rather than text --
    // and the rule does not have an exemption for content that looks harmless.
    let magnitude: f64 = padded.parse().map_err(|_| {
        Error::Malformed("pdf syntax: an operand that is not a number burrow can read".to_owned())
    })?;
    if !magnitude.is_finite() {
        return Err(Error::Malformed(
            "pdf syntax: a number too large to represent".to_owned(),
        ));
    }
    Ok(Operand::Number {
        span,
        value: if negative { -magnitude } else { magnitude },
    })
}

#[cfg(test)]
mod tests {
    use super::{MAX_OPERANDS, Operand, Operation, operations};

    fn ops(content: &[u8]) -> Vec<Operation> {
        operations(content).expect("reads")
    }

    fn operators(content: &[u8]) -> Vec<String> {
        ops(content)
            .into_iter()
            .map(|o| String::from_utf8_lossy(&o.operator).into_owned())
            .collect()
    }

    #[test]
    fn operands_belong_to_the_operator_that_follows_them() {
        let read = ops(b"1 0 0 1 72 720 cm /F1 12 Tf");
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].operator, b"cm");
        assert_eq!(
            read[0]
                .operands
                .iter()
                .filter_map(Operand::as_number)
                .collect::<Vec<_>>(),
            [1.0, 0.0, 0.0, 1.0, 72.0, 720.0]
        );
        assert_eq!(read[1].operator, b"Tf");
        assert_eq!(read[1].operands.len(), 2);
    }

    #[test]
    fn an_operations_span_covers_its_operands_and_nothing_around_them() {
        // The property the rewriter needs: replacing the span with nothing must leave the
        // surrounding white space alone, so the bytes either side stay where they were.
        let content = b"q  1 0 0 1 72 720 cm  Q";
        let read = ops(content);
        let (start, end) = read[1].span;
        assert_eq!(&content[start..end], b"1 0 0 1 72 720 cm");
        let mut edited = content.to_vec();
        edited.splice(start..end, std::iter::empty());
        assert_eq!(&edited, b"q    Q");
    }

    #[test]
    fn an_operator_with_no_operands_spans_only_itself() {
        let content = b"BT ET";
        let read = ops(content);
        assert_eq!(read[0].span, (0, 2));
        assert_eq!(read[1].span, (3, 5));
    }

    #[test]
    fn tj_arrays_are_one_operand_with_their_elements_intact() {
        // `TJ` is where redaction has to reach INSIDE an operand: the secret may be one element
        // of the array with kerning numbers either side of it. A flattened token stream cannot
        // say which brackets an element was in.
        let read = ops(b"[(Hel) -120 (lo)] TJ");
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].operator, b"TJ");
        let Some(Operand::Array { items, .. }) = read[0].operands.first() else {
            panic!("the array should be one operand: {:?}", read[0].operands);
        };
        assert_eq!(items.len(), 3);
        assert!(matches!(items[0], Operand::Str { .. }));
        assert_eq!(items[1].as_number(), Some(-120.0));
    }

    #[test]
    fn a_marked_content_dictionary_is_one_operand() {
        let read = ops(b"/Span << /ActualText (secret) >> BDC");
        assert_eq!(read[0].operator, b"BDC");
        assert_eq!(read[0].operands.len(), 2);
        assert!(matches!(read[0].operands[1], Operand::Dict { .. }));
    }

    #[test]
    fn true_false_and_null_inside_brackets_are_values_rather_than_operators() {
        // The positional rule needs the bracket context here and nowhere else. Without it the
        // `true` would end the operand run and `[` would never close.
        let read = ops(b"<< /IM true /D [0 1] >> BDC");
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].operator, b"BDC");
    }

    #[test]
    fn pdfs_numeric_grammar_is_looser_than_rusts_and_all_of_it_reads() {
        let read = ops(b"4. .5 -.002 +3 0 -0 cm");
        let values: Vec<f64> = read[0]
            .operands
            .iter()
            .filter_map(Operand::as_number)
            .collect();
        assert_eq!(values, [4.0, 0.5, -0.002, 3.0, 0.0, 0.0]);
    }

    #[test]
    fn a_number_with_more_than_one_sign_is_refused_because_no_value_agrees_with_the_renderer() {
        // Measured, not argued: PDFium places `--60 0 Td` where NO translation would put it,
        // so neither the parity reading (+60) nor the last-sign reading (-60) matches the
        // renderer ADR 0022 reads back through. `number`'s rustdoc has the ink columns.
        for content in [&b"--5 0 Td"[..], b"+-5 0 Td", b"-+5 0 Td", b"1-2 cm"] {
            assert!(
                operations(content).is_err(),
                "a multi-sign operand must be refused: {}",
                String::from_utf8_lossy(content)
            );
        }
        // A single sign, either way, is ordinary and still reads.
        assert_eq!(ops(b"-5 +5 cm")[0].operands.len(), 2);
    }

    #[test]
    fn an_inline_image_arrives_as_bi_then_id_with_the_data_inside_ids_span() {
        // The lexer skips the binary itself, so `ID`'s span runs through `EI`. Every byte is
        // accounted for by exactly one operation, which is what lets a rewriter rebuild the
        // stream from spans without losing an image.
        let content = b"q BI /W 14 /H 1 /CS /G ID \x00(/F9 <</a 1>> EI Q";
        let read = ops(content);
        assert_eq!(operators(content), ["q", "BI", "ID", "Q"]);
        let (start, end) = read[2].span;
        assert!(
            content[start..end].ends_with(b"EI"),
            "ID's span should reach past the image data"
        );
        // And the operations tile the stream: no byte between the first and last is in two.
        let mut previous_end = 0usize;
        for operation in &read {
            assert!(operation.span.0 >= previous_end, "spans overlap: {read:?}");
            previous_end = operation.span.1;
        }
    }

    #[test]
    fn an_unknown_operator_is_carried_through_rather_than_refused() {
        // The inversion this module's header is about: it does NOT hold a table of operators, so
        // an extension nobody listed keeps its operands and its bytes.
        let read = ops(b"1 2 /Weird ThisIsNotAnOperator");
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].operator, b"ThisIsNotAnOperator");
        assert_eq!(read[0].operands.len(), 3);
    }

    #[test]
    fn a_stream_ending_in_operands_with_no_operator_is_refused() {
        // A truncated last operation. Emitting the operations before it would produce a document
        // with content silently deleted, which is the failure this whole module exists under.
        assert!(operations(b"BT /F1 12").is_err());
    }

    #[test]
    fn brackets_that_never_close_or_close_in_the_wrong_order_are_refused() {
        assert!(operations(b"[(a) 1 TJ").is_err());
        assert!(operations(b"<< /a [1 >> ] BDC").is_err());
        assert!(operations(b"] TJ").is_err());
    }

    #[test]
    fn the_aggregate_operand_cap_refuses_what_the_per_operator_one_cannot_see() {
        // THE 7.16 GiB MEASUREMENT, as a test. Sixty-four operands on each of many operations
        // passes `MAX_OPERANDS` every time and allocates without bound; the operation count
        // never gets near its own cap, because the operands do the allocating.
        let mut content = Vec::new();
        let mut operands = 0usize;
        while operands <= super::MAX_TOTAL_OPERANDS {
            content.extend_from_slice(&b"/a ".repeat(64));
            content.extend_from_slice(b"op ");
            operands += 64;
        }
        // The result is NOT interpolated into the message: an `Ok` here is a million operations
        // and `{:?}` on it wrote 47 MB to the test log the first time this ran.
        assert!(
            matches!(
                operations(&content),
                Err(burrow_types::Error::Unsupported(_))
            ),
            "the aggregate operand cap must refuse {operands} operands"
        );
    }

    #[test]
    fn a_name_longer_than_the_cap_is_refused_rather_than_copied() {
        let mut content = b"/".to_vec();
        content.extend(std::iter::repeat_n(b'a', super::MAX_OPERAND_BYTES + 1));
        content.extend_from_slice(b" Do");
        assert!(operations(&content).is_err());
        // And the boundary is accepted, so the refusal is a ceiling rather than a wall one byte
        // lower than it says.
        let mut content = b"/".to_vec();
        content.extend(std::iter::repeat_n(b'a', super::MAX_OPERAND_BYTES));
        content.extend_from_slice(b" Do");
        assert_eq!(ops(&content).len(), 1);
    }

    #[test]
    fn the_operation_cap_admits_exactly_what_it_says() {
        // Checked BEFORE the push. Checked after, it admitted MAX_OPERATIONS + 1 and had
        // allocated the whole list to discover that.
        let content = b"q ".repeat(super::MAX_OPERATIONS);
        assert_eq!(ops(&content).len(), super::MAX_OPERATIONS);
        let content = b"q ".repeat(super::MAX_OPERATIONS + 1);
        assert!(operations(&content).is_err());
    }

    #[test]
    fn too_many_operands_is_a_refusal_rather_than_a_truncated_operation() {
        let mut content = Vec::new();
        for _ in 0..=MAX_OPERANDS {
            content.extend_from_slice(b"0 ");
        }
        content.extend_from_slice(b"cm");
        assert!(operations(&content).is_err());
    }

    #[test]
    fn the_empty_stream_holds_no_operations_and_is_not_an_error() {
        assert!(ops(b"").is_empty());
        assert!(ops(b"  % just a comment\n").is_empty());
    }
}

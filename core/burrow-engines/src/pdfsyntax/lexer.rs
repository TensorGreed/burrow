//! A bounded tokeniser for PDF syntax.
//!
//! It answers two questions and deliberately no others: *which names does this byte sequence
//! mention* ([`super::names`]) and *what are this dictionary's top-level keys* ([`super::dict`]).
//! It builds no object graph, resolves no reference, decompresses nothing and allocates nothing
//! in proportion to its input beyond the caller's own collection.
//!
//! # Every surprise is a refusal, never a shorter answer
//!
//! This matters more here than the usual "fail closed", and in the opposite direction from most
//! of this crate. The name set feeds a *filter*: a resource whose name is in the set is kept and
//! one whose name is not is removed. So a token this lexer mishandles by **dropping** a name
//! removes a resource the page actually draws with, and the page renders wrong — a correct-looking
//! document with a missing font. Mishandling one by **inventing** a name only keeps a resource
//! that was going to be kept anyway.
//!
//! The two directions are therefore not symmetric, and the lexer is written to be wrong in the
//! harmless one. Anything it cannot account for — an unterminated string, a run of bytes it cannot
//! classify, an inline image with no `EI` — returns [`Error::Malformed`] rather than the names it
//! managed to collect before giving up. A partial name set is the one output that must never
//! escape this module.
//!
//! # Inline images are the reason this is a lexer and not a byte scan
//!
//! `BI … ID <arbitrary binary> EI` puts uninterpreted bytes in the middle of a content stream.
//! Those bytes routinely contain `(`, and a scan that did not know about `ID` would read the rest
//! of the image as a literal string and swallow every name after it — an under-approximation, the
//! direction that breaks pages. So `ID` is handled explicitly.
//!
//! **Where the image ends comes from its dictionary, and an image whose dictionary will not say
//! is refused.** Scanning for an `EI` that stands alone is what every reader falls back on and it
//! is a hiding place: an image declaring one byte of data, followed by an `EI` not preceded by
//! white space, lets the scan run past a whole text object that PDFium draws. See
//! [`Lexer::skip_inline_image_data`] for the measurement and for what the refusal costs.

use burrow_types::{Error, Result};

/// The deepest `<<` / `[` nesting this will follow.
///
/// qpdf's own `parser_max_nesting` is set to 64 in `crate::qpdf::limits`, so a document that
/// nests deeper than this has already been refused by the engine that produced the bytes being
/// lexed. Matching the number rather than choosing a new one keeps the two from disagreeing.
pub(super) const MAX_NESTING: usize = 64;

/// One PDF token, with the parts a caller here actually needs.
///
/// Strings carry no value: nothing this module answers depends on what a string says, and
/// carrying the bytes would mean copying file content for no reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Token {
    /// A name, with `#xx` escapes already decoded and the leading `/` removed.
    Name(Vec<u8>),
    /// `<<`
    DictOpen,
    /// `>>`
    DictClose,
    /// `[`
    ArrayOpen,
    /// `]`
    ArrayClose,
    /// `{` or `}`, which appear in PostScript calculator functions.
    Brace,
    /// A literal `(…)` or hex `<…>` string. The value is not carried.
    Str,
    /// A number, integer or real.
    Number,
    /// A bare keyword: `obj`, `R`, `true`, `BT`, an operator.
    Keyword(Vec<u8>),
}

/// Whether `byte` ends a token.
const fn is_delimiter(byte: u8) -> bool {
    matches!(
        byte,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

/// Whether `byte` ends a word without being a delimiter: PDF's six white-space characters, and the
/// two PDFium adds.
///
/// NUL is white space in PDF, which is easy to miss and is why this is a named function with a
/// test rather than a `matches!` at three call sites.
///
/// **0x80 and 0xFF are white space to PDFium**, which the specification does not say. Measured by
/// #228's third security review, every byte value placed between two tokens and rendered: these two
/// end a word as white space does, and are skipped as the one byte after `ID`. Without them,
/// `(SECRET) Tj\xff` was an unknown operator `Tj\xff` here and drew text there -- redaction `Ok` over
/// 1,842 dark pixels -- and `/Fm1\xff Do` named no form, so `split` never walked into a hidden
/// layer it then carried through. Vertical tab and 0xA0 do NOT end a word to PDFium, and are not
/// here.
const fn is_whitespace(byte: u8) -> bool {
    matches!(
        byte,
        b'\0' | b'\t' | b'\n' | 0x0c | b'\r' | b' ' | 0x80 | 0xff
    )
}

/// Whether `byte` can appear inside a number.
const fn is_numeric(byte: u8) -> bool {
    byte.is_ascii_digit() || matches!(byte, b'+' | b'-' | b'.')
}

/// A cursor over PDF syntax.
///
/// `Clone` is one `usize` and a shared slice, and it is what `dict::skip_one_object` looks ahead
/// with: deciding whether `5` begins a plain number or an `N G R` reference needs two tokens of
/// look-ahead and no way to put them back.
#[derive(Clone)]
pub(super) struct Lexer<'a> {
    bytes: &'a [u8],
    at: usize,
    /// Where the token most recently returned by [`next_token`](Lexer::next_token) began.
    ///
    /// **Added for the rewriter (#128).** Reading a content stream needs no offsets; REWRITING
    /// one needs to know which bytes to replace, and a caller that tried to track this itself
    /// would be re-implementing `skip_trivia`.
    ///
    /// It is also how a string's VALUE is reached without this module carrying one. `Token::Str`
    /// stays a marker — the header's reason for that is still good, and a `Vec` per string on
    /// every resource scan is a cost `names.rs` should not pay — and a caller that wants the
    /// bytes slices them with [`span`](Lexer::span) and decodes with
    /// [`decode_string`](super::decode_string).
    token_start: usize,
    /// Where the most recent `BI` keyword began, if one is open.
    ///
    /// An inline image's extent is a function of its own dictionary — `/L`, or `/W`, `/H`,
    /// `/BPC` and `/CS` — and by the time `ID` arrives those tokens have been returned and
    /// forgotten. Rather than make the lexer accumulate them, this records where the dictionary
    /// STARTS, and [`skip_inline_image_data`](Lexer::skip_inline_image_data) re-lexes that short
    /// range when it needs the numbers. The cost is paid once per inline image and is
    /// proportional to the dictionary, not to the data.
    ///
    /// **Added after the security review of #128**, which measured PDFium — the renderer burrow
    /// ships — drawing text that this lexer had swallowed as image data. See that method.
    bi_at: Option<usize>,
    /// What this lexer does with an inline image whose extent it cannot derive (#228).
    images: InlineImages,
    /// Set when [`InlineImages::Prune`] met such an image; the lexer then stops.
    extent_unknown: bool,
}

/// What a lexer does with an inline image whose extent it cannot derive -- a filtered one, which
/// PDFium ends at its filter's own end-of-data and this module cannot decode (#228).
///
/// **Named by every caller, with no default** (owner, 2026-10-03): a default is a caller that
/// never decided. Both callers end in a refusal today, written in two places for two reasons.
///
/// **Redaction's refusal travels in the variant**, and callers name it as [`Self::REDACTION`].
/// A plain `Redaction` variant kept the refusal's text in the base web module, which reaches this
/// lexer through `split` and never redacts: the mode is a runtime value, so the compiler could not
/// drop the arm. Carried as a function that only redaction's callers name, it is unreferenced
/// wherever they are absent (#228).
#[derive(Clone, Copy, Debug)]
pub(crate) enum InlineImages {
    /// Refuse it, with the error the function returns: redaction cannot say what the page draws
    /// past an image it cannot end, and a guess would be a page reported clean that is not.
    Redaction(fn() -> Error),
    /// Stop lexing and say so: `split`'s pruning then refuses the page, since what it uses past the
    /// image can be neither pruned by nor carried whole (ADR 0019's #228 amendment).
    Prune,
}

impl InlineImages {
    /// Redaction's mode: refuse a filtered inline image, `[inline-image-filtered]`.
    pub(crate) const REDACTION: Self = Self::Redaction(inline_image_filtered);
}

/// Redaction's refusal of a filtered inline image. Reached only through
/// [`InlineImages::REDACTION`].
fn inline_image_filtered() -> Error {
    Error::Malformed(
        "pdf syntax [inline-image-filtered]: an inline image with a /F filter, which the renderer \
         ends at its filter's own end of data rather than at any /L, and which burrow cannot \
         decode to find"
            .to_owned(),
    )
}

impl<'a> Lexer<'a> {
    /// Start at the beginning of `bytes`, answering an underivable inline image as `images` says.
    pub(super) const fn new(bytes: &'a [u8], images: InlineImages) -> Self {
        Self {
            bytes,
            at: 0,
            token_start: 0,
            bi_at: None,
            images,
            extent_unknown: false,
        }
    }

    /// Whether lexing stopped at an inline image whose extent could not be derived, under
    /// [`InlineImages::Prune`]. What came before it was read; nothing after it was.
    pub(super) const fn extent_unknown(&self) -> bool {
        self.extent_unknown
    }

    /// The byte range the last token occupied, as `(start, end)`.
    ///
    /// Empty before the first [`next_token`](Lexer::next_token). A call that returns `None`
    /// **does** move it: the start is set after trivia is skipped and before the end of input
    /// is noticed, so it lands on `(len, len)`. Nothing depends on that today and the sentence
    /// here used to claim the opposite, which is the kind of wrong that is discovered by
    /// someone relying on it.
    pub(super) const fn span(&self) -> (usize, usize) {
        (self.token_start, self.at)
    }

    /// The byte at `self.at`, without advancing.
    ///
    /// `get` rather than an index: `indexing_slicing` is denied in this crate, and a bounds
    /// check on every byte of a content stream is not what costs anything here.
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    /// Advance past white space and comments.
    ///
    /// A comment runs to the next end-of-line. `%` inside a string is not a comment, which is
    /// why this is only ever called between tokens.
    fn skip_trivia(&mut self) {
        while let Some(byte) = self.peek() {
            if is_whitespace(byte) {
                self.at += 1;
            } else if byte == b'%' {
                while let Some(byte) = self.peek() {
                    if byte == b'\n' || byte == b'\r' {
                        break;
                    }
                    self.at += 1;
                }
            } else {
                break;
            }
        }
    }

    /// The next token, or `None` at the end of the input.
    ///
    /// # Errors
    ///
    /// [`Error::Malformed`] for anything this cannot account for. See the module header: a
    /// partial answer is the one result that may not escape.
    pub(super) fn next_token(&mut self) -> Result<Option<Token>> {
        self.skip_trivia();
        self.token_start = self.at;
        let Some(byte) = self.peek() else {
            return Ok(None);
        };
        match byte {
            b'/' => Ok(Some(Token::Name(self.read_name()?))),
            b'(' => {
                self.read_literal_string()?;
                Ok(Some(Token::Str))
            }
            b'<' => {
                if self.bytes.get(self.at + 1) == Some(&b'<') {
                    self.at += 2;
                    Ok(Some(Token::DictOpen))
                } else {
                    self.read_hex_string()?;
                    Ok(Some(Token::Str))
                }
            }
            b'>' => {
                if self.bytes.get(self.at + 1) == Some(&b'>') {
                    self.at += 2;
                    Ok(Some(Token::DictClose))
                } else {
                    // A lone `>` is not a token in any production. Refusing is the point.
                    Err(Error::Malformed(
                        "pdf syntax: a '>' that does not close a dictionary or a string".to_owned(),
                    ))
                }
            }
            b'[' => {
                self.at += 1;
                Ok(Some(Token::ArrayOpen))
            }
            b']' => {
                self.at += 1;
                Ok(Some(Token::ArrayClose))
            }
            b'{' | b'}' => {
                self.at += 1;
                Ok(Some(Token::Brace))
            }
            b')' => Err(Error::Malformed(
                "pdf syntax: a ')' with no string to close".to_owned(),
            )),
            _ if is_numeric(byte) => {
                let start = self.at;
                self.read_run(is_numeric);
                // A NUMBER ENDS WHERE A WORD ENDS, as PDFium reads it: `8ID` is one unknown
                // keyword to the renderer, not `8` and then `ID`, and `5BI` is not `BI`. Read
                // as two tokens, either one started an inline image burrow skipped while
                // PDFium drew its bytes -- measured by #228's second security review.
                if self
                    .peek()
                    .is_some_and(|next| !is_whitespace(next) && !is_delimiter(next))
                {
                    self.read_run(|b| !is_whitespace(b) && !is_delimiter(b));
                    let word = self.bytes.get(start..self.at).ok_or_else(|| {
                        Error::Internal("pdf syntax: a run left its input".to_owned())
                    })?;
                    return Ok(Some(Token::Keyword(word.to_vec())));
                }
                Ok(Some(Token::Number))
            }
            _ => {
                let start = self.at;
                self.read_run(|b| !is_whitespace(b) && !is_delimiter(b));
                let Some(word) = self.bytes.get(start..self.at) else {
                    return Err(Error::Internal(
                        "pdf syntax: a run left its input".to_owned(),
                    ));
                };
                if word == b"BI" {
                    // ONE IMAGE AT A TIME. A second `BI` used to restart the dictionary, so
                    // everything before it went unread here while PDFium read all of it --
                    // `/W 8 /D BI /Width 96` was 8 bytes to PDFium and 96 to burrow, past the
                    // repeated-key and filtered-image refusals both (#228, second review).
                    if self.bi_at.is_some() {
                        return Err(Error::Malformed(
                            "pdf syntax [inline-image-nested-bi]: a 'BI' inside an inline image's \
                             dictionary"
                                .to_owned(),
                        ));
                    }
                    self.bi_at = Some(start);
                }
                if word == b"ID" {
                    // THE INLINE IMAGE IS SKIPPED HERE, not by the caller. It began as a
                    // method callers were expected to call after seeing `ID`, and that is a
                    // discipline: `names_in_content` remembered and `top_level_keys` did not,
                    // which is one lexer with two behaviours depending on who is holding it.
                    // A caller cannot forget something it never has to do.
                    let dictionary = self.bi_at.take().and_then(|bi| {
                        // From just past `BI` to just before `ID`.
                        self.bytes.get(bi.saturating_add(2)..start)
                    });
                    self.skip_inline_image_data(dictionary)?;
                }
                if word.is_empty() {
                    // Unreachable given the match arms above, and a refusal rather than an
                    // infinite loop if it ever became reachable: a zero-length token would
                    // leave `self.at` where it was and this function would never terminate.
                    return Err(Error::Malformed(
                        "pdf syntax: a byte that begins no token".to_owned(),
                    ));
                }
                Ok(Some(Token::Keyword(word.to_vec())))
            }
        }
    }

    /// Advance while `keep` holds.
    fn read_run(&mut self, keep: impl Fn(u8) -> bool) {
        while let Some(byte) = self.peek() {
            if keep(byte) {
                self.at += 1;
            } else {
                break;
            }
        }
    }

    /// Read `/Name`, decoding `#xx`.
    ///
    /// A `#` not followed by two hex digits is a refusal rather than a literal `#`. Producers do
    /// not emit it, and guessing would mean two readers of the same document could disagree about
    /// what a resource is called — which, for a filter keyed on the name, is the under-keep that
    /// breaks a page.
    fn read_name(&mut self) -> Result<Vec<u8>> {
        // The `/` itself.
        self.at += 1;
        let mut name = Vec::new();
        while let Some(byte) = self.peek() {
            if is_whitespace(byte) || is_delimiter(byte) {
                break;
            }
            if byte == b'#' {
                let hex = self
                    .bytes
                    .get(self.at + 1..self.at + 3)
                    .ok_or_else(|| Error::Malformed("pdf syntax: a truncated #xx".to_owned()))?;
                let decoded = hex_value(hex).ok_or_else(|| {
                    Error::Malformed("pdf syntax: a '#' not followed by two hex digits".to_owned())
                })?;
                name.push(decoded);
                self.at += 3;
            } else {
                name.push(byte);
                self.at += 1;
            }
        }
        Ok(name)
    }

    /// Skip a `(…)` string, honouring `\` escapes and balanced inner parentheses.
    fn read_literal_string(&mut self) -> Result<()> {
        // The `(` itself.
        self.at += 1;
        let mut depth = 1usize;
        while let Some(byte) = self.peek() {
            self.at += 1;
            match byte {
                b'\\' => {
                    // Whatever follows is consumed uninterpreted, which is all that is needed
                    // to find the end: `\)` must not close the string and `\\` must not escape
                    // the next byte.
                    if self.peek().is_some() {
                        self.at += 1;
                    }
                }
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                _ => {}
            }
        }
        Err(Error::Malformed(
            "pdf syntax: a '(' string that is never closed".to_owned(),
        ))
    }

    /// Skip a `<…>` hex string.
    fn read_hex_string(&mut self) -> Result<()> {
        // The `<` itself.
        self.at += 1;
        while let Some(byte) = self.peek() {
            self.at += 1;
            if byte == b'>' {
                return Ok(());
            }
        }
        Err(Error::Malformed(
            "pdf syntax: a '<' string that is never closed".to_owned(),
        ))
    }

    /// Skip an inline image's binary data, from just after `ID` to just past `EI`.
    ///
    /// **The one place this lexer reads bytes it does not tokenise**, and the reason it has to:
    /// the data between `ID` and `EI` is arbitrary and routinely contains `(`, `<` and `/`. A
    /// tokeniser that walked into it would read the rest of the image as a string and drop every
    /// name after it.
    ///
    /// # The extent comes from the dictionary, or the stream is refused
    ///
    /// The first version of this recognised `EI` only where it stood alone — preceded by white
    /// space, followed by white space, a delimiter or the end of the stream. That is the
    /// heuristic every PDF reader falls back on, and **as a rule for deciding what a page draws
    /// it is a hiding place.** The security review of #128 measured it:
    ///
    /// ```text
    /// q BI /W 1 /H 1 /BPC 8 /CS /G ID AEI
    /// BT /F1 24 Tf 1 0 0 1 72 700 Tm (BURROW-SECRET) Tj ET
    ///  EI Q
    /// ```
    ///
    /// The dictionary declares **one** byte of data, so the image ends at the `EI` right after
    /// `A` — but that `EI` is preceded by `A` rather than by white space, so the scan ran on to
    /// the trailing ` EI` and swallowed the text object whole. **PDFium draws `BURROW-SECRET`
    /// from that page**, and PDFium is the renderer burrow ships and the one ADR 0022's read-back
    /// reads through. The tokeniser reported a page with no text operator on it. A redactor built
    /// on this would report the page clean.
    ///
    /// So the length is computed from `/W`, `/H`, `/BPC` and `/CS` by the renderer's own rule, and
    /// an `EI` is **required** there. Where that rule cannot be followed here, or differs from the
    /// specification's — a colour space from the page's `/Resources`, a missing `/W` or `/H`, the
    /// shapes `inline_image_length` lists — **the stream is refused.**
    ///
    /// # Why refusing, rather than falling back
    ///
    /// The fall-back was kept at first and recorded as a known residue. A recorded residue is
    /// still a leak: for those images the page above works exactly as it did, and the whole
    /// point of the dictionary rule is that this tokeniser must not report a page as carrying
    /// less than it draws. Refusing turns an unreadable extent into a refusal, which is an
    /// outcome burrow already has a vocabulary for and which cannot be mistaken for "clean".
    ///
    /// **The extent is computed, never taken from `/L`** (#228): PDFium does not read `/L`, and an
    /// `/L` it does not read was a hiding place of its own -- see `inline_image_length`. A
    /// **filtered** image's extent is its filter's own end of data, which this module cannot
    /// decode to find, so what happens to it is the caller's [`InlineImages`]: redaction refuses
    /// it here, and `split`'s pruning stops the read and refuses the page once it knows which page
    /// that is. ADR 0029's and ADR 0019's #228 amendments say why. #142 tracks deriving the
    /// extents that are currently out of reach.
    ///
    /// An `ID` with no `BI` before it is refused for the same reason: it has no dictionary, so
    /// there is nothing to derive an extent from.
    fn skip_inline_image_data(&mut self, dictionary: Option<&[u8]>) -> Result<()> {
        let Some(dictionary) = dictionary else {
            return Err(Error::Malformed(
                "pdf syntax: an 'ID' with no 'BI' before it, so the image has no extent".to_owned(),
            ));
        };
        // One byte of white space after `ID` belongs to the operator, not the data.
        if self.peek().is_some_and(is_whitespace) {
            self.at += 1;
        }

        let length = match inline_image_length(dictionary)? {
            Extent::Bytes(length) => length,
            Extent::Filtered => match self.images {
                InlineImages::Redaction(refusal) => return Err(refusal()),
                InlineImages::Prune => {
                    // NOTHING AFTER IT CAN BE READ, so nothing is: the caller is told, and
                    // decides what an unread remainder means for it.
                    self.extent_unknown = true;
                    self.at = self.bytes.len();
                    return Ok(());
                }
            },
        };
        let data_end = self.at.checked_add(length).ok_or_else(|| {
            Error::Malformed("pdf syntax: an inline image longer than its stream".to_owned())
        })?;
        // The specification allows white space between the data and `EI`, and producers emit it.
        // Anything else there means the dictionary and the data disagree about how long the
        // image is, and two conforming readers would end it in two places.
        let mut at = data_end;
        while self.bytes.get(at).copied().is_some_and(is_whitespace) {
            at += 1;
        }
        // `EI` MUST STAND AS A WORD, as PDFium requires: `EI5` is not the end of the image to the
        // renderer, which scanned on into the next image's data for one that was (#228, second
        // review).
        let ends_a_word = self
            .bytes
            .get(at.saturating_add(2))
            .is_none_or(|next| is_whitespace(*next) || is_delimiter(*next));
        if self.bytes.get(at..at.saturating_add(2)) == Some(b"EI") && ends_a_word {
            self.at = at + 2;
            return Ok(());
        }
        Err(Error::Malformed(
            "pdf syntax: an inline image whose dictionary says one length and whose data ends \
             somewhere else"
                .to_owned(),
        ))
    }
}

/// Read past a composite value in an inline image's dictionary: a flat array, or a dictionary of
/// name keys each with one plain value -- a number, a string, a name, or `true`/`false`/`null` (#228).
///
/// **Nothing nested.** Bracket-balancing was the first version, and PDFium does not read these
/// that way: `/D [[1 0]]`, `/D [1 [0]]`, `/DP << /A 1 2 >>` and `/DP << (a) 1 >>` each made it
/// abandon the image and draw its data as content, while burrow skipped the data -- redaction
/// `Ok` over 1,842 dark pixels, measured by #228's second security review. Each refuses
/// `[inline-image-nested-value]`. A flat `/D [1 0]` and a flat `/DP << /K -1 >>` read on.
fn skip_composite(lexer: &mut Lexer<'_>, array: bool) -> Result<()> {
    let refusal = || {
        Error::Malformed(
            "pdf syntax [inline-image-nested-value]: an inline image dictionary with a value \
             the renderer does not read as one value"
                .to_owned(),
        )
    };
    let mut expect_key = true;
    while let Some(token) = lexer.next_token()? {
        match token {
            Token::ArrayClose if array => return Ok(()),
            Token::DictClose if !array && expect_key => return Ok(()),
            Token::ArrayOpen
            | Token::DictOpen
            | Token::ArrayClose
            | Token::DictClose
            | Token::Brace => return Err(refusal()),
            Token::Name(_) if !array && expect_key => expect_key = false,
            _ if !array && expect_key => return Err(refusal()),
            // A KEYWORD AS A DICTIONARY VALUE is `true`, `false` or `null`. Any other word made
            // PDFium give up on the value and end the image's dictionary at `>>`, ahead of keys
            // it then never read -- `/DP << /K foo >> /W 96` was `Ok` over 1,842 dark pixels
            // (#228, third reviews). In an ARRAY PDFium skips the word, measured, so arrays keep it.
            Token::Keyword(ref word)
                if !array && !matches!(word.as_slice(), b"true" | b"false" | b"null") =>
            {
                return Err(refusal());
            }
            _ => expect_key = true,
        }
    }
    Err(Error::Malformed(if array {
        "pdf syntax: an inline image dictionary with an array that never closes".to_owned()
    } else {
        "pdf syntax: an inline image dictionary with a dictionary that never closes".to_owned()
    }))
}

/// An inline image's extent, as its dictionary gives it.
enum Extent {
    /// Unfiltered: this many bytes, computed from the dictionary as PDFium computes it.
    Bytes(usize),
    /// Filtered: whatever the filter's own end of data says, which this module cannot find.
    Filtered,
}

/// The keys an inline image dictionary may carry, abbreviated (PDF 32000-1 Table 92) and in full.
/// Anything else refuses: a key nobody enumerated is one whose meaning to the renderer is unknown.
const IMAGE_KEYS: [[&[u8]; 2]; 11] = [
    [b"W", b"Width"],
    [b"H", b"Height"],
    [b"BPC", b"BitsPerComponent"],
    [b"CS", b"ColorSpace"],
    [b"F", b"Filter"],
    [b"D", b"Decode"],
    [b"DP", b"DecodeParms"],
    [b"IM", b"ImageMask"],
    [b"I", b"Interpolate"],
    [b"L", b"Length"],
    // `/Intent` has no abbreviation (PDF 32000-1 Table 92), so both "spellings" are the one.
    [b"Intent", b"Intent"],
];

/// How many bytes of data an inline image occupies, as PDFium ends it (#228).
///
/// `dictionary` is the bytes between `BI` and `ID`.
///
/// # PDFium's rule, not the specification's
///
/// PDFium never reads `/L`. It ends a filtered image at the filter's own end of data -- `>` for
/// `/AHx`, `~>` for `/A85`, the zlib stream's end for `/Fl`, the end-of-data code for `/RL` and
/// `/LZW` -- and an unfiltered one after the bytes it computes. That computation is **not** PDF
/// 32000-1's, measured by both #228 reviewers by placing an `EI` at every offset:
///
/// - with **no `/CS`**, one component of one bit, whatever `/BPC` and `/IM` say;
/// - with a **device `/CS`** (`/G`, `/RGB`, `/CMYK` or their `Device` spellings), that space's
///   component count, and a missing `/BPC` reads as **0**, not 8;
/// - **any other `/CS` name** -- `/CalRGB`, `/I` and `/Indexed` included -- is looked up in the
///   page's resources, and is one bit of one component when it is not there;
/// - `/IM` plays no part in the size;
/// - keys are read **raw**, `#xx` undecoded, and the abbreviated key beats the full one in either
///   order.
///
/// Where those agree with the specification the size is computed, and where they do not, or where
/// the answer lives in a document this module does not hold, **the image refuses**: no `/CS` with
/// a `/BPC` other than 1, a device `/CS` with no `/BPC` or with `/IM true`, any other `/CS`, a key
/// written with `#`, and a key given twice in any spelling. The shapes the reviews ran end to end
/// -- no `/CS`, a device `/CS` with no `/BPC`, a `Cal`, `/I` or `/Indexed` name, an array, a mask
/// naming a `/CS`, both spellings of one key, an escaped key -- each returned `Ok` from redaction
/// over 1,842 dark pixels of text PDFium still drew, because the two readers ended the image in
/// two places. The rest -- one spelling given twice, an escaped `/CS` value, a `/BPC` that is not an
/// integer -- were not measured, and refuse because how the renderer reads them is not known here.
/// An `/L` that disagrees with the computed size refuses for the same
/// reason; a filtered image is [`Extent::Filtered`], which the caller's [`InlineImages`] decides.
///
/// # Errors
///
/// [`Error::Malformed`], naming which rule failed: an unknown, escaped or repeated key, a
/// disagreeing `/L`, a `/BPC` or `/CS` whose size the two readers do not agree on, a missing `/W`
/// or `/H`, or a size past this machine.
fn inline_image_length(dictionary: &[u8]) -> Result<Extent> {
    // `Prune`: the bytes between `BI` and `ID` hold no `ID`, so no image can start in them, and
    // one that did stops the read and is refused below. Naming `Redaction` here kept redaction's
    // own refusal text in the base web module, which `split` reaches through this function (#228).
    let mut lexer = Lexer::new(dictionary, InlineImages::Prune);
    let mut key: Option<Vec<u8>> = None;
    // Which of the keys has been seen, in either spelling.
    let mut seen = [false; IMAGE_KEYS.len()];
    let (mut width, mut height, mut bits) = (None, None, None);
    let mut colour_space: Option<Vec<u8>> = None;
    let mut mask = false;
    let mut filtered = false;
    let mut declared: Option<u64> = None;

    while let Some(token) = lexer.next_token()? {
        let span = lexer.span();
        let raw_has_escape = dictionary
            .get(span.0..span.1)
            .is_some_and(|raw| raw.contains(&b'#'));
        let Some(name) = key.take() else {
            // A key position. Anything but a name here is a dictionary this cannot read.
            if let Token::Name(name) = token {
                // PDFIUM READS A KEY RAW: `/#57` is `W` to this lexer and not to the renderer.
                if raw_has_escape {
                    return Err(Error::Malformed(
                        "pdf syntax [inline-image-escaped-key]: an inline image whose dictionary \
                         writes a key with '#', which the renderer does not decode"
                            .to_owned(),
                    ));
                }
                let Some(index) = IMAGE_KEYS
                    .iter()
                    .position(|spellings| spellings.contains(&name.as_slice()))
                else {
                    return Err(Error::Malformed(
                        "pdf syntax [inline-image-unknown-key]: an inline image whose dictionary has a key \
                         burrow does not know, so what it means to the renderer is unknown"
                            .to_owned(),
                    ));
                };
                // ONE VALUE PER KEY. The renderer lets the abbreviated spelling win in either
                // order; which one this lexer would keep is a second rule nobody needs.
                let slot = seen.get_mut(index).ok_or_else(|| {
                    Error::Internal("an inline image key outside its own table".to_owned())
                })?;
                if *slot {
                    return Err(Error::Malformed(
                        "pdf syntax [inline-image-repeated-key]: an inline image whose dictionary \
                         gives the same key twice, in one spelling or two"
                            .to_owned(),
                    ));
                }
                *slot = true;
                key = Some(name);
                continue;
            }
            return Err(Error::Malformed(
                "pdf syntax: an inline image whose dictionary has something other than a name \
                 where a key belongs"
                    .to_owned(),
            ));
        };
        // AN ARRAY OR DICTIONARY VALUE IS ONE VALUE, not four tokens. This loop alternates
        // key, value, key, value, so `/D [1 0]` put `1` in a key position and the whole
        // dictionary was refused -- and `/D` is the Decode array, which every one-bit image
        // mask carries. Measured on `producer-latex.pdf`, whose Type 3 bitmap glyphs are
        // inline images written exactly that way: fifty-five of them, and the first refused
        // the page. Nothing caught it because the walk had no reason to read a glyph
        // procedure until the Type 3 check did.
        if matches!(token, Token::ArrayOpen | Token::DictOpen) {
            skip_composite(&mut lexer, matches!(token, Token::ArrayOpen))?;
            match name.as_slice() {
                b"F" | b"Filter" => filtered = true,
                // An array colour space (`[/CalRGB <<...>>]`, `[/Indexed ...]`) is sized by a
                // rule this lexer does not share with the renderer; measured, they disagree.
                b"CS" | b"ColorSpace" => return Err(unshared_colour_space()),
                _ => {}
            }
            continue;
        }
        // A KEYWORD VALUE is `true`, `false` or `null`; any other word is one the renderer does
        // not read as a value, and where it then thinks the dictionary ends is not known here.
        if let Token::Keyword(ref word) = token
            && !matches!(word.as_slice(), b"true" | b"false" | b"null")
        {
            return Err(Error::Malformed(
                "pdf syntax [inline-image-nested-value]: an inline image dictionary with a value \
                 the renderer does not read as one value"
                    .to_owned(),
            ));
        }
        let number = || -> Option<u64> {
            let raw = dictionary.get(span.0..span.1)?;
            std::str::from_utf8(raw).ok()?.parse::<u64>().ok()
        };
        match name.as_slice() {
            b"L" | b"Length" => declared = number(),
            b"W" | b"Width" => width = number(),
            b"H" | b"Height" => height = number(),
            b"BPC" | b"BitsPerComponent" => {
                // A `/BPC` that is there but not a plain integer is not "absent": the renderer
                // reads something from it, and what is not known here.
                bits = Some(number().ok_or_else(|| {
                    Error::Malformed(
                        "pdf syntax: an inline image whose /BPC is not a plain integer".to_owned(),
                    )
                })?);
            }
            b"F" | b"Filter" => filtered = true,
            b"IM" | b"ImageMask" => mask = matches!(token, Token::Keyword(ref w) if w == b"true"),
            b"CS" | b"ColorSpace" => {
                if raw_has_escape {
                    return Err(unshared_colour_space());
                }
                if let Token::Name(value) = token {
                    colour_space = Some(value);
                } else {
                    return Err(Error::Malformed(
                        "pdf syntax: an inline image whose /CS is not a name, so its component \
                         count cannot be worked out"
                            .to_owned(),
                    ));
                }
            }
            _ => {}
        }
    }

    // A KEY WITH NO VALUE BEFORE `ID`: PDFium takes `ID` itself as the value and never starts the
    // image, so its data is drawn as content (#228, second review).
    if key.is_some() {
        return Err(Error::Malformed(
            "pdf syntax [inline-image-key-without-value]: an inline image whose last key has no \
             value before 'ID'"
                .to_owned(),
        ));
    }
    // CANNOT FIRE: these bytes end before the first `ID`, so no image starts inside them. An
    // internal error rather than a refusal, so that a change making it reachable says so.
    if lexer.extent_unknown() {
        return Err(Error::Internal(
            "pdf syntax: an inline image dictionary held an image".to_owned(),
        ));
    }
    if filtered {
        return Ok(Extent::Filtered);
    }

    let (components, bits) = match colour_space.as_deref() {
        // NO `/CS`: the renderer reads one bit of one component, ignoring `/BPC` and `/IM`. That
        // agrees with the specification only for a one-bit image -- every image mask written
        // as written, which is what real documents carry -- so anything else refuses.
        None => {
            if bits.is_some_and(|declared| declared != 1) {
                return Err(Error::Malformed(
                    "pdf syntax [inline-image-bpc-without-cs]: an inline image with no /CS and a \
                     /BPC other than 1, which the renderer reads as one bit whatever it says"
                        .to_owned(),
                ));
            }
            (1, 1)
        }
        Some(space) => {
            let components = match space {
                b"G" | b"DeviceGray" => 1,
                b"RGB" | b"DeviceRGB" => 3,
                b"CMYK" | b"DeviceCMYK" => 4,
                // Everything else -- `/CalRGB`, `/I`, `/Indexed` and any resource name -- is
                // resolved through the page's `/Resources`, which this module does not hold.
                // #142 is where that changes, if it does.
                _ => return Err(unshared_colour_space()),
            };
            if mask {
                return Err(Error::Malformed(
                    "pdf syntax [inline-image-mask-with-cs]: an image mask that also names a \
                     colour space, which the renderer sizes by the colour space and the \
                     specification by the mask"
                        .to_owned(),
                ));
            }
            // The renderer reads a missing `/BPC` here as 0, the specification as 8.
            let Some(bits) = bits else {
                return Err(Error::Malformed(
                    "pdf syntax [inline-image-cs-without-bpc]: an inline image with a /CS and no \
                     /BPC, which the renderer reads as 0 bits and the specification as 8"
                        .to_owned(),
                ));
            };
            (components, bits)
        }
    };
    let (Some(width), Some(height)) = (width, height) else {
        return Err(Error::Malformed(
            "pdf syntax: an inline image with no /W and /H, so nothing says how long it is"
                .to_owned(),
        ));
    };
    if !matches!(bits, 1 | 2 | 4 | 8 | 16) {
        return Err(Error::Malformed(
            "pdf syntax: an inline image whose /BPC is not one of the five the specification \
             allows"
                .to_owned(),
        ));
    }

    // Rows are padded to a byte boundary; the total is rows x height. Checked throughout: these
    // are three numbers straight out of the file and their product is not bounded by anything.
    let row_bits = width
        .checked_mul(bits)
        .and_then(|b| b.checked_mul(components))
        .ok_or_else(|| {
            Error::Malformed("pdf syntax: an inline image row larger than a u64".to_owned())
        })?;
    let row_bytes = row_bits.div_ceil(8);
    let total = row_bytes.checked_mul(height).ok_or_else(|| {
        Error::Malformed("pdf syntax: an inline image larger than a u64".to_owned())
    })?;
    // A DECLARED `/L` MUST AGREE. The renderer does not read it, so one that says otherwise is a
    // length only this lexer would believe.
    if declared.is_some_and(|declared| declared != total) {
        return Err(Error::Malformed(
            "pdf syntax [inline-image-length-disagrees]: an inline image whose /L disagrees with \
             the size its dictionary implies, which is the one the renderer reads"
                .to_owned(),
        ));
    }
    usize::try_from(total).map(Extent::Bytes).map_err(|_| {
        Error::Malformed("pdf syntax: an inline image longer than this machine".to_owned())
    })
}

/// The refusal for a colour space whose component count the renderer resolves differently.
fn unshared_colour_space() -> Error {
    Error::Malformed(
        "pdf syntax [inline-image-unshared-cs]: an inline image whose /CS is not one of the device \
         colour spaces, which the renderer resolves through the page's resources and this cannot"
            .to_owned(),
    )
}

/// Two ASCII hex digits as a byte.
fn hex_value(pair: &[u8]) -> Option<u8> {
    let digit = |b: u8| -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    };
    let high = digit(*pair.first()?)?;
    let low = digit(*pair.get(1)?)?;
    Some(high * 16 + low)
}

#[cfg(test)]
mod tests {
    use super::{InlineImages, Lexer, Token, hex_value, is_whitespace};

    fn tokens(bytes: &[u8]) -> Vec<Token> {
        let mut lexer = Lexer::new(bytes, InlineImages::REDACTION);
        let mut out = Vec::new();
        while let Some(token) = lexer.next_token().expect("lexes") {
            out.push(token);
        }
        out
    }

    fn names(bytes: &[u8]) -> Vec<String> {
        tokens(bytes)
            .into_iter()
            .filter_map(|t| match t {
                Token::Name(name) => Some(String::from_utf8_lossy(&name).into_owned()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn nul_is_white_space_and_the_other_five_are_too() {
        // PDF 32000-1 table 1. NUL is the one that gets forgotten, and forgetting it turns a
        // name followed by NUL into a name with a NUL in it -- a name that matches no resource.
        for byte in [b'\0', b'\t', b'\n', 0x0c, b'\r', b' '] {
            assert!(is_whitespace(byte), "{byte:#04x}");
        }
        assert!(!is_whitespace(b'a'));
        assert_eq!(names(b"/A\0/B"), ["A", "B"]);
    }

    #[test]
    fn a_name_decodes_its_hash_escapes() {
        assert_eq!(
            names(b"/A#20B /Lime#20Green /paired#23"),
            ["A B", "Lime Green", "paired#"]
        );
    }

    #[test]
    fn a_truncated_or_invalid_hash_escape_is_refused_rather_than_taken_literally() {
        // The under-keep direction: guessing that `/F#1` means `F#1` would produce a name that
        // matches no resource, and the resource it should have kept is removed.
        for bad in [&b"/F#1"[..], b"/F#zz", b"/F#"] {
            let mut lexer = Lexer::new(bad, InlineImages::REDACTION);
            assert!(
                lexer.next_token().is_err(),
                "{:?}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    #[test]
    fn a_string_hides_what_looks_like_a_name() {
        // The whole reason this is a lexer: `(/F9)` mentions no resource.
        assert_eq!(names(b"/F1 (/F9) Tj /F2"), ["F1", "F2"]);
        assert_eq!(names(b"/F1 <2F4639> /F2"), ["F1", "F2"]);
    }

    #[test]
    fn a_string_may_contain_escaped_and_balanced_parentheses() {
        assert_eq!(names(b"(a\\)b) /F1"), ["F1"]);
        assert_eq!(names(b"(a(b)c) /F1"), ["F1"]);
        assert_eq!(names(b"(a\\\\) /F1"), ["F1"]);
    }

    #[test]
    fn an_unterminated_string_is_refused_rather_than_swallowing_the_rest() {
        let mut lexer = Lexer::new(b"/F1 (never closed /F2", InlineImages::REDACTION);
        assert_eq!(
            lexer.next_token().expect("first"),
            Some(Token::Name(b"F1".to_vec()))
        );
        assert!(lexer.next_token().is_err());
    }

    #[test]
    fn a_comment_runs_to_the_end_of_the_line() {
        assert_eq!(names(b"/F1 % /F9 a comment\n/F2"), ["F1", "F2"]);
    }

    #[test]
    fn dictionary_and_array_delimiters_are_their_own_tokens() {
        assert_eq!(
            tokens(b"<< /K [1 2] >>"),
            [
                Token::DictOpen,
                Token::Name(b"K".to_vec()),
                Token::ArrayOpen,
                Token::Number,
                Token::Number,
                Token::ArrayClose,
                Token::DictClose,
            ]
        );
    }

    #[test]
    fn an_indirect_reference_is_three_tokens() {
        assert_eq!(
            tokens(b"7 0 R"),
            [Token::Number, Token::Number, Token::Keyword(b"R".to_vec()),]
        );
    }

    #[test]
    fn a_composite_value_in_an_image_dictionary_is_one_value_and_not_four_tokens() {
        // `/D [1 0]` is the Decode array every one-bit image mask carries, and this loop used
        // to read the `1` as the next key and refuse the whole dictionary. `producer-latex.pdf`
        // writes its Type 3 bitmap glyphs exactly this way.
        let stream = b"q BI /W 9 /H 1 /BPC 8 /CS /G /D [1 0] ID \x00(/F9<<\xff\xfe EI Q /F2";
        assert_eq!(names(stream), ["W", "H", "BPC", "CS", "G", "D", "F2"]);
    }

    #[test]
    fn a_filter_written_as_an_array_still_counts_as_filtered() {
        // `/F [/AHx]` is legal and means the same as `/F /AHx`. The extent is then wherever the
        // filter's data ends, so it must still refuse rather than compute one from `/W` and `/H`
        // -- which would end the image in the middle of its own data.
        let stream = b"q BI /W 1 /H 1 /F [/AHx] ID abcd EI Q";
        let mut lexer = Lexer::new(stream, InlineImages::REDACTION);
        let mut refused = false;
        loop {
            match lexer.next_token() {
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(_) => {
                    refused = true;
                    break;
                }
            }
        }
        assert!(
            refused,
            "a filtered image with no /L has no derivable extent"
        );
    }

    #[test]
    fn a_nested_value_in_an_image_dictionary_refuses_and_a_flat_one_reads_on() {
        // #228's SECOND SECURITY REVIEW. Bracket-balancing read each of these as one value; PDFium
        // abandoned the image and drew its data as content -- redaction `Ok` over 1,842 dark
        // pixels, each measured end to end. (This replaces a nesting-ceiling test: nothing nested
        // is read now, so there is no ceiling to reach.)
        for value in [
            "/D [[1 0]]",
            "/D [1 [0]]",
            "/I [[true]]",
            "/DP << /A 1 2 >>",
            "/DP << 1 2 >>",
            "/DP << (a) 1 >>",
            "/DP << /A [1] >>",
        ] {
            let refused = read(
                &image(&format!("/W 9 /H 1 /BPC 8 /CS /G {value}"), 9),
                InlineImages::REDACTION,
            )
            .expect_err(value);
            assert!(
                refused.contains("[inline-image-nested-value]"),
                "{value}: {refused}"
            );
        }
        // THE NEAR-MISSES: flat, which is what every real image here writes.
        for value in [
            "/D [1 0]",
            "/DP << /K -1 /Columns 9 >>",
            "/I true",
            "/D null",
        ] {
            let names = read(
                &image(&format!("/W 9 /H 1 /BPC 8 /CS /G {value}"), 9),
                InlineImages::REDACTION,
            )
            .unwrap_or_else(|error| panic!("{value}: {error}"));
            assert!(names.contains(&"AFTER".to_owned()), "{value}");
        }
    }

    #[test]
    fn bytes_0x80_and_0xff_end_a_word_as_the_renderer_reads_them() {
        // #228's THIRD SECURITY REVIEW, every byte value measured between two tokens: PDFium ends a
        // word at 0x80 and 0xFF as at white space. Without them `Tj\xff` was an unknown operator
        // here and showed text there, and `/Fm1\xff` named no form.
        for byte in [0x80u8, 0xff] {
            let mut stream = b"(S) Tj".to_vec();
            stream.push(byte);
            stream.extend_from_slice(b"ET");
            assert_eq!(
                tokens(&stream),
                [
                    Token::Str,
                    Token::Keyword(b"Tj".to_vec()),
                    Token::Keyword(b"ET".to_vec())
                ],
                "{byte:#04x} ends the operator"
            );
            let mut name = b"/Fm1".to_vec();
            name.push(byte);
            name.extend_from_slice(b"Do");
            assert_eq!(names(&name), ["Fm1"], "{byte:#04x} ends the name");
            // And it is the one byte after `ID`, so the image begins where PDFium begins it.
            let mut image = b"BI /W 1 /H 1 /BPC 8 /CS /G ID".to_vec();
            image.push(byte);
            image.extend_from_slice(b"X EI /AFTER 1 Tf");
            let read = read(&image, InlineImages::REDACTION).expect("an image");
            assert!(read.contains(&"AFTER".to_owned()), "{byte:#04x}: {read:?}");
        }
        // THE NEAR-MISSES: vertical tab and 0xA0 do NOT end a word to PDFium, measured.
        for byte in [0x0bu8, 0xa0] {
            let stream = [b'T', b'j', byte];
            assert_eq!(
                tokens(&stream),
                [Token::Keyword(stream.to_vec())],
                "{byte:#04x} is part of the word"
            );
        }
    }

    #[test]
    fn a_keyword_inside_a_dictionary_value_refuses_and_inside_an_array_reads_on() {
        // #228's THIRD REVIEWS: an unknown word as a value inside `<< >>` made PDFium end the
        // image's dictionary at `>>`, ahead of `/W` -- `Ok` over 1,842 dark pixels. In an array
        // PDFium skips the word, measured, so `/D [0 foo 1]` agrees and reads on.
        for value in [
            "/DP << /K foo >>",
            "/DP << /K 1x >>",
            "/DP << /K EI >>",
            "/DecodeParms << /Predictor foo >>",
        ] {
            let refused = read(
                &image(&format!("{value} /W 9 /H 1 /BPC 8 /CS /G"), 9),
                InlineImages::REDACTION,
            )
            .expect_err(value);
            assert!(
                refused.contains("[inline-image-nested-value]"),
                "{value}: {refused}"
            );
        }
        for value in ["/DP << /K true >>", "/DP << /K null >>", "/D [0 foo 1]"] {
            let names = read(
                &image(&format!("{value} /W 9 /H 1 /BPC 8 /CS /G"), 9),
                InlineImages::REDACTION,
            )
            .unwrap_or_else(|error| panic!("{value}: {error}"));
            assert!(names.contains(&"AFTER".to_owned()), "{value}");
        }
    }

    #[test]
    fn a_keyword_value_other_than_true_false_or_null_refuses() {
        let refused = read(
            &image("/W 9 /H 1 /BPC 8 /CS /G /I maybe", 9),
            InlineImages::REDACTION,
        )
        .expect_err("an unknown word as a value");
        assert!(refused.contains("[inline-image-nested-value]"), "{refused}");
    }

    #[test]
    fn a_key_with_no_value_before_id_refuses() {
        // PDFium takes `ID` as the value of `/D` and never starts the image (#228, second review).
        let refused = read(
            &image("/H 1 /CS /G /BPC 8 /W 9 /D", 9),
            InlineImages::REDACTION,
        )
        .expect_err("a dangling key");
        assert!(
            refused.contains("[inline-image-key-without-value]"),
            "{refused}"
        );
    }

    #[test]
    fn a_second_bi_inside_an_open_dictionary_refuses() {
        // It restarted the dictionary: `/W 8` unread here, read by PDFium -- 8 bytes to it, 96 to
        // burrow, past the repeated-key refusal; with `/F` before it, past the filtered one too.
        for dictionary in [
            "/W 8 /D BI /Width 96 /H 1 /CS /G /BPC 8",
            "/F /AHx /D BI /W 96 /H 1 /CS /G /BPC 8",
        ] {
            for mode in [InlineImages::REDACTION, InlineImages::Prune] {
                let refused = read(&image(dictionary, 96), mode).expect_err(dictionary);
                assert!(
                    refused.contains("[inline-image-nested-bi]"),
                    "{dictionary}: {refused}"
                );
            }
        }
        // THE NEAR-MISS: two images one after the other are two images.
        let mut two = image("/W 1 /H 1 /BPC 8 /CS /G", 1);
        two.extend_from_slice(b" ");
        two.extend(image("/W 1 /H 1 /BPC 8 /CS /G", 1));
        assert!(read(&two, InlineImages::REDACTION).is_ok());
    }

    #[test]
    fn ei_must_stand_as_a_word() {
        // `EI5` and `EIx` are not the end of the image to PDFium, which scanned on into the next
        // image's data for an `EI` that was, and drew what burrow had read as image bytes.
        for glued in ["EI5", "EIx", "EI/"] {
            let stream = format!("BI /W 1 /H 1 /CS /G /BPC 8 ID X {glued} Q");
            let result = read(stream.as_bytes(), InlineImages::REDACTION);
            if glued == "EI/" {
                // A delimiter ends a word, so this one is `EI` and then a name.
                assert!(result.is_ok(), "{glued}: {result:?}");
            } else {
                assert!(
                    result.is_err_and(|refused| refused.contains("ends somewhere else")),
                    "{glued}: an EI that is not a word ends nothing"
                );
            }
        }
        // And the end of the stream ends a word.
        assert!(
            read(
                b"BI /W 1 /H 1 /CS /G /BPC 8 ID X EI",
                InlineImages::REDACTION
            )
            .is_ok()
        );
    }

    #[test]
    fn a_number_glued_to_letters_is_one_word_as_the_renderer_reads_it() {
        // `8ID` is one unknown keyword to PDFium, not `8` and then `ID`; read as two, it began an
        // image whose bytes burrow skipped and PDFium drew. As one word, the text after it is
        // read here as PDFium reads it.
        assert_eq!(
            tokens(b"8ID 10Tf"),
            [
                Token::Keyword(b"8ID".to_vec()),
                Token::Keyword(b"10Tf".to_vec())
            ]
        );
        let stream = b"BI /W 96 /H 1 /CS /G /BPC 8ID xx BT /AFTER 12 Tf (S) Tj ET";
        let names = read(stream, InlineImages::REDACTION).expect("no image began");
        assert!(names.contains(&"AFTER".to_owned()), "{names:?}");
        // `5BI` is not `BI`, so a later `ID` has no image to begin.
        assert!(
            read(
                b"5BI /W 1 /H 1 /CS /G /BPC 8 ID X EI",
                InlineImages::REDACTION
            )
            .is_err_and(|refused| refused.contains("no 'BI' before it"))
        );
        // THE NEAR-MISSES: a delimiter or white space ends a number as before.
        assert_eq!(
            tokens(b"1(a)2/F 3 [4]"),
            [
                Token::Number,
                Token::Str,
                Token::Number,
                Token::Name(b"F".to_vec()),
                Token::Number,
                Token::ArrayOpen,
                Token::Number,
                Token::ArrayClose
            ]
        );
    }

    #[test]
    fn an_array_that_never_closes_in_an_image_dictionary_is_a_refusal() {
        let stream = b"q BI /W 1 /H 1 /D [1 0 ID abcd EI Q";
        let mut lexer = Lexer::new(stream, InlineImages::REDACTION);
        let mut refused = false;
        loop {
            match lexer.next_token() {
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(_) => {
                    refused = true;
                    break;
                }
            }
        }
        assert!(
            refused,
            "an unclosed array is not a dictionary burrow can read"
        );
    }

    #[test]
    fn inline_image_data_is_skipped_rather_than_tokenised() {
        // The binary payload contains `(`, `/` and `<`, every one of which would derail a
        // tokeniser that walked into it -- and `/F9` after them would be read as a used name
        // while `/F2` after the image would be lost. `/W 9 /H 1` declares the nine bytes
        // between `ID ` and ` EI`, so the extent is the dictionary's rather than a guess.
        let stream = b"BI /W 9 /H 1 /BPC 8 /CS /G ID \x00(/F9<<\xff\xfe EI Q /F2";
        assert_eq!(names(stream), ["W", "H", "BPC", "CS", "G", "F2"]);
    }

    #[test]
    fn the_declared_length_ends_the_image_even_when_a_later_ei_stands_alone() {
        // THE SECURITY REVIEW'S FINDING, as a test. The dictionary declares ONE byte, so the
        // image ends at the `EI` right after `A` -- which is preceded by `A` rather than by
        // white space, so the old standalone-`EI` scan ran past it and swallowed the text
        // object. PDFium -- the renderer burrow ships -- draws `SECRET` from this page.
        let stream = b"q BI /W 1 /H 1 /BPC 8 /CS /G ID AEI\nBT /F1 24 Tf (SECRET) Tj ET\n EI Q";
        let read = tokens(stream);
        assert!(
            read.contains(&Token::Str),
            "the text object after the image must be tokenised, not swallowed: {read:?}"
        );
        assert!(
            read.iter()
                .any(|t| matches!(t, Token::Keyword(w) if w == b"Tj")),
            "the drawing operator must be visible to a rewriter: {read:?}"
        );
    }

    #[test]
    fn a_declared_length_that_does_not_land_on_ei_is_refused() {
        // The dictionary says one byte and the data runs on. Two conforming readers would end
        // the image in two places, so there is no answer to carry on with.
        let mut lexer = Lexer::new(
            b"BI /W 1 /H 1 /BPC 8 /CS /G ID AAAAAAAA EI Q",
            InlineImages::REDACTION,
        );
        loop {
            match lexer.next_token() {
                Ok(Some(_)) => {}
                Ok(None) => panic!("the disagreement should have been refused"),
                Err(_) => break,
            }
        }
    }

    /// Lex `stream` to the end under `mode`: the names, or the refusal.
    fn read(stream: &[u8], mode: InlineImages) -> std::result::Result<Vec<String>, String> {
        let mut lexer = Lexer::new(stream, mode);
        let mut found = Vec::new();
        loop {
            match lexer.next_token() {
                Ok(Some(Token::Name(name))) => {
                    found.push(String::from_utf8_lossy(&name).into_owned())
                }
                Ok(Some(_)) => {}
                Ok(None) => return Ok(found),
                Err(error) => return Err(format!("{error:?}")),
            }
        }
    }

    /// An image of `size` bytes after `ID `, then `EI` and a name that must be read.
    fn image(dictionary: &str, size: usize) -> Vec<u8> {
        let mut stream = format!("BI {dictionary} ID ").into_bytes();
        stream.extend(std::iter::repeat_n(b'x', size));
        stream.extend_from_slice(b" EI /AFTER 12 Tf");
        stream
    }

    #[test]
    fn the_extent_is_computed_as_the_renderer_computes_it_at_every_boundary() {
        // #228: PDFium ends an unfiltered image after the bytes its dictionary implies, and each
        // of these was measured against it -- `EI` at the computed size drew the text after it,
        // one byte earlier did not. Here: the computed size reads on; one byte more refuses. The
        // last four are the near-miss twins of the shapes the next test refuses: where PDFium's
        // rule and the specification's agree, the image is read.
        let cases: [(&str, &str, usize); 11] = [
            (
                "a one-bit row padded to a byte",
                "/W 9 /H 2 /BPC 1 /CS /G",
                4,
            ),
            ("four components", "/W 1 /H 1 /BPC 8 /CS /DeviceCMYK", 4),
            ("sixteen bits", "/W 1 /H 1 /BPC 16 /CS /RGB", 6),
            (
                "a /Decode, which changes nothing",
                "/W 1 /H 1 /BPC 8 /CS /G /D [1 0]",
                1,
            ),
            ("an image mask: one bit, padded", "/W 9 /H 1 /IM true", 2),
            (
                "an image mask saying /BPC 1",
                "/W 9 /H 1 /IM true /BPC 1",
                2,
            ),
            ("no /CS and no /BPC: one bit", "/W 64 /H 1", 8),
            ("no /CS and /BPC 1, not a mask", "/W 9 /H 1 /BPC 1", 2),
            (
                "the full device gray",
                "/W 2 /H 1 /BPC 8 /CS /DeviceGray",
                2,
            ),
            ("the full device RGB", "/W 1 /H 1 /BPC 4 /CS /DeviceRGB", 2),
            ("the abbreviated CMYK", "/W 1 /H 1 /BPC 8 /CS /CMYK", 4),
        ];
        for (why, dictionary, size) in cases {
            let names = read(&image(dictionary, size), InlineImages::REDACTION)
                .unwrap_or_else(|error| panic!("{why}: {error}"));
            assert!(
                names.contains(&"AFTER".to_owned()),
                "{why}: the name after it is read"
            );
            assert!(
                read(&image(dictionary, size + 1), InlineImages::REDACTION).is_err(),
                "{why}: one byte more is a disagreement, refused"
            );
        }
    }

    #[test]
    fn a_declared_length_must_agree_with_the_computed_one() {
        // THE ISSUE'S SHAPE: an /L longer than the data, so this lexer swallowed the text after
        // the image while PDFium drew it -- `Ok`, 1,842 dark pixels in the region, measured.
        // `/L` reaching exactly to a second `EI`, so a lexer that believed it would end the image
        // there and never see the text -- the attack, not merely a wrong number.
        let tail = " EI Q BT /F1 24 Tf (SECRET) Tj ET";
        let overstated = format!(
            "BI /W 1 /H 1 /BPC 8 /CS /G /L {} ID x{tail} EI Q",
            1 + tail.len()
        );
        assert!(
            read(overstated.as_bytes(), InlineImages::REDACTION).is_err(),
            "an /L the renderer does not read refuses"
        );
        // THE TWIN: an /L that agrees reads on.
        let agrees = read(
            &image("/W 3 /H 1 /BPC 8 /CS /RGB /L 9", 9),
            InlineImages::REDACTION,
        )
        .expect("an agreeing /L");
        assert!(agrees.contains(&"AFTER".to_owned()));
    }

    #[test]
    fn every_shape_the_two_readers_size_differently_refuses_naming_why() {
        // #228's REVIEWS, as fixtures. Each of these is a dictionary PDFium sizes by a rule the
        // specification does not share -- measured by both reviewers with an `EI` at every
        // offset -- and each returned `Ok` from redaction over 1,842 dark pixels of text PDFium
        // still drew, because burrow ended the image somewhere else. The size given here is
        // the one burrow used to compute, so a lexer that went back to it would read on rather
        // than refuse.
        let cases: [(&str, &str, usize); 16] = [
            ("no /CS, /BPC 8 (PDFium: 2)", "/W 9 /H 1 /BPC 8", 9),
            (
                "a mask with /BPC 8 (PDFium: 2)",
                "/W 9 /H 1 /IM true /BPC 8",
                2,
            ),
            ("device RGB, no /BPC (PDFium: 0)", "/W 4 /H 1 /CS /RGB", 12),
            ("device gray, no /BPC (PDFium: 0)", "/W 9 /H 1 /CS /G", 9),
            (
                "/CalRGB as a name (PDFium: 2)",
                "/W 9 /H 1 /BPC 8 /CS /CalRGB",
                27,
            ),
            ("/CalGray as a name", "/W 9 /H 1 /BPC 8 /CS /CalGray", 9),
            ("/I as a name (PDFium: 2)", "/W 9 /H 1 /BPC 8 /CS /I", 9),
            ("/Indexed as a name", "/W 9 /H 1 /BPC 8 /CS /Indexed", 9),
            (
                "an array colour space (PDFium: 192 where burrow said 64 for [/CalRGB <<...>>])",
                "/W 64 /H 1 /BPC 8 /CS [/Indexed /DeviceRGB 1 <00ff00ff00ff>]",
                64,
            ),
            (
                "a mask naming /RGB (PDFium: 3)",
                "/W 8 /H 1 /IM true /BPC 1 /CS /RGB",
                1,
            ),
            (
                "/W then /Width (PDFium: 1)",
                "/W 1 /Width 9 /H 1 /BPC 8 /CS /G",
                9,
            ),
            ("/Width then /W", "/Width 9 /W 1 /H 1 /BPC 8 /CS /G", 1),
            ("one spelling twice", "/W 1 /W 9 /H 1 /BPC 8 /CS /G", 9),
            ("an escaped key (PDFium: 0)", "/#57 9 /H 1 /BPC 8 /CS /G", 9),
            (
                "an escaped colour space",
                "/W 9 /H 1 /BPC 8 /CS /Device#47ray",
                9,
            ),
            (
                "a /BPC that is not an integer",
                "/W 9 /H 1 /BPC 8.0 /CS /G",
                9,
            ),
        ];
        let codes = [
            "[inline-image-bpc-without-cs]",
            "[inline-image-bpc-without-cs]",
            "[inline-image-cs-without-bpc]",
            "[inline-image-cs-without-bpc]",
            "[inline-image-unshared-cs]",
            "[inline-image-unshared-cs]",
            "[inline-image-unshared-cs]",
            "[inline-image-unshared-cs]",
            "[inline-image-unshared-cs]",
            "[inline-image-mask-with-cs]",
            "[inline-image-repeated-key]",
            "[inline-image-repeated-key]",
            "[inline-image-repeated-key]",
            "[inline-image-escaped-key]",
            "[inline-image-unshared-cs]",
            "/BPC is not a plain integer",
        ];
        for ((why, dictionary, size), code) in cases.into_iter().zip(codes) {
            let refused = read(&image(dictionary, size), InlineImages::REDACTION).expect_err(why);
            assert!(
                refused.contains(code),
                "{why}: expected {code}, got {refused}"
            );
        }
    }

    #[test]
    fn both_spellings_of_every_key_are_read_and_an_unknown_key_refuses() {
        let abbreviated =
            "/W 2 /H 1 /BPC 8 /CS /G /D [0 1] /DP << >> /I false /L 2 /Intent /Perceptual";
        let full = "/Width 2 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray /Decode [0 1] \
                    /DecodeParms << >> /Interpolate false /Length 2 /Intent /Perceptual";
        for (family, dictionary) in [("abbreviated", abbreviated), ("full", full)] {
            let names = read(&image(dictionary, 2), InlineImages::REDACTION)
                .unwrap_or_else(|error| panic!("{family}: {error}"));
            assert!(names.contains(&"AFTER".to_owned()), "{family}");
        }
        for mask in ["/IM true", "/ImageMask true"] {
            assert!(
                read(
                    &image(&format!("/W 8 /H 1 {mask}"), 1),
                    InlineImages::REDACTION
                )
                .is_ok()
            );
        }
        for unknown in ["/Metadata 1", "/Foo 1"] {
            assert!(
                read(
                    &image(&format!("/W 1 /H 1 /BPC 8 /CS /G {unknown}"), 1),
                    InlineImages::REDACTION
                )
                .is_err_and(|refused| refused.contains("[inline-image-unknown-key]")),
                "{unknown}: a key nobody enumerated refuses"
            );
        }
    }

    #[test]
    fn a_filtered_image_refuses_for_redaction_and_stops_the_read_for_pruning() {
        // A FILTERED IMAGE ENDS where its filter's data ends, which this module cannot decode.
        for filter in ["/F /Fl", "/Filter /FlateDecode", "/F [/AHx /Fl]"] {
            let stream = image(&format!("/W 1 /H 1 /CS /G {filter} /L 4"), 4);
            assert!(read(&stream, InlineImages::REDACTION).is_err(), "{filter}");
            let mut lexer = Lexer::new(&stream, InlineImages::Prune);
            let mut names = Vec::new();
            while let Some(token) = lexer.next_token().expect("Prune does not refuse it") {
                if let Token::Name(name) = token {
                    names.push(name);
                }
            }
            assert!(lexer.extent_unknown(), "{filter}: the read says it stopped");
            assert!(
                !names.contains(&b"AFTER".to_vec()),
                "{filter}: nothing after it is claimed"
            );
        }
    }

    #[test]
    fn every_underivable_extent_is_a_refusal_rather_than_a_guess() {
        // ONE FIXTURE PER WAY THE DICTIONARY CAN FAIL TO SAY HOW LONG THE IMAGE IS. Each used to
        // fall back to scanning for a standalone `EI`, and that fall-back is the hiding place
        // this rule exists to close -- a recorded residue is still a leak. Every one of these
        // hides a `Tj` behind the data exactly as the security review's page did, so a
        // fall-back would tokenise none of them.
        let hidden = b" ID x\nBT /F1 24 Tf (SECRET) Tj ET\n EI Q";
        let cases: [(&str, &[u8]); 5] = [
            (
                "a filter: the renderer ends it at the filter's own end of data",
                b"q BI /W 1 /H 1 /F /Fl",
            ),
            (
                "a /CS naming a colour space from the page's resources, which this cannot resolve",
                b"q BI /W 1 /H 1 /CS /MySpace",
            ),
            (
                "no /W and no /H, so nothing says how many rows there are",
                b"q BI /CS /G /BPC 8",
            ),
            (
                "a /BPC the specification does not allow",
                b"q BI /W 1 /H 1 /CS /G /BPC 7",
            ),
            (
                "a dictionary with something other than a name where a key belongs",
                b"q BI 42 /W 1 /H 1 /CS /G",
            ),
        ];
        for (why, dictionary) in cases {
            let mut stream = dictionary.to_vec();
            stream.extend_from_slice(hidden);
            let mut lexer = Lexer::new(&stream, InlineImages::REDACTION);
            let refused = loop {
                match lexer.next_token() {
                    Ok(Some(_)) => {}
                    Ok(None) => break false,
                    Err(_) => break true,
                }
            };
            assert!(refused, "should have been refused -- {why}");
        }
    }

    #[test]
    fn an_id_with_no_bi_is_refused_because_it_has_no_extent() {
        // It has no dictionary, so there is nothing to compute an extent from and nothing to
        // check a guess against.
        let mut lexer = Lexer::new(b"q ID x\nBT (SECRET) Tj ET\n EI Q", InlineImages::REDACTION);
        loop {
            match lexer.next_token() {
                Ok(Some(_)) => {}
                Ok(None) => panic!("an 'ID' with no 'BI' should have been refused"),
                Err(_) => break,
            }
        }
    }

    #[test]
    fn an_inline_image_whose_declared_data_runs_off_the_end_is_refused() {
        // The dictionary implies more bytes than the stream holds. Truncated rather than hostile,
        // and the answer is the same: there is no `EI` where the dictionary says there is one.
        let mut lexer = Lexer::new(
            b"BI /W 4096 /H 1 /BPC 8 /CS /G ID ab",
            InlineImages::REDACTION,
        );
        loop {
            match lexer.next_token() {
                Ok(Some(_)) => {}
                Ok(None) => panic!("a declared length past the end should have been refused"),
                Err(_) => break,
            }
        }
    }
    #[test]
    fn hex_pairs_decode_in_both_cases_and_reject_anything_else() {
        assert_eq!(hex_value(b"20"), Some(0x20));
        assert_eq!(hex_value(b"ff"), Some(0xff));
        assert_eq!(hex_value(b"FF"), Some(0xff));
        assert_eq!(hex_value(b"g0"), None);
        assert_eq!(hex_value(b"0"), None);
    }

    #[test]
    fn an_unmatched_closing_delimiter_is_refused() {
        for bad in [&b">"[..], b")"] {
            let mut lexer = Lexer::new(bad, InlineImages::REDACTION);
            assert!(
                lexer.next_token().is_err(),
                "{:?}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    #[test]
    fn the_lexer_terminates_on_every_byte_value() {
        // A zero-length token would leave the cursor where it was and loop forever. Every byte
        // is fed as a one-byte input and as a byte inside a run; the assertion is that the call
        // returns at all, whichever way it returns.
        for byte in 0u8..=255 {
            let alone = [byte];
            let mut lexer = Lexer::new(&alone, InlineImages::REDACTION);
            let _ = lexer.next_token();
            let in_a_run = [b'/', byte, b' ', byte];
            let mut lexer = Lexer::new(&in_a_run, InlineImages::REDACTION);
            let mut budget = 8;
            while budget > 0 {
                budget -= 1;
                match lexer.next_token() {
                    Ok(Some(_)) => {}
                    Ok(None) | Err(_) => break,
                }
            }
            assert!(budget > 0, "byte {byte:#04x} did not terminate");
        }
    }
}

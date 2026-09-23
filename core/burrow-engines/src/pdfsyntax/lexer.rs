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

/// Whether `byte` is one of PDF's six white-space characters.
///
/// NUL is white space in PDF, which is easy to miss and is why this is a named function with a
/// test rather than a `matches!` at three call sites.
const fn is_whitespace(byte: u8) -> bool {
    matches!(byte, b'\0' | b'\t' | b'\n' | 0x0c | b'\r' | b' ')
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
}

impl<'a> Lexer<'a> {
    /// Start at the beginning of `bytes`.
    pub(super) const fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            at: 0,
            token_start: 0,
            bi_at: None,
        }
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
                self.read_run(is_numeric);
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
    /// So the length comes from `/L` (`/Length`), or is computed from `/W`, `/H`, `/BPC` and
    /// `/CS`, and an `EI` is **required** there. Where neither is possible — a `/F` filter with
    /// no `/L`, a `/CS` naming a colour space from the page's `/Resources`, a missing `/W` or
    /// `/H` — **the stream is refused.**
    ///
    /// # Why refusing, rather than falling back
    ///
    /// The fall-back was kept at first and recorded as a known residue. A recorded residue is
    /// still a leak: for those images the page above works exactly as it did, and the whole
    /// point of the dictionary rule is that this tokeniser must not report a page as carrying
    /// less than it draws. Refusing turns an unreadable extent into a refusal, which is an
    /// outcome burrow already has a vocabulary for and which cannot be mistaken for "clean".
    ///
    /// It costs fidelity, and the cost is stated rather than discovered: a document whose inline
    /// image is filtered and carries no `/L` is now refused by every caller of this lexer,
    /// `split`'s resource scan included. **No committed fixture is affected** — the corpus's one
    /// inline image (`tests/redaction/generated/evade-inline-image.pdf`) declares `/L 135` — and
    /// #142 tracks deriving the extents that are currently out of reach, which is what would
    /// narrow the refusal again.
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

        let length = inline_image_length(dictionary)?;
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
        if self.bytes.get(at..at.saturating_add(2)) == Some(b"EI") {
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

/// Read past a composite value in an inline image's dictionary, to its matching close.
///
/// Bounded by [`MAX_NESTING`], the same ceiling the operand reader uses: a value nested deeper
/// than that is generated rather than written, and reading on would be unbounded recursion in
/// the one place that must not have any.
fn skip_composite(lexer: &mut Lexer<'_>, array: bool) -> Result<()> {
    let mut depth = 1usize;
    while let Some(token) = lexer.next_token()? {
        match token {
            Token::ArrayOpen | Token::DictOpen => {
                depth += 1;
                if depth > MAX_NESTING {
                    return Err(Error::Unsupported(
                        "an inline image dictionary nested deeper than burrow will read".to_owned(),
                    ));
                }
            }
            Token::ArrayClose | Token::DictClose => {
                depth -= 1;
                if depth == 0 {
                    return Ok(());
                }
            }
            _ => {}
        }
    }
    Err(Error::Malformed(if array {
        "pdf syntax: an inline image dictionary with an array that never closes".to_owned()
    } else {
        "pdf syntax: an inline image dictionary with a dictionary that never closes".to_owned()
    }))
}

/// How many bytes of data an inline image's dictionary declares.
///
/// `dictionary` is the bytes between `BI` and `ID`.
///
/// # Errors
///
/// [`Error::Malformed`], naming which of the derivations failed, when the extent cannot be
/// worked out from the dictionary alone. That is a refusal rather than a fall-back, and
/// [`skip_inline_image_data`](Lexer::skip_inline_image_data) has the reasoning; the message says
/// which case it was, because "an inline image burrow cannot read" is not something a person can
/// act on and these four are.
fn inline_image_length(dictionary: &[u8]) -> Result<usize> {
    let mut lexer = Lexer::new(dictionary);
    let mut key: Option<Vec<u8>> = None;
    let (mut width, mut height, mut bits) = (None, None, None);
    let mut colour_space: Option<Vec<u8>> = None;
    let mut mask = false;
    let mut filtered = false;
    let mut declared: Option<u64> = None;

    while let Some(token) = lexer.next_token()? {
        let span = lexer.span();
        let Some(name) = key.take() else {
            // A key position. Anything but a name here is a dictionary this cannot read.
            if let Token::Name(name) = token {
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
            if matches!(name.as_slice(), b"F" | b"Filter") {
                filtered = true;
            }
            continue;
        }
        let number = || -> Option<u64> {
            let raw = dictionary.get(span.0..span.1)?;
            std::str::from_utf8(raw).ok()?.parse::<u64>().ok()
        };
        match name.as_slice() {
            b"L" | b"Length" => declared = number(),
            b"W" | b"Width" => width = number(),
            b"H" | b"Height" => height = number(),
            b"BPC" | b"BitsPerComponent" => bits = number(),
            b"F" | b"Filter" => filtered = true,
            b"IM" | b"ImageMask" => mask = matches!(token, Token::Keyword(ref w) if w == b"true"),
            b"CS" | b"ColorSpace" => {
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

    // `/L` is the producer saying it outright, and it beats any computation -- including for a
    // filtered image, which is the only way a filtered one is derivable at all.
    if let Some(length) = declared {
        return usize::try_from(length).map_err(|_| {
            Error::Malformed("pdf syntax: an inline image longer than this machine".to_owned())
        });
    }
    if filtered {
        return Err(Error::Malformed(
            "pdf syntax: an inline image with a /F filter and no /L, so how many bytes it \
             occupies is whatever the filter produced and nothing on the page says"
                .to_owned(),
        ));
    }

    let components = if mask {
        1
    } else {
        match colour_space.as_deref() {
            Some(b"G" | b"DeviceGray" | b"CalGray" | b"I" | b"Indexed") => 1,
            Some(b"RGB" | b"DeviceRGB" | b"CalRGB") => 3,
            Some(b"CMYK" | b"DeviceCMYK") => 4,
            // A name this does not know is a colour space from the page's `/Resources`, whose
            // component count lives in a dictionary this module cannot resolve -- it holds no
            // document, by design. #142 is where that changes, if it does.
            Some(_) => {
                return Err(Error::Malformed(
                    "pdf syntax: an inline image whose /CS names a colour space from the page's \
                     resources, whose component count this cannot resolve"
                        .to_owned(),
                ));
            }
            None => 1,
        }
    };
    let (Some(width), Some(height)) = (width, height) else {
        return Err(Error::Malformed(
            "pdf syntax: an inline image with no /L and no /W and /H, so nothing says how long \
             it is"
                .to_owned(),
        ));
    };
    // `/BPC` defaults to 8, and to 1 for an image mask -- PDF 32000-1 Table 91.
    let bits = bits.unwrap_or(if mask { 1 } else { 8 });
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
    usize::try_from(total).map_err(|_| {
        Error::Malformed("pdf syntax: an inline image longer than this machine".to_owned())
    })
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
    use super::{Lexer, MAX_NESTING, Token, hex_value, is_whitespace};

    fn tokens(bytes: &[u8]) -> Vec<Token> {
        let mut lexer = Lexer::new(bytes);
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
            let mut lexer = Lexer::new(bad);
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
        let mut lexer = Lexer::new(b"/F1 (never closed /F2");
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
        let stream = b"q BI /W 9 /H 1 /D [1 0] ID \x00(/F9<<\xff\xfe EI Q /F2";
        assert_eq!(names(stream), ["W", "H", "D", "F2"]);
    }

    #[test]
    fn a_filter_written_as_an_array_still_counts_as_filtered() {
        // `/F [/AHx]` is legal and means the same as `/F /AHx`. The extent is then whatever the
        // filter produced, so without an `/L` it must still refuse rather than compute one from
        // `/W` and `/H` -- which would end the image in the middle of its own data.
        let stream = b"q BI /W 1 /H 1 /F [/AHx] ID abcd EI Q";
        let mut lexer = Lexer::new(stream);
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
    fn an_image_dictionary_nested_past_the_ceiling_is_a_refusal() {
        // A mutation sweep deleted `skip_composite`'s `MAX_NESTING` check and the suite stayed
        // green: the bound was real and nothing failed for its absence, which `CLAUDE.md` says
        // is not a defence. The depth here is one past the lexer's own ceiling.
        let mut stream = b"q BI /W 1 /H 1 /D ".to_vec();
        stream.extend(std::iter::repeat_n(b'[', MAX_NESTING + 1));
        stream.extend_from_slice(b" 1 ");
        stream.extend(std::iter::repeat_n(b']', MAX_NESTING + 1));
        stream.extend_from_slice(b" ID a EI Q");
        let mut lexer = Lexer::new(&stream);
        let mut refused = None;
        loop {
            match lexer.next_token() {
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(error) => {
                    refused = Some(format!("{error}"));
                    break;
                }
            }
        }
        let refused = refused.expect("nesting past the ceiling is a refusal");
        assert!(
            refused.contains("nested deeper than burrow will read"),
            "the refusal must name the nesting, got: {refused}"
        );
    }

    #[test]
    fn an_image_dictionary_nested_to_the_ceiling_is_read() {
        // THE NEAR-MISS. Without it the test above passes for a ceiling of one.
        let mut stream = b"q BI /W 9 /H 1 /D ".to_vec();
        stream.extend(std::iter::repeat_n(b'[', MAX_NESTING - 1));
        stream.extend_from_slice(b" 1 ");
        stream.extend(std::iter::repeat_n(b']', MAX_NESTING - 1));
        stream.extend_from_slice(b" ID \x00(/F9<<\xff\xfe EI Q /F2");
        assert_eq!(names(&stream), ["W", "H", "D", "F2"]);
    }

    #[test]
    fn an_array_that_never_closes_in_an_image_dictionary_is_a_refusal() {
        let stream = b"q BI /W 1 /H 1 /D [1 0 ID abcd EI Q";
        let mut lexer = Lexer::new(stream);
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
        let stream = b"BI /W 9 /H 1 ID \x00(/F9<<\xff\xfe EI Q /F2";
        assert_eq!(names(stream), ["W", "H", "F2"]);
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
        let mut lexer = Lexer::new(b"BI /W 1 /H 1 /CS /G ID AAAAAAAA EI Q");
        loop {
            match lexer.next_token() {
                Ok(Some(_)) => {}
                Ok(None) => panic!("the disagreement should have been refused"),
                Err(_) => break,
            }
        }
    }

    #[test]
    fn an_explicit_length_beats_the_computation_and_works_for_a_filtered_image() {
        let stream = b"BI /W 99 /H 99 /F /AHx /L 4 ID abcd EI Q /F2";
        assert_eq!(names(stream), ["W", "H", "F", "AHx", "L", "F2"]);
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
                "a filter with no /L: the encoded length is whatever the filter produced",
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
            let mut lexer = Lexer::new(&stream);
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
        let mut lexer = Lexer::new(b"q ID x\nBT (SECRET) Tj ET\n EI Q");
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
        // `/L` says more bytes than the stream holds. Truncated rather than hostile, and the
        // answer is the same: there is no `EI` where the dictionary says there is one.
        let mut lexer = Lexer::new(b"BI /W 1 /H 1 /L 4096 ID ab");
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
            let mut lexer = Lexer::new(bad);
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
            let mut lexer = Lexer::new(&alone);
            let _ = lexer.next_token();
            let in_a_run = [b'/', byte, b' ', byte];
            let mut lexer = Lexer::new(&in_a_run);
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

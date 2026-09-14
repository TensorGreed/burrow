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
//! direction that breaks pages. So `ID` is handled explicitly, and an inline image whose `EI`
//! cannot be found is a refusal.

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
}

impl<'a> Lexer<'a> {
    /// Start at the beginning of `bytes`.
    pub(super) const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
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
                if word == b"ID" {
                    // THE INLINE IMAGE IS SKIPPED HERE, not by the caller. It began as a
                    // method callers were expected to call after seeing `ID`, and that is a
                    // discipline: `names_in_content` remembered and `top_level_keys` did not,
                    // which is one lexer with two behaviours depending on who is holding it.
                    // A caller cannot forget something it never has to do.
                    self.skip_inline_image_data()?;
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
    /// `EI` is recognised only when it stands alone — preceded by white space and followed by
    /// white space, a delimiter or the end of the stream — because the two bytes occur inside
    /// image data constantly. That is the same rule every PDF reader uses and it is a heuristic
    /// in all of them; when it finds no `EI` at all this refuses rather than treating the
    /// remainder as data.
    fn skip_inline_image_data(&mut self) -> Result<()> {
        // One byte of white space after `ID` belongs to the operator, not the data.
        if self.peek().is_some_and(is_whitespace) {
            self.at += 1;
        }
        let mut at = self.at;
        while at + 1 < self.bytes.len() {
            let two = self.bytes.get(at..at + 2);
            if two == Some(b"EI") {
                let before_is_space = at
                    .checked_sub(1)
                    .and_then(|before| self.bytes.get(before).copied())
                    .is_some_and(is_whitespace);
                let after_ends_it = self
                    .bytes
                    .get(at + 2)
                    .copied()
                    .is_none_or(|b| is_whitespace(b) || is_delimiter(b));
                if before_is_space && after_ends_it {
                    self.at = at + 2;
                    return Ok(());
                }
            }
            at += 1;
        }
        Err(Error::Malformed(
            "pdf syntax: an inline image with no 'EI' to end it".to_owned(),
        ))
    }
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
    use super::{Lexer, Token, hex_value, is_whitespace};

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
    fn inline_image_data_is_skipped_rather_than_tokenised() {
        // The binary payload contains `(`, `/` and `<`, every one of which would derail a
        // tokeniser that walked into it -- and `/F9` after them would be read as a used name
        // while `/F2` after the image would be lost.
        let stream = b"BI /W 2 /H 2 ID \x00(/F9<<\xff\xfe EI Q /F2";
        assert_eq!(names(stream), ["W", "H", "F2"]);
    }

    #[test]
    fn an_inline_image_with_no_ei_is_refused() {
        let mut lexer = Lexer::new(b"ID \x00\x01\x02");
        assert!(lexer.next_token().is_err());
    }

    #[test]
    fn ei_inside_image_data_does_not_end_it_unless_it_stands_alone() {
        // `xEIy` is data. Only a delimited `EI` ends the image.
        let stream = b"BI ID \x00xEIy\x00 EI /F2";
        assert_eq!(names(stream), ["F2"]);
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

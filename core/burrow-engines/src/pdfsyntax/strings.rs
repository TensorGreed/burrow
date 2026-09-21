//! What a PDF string says, and how to write one back.
//!
//! `pdfsyntax::lexer` deliberately does not carry string values: *"nothing this module answers
//! depends on what a string says, and carrying the bytes would mean copying file content for no
//! reason."* That reason is still good for the two questions `split` asks, and it stops being
//! good for redaction, which is entirely about what a string says.
//!
//! The resolution is that the lexer stays as it is and grows a `Lexer::span`
//! instead. A caller that needs the bytes slices them out and brings them here; a caller that does
//! not pays nothing. So `Token::Str` remains a marker, `names_in_content`
//! still allocates nothing per string, and the decode happens exactly where somebody asked for it.
//!
//! # Decoding refuses rather than guesses, and that is the opposite lean from [`super::names`]
//!
//! `names` over-approximates because a name it invents costs a kept resource and a name it drops
//! costs a page. Here the asymmetry runs the other way: this feeds a redactor deciding **which
//! bytes of a string to remove**, so a value that is wrong in either direction is wrong. Where
//! this cannot account for the bytes it is given, the answer is [`Error::Malformed`] — never the
//! prefix it managed to decode.
//!
//! **Exactly three things are refused**, and an earlier version of this paragraph claimed a
//! fourth. A token that is not a complete `(…)` or `<…>`; a trailing `\` or an unbalanced `)`
//! inside one; and a byte in a `<…>` string that is not a hex digit and not white space. An
//! **unknown escape is not refused** — PDF 32000-1 says "the backslash is ignored" before any
//! other character, so `\d` decodes to `d`, and the specification stating the answer is the
//! difference between reading it and guessing.
//!
//! What it is NOT is a text decoder. It returns the string's **bytes**, not characters: turning
//! those into text needs the font's `/Encoding`, its `/ToUnicode` CMap, or the document's
//! `/Differences` array, none of which this module can see. Spike 0006's channels 4, 5, 6 and 21
//! are exactly that gap, and ADR 0029 §1 decides what redaction does about it.

use burrow_types::{Error, Result};

/// The value of the string token occupying `raw`, delimiters included.
///
/// `raw` is what the lexer's `span` delimits after `next_token` returned a `Token::Str` —
/// `(…)` or `<…>`, the brackets part of the slice. Those three are private to `pdfsyntax`, so
/// they are named rather than linked.
///
/// # Errors
///
/// [`Error::Malformed`] if `raw` is not a complete string token, carries a trailing `\` or an
/// unbalanced `)`, or holds a byte in a `<…>` string that is neither a hex digit nor white
/// space. Never a partial value, and never for an unknown escape — see the module header for
/// why that one is decoded rather than refused.
pub fn decode_string(raw: &[u8]) -> Result<Vec<u8>> {
    match raw.first() {
        Some(&b'(') => decode_literal(raw),
        Some(&b'<') => decode_hex(raw),
        _ => Err(Error::Malformed(
            "pdf syntax: a string token that begins with neither '(' nor '<'".to_owned(),
        )),
    }
}

/// `(…)`, per PDF 32000-1 §7.3.4.2.
fn decode_literal(raw: &[u8]) -> Result<Vec<u8>> {
    let inner = raw
        .len()
        .checked_sub(1)
        .filter(|_| raw.last() == Some(&b')'))
        .and_then(|end| raw.get(1..end))
        .ok_or_else(|| {
            Error::Malformed("pdf syntax: a '(' string token with no closing ')'".to_owned())
        })?;

    let mut out = Vec::with_capacity(inner.len());
    let mut at = 0usize;
    // Balanced inner parentheses need no escape and are part of the value. The lexer already
    // found the matching `)`, so the depth here only decides which bytes are kept.
    let mut depth = 1usize;
    while let Some(&byte) = inner.get(at) {
        at += 1;
        match byte {
            b'\\' => {
                let Some(&escaped) = inner.get(at) else {
                    // A trailing backslash inside the token. The lexer consumes the byte after
                    // a backslash uninterpreted, so this shape does not reach here from a whole
                    // stream -- it reaches here from a caller that sliced badly, and guessing
                    // would hand back a value one byte short of the truth.
                    return Err(Error::Malformed(
                        "pdf syntax: a '\\' at the end of a string".to_owned(),
                    ));
                };
                at += 1;
                match escaped {
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'b' => out.push(0x08),
                    b'f' => out.push(0x0c),
                    b'(' => out.push(b'('),
                    b')' => out.push(b')'),
                    b'\\' => out.push(b'\\'),
                    // A backslash before an end-of-line is a line continuation: the string
                    // carries neither the backslash nor the break. `\r\n` counts once.
                    b'\n' => {}
                    b'\r' => {
                        if inner.get(at) == Some(&b'\n') {
                            at += 1;
                        }
                    }
                    b'0'..=b'7' => {
                        // One to three octal digits, high bits beyond a byte discarded per the
                        // specification. `\8` is not an escape and is handled by the arm below.
                        let mut value = u32::from(escaped - b'0');
                        for _ in 0..2 {
                            match inner.get(at) {
                                Some(&digit @ b'0'..=b'7') => {
                                    value = value * 8 + u32::from(digit - b'0');
                                    at += 1;
                                }
                                _ => break,
                            }
                        }
                        let truncated = u8::try_from(value & 0xff).map_err(|_| {
                            Error::Internal("pdf syntax: a masked octal left a byte".to_owned())
                        })?;
                        out.push(truncated);
                    }
                    // PDF 32000-1: "the backslash is ignored" before any other character. This
                    // is the one place the module does not refuse, because the specification
                    // states the answer rather than leaving it open.
                    other => out.push(other),
                }
            }
            b'(' => {
                depth += 1;
                out.push(byte);
            }
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Err(Error::Malformed(
                        "pdf syntax: a ')' inside what was given as one string token".to_owned(),
                    ));
                }
                out.push(byte);
            }
            // An end-of-line inside a literal string is a LINE FEED whatever it was written as,
            // so a CRLF is one byte of value and not two. A redactor that got this wrong would
            // compute a length one longer than the string it is replacing.
            b'\r' => {
                if inner.get(at) == Some(&b'\n') {
                    at += 1;
                }
                out.push(b'\n');
            }
            _ => out.push(byte),
        }
    }
    if depth == 1 {
        Ok(out)
    } else {
        Err(Error::Malformed(
            "pdf syntax: a '(' string with unbalanced parentheses".to_owned(),
        ))
    }
}

/// `<…>`, per PDF 32000-1 §7.3.4.3.
fn decode_hex(raw: &[u8]) -> Result<Vec<u8>> {
    let inner = raw
        .len()
        .checked_sub(1)
        .filter(|_| raw.last() == Some(&b'>'))
        .and_then(|end| raw.get(1..end))
        .ok_or_else(|| {
            Error::Malformed("pdf syntax: a '<' string token with no closing '>'".to_owned())
        })?;

    let mut out = Vec::with_capacity(inner.len().div_ceil(2));
    let mut high: Option<u8> = None;
    for &byte in inner {
        if byte.is_ascii_whitespace() || byte == b'\0' {
            continue;
        }
        let nibble = hex_nibble(byte).ok_or_else(|| {
            Error::Malformed("pdf syntax: a '<' string holding a byte that is not hex".to_owned())
        })?;
        match high.take() {
            Some(first) => out.push((first << 4) | nibble),
            None => high = Some(nibble),
        }
    }
    // An odd number of digits is padded with a trailing zero -- the specification says so, so it
    // is not a guess. Identity-H text is two bytes per glyph and an odd count means a truncated
    // document, but the answer is still defined and a caller that wants to reject it can.
    if let Some(first) = high {
        out.push(first << 4);
    }
    Ok(out)
}

const fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// `value` as a `(…)` literal string token, delimiters included.
///
/// Round-trips with [`decode_string`] for every input, which is the property the rewriter needs:
/// a string it replaces must read back as what it meant to write.
///
/// Escapes the three characters that must be escaped and nothing else, except that a byte which
/// would end a line is written as `\n` or `\r` so the token cannot be split across lines by a
/// later tool.
pub fn encode_literal(value: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(value.len() + 2);
    out.push(b'(');
    for &byte in value {
        match byte {
            b'(' | b')' | b'\\' => {
                out.push(b'\\');
                out.push(byte);
            }
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            _ => out.push(byte),
        }
    }
    out.push(b')');
    out
}

#[cfg(test)]
mod tests {
    use super::{decode_string, encode_literal};

    fn decoded(raw: &[u8]) -> Vec<u8> {
        decode_string(raw).expect("decodes")
    }

    #[test]
    fn a_plain_literal_is_its_own_bytes() {
        assert_eq!(decoded(b"(BURROW-SECRET)"), b"BURROW-SECRET");
        assert_eq!(decoded(b"()"), b"");
    }

    #[test]
    fn the_three_escapes_a_producer_actually_writes() {
        assert_eq!(decoded(br"(a\(b\)c)"), b"a(b)c");
        assert_eq!(decoded(br"(a\\b)"), br"a\b");
    }

    #[test]
    fn balanced_parentheses_need_no_escape_and_stay_in_the_value() {
        assert_eq!(decoded(b"(a(b)c)"), b"a(b)c");
    }

    #[test]
    fn octal_escapes_take_one_to_three_digits() {
        assert_eq!(decoded(br"(\101\10\1)"), b"A\x08\x01");
        // Stops at the first non-octal digit, so `\0608` is `0` then a literal `8`.
        assert_eq!(decoded(br"(\0608)"), b"08");
    }

    #[test]
    fn an_octal_above_a_byte_is_truncated_rather_than_refused() {
        // PDF 32000-1 states the answer -- "high-order overflow shall be ignored" -- so this is
        // the specification's rule rather than this module guessing.
        assert_eq!(decoded(br"(\400)"), b"\x00");
    }

    #[test]
    fn a_backslash_before_an_end_of_line_is_a_continuation_and_carries_nothing() {
        assert_eq!(decoded(b"(one\\\ntwo)"), b"onetwo");
        assert_eq!(decoded(b"(one\\\r\ntwo)"), b"onetwo");
    }

    #[test]
    fn a_raw_end_of_line_is_one_line_feed_however_it_was_written() {
        // A CRLF is ONE byte of value. A redactor computing a replacement length from the raw
        // span would be one byte out on every line of a multi-line field.
        assert_eq!(decoded(b"(one\r\ntwo)"), b"one\ntwo");
        assert_eq!(decoded(b"(one\rtwo)"), b"one\ntwo");
        assert_eq!(decoded(b"(one\ntwo)"), b"one\ntwo");
    }

    #[test]
    fn an_unknown_escape_drops_the_backslash_because_the_specification_says_so() {
        assert_eq!(decoded(br"(\q)"), b"q");
    }

    #[test]
    fn hex_strings_decode_in_pairs_and_ignore_white_space() {
        assert_eq!(decoded(b"<48656C6C6F>"), b"Hello");
        assert_eq!(decoded(b"<48 65\n6c 6C 6f>"), b"Hello");
        assert_eq!(decoded(b"<>"), b"");
    }

    #[test]
    fn an_odd_hex_digit_count_is_padded_with_a_zero() {
        assert_eq!(decoded(b"<4>"), b"\x40");
        // `41` then a lone `4`, padded to `40` -- NOT a repeat of the previous byte.
        assert_eq!(decoded(b"<414>"), b"A\x40");
    }

    #[test]
    fn a_hex_string_holding_a_byte_that_is_not_hex_is_refused() {
        // The opposite lean from `names`: a value that is wrong in EITHER direction is wrong,
        // so there is no harmless way to skip the byte.
        assert!(decode_string(b"<41G1>").is_err());
    }

    #[test]
    fn a_token_that_is_not_a_string_is_refused_rather_than_read_as_one() {
        assert!(decode_string(b"BT").is_err());
        assert!(decode_string(b"").is_err());
        assert!(decode_string(b"(unterminated").is_err());
        assert!(decode_string(b"<unterminated").is_err());
    }

    #[test]
    fn encoding_round_trips_through_decoding() {
        for value in [
            &b""[..],
            b"plain",
            b"a(b)c",
            br"back\slash",
            b"line\nbreak",
            b"carriage\rreturn",
            &[0x00, 0xff, 0x80, 0x7f],
        ] {
            let encoded = encode_literal(value);
            assert_eq!(
                decode_string(&encoded).expect("its own output decodes"),
                value,
                "round trip failed for {value:?} encoded as {encoded:?}"
            );
        }
    }

    #[test]
    fn the_encoder_escapes_exactly_what_must_be_escaped() {
        assert_eq!(encode_literal(b"a(b)c\\"), br"(a\(b\)c\\)");
        assert_eq!(encode_literal(b"plain"), b"(plain)");
    }
}

//! The top-level keys of an unparsed dictionary.
//!
//! The input is what `qpdf_oh_unparse` produces for a dictionary: `<< /Type /Page /Parent 2 0 R
//! /Contents 5 0 R >>`. It is **shallow** — an indirect reference unparses as `N G R` rather than
//! expanding — so this reads a page dictionary, not the document behind it.
//!
//! # Why the engine cannot answer this instead
//!
//! qpdf has `qpdf_oh_begin_dict_key_iter`, and burrow may not call it: its body is
//! `qpdf->cur_iter_dict_keys = do_with_oh<…>(…)`, and the pair that walks the result —
//! `qpdf_oh_dict_more_keys` and `qpdf_oh_dict_next_key` — reach `trap_errors` not at all. None of
//! the three is on `engines/qpdf-trapped-functions.txt` (ADR 0013 §1). `qpdf_oh_unparse` **is**
//! trapped, so the keys come back as syntax and are read here, in Rust, with `forbid(unsafe_code)`
//! and a fuzz target.
//!
//! # What it is for: an allowlist over keys, not a denylist
//!
//! `split`'s pruning keeps the page keys a page legitimately has and removes the rest. That is only
//! possible if the rest can be **named**, which means enumerating what is there. A denylist — remove
//! `/B`, remove `/AA`, remove `/Thumb` — is a list of keys somebody thought of, over a structure
//! whose whole design is that a dictionary may contain keys nobody enumerated. ADR 0019's
//! *Alternatives considered* rejects carve for exactly that shape; taking it here would be the same
//! mistake one level down.

use burrow_types::{Error, Result};

use super::lexer::{Lexer, MAX_NESTING, Token};

/// The most keys one dictionary may have before this refuses.
///
/// As with [`super::names::MAX_NAMES`], a refusal rather than a truncation — and here the
/// direction is the opposite one and worse. A truncated key list means keys this never saw are
/// never removed, so a page would keep `/B` or `/AA` because the reader gave up before reaching
/// it. A leak, silently, out of a check that reported success.
pub const MAX_KEYS: usize = 4_096;

/// The top-level keys of the dictionary `unparsed` begins with, in the order they appear.
///
/// Duplicates are returned as they appear rather than deduplicated: a dictionary with the same key
/// twice is malformed, and the caller removing a key twice is harmless where a caller silently
/// seeing one of them is not.
///
/// # Errors
///
/// - [`Error::Malformed`] — `unparsed` does not begin with `<<`, ends before its `>>`, has a
///   non-name where a key belongs, or nests deeper than the engine's own `parser_max_nesting`.
/// - [`Error::Unsupported`] — more than [`MAX_KEYS`] keys.
pub fn top_level_keys(unparsed: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut lexer = Lexer::new(unparsed);
    match lexer.next_token()? {
        Some(Token::DictOpen) => {}
        _ => {
            return Err(Error::Malformed(
                "pdf syntax: asked for the keys of something that is not a dictionary".to_owned(),
            ));
        }
    }

    let mut keys = Vec::new();
    loop {
        let Some(token) = lexer.next_token()? else {
            return Err(Error::Malformed(
                "pdf syntax: a dictionary that is never closed".to_owned(),
            ));
        };
        match token {
            Token::DictClose => return Ok(keys),
            Token::Name(key) => {
                keys.push(key);
                if keys.len() > MAX_KEYS {
                    return Err(Error::Unsupported(
                        "a dictionary with more keys than burrow will read".to_owned(),
                    ));
                }
                // EXACTLY ONE VALUE, so the next name really is the next key. Alternating
                // key/value and calling every other name a key is the obvious version and it is
                // wrong the moment a value is `1 0 R` -- three tokens -- or `[/a /b]`, whose
                // members would be read as keys and removed from the page.
                skip_one_object(&mut lexer)?;
            }
            _ => {
                return Err(Error::Malformed(
                    "pdf syntax: a dictionary key that is not a name".to_owned(),
                ));
            }
        }
    }
}

/// Advance past exactly one object.
///
/// `N G R` is consumed as one object, which is what makes the caller's "the next name is the next
/// key" true. Reading the reference as three objects would leave the cursor mid-value and the
/// generation number would be read as a key.
fn skip_one_object(lexer: &mut Lexer<'_>) -> Result<()> {
    let Some(first) = lexer.next_token()? else {
        return Err(Error::Malformed(
            "pdf syntax: a dictionary key with no value".to_owned(),
        ));
    };
    match first {
        Token::DictOpen => skip_container(lexer, Token::DictOpen, Token::DictClose),
        Token::ArrayOpen => skip_container(lexer, Token::ArrayOpen, Token::ArrayClose),
        Token::Number => {
            // MAY be `N G R` and may be a plain number. The reference is the only three-token
            // value in PDF, and it is recognised by looking ahead rather than by guessing:
            // `/Count 5 /Kids …` and `/Contents 5 0 R` both start `Name Number`.
            let mut ahead = lexer.clone();
            match ahead.next_token()? {
                Some(Token::Number) => match ahead.next_token()? {
                    Some(Token::Keyword(word)) if word == b"R" => {
                        *lexer = ahead;
                        Ok(())
                    }
                    _ => Ok(()),
                },
                _ => Ok(()),
            }
        }
        Token::DictClose | Token::ArrayClose => Err(Error::Malformed(
            "pdf syntax: a dictionary key whose value is a closing delimiter".to_owned(),
        )),
        Token::Name(_) | Token::Str | Token::Keyword(_) | Token::Brace => Ok(()),
    }
}

/// Advance past the rest of a container whose opening token has been consumed.
fn skip_container(lexer: &mut Lexer<'_>, open: Token, close: Token) -> Result<()> {
    let mut depth = 1usize;
    loop {
        let Some(token) = lexer.next_token()? else {
            return Err(Error::Malformed(
                "pdf syntax: a container that is never closed".to_owned(),
            ));
        };
        if token == open {
            depth += 1;
            if depth > MAX_NESTING {
                return Err(Error::Malformed(
                    "pdf syntax: nesting deeper than the engine's own limit".to_owned(),
                ));
            }
        } else if token == close {
            depth -= 1;
            if depth == 0 {
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_KEYS, top_level_keys};

    fn keys(unparsed: &[u8]) -> Vec<String> {
        top_level_keys(unparsed)
            .expect("reads")
            .into_iter()
            .map(|k| String::from_utf8_lossy(&k).into_owned())
            .collect()
    }

    #[test]
    fn it_reads_a_page_dictionary_as_qpdf_unparses_one() {
        let page = b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 5 0 R \
                     /Resources << /Font << /F1 6 0 R >> >> /Rotate 90 /B [7 0 R] >>";
        assert_eq!(
            keys(page),
            [
                "Type",
                "Parent",
                "MediaBox",
                "Contents",
                "Resources",
                "Rotate",
                "B"
            ]
        );
    }

    #[test]
    fn an_indirect_reference_does_not_turn_its_generation_into_a_key() {
        // The failure this is written against: reading `2 0 R` as three objects leaves the
        // cursor on `R`, and everything after shifts by one -- so `/MediaBox` is read as a
        // value and its array members as keys. The page then keeps `/B` and loses `/Contents`.
        assert_eq!(keys(b"<< /Parent 2 0 R /Type /Page >>"), ["Parent", "Type"]);
    }

    #[test]
    fn a_plain_number_value_is_one_object_even_when_a_name_follows_it() {
        assert_eq!(
            keys(b"<< /Count 5 /Kids [1 0 R] /Type /Pages >>"),
            ["Count", "Kids", "Type"]
        );
    }

    #[test]
    fn two_numbers_that_are_not_a_reference_do_not_consume_the_next_key() {
        // `/A 1` then `/B 2` -- the look-ahead sees `Number` and must NOT swallow it.
        assert_eq!(keys(b"<< /A 1 /B 2 >>"), ["A", "B"]);
        // And an array of two numbers followed by a key.
        assert_eq!(keys(b"<< /A [1 2] /B 3 >>"), ["A", "B"]);
    }

    #[test]
    fn nested_containers_do_not_contribute_keys() {
        let nested = b"<< /Outer << /Inner << /Deep 1 >> /Also [<< /Hidden 2 >>] >> /Next 3 >>";
        assert_eq!(keys(nested), ["Outer", "Next"]);
    }

    #[test]
    fn a_name_inside_an_array_value_is_not_a_key() {
        // `/Filter [/FlateDecode /DCTDecode]` -- reading those as keys would remove `/Filter`'s
        // neighbours from the page.
        assert_eq!(
            keys(b"<< /Filter [/FlateDecode /DCTDecode] /Length 12 >>"),
            ["Filter", "Length"]
        );
    }

    #[test]
    fn a_string_value_does_not_contribute_keys() {
        assert_eq!(keys(b"<< /T (/NotAKey) /V <2F41> /Q 0 >>"), ["T", "V", "Q"]);
    }

    #[test]
    fn something_that_is_not_a_dictionary_is_refused() {
        for bad in [&b"[1 2 3]"[..], b"/Name", b"", b"7 0 R"] {
            assert!(
                top_level_keys(bad).is_err(),
                "{:?} was read as a dictionary",
                String::from_utf8_lossy(bad)
            );
        }
    }

    #[test]
    fn an_unclosed_dictionary_is_refused_rather_than_returning_what_it_had() {
        // THE DIRECTION THAT LEAKS. A short key list means keys nobody saw are never removed,
        // and the page keeps them -- out of a check that reported success.
        assert!(top_level_keys(b"<< /A 1 /B 2").is_err());
        assert!(top_level_keys(b"<< /A << /B 1 >>").is_err());
    }

    #[test]
    fn a_key_that_is_not_a_name_is_refused() {
        assert!(top_level_keys(b"<< 1 2 >>").is_err());
        assert!(top_level_keys(b"<< /A 1 (key) 2 >>").is_err());
    }

    #[test]
    fn a_key_with_no_value_is_refused() {
        assert!(top_level_keys(b"<< /A >>").is_err());
    }

    #[test]
    fn too_many_keys_is_a_refusal_rather_than_a_truncated_list() {
        let mut dict = b"<<".to_vec();
        for n in 0..=MAX_KEYS {
            dict.extend_from_slice(format!(" /K{n} {n}").as_bytes());
        }
        dict.extend_from_slice(b" >>");
        assert!(top_level_keys(&dict).is_err());
    }

    #[test]
    fn nesting_past_the_engines_own_limit_is_refused() {
        let mut dict = b"<< /A ".to_vec();
        dict.extend(std::iter::repeat_n(b'[', 200));
        dict.extend(std::iter::repeat_n(b']', 200));
        dict.extend_from_slice(b" >>");
        assert!(top_level_keys(&dict).is_err());
    }
}

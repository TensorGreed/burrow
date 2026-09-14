//! Every resource name a byte sequence mentions.
//!
//! The input is a **decoded** content stream — a page's contents, a Form XObject's body, a tiling
//! pattern's, a Type 3 glyph procedure's, an annotation appearance stream's. The output is the set
//! of names that appear anywhere in it.
//!
//! # It over-approximates on purpose, and the asymmetry is the whole design
//!
//! A name is collected because it *appears*, not because it appears as an operand of an operator
//! that takes a resource. `/F1 /F2 /F3 Tf` collects three names where only one is a font, and a
//! name used as an operand to `gs`, `Do`, `scn`, `sh`, `Tf`, `ri`, `BDC`, `BMC`, `DP` or `MP` is
//! collected the same way.
//!
//! That is not laziness about writing an operator table. The set is used to **filter** a resource
//! dictionary: a name in the set keeps its resource, a name absent removes it. So the two errors
//! are not equal —
//!
//! | error | effect |
//! |---|---|
//! | a name collected that is not a resource operand | one entry kept that could have gone. A **larger output**, and nothing else. |
//! | a name missed that is a resource operand | the font, image or colour space the page draws with is **deleted**. The page renders wrong, and it still opens. |
//!
//! An operator table is a list of operators somebody thought of, and PDF 32000-1 plus its
//! extensions is not a closed set. Every operator nobody listed would take the second row. Not
//! having a table means there is nothing to be incomplete.
//!
//! **What it costs, stated rather than left to be discovered:** a resource only ever mentioned in
//! a comment, in text drawn on the page, or as a name operand to something that is not a resource
//! lookup, survives pruning. It is a fidelity cost — a larger file — not a leak, *unless* the
//! resource itself is the excluded page's data. That residue is real, and it is why the structural
//! closure harness rather than this function is the gate on ADR 0019 §2.

use std::collections::BTreeSet;

use burrow_types::{Error, Result};

use super::lexer::{Lexer, Token};

/// The most distinct names one stream may mention.
///
/// A real page names tens; a generated one might name thousands. Past this, the answer is a
/// refusal rather than a truncated set: a set that quietly stopped growing is an under-
/// approximation, which is the row of the table above that breaks a page.
///
/// It bounds allocation too. The decoded stream is already capped by qpdf's `flate_max_memory`
/// (256 MiB, `crate::qpdf::limits`), and 256 MiB of `/a /b /c …` is some eighty million distinct
/// names — a set several gigabytes wide, built out of an input nothing else in this path would
/// have let through.
pub const MAX_NAMES: usize = 65_536;

/// The longest name this will record.
///
/// PDF 32000-1 §7.3.5 sets an implementation limit of 127 bytes. A longer one is recorded up to
/// this bound and anything past it is a refusal, for the same reason a truncated set is: two
/// names sharing a 255-byte prefix would collapse into one and the second one's resource would be
/// removed.
pub const MAX_NAME_LENGTH: usize = 255;

/// Every name mentioned anywhere in `content`.
///
/// # Errors
///
/// - [`Error::Malformed`] — the bytes could not be tokenised. A partial
///   name set may not escape — it would delete a resource the page draws with — so a lexing
///   failure is returned rather than the names collected so far. The module header has the
///   asymmetry in full.
/// - [`Error::Unsupported`] — more than [`MAX_NAMES`] distinct names, or one longer than
///   [`MAX_NAME_LENGTH`]. Both are refusals rather than truncations.
pub fn names_in_content(content: &[u8]) -> Result<BTreeSet<Vec<u8>>> {
    let mut found = BTreeSet::new();
    let mut lexer = Lexer::new(content);
    // Inline images need no handling here: `Lexer::next_token` skips an image's binary data
    // itself, so `ID` arrives as an ordinary keyword with the cursor already past `EI`. See its
    // comment for why that is not the caller's job.
    while let Some(token) = lexer.next_token()? {
        if let Token::Name(name) = token {
            if name.len() > MAX_NAME_LENGTH {
                return Err(Error::Unsupported(
                    "a content stream names a resource longer than burrow will record".to_owned(),
                ));
            }
            found.insert(name);
            if found.len() > MAX_NAMES {
                return Err(Error::Unsupported(
                    "a content stream names more distinct resources than burrow will record"
                        .to_owned(),
                ));
            }
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::{MAX_NAME_LENGTH, MAX_NAMES, names_in_content};

    fn set(content: &[u8]) -> Vec<String> {
        names_in_content(content)
            .expect("collects")
            .into_iter()
            .map(|n| String::from_utf8_lossy(&n).into_owned())
            .collect()
    }

    #[test]
    fn it_collects_the_names_a_page_actually_draws_with() {
        let content = b"q /GS0 gs /Cs6 cs BT /F1 12 Tf (hello) Tj ET /Im3 Do Q";
        assert_eq!(set(content), ["Cs6", "F1", "GS0", "Im3"]);
    }

    #[test]
    fn a_name_inside_drawn_text_is_collected_too_and_that_is_the_safe_direction() {
        // `(/F9)` is text on the page, not a resource. Collecting it keeps a resource that
        // could have gone; the module header's table is why that is the error to make. The
        // LEXER is what stops the string's contents becoming a name -- this asserts the
        // over-approximation is bounded to the operand, not the string.
        assert_eq!(set(b"BT /F1 12 Tf (/F9) Tj ET"), ["F1"]);
    }

    #[test]
    fn marked_content_property_names_are_collected() {
        // `/OC /MC0 BDC` looks up `MC0` in the page's `/Properties`. A filter that missed it
        // would delete the entry and leave a `BDC` naming nothing -- which, for an optional
        // content group, changes what is visible.
        assert_eq!(set(b"/OC /MC0 BDC /F1 Tf EMC"), ["F1", "MC0", "OC"]);
    }

    #[test]
    fn an_inline_image_does_not_hide_the_names_after_it() {
        let content = b"BI /W 1 /H 1 /CS /G ID \x00(/F9 <</a 1>> EI Q /F2 12 Tf";
        assert_eq!(set(content), ["CS", "F2", "G", "H", "W"]);
    }

    #[test]
    fn a_lexing_failure_returns_an_error_rather_than_the_names_it_already_had() {
        // THE RULE THIS MODULE EXISTS TO KEEP. A partial set removes resources that are used.
        let result = names_in_content(b"/F1 12 Tf (unterminated /F2");
        assert!(
            result.is_err(),
            "a truncated scan must not look like a complete one"
        );
    }

    #[test]
    fn too_many_distinct_names_is_a_refusal_rather_than_a_truncated_set() {
        let mut content = Vec::new();
        for n in 0..=MAX_NAMES {
            content.extend_from_slice(format!("/N{n} ").as_bytes());
        }
        let result = names_in_content(&content);
        assert!(result.is_err(), "the set silently stopped growing");
    }

    #[test]
    fn an_over_long_name_is_a_refusal_rather_than_a_truncated_one() {
        let mut content = b"/".to_vec();
        content.extend(std::iter::repeat_n(b'a', MAX_NAME_LENGTH + 1));
        assert!(names_in_content(&content).is_err());
        // And the boundary itself is accepted, so the refusal is a ceiling rather than a wall
        // one byte lower than it says.
        let mut content = b"/".to_vec();
        content.extend(std::iter::repeat_n(b'a', MAX_NAME_LENGTH));
        assert_eq!(names_in_content(&content).expect("at the ceiling").len(), 1);
    }

    #[test]
    fn the_empty_stream_names_nothing_and_is_not_an_error() {
        assert!(names_in_content(b"").expect("empty").is_empty());
        assert!(
            names_in_content(b"   \n% only a comment\n")
                .expect("trivia")
                .is_empty()
        );
    }
}

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
//! # That argument is about THIS answer, and it inverts for a rewriter
//!
//! [`super::ops`] does associate operands with their operator, which reads like a contradiction
//! of the paragraph above and is not one. The argument here is not *operator tables are bad*; it
//! is *this function over-approximates, so an incomplete table could only ever make its answer
//! narrower, and a narrower answer is the one that breaks a page*. A rewriter has the opposite
//! asymmetry — an operator it does not understand is output it gets **wrong** — so the same
//! reasoning lands on the other side.
//!
//! `ops` resolves it by grouping operands **positionally** and still holding no table: every
//! operand precedes its operator and an operator ends the run, which is a property of PDF's
//! postfix syntax rather than of anyone's list. A caller that needs to know what an operator
//! *means* keeps that table itself and owns being incomplete about it, where it can refuse.
//! ADR 0029's *What this changes for the operation*, item 3.
//!
//! **What it costs, stated rather than left to be discovered:** a resource only ever mentioned in
//! a comment, in text drawn on the page, or as a name operand to something that is not a resource
//! lookup, survives pruning. It is a fidelity cost — a larger file — not a leak, *unless* the
//! resource itself is the excluded page's data. That residue is real, and it is why the structural
//! closure harness rather than this function is the gate on ADR 0019 §2.

use std::collections::BTreeSet;

use burrow_types::{Error, Result};

use super::lexer::{Lexer, Token};

/// The raw-token byte limit a name is refused past, re-exported from the lexer so the one
/// boundary has a public home. The lexer's `read_name` enforces it; this module, its fuzz target,
/// and `ops::MAX_OPERAND_BYTES` all name the same constant so it cannot drift into two 255s.
pub use super::lexer::MAX_TOKEN_BYTES;

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

/// What a content stream names, and whether it was read to the end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentNames {
    /// Every name the stream mentions, up to where it was read.
    pub names: BTreeSet<Vec<u8>>,
    /// False where lexing stopped at a filtered inline image, whose extent the renderer finds by
    /// decoding and this module cannot (#228): the names past it are unread, so `names` is a
    /// **partial** set, which the caller may not prune by. ADR 0019's #228 amendment says what
    /// `split` does instead.
    pub read_to_end: bool,
}

/// Every name mentioned anywhere in `content`, and whether all of it was read.
///
/// # Errors
///
/// - [`Error::Malformed`] — the bytes could not be tokenised. A partial
///   name set may not escape — it would delete a resource the page draws with — so a lexing
///   failure is returned rather than the names collected so far. The module header has the
///   asymmetry in full.
/// - [`Error::Unsupported`] — more than [`MAX_NAMES`] distinct names. A name longer than a
///   renderer reads is refused one layer down, by the lexer's raw-token limit
///   ([`MAX_TOKEN_BYTES`]), so it surfaces here as the lexer's `Err`. Both are
///   refusals rather than truncations.
pub fn names_in_content(content: &[u8]) -> Result<ContentNames> {
    let mut found = BTreeSet::new();
    let mut lexer = Lexer::new(content, super::lexer::InlineImages::Prune);
    // Inline images are the lexer's job: `Lexer::next_token` skips an unfiltered image's data
    // itself, so `ID` arrives as an ordinary keyword with the cursor already past `EI`. A
    // filtered one, under `Prune`, ends the read instead, and `extent_unknown` says so below.
    while let Some(token) = lexer.next_token()? {
        if let Token::Name(name) = token {
            // A name longer than a renderer reads is refused by the lexer (`read_name`, measuring
            // the RAW token against `MAX_TOKEN_BYTES`), so `next_token()?` above has already
            // returned `Err` for one -- it never reaches here. The check used to live here and
            // measured the DECODED length, which read a 300-byte escape name as 100 and let it
            // through; moving it to the lexer gives every name consumer (these content operands,
            // and the resource/page dict keys) the same raw boundary. See #257.
            found.insert(name);
            if found.len() > MAX_NAMES {
                return Err(Error::Unsupported(
                    "a content stream names more distinct resources than burrow will record"
                        .to_owned(),
                ));
            }
        }
    }
    Ok(ContentNames {
        names: found,
        read_to_end: !lexer.extent_unknown(),
    })
}

#[cfg(test)]
mod tests {
    use super::super::lexer::MAX_TOKEN_BYTES;
    use super::{MAX_NAMES, names_in_content};

    fn set(content: &[u8]) -> Vec<String> {
        let read = names_in_content(content).expect("collects");
        assert!(read.read_to_end, "read to the end");
        read.names
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
        // `/W 14 /H 1 /BPC 8 /CS /G` declares the fourteen bytes between `ID ` and ` EI`, so the
        // extent is the dictionary's. `/F9` inside the data stays data.
        let content = b"BI /W 14 /H 1 /BPC 8 /CS /G ID \x00(/F9 <</a 1>> EI Q /F2 12 Tf";
        assert_eq!(set(content), ["BPC", "CS", "F2", "G", "H", "W"]);
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
        // RAW token length, the `/` counted (`MAX_TOKEN_BYTES`). A name of 255 bytes is a 256-byte
        // raw token -- one past the cut -- so it is refused; 254 bytes is a 255-byte token, exactly
        // the ceiling, and is accepted. This is the #257 boundary (PDFium keeps 255 raw incl `/`),
        // and the ceiling is a ceiling, not a wall one byte low.
        let mut content = b"/".to_vec();
        content.extend(std::iter::repeat_n(b'a', MAX_TOKEN_BYTES)); // raw = 1 + 255 = 256
        assert!(names_in_content(&content).is_err());
        let mut content = b"/".to_vec();
        content.extend(std::iter::repeat_n(b'a', MAX_TOKEN_BYTES - 1)); // raw = 1 + 254 = 255
        assert_eq!(
            names_in_content(&content)
                .expect("at the ceiling")
                .names
                .len(),
            1
        );
    }

    #[test]
    fn a_name_long_in_raw_escapes_is_refused_even_though_it_decodes_short() {
        // THE #257 CASE. `/` + 100 x `#41` is 301 raw bytes but decodes to "A" x 100 -- the old
        // decoded-length check read it as 100 and let it through, and PDFium cut it to `A{84}#4`,
        // naming a different resource split's walk never found. The raw check refuses it.
        let mut content = b"/".to_vec();
        for _ in 0..100 {
            content.extend_from_slice(b"#41");
        }
        content.extend_from_slice(b" Do");
        assert!(
            names_in_content(&content).is_err(),
            "a name 301 raw bytes long must refuse, not pass as its 100-byte decoding"
        );
    }

    #[test]
    fn the_empty_stream_names_nothing_and_is_not_an_error() {
        assert!(names_in_content(b"").expect("empty").names.is_empty());
        assert!(
            names_in_content(b"   \n% only a comment\n")
                .expect("trivia")
                .names
                .is_empty()
        );
    }

    #[test]
    fn a_filtered_inline_image_stops_the_read_and_says_so() {
        // #228: PDFium ends a filtered inline image at its filter's own end of data, which this
        // module cannot find. The names before it are read; the read is reported partial, so a
        // caller cannot prune by it.
        let read =
            names_in_content(b"/F1 12 Tf BI /W 1 /H 1 /F /Fl /L 9 ID xxxxxxxxx EI /F2 12 Tf")
                .expect("not an error under Prune");
        assert!(!read.read_to_end, "a partial read must say it is partial");
        assert!(
            read.names.contains(b"F1".as_slice()),
            "what came before is read"
        );
        assert!(
            !read.names.contains(b"F2".as_slice()),
            "nothing after it is claimed"
        );
    }
}

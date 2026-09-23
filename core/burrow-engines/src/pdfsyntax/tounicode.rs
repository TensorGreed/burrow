//! Reading and re-writing a `/ToUnicode` CMap, so a redaction can narrow one instead of
//! deleting it.
//!
//! # Why this exists
//!
//! `/ToUnicode` is the plain text of the page, beside the page, keyed by character code. A
//! redaction that leaves it intact has removed the glyphs and kept the sentence.
//!
//! The first implementation therefore removed the whole stream, with a comment arguing that
//! what it said about the *kept* codes was already in the font's own `cmap`. **That was wrong,
//! and the end-to-end test measured it**: with `/ToUnicode` gone, PDFium returned `U+0001` at
//! the origin where readable text had been. A simple font's built-in `cmap` maps glyph
//! selectors, not Unicode, and for a subset encoding there is nothing to fall back to. So the
//! conservative-looking edit corrupted the text the redaction was meant to keep — a correctness
//! failure of the kind ADR 0029 §1 already ruled out in words: remove the entries for codes the
//! document no longer draws, not the stream.
//!
//! # What it does not attempt
//!
//! `bfrange` is expanded into individual entries rather than preserved as ranges, and the
//! emitted program is written from scratch rather than patched. Both trade output size for a
//! shape that can be reasoned about: a patched program would need the original's byte offsets
//! to stay valid across a removal in the middle of a range, which is the same class of
//! positional edit `/Widths` is not allowed to make.
//!
//! The `/CMapName`, `/Registry` and `/Ordering` of the original are **not** carried over. They
//! are producer metadata, and a subset font's CMap name routinely embeds the subset tag, which
//! is a per-document identifier this operation has no reason to reproduce.

use std::collections::BTreeMap;

use burrow_types::{Error, Result};

use super::lexer::{Lexer, Token};
use super::strings::decode_string;

/// The most code-to-text entries one CMap may carry.
///
/// A `/ToUnicode` for a subset font holds one entry per code, so 65,536 is every code a
/// two-byte encoding can express. Past that the program is generated rather than written, and
/// the expansion of a `bfrange` is where an adversarial one would buy the work: a single
/// `<0000> <ffff> <0041>` line is four entries of source text.
pub const MAX_ENTRIES: usize = 65_536;

/// The most `begincodespacerange` pairs one CMap may declare.
const MAX_CODESPACES: usize = 256;

/// A `/ToUnicode` CMap, read.
#[derive(Debug, Default)]
pub struct ToUnicode {
    /// The declared code ranges, as their raw hex bytes, so the narrowed program can re-declare
    /// exactly what the original did. The byte width of the source codes lives here and nowhere
    /// else, and getting it wrong re-frames every code in the stream.
    codespace: Vec<(Vec<u8>, Vec<u8>)>,
    /// Code to its UTF-16BE destination bytes, with the byte width the source was written at.
    map: BTreeMap<u32, (usize, Vec<u8>)>,
}

impl ToUnicode {
    /// How many entries the CMap carries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether the CMap maps nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Whether the CMap maps `code`.
    #[must_use]
    pub fn maps(&self, code: u32) -> bool {
        self.map.contains_key(&code)
    }

    /// Read a CMap program.
    ///
    /// # Errors
    ///
    /// - [`Error::Malformed`] — the program could not be tokenised, or a `bfrange` runs
    ///   backwards.
    /// - [`Error::Unsupported`] — more than [`MAX_ENTRIES`] entries or [`MAX_CODESPACES`]
    ///   code ranges. A refusal rather than a truncation: a truncated CMap is one that still
    ///   maps the secret and no longer maps something else.
    pub fn parse(program: &[u8]) -> Result<Self> {
        let mut lexer = Lexer::new(program);
        let mut read = Self::default();
        // Operands accumulate until a keyword, exactly as a content stream's do. A CMap is
        // PostScript rather than a content stream, so `ops::operations` is not reused: its
        // per-operator cap is sixty-four, and a `bfchar` section is a hundred entries by
        // convention, which is two hundred operands.
        let mut pending: Vec<Item> = Vec::new();
        while let Some(token) = lexer.next_token()? {
            let span = lexer.span();
            match token {
                Token::Str => {
                    let value = decode_string(program.get(span.0..span.1).ok_or_else(|| {
                        Error::Internal("a token span outside the CMap program".to_owned())
                    })?)?;
                    let width = span.1.saturating_sub(span.0).saturating_sub(2);
                    pending.push(Item::Str {
                        value,
                        // `<0041>` is four hex digits: two source bytes. The decoded length is
                        // the same number, but only for an even count, so the raw width is what
                        // is carried.
                        digits: width,
                    });
                }
                Token::ArrayOpen => pending.push(Item::ArrayOpen),
                Token::ArrayClose => collapse_array(&mut pending)?,
                Token::Keyword(word) => {
                    read.take(&word, &pending)?;
                    pending.clear();
                }
                // Numbers, names, dictionaries and braces are the CMap's scaffolding. None of
                // them carries a mapping, and none may be mistaken for one, so they drop the
                // operand run rather than joining it.
                _ => pending.clear(),
            }
            if pending.len() > MAX_ENTRIES {
                return Err(Error::Unsupported(
                    "a /ToUnicode CMap holds a section longer than burrow will read".to_owned(),
                ));
            }
        }
        Ok(read)
    }

    /// Consume one keyword's operand run.
    fn take(&mut self, word: &[u8], pending: &[Item]) -> Result<()> {
        match word {
            b"endcodespacerange" => {
                for [low, high] in pending.as_chunks::<2>().0 {
                    let (Item::Str { value: lo, .. }, Item::Str { value: hi, .. }) = (low, high)
                    else {
                        continue;
                    };
                    if self.codespace.len() >= MAX_CODESPACES {
                        return Err(Error::Unsupported(
                            "a /ToUnicode CMap declares more code ranges than burrow will read"
                                .to_owned(),
                        ));
                    }
                    self.codespace.push((lo.clone(), hi.clone()));
                }
                Ok(())
            }
            b"endbfchar" => {
                for [source, destination] in pending.as_chunks::<2>().0 {
                    let (Item::Str { value: src, digits }, Item::Str { value: dst, .. }) =
                        (source, destination)
                    else {
                        continue;
                    };
                    self.insert(code_of(src), *digits, dst.clone())?;
                }
                Ok(())
            }
            b"endbfrange" => self.take_ranges(pending),
            _ => Ok(()),
        }
    }

    /// `<lo> <hi> <dst>` and `<lo> <hi> [<d0> <d1> …]`, expanded.
    fn take_ranges(&mut self, pending: &[Item]) -> Result<()> {
        for [low, high, destination] in pending.as_chunks::<3>().0 {
            let (Item::Str { value: lo, digits }, Item::Str { value: hi, .. }) = (low, high) else {
                continue;
            };
            let (first, last) = (code_of(lo), code_of(hi));
            if last < first {
                return Err(Error::Malformed(
                    "pdf syntax: a /ToUnicode bfrange whose last code precedes its first"
                        .to_owned(),
                ));
            }
            match destination {
                Item::Str { value: dst, .. } => {
                    for (step, code) in (first..=last).enumerate() {
                        self.insert(code, *digits, incremented(dst, step)?)?;
                    }
                }
                Item::Array(items) => {
                    for (step, code) in (first..=last).enumerate() {
                        let Some(dst) = items.get(step) else {
                            // A short array maps the codes it covers and no more. Inventing a
                            // destination for the rest would put text in the output that the
                            // input never said.
                            break;
                        };
                        self.insert(code, *digits, dst.clone())?;
                    }
                }
                Item::ArrayOpen => {}
            }
        }
        Ok(())
    }

    /// Record one mapping, refusing past [`MAX_ENTRIES`].
    fn insert(&mut self, code: u32, digits: usize, destination: Vec<u8>) -> Result<()> {
        if self.map.len() >= MAX_ENTRIES && !self.map.contains_key(&code) {
            return Err(Error::Unsupported(
                "a /ToUnicode CMap maps more codes than burrow will read".to_owned(),
            ));
        }
        self.map.insert(code, (digits, destination));
        Ok(())
    }

    /// The program that maps exactly the codes in `keep`, and nothing else.
    ///
    /// Returns `None` when nothing is left to map: a CMap with no `bfchar` section is not a
    /// CMap, and the caller removes the key instead — at which point no code is drawn through
    /// it, so nothing readable is lost.
    #[must_use]
    pub fn narrowed(&self, keep: &dyn Fn(u32) -> bool) -> Option<Vec<u8>> {
        let kept: Vec<(&u32, &(usize, Vec<u8>))> =
            self.map.iter().filter(|(code, _)| keep(**code)).collect();
        if kept.is_empty() {
            return None;
        }
        let mut out = Vec::new();
        out.extend_from_slice(
            b"/CIDInit /ProcSet findresource begin\n\
              12 dict begin\n\
              begincmap\n\
              /CMapType 2 def\n",
        );
        if !self.codespace.is_empty() {
            out.extend_from_slice(
                format!("{} begincodespacerange\n", self.codespace.len()).as_bytes(),
            );
            for (lo, hi) in &self.codespace {
                out.push(b'<');
                push_hex(&mut out, lo);
                out.extend_from_slice(b"> <");
                push_hex(&mut out, hi);
                out.extend_from_slice(b">\n");
            }
            out.extend_from_slice(b"endcodespacerange\n");
        }
        // A hundred per section: PDF 32000-1 §9.10.3 sets that as the limit for one `bfchar`,
        // and a reader that enforces it would stop at the hundred-and-first entry -- which for
        // a redaction is text going missing from a document that was supposed to keep it.
        for chunk in kept.chunks(100) {
            out.extend_from_slice(format!("{} beginbfchar\n", chunk.len()).as_bytes());
            for (code, (digits, destination)) in chunk {
                out.push(b'<');
                push_code(&mut out, **code, *digits);
                out.extend_from_slice(b"> <");
                push_hex(&mut out, destination);
                out.extend_from_slice(b">\n");
            }
            out.extend_from_slice(b"endbfchar\n");
        }
        out.extend_from_slice(
            b"endcmap\n\
              CMapName currentdict /CMap defineresource pop\n\
              end\n\
              end\n",
        );
        Some(out)
    }
}

/// One operand in a CMap section.
#[derive(Debug)]
enum Item {
    /// A decoded string, with the number of hex digits it was written as.
    Str { value: Vec<u8>, digits: usize },
    /// The array under construction; collapsed at `]`.
    ArrayOpen,
    /// A closed `[…]` of destination strings.
    Array(Vec<Vec<u8>>),
}

/// Collapse everything back to the last [`Item::ArrayOpen`] into one [`Item::Array`].
fn collapse_array(pending: &mut Vec<Item>) -> Result<()> {
    let Some(at) = pending
        .iter()
        .rposition(|item| matches!(item, Item::ArrayOpen))
    else {
        return Err(Error::Malformed(
            "pdf syntax: a /ToUnicode CMap with a ']' that closes nothing".to_owned(),
        ));
    };
    let items = pending
        .split_off(at)
        .into_iter()
        .filter_map(|item| match item {
            Item::Str { value, .. } => Some(value),
            _ => None,
        })
        .collect();
    pending.push(Item::Array(items));
    Ok(())
}

/// A source code's integer value, big-endian over its bytes.
///
/// Saturating rather than wrapping: a code written at more than four bytes is outside every
/// encoding burrow reads, and folding it onto a small value would map a code the document does
/// draw onto text it does not say.
fn code_of(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .take(4)
        .fold(0u32, |value, byte| (value << 8) | u32::from(*byte))
}

/// A destination advanced by `step`, which is what a `bfrange` with a single destination means.
///
/// The increment applies to the **last** UTF-16 code unit, per PDF 32000-1 §9.10.3. A carry out
/// of it is a refusal rather than a wrap: `<FFFF>` plus one is not `<0000>`, and a wrapped
/// value is a different character silently substituted.
fn incremented(destination: &[u8], step: usize) -> Result<Vec<u8>> {
    if step == 0 || destination.len() < 2 {
        return Ok(destination.to_vec());
    }
    let mut out = destination.to_vec();
    let at = out.len().saturating_sub(2);
    let tail = out
        .get(at..)
        .and_then(|bytes| <[u8; 2]>::try_from(bytes).ok());
    let Some(tail) = tail else {
        return Ok(out);
    };
    let last = u32::from(u16::from_be_bytes(tail));
    let step = u32::try_from(step).map_err(|_| {
        Error::Unsupported("a /ToUnicode bfrange longer than burrow will expand".to_owned())
    })?;
    let value = u16::try_from(last.saturating_add(step)).map_err(|_| {
        Error::Malformed(
            "pdf syntax: a /ToUnicode bfrange whose destination runs past U+FFFF".to_owned(),
        )
    })?;
    if let Some(slot) = out.get_mut(at..) {
        slot.copy_from_slice(&value.to_be_bytes());
    }
    Ok(out)
}

/// Append `bytes` as uppercase hex.
fn push_hex(out: &mut Vec<u8>, bytes: &[u8]) {
    for byte in bytes {
        out.extend_from_slice(format!("{byte:02X}").as_bytes());
    }
}

/// Append `code` as `digits` hex digits, rounded up to a whole number of bytes.
fn push_code(out: &mut Vec<u8>, code: u32, digits: usize) {
    let width = digits.clamp(2, 8);
    let text = format!("{code:0width$X}");
    out.extend_from_slice(text.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::ToUnicode;

    /// What a producer writes for a four-glyph subset.
    const SUBSET: &[u8] = b"/CIDInit /ProcSet findresource begin\n\
        12 dict begin\n\
        begincmap\n\
        /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
        /CMapName /ABCDEF+Helvetica-UCS def\n\
        /CMapType 2 def\n\
        1 begincodespacerange\n<00> <FF>\nendcodespacerange\n\
        4 beginbfchar\n\
        <01> <0053>\n<02> <0065>\n<03> <0063>\n<04> <0072>\n\
        endbfchar\n\
        endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n";

    fn text_for(program: &[u8], code: u32) -> Option<Vec<u8>> {
        let read = ToUnicode::parse(program).expect("a CMap burrow wrote");
        read.map.get(&code).map(|(_, text)| text.clone())
    }

    #[test]
    fn a_producers_bfchar_section_reads_back_entry_for_entry() {
        let read = ToUnicode::parse(SUBSET).expect("parses");
        assert_eq!(read.len(), 4);
        assert_eq!(
            read.map.get(&1).map(|(_, t)| t.clone()),
            Some(vec![0x00, 0x53])
        );
        assert_eq!(
            read.map.get(&4).map(|(_, t)| t.clone()),
            Some(vec![0x00, 0x72])
        );
        assert_eq!(read.codespace.len(), 1);
    }

    #[test]
    fn narrowing_keeps_the_text_of_the_codes_that_stay_and_drops_the_rest() {
        // The defect this module exists for: dropping the whole stream took the kept codes'
        // text with it, and PDFium returned U+0001 where readable text had been. So the
        // assertion is on BOTH halves -- the removed code is gone AND the kept ones still say
        // what they said.
        let read = ToUnicode::parse(SUBSET).expect("parses");
        let narrowed = read
            .narrowed(&|code| code == 1 || code == 4)
            .expect("two codes remain");
        assert_eq!(text_for(&narrowed, 1), Some(vec![0x00, 0x53]));
        assert_eq!(text_for(&narrowed, 4), Some(vec![0x00, 0x72]));
        assert_eq!(text_for(&narrowed, 2), None);
        assert_eq!(text_for(&narrowed, 3), None);
    }

    #[test]
    fn the_narrowed_program_does_not_carry_the_removed_text_in_any_form() {
        // The byte scan the leak harness runs, applied here: `0065` is code 2's destination and
        // must not appear at all, not merely fail to be reachable through a mapping.
        let read = ToUnicode::parse(SUBSET).expect("parses");
        let narrowed = read.narrowed(&|code| code == 1).expect("one code remains");
        assert!(
            !narrowed.windows(4).any(|window| window == b"0065"),
            "the removed code's destination survived the narrowing"
        );
    }

    #[test]
    fn a_bfrange_with_one_destination_increments_the_last_code_unit() {
        let program = b"1 beginbfrange\n<20> <23> <0041>\nendbfrange\n";
        let read = ToUnicode::parse(program).expect("parses");
        assert_eq!(read.len(), 4);
        assert_eq!(
            read.map.get(&0x20).map(|(_, t)| t.clone()),
            Some(vec![0, 0x41])
        );
        assert_eq!(
            read.map.get(&0x23).map(|(_, t)| t.clone()),
            Some(vec![0, 0x44])
        );
    }

    #[test]
    fn a_bfrange_with_an_array_takes_each_destination_in_turn() {
        let program = b"1 beginbfrange\n<10> <12> [<0041> <00C6> <0042>]\nendbfrange\n";
        let read = ToUnicode::parse(program).expect("parses");
        assert_eq!(
            read.map.get(&0x11).map(|(_, t)| t.clone()),
            Some(vec![0x00, 0xC6])
        );
        assert_eq!(read.len(), 3);
    }

    #[test]
    fn a_short_array_maps_what_it_covers_and_invents_nothing() {
        let program = b"1 beginbfrange\n<10> <14> [<0041>]\nendbfrange\n";
        let read = ToUnicode::parse(program).expect("parses");
        assert_eq!(
            read.len(),
            1,
            "four codes had no destination written for them"
        );
    }

    #[test]
    fn a_two_byte_code_is_re_emitted_at_two_bytes() {
        // The width lives in the source spelling and nowhere else. Re-emitting `<0041>` as
        // `<41>` re-frames every following code in a two-byte encoding.
        let program = b"1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n\
                        1 beginbfchar\n<0041> <0061>\nendbfchar\n";
        let read = ToUnicode::parse(program).expect("parses");
        let narrowed = read.narrowed(&|_| true).expect("kept");
        assert!(
            narrowed.windows(6).any(|window| window == b"<0041>"),
            "got: {}",
            String::from_utf8_lossy(&narrowed)
        );
        assert_eq!(text_for(&narrowed, 0x41), Some(vec![0x00, 0x61]));
    }

    #[test]
    fn narrowing_everything_away_yields_no_program_rather_than_an_empty_one() {
        let read = ToUnicode::parse(SUBSET).expect("parses");
        assert!(read.narrowed(&|_| false).is_none());
    }

    #[test]
    fn re_mapping_a_code_already_mapped_is_not_counted_as_growth() {
        let program = b"1 beginbfrange\n<00> <FF> <0000>\nendbfrange\n\
                        1 beginbfrange\n<00> <FF> <0100>\nendbfrange\n";
        let read = ToUnicode::parse(program).expect("parses");
        assert_eq!(read.len(), 256, "the second range re-maps rather than adds");
        assert_eq!(
            read.map.get(&0).map(|(_, t)| t.clone()),
            Some(vec![0x01, 0x00]),
            "the later section wins, as a reader taking them in order would have it"
        );
    }

    #[test]
    fn more_codes_than_the_cap_is_a_refusal_rather_than_a_truncated_map() {
        // A truncated CMap is the worst of both: it still maps the secret and has silently
        // stopped mapping something else. Three-byte codes, because a single `bfrange` wide
        // enough to pass the cap with two-byte codes runs its destination past U+FFFF first.
        let program = b"1 beginbfrange\n<000000> <00FFFF> <0000>\nendbfrange\n\
                        1 beginbfrange\n<010000> <0100FF> <0000>\nendbfrange\n";
        let error = ToUnicode::parse(program).expect_err("past MAX_ENTRIES");
        assert!(
            format!("{error}").contains("maps more codes than burrow will read"),
            "got: {error}"
        );
    }

    #[test]
    fn a_destination_running_past_the_last_code_unit_is_a_refusal() {
        let program = b"1 beginbfrange\n<00> <10> <FFFF>\nendbfrange\n";
        assert!(ToUnicode::parse(program).is_err());
    }

    #[test]
    fn a_bfrange_that_runs_backwards_is_a_refusal() {
        let program = b"1 beginbfrange\n<40> <20> <0041>\nendbfrange\n";
        assert!(ToUnicode::parse(program).is_err());
    }

    #[test]
    fn the_subset_tag_in_the_original_cmaps_name_is_not_carried_into_the_output() {
        let read = ToUnicode::parse(SUBSET).expect("parses");
        let narrowed = read.narrowed(&|_| true).expect("kept");
        assert!(
            !narrowed.windows(6).any(|window| window == b"ABCDEF"),
            "the subset tag reached the narrowed program"
        );
    }
}

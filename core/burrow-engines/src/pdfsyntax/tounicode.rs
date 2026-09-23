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

/// The most mappings this will **perform**, as opposed to hold.
///
/// [`MAX_ENTRIES`] bounds the map and bounds nothing about the work. A `bfrange` that re-maps
/// codes already present passes the size check every time, so the same 65,536 inserts can be
/// paid arbitrarily often — and a range is four bytes of source text per 65,536 of them.
///
/// Measured, release build, on `<0000> <FFFF> <0000>` repeated: 47 kB of program took 3.9 s,
/// 188 kB took 15.4 s, 752 kB took 61.8 s. Linear at ~82 µs/kB. That text Flate-compresses
/// **343:1**, so a 30 kB `/ToUnicode` stream in a file decompresses to 10 MB and costs about
/// fourteen minutes inside one engine call — against a `max_duration_ms` documented as
/// tolerating "overshoot of up to one engine call".
///
/// Four times [`MAX_ENTRIES`] leaves room for a CMap that legitimately re-states a range
/// (producers do) and refuses the one that is buying work with it.
const MAX_INSERTS: usize = MAX_ENTRIES * 4;

/// A `/ToUnicode` CMap, read.
#[derive(Debug, Default)]
pub struct ToUnicode {
    /// The declared code ranges, as their raw hex bytes, so the narrowed program can re-declare
    /// exactly what the original did. The byte width of the source codes lives here and nowhere
    /// else, and getting it wrong re-frames every code in the stream.
    codespace: Vec<(Vec<u8>, Vec<u8>)>,
    /// Code to its UTF-16BE destination bytes, with the byte width the source was written at.
    map: BTreeMap<u32, (usize, Vec<u8>)>,
    /// How many mappings have been performed, which is not how many are held. See
    /// [`MAX_INSERTS`].
    inserts: usize,
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
    /// - [`Error::Unsupported`] — more than [`MAX_ENTRIES`] entries, or more code ranges
    ///   than this module will read. A refusal rather than a truncation: a truncated CMap is
    ///   one that still maps the secret and no longer maps something else.
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
                    pending.push(Item::Str {
                        // TWO HEX DIGITS PER DECODED BYTE, not the raw span's width.
                        //
                        // It was the span: `span.1 - span.0 - 2`. White space inside a hex
                        // string is ignored by the decoder and counted by that subtraction, and
                        // **NUL is white space** in PDF 32000-1 §7.2.3. A fuzzer found
                        // `<` + twenty-six NULs + `20>` in its second minute: one decoded byte
                        // reported as twenty-eight digits, so the narrowed program emitted a
                        // fourteen-byte code and burrow then refused to read its own output.
                        //
                        // The decoded length cannot disagree with itself this way, and it is
                        // the quantity the emitter actually wants: how many bytes the code
                        // occupies.
                        digits: value.len().saturating_mul(2),
                        value,
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
                    self.insert(code_of(src)?, *digits, dst.clone())?;
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
            let (first, last) = (code_of(lo)?, code_of(hi)?);
            if last < first {
                return Err(Error::Malformed(
                    "pdf syntax: a /ToUnicode bfrange whose last code precedes its first"
                        .to_owned(),
                ));
            }
            // REFUSED BEFORE IT IS WALKED. Checking inside the loop would still pay for the
            // first `MAX_INSERTS` of a range declaring four billion codes.
            let span = u64::from(last - first).saturating_add(1);
            if span > u64::try_from(MAX_INSERTS).unwrap_or(u64::MAX) {
                return Err(Error::Unsupported(
                    "a /ToUnicode bfrange wider than burrow will expand".to_owned(),
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
        // THE WORK, COUNTED BEFORE THE SIZE. A re-map costs the same as a new mapping and the
        // size check does not see it; see `MAX_INSERTS` for the measurement.
        self.inserts = self.inserts.saturating_add(1);
        if self.inserts > MAX_INSERTS {
            return Err(Error::Unsupported(
                "a /ToUnicode CMap asks for more mappings than burrow will perform".to_owned(),
            ));
        }
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
/// # A code wider than four bytes is refused, and the comment here used to say otherwise
///
/// It read *"saturating rather than wrapping: … folding it onto a small value would map a code
/// the document does draw onto text it does not say"*, over a body that did `take(4)` — which
/// keeps the **first** four bytes and discards the rest, so `<0000000041>` came out as `0`.
/// That is the folding the comment said it avoided, and two distinct long codes collided onto
/// one key.
///
/// No encoding burrow reads uses a code wider than four bytes, so the honest answer is to
/// refuse rather than to pick one.
fn code_of(bytes: &[u8]) -> Result<u32> {
    if bytes.len() > 4 {
        return Err(Error::Unsupported(
            "a /ToUnicode CMap with a character code wider than burrow will read".to_owned(),
        ));
    }
    Ok(bytes
        .iter()
        .fold(0u32, |value, byte| (value << 8) | u32::from(*byte)))
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

/// Append `code` as hex, at a **whole number of bytes** and never narrower than its source.
///
/// # An odd number of hex digits is a different code
///
/// This was `format!("{code:0width$X}")` with `width` the source's digit count clamped to
/// `2..=8`, and `width` is a *minimum*: code 272 came out as `<110>`. PDF 32000-1 §7.3.4.3
/// pads a final odd hex digit with zero, so a reader takes `<110>` as the bytes `0x11 0x00` —
/// code **4352**, which the input never mapped.
///
/// Found by this module's own fuzz target inside sixty seconds, on
/// `1 beginbfrange <00> <10FF ><FF> endbfrange`, through the claim that burrow must be able to
/// read back what it writes. It is the exact defect that claim exists for, and no unit test
/// here had reached it: every hand-written fixture used codes below 256.
fn push_code(out: &mut Vec<u8>, code: u32, digits: usize) {
    let natural = format!("{code:X}").len();
    // At least the source's width, at least two digits, and always even — a byte is two hex
    // digits and a code is a whole number of bytes.
    let width = natural.max(digits).max(2);
    let width = width + width % 2;
    out.extend_from_slice(format!("{code:0width$X}").as_bytes());
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
    fn a_code_whose_hex_is_an_odd_number_of_digits_is_emitted_as_whole_bytes() {
        // THE FUZZER'S FINDING, reduced to its input. `format!("{code:0width$X}")` took `width`
        // from the SOURCE's digit count, and `width` is a minimum: a range written `<00>` to
        // `<10FF>` has a two-digit source, so code 272 came out as `<110>` -- three digits. A
        // reader pads the final odd digit with zero per PDF 32000-1 §7.3.4.3, so `<110>` is the
        // bytes `0x11 0x00`: code **4352**, which the document never drew.
        //
        // **The first version of this test used a four-digit source and did not reproduce it.**
        // Re-planting the defect left the suite green, which is the whole reason `CLAUDE.md`
        // says to assert a mutation applied before believing what a green run means. The source
        // width is the parameter that matters and the fixture has to vary it.
        let program = b"1 beginbfrange\n<00> <10FF> <0041>\nendbfrange\n";
        let read = ToUnicode::parse(program).expect("parses");
        assert!(read.maps(0x0110), "the fixture maps code 272");
        assert!(!read.maps(0x1100), "and does not map 4352");

        let narrowed = read.narrowed(&|_| true).expect("kept");
        let again = ToUnicode::parse(&narrowed).expect("burrow reads its own output");
        assert!(
            !again.maps(0x1100),
            "an odd digit count re-framed a code: the narrowed program maps 4352, which the \
             original never did"
        );
        assert!(
            again.maps(0x0110),
            "and the code it did map is still mapped"
        );
    }

    #[test]
    fn a_narrowed_program_never_maps_a_code_the_original_did_not() {
        // The general form, over a two-digit source spanning every width boundary: 0xFF is two
        // hex digits, 0x100 is three, 0x1000 is four. The round trip must map exactly the same
        // set across all of them.
        let program = b"1 begincodespacerange\n<00> <FF>\nendcodespacerange\n\
                        1 beginbfrange\n<00> <2000> <0041>\nendbfrange\n";
        let read = ToUnicode::parse(program).expect("parses");
        let narrowed = read.narrowed(&|code| code % 3 == 0).expect("kept");
        let again = ToUnicode::parse(&narrowed).expect("burrow reads its own output");
        for code in 0..=u32::from(u16::MAX) {
            assert_eq!(
                again.maps(code),
                read.maps(code) && code % 3 == 0,
                "code {code} disagrees across the round trip"
            );
        }
    }

    #[test]
    fn white_space_inside_a_hex_code_does_not_widen_what_is_emitted() {
        // THE FUZZER'S SECOND FINDING, in its second minute. The source width was taken from
        // the raw span, and the decoder ignores white space inside `<...>` -- including NUL,
        // which PDF 32000-1 §7.2.3 lists as white space. One decoded byte was reported as
        // twenty-eight digits, the narrowed program emitted a fourteen-byte code, and burrow
        // refused to read its own output.
        //
        // Spaces rather than NULs here because a NUL in a Rust byte-string literal reads as an
        // escape everyone has to decode; the mechanism is the same and `strings.rs` treats both
        // as white space, which the companion assertion below pins.
        let program = b"1 beginbfrange\n<          20> <7E> <0020>\nendbfrange\n";
        let read = ToUnicode::parse(program).expect("parses");
        assert!(read.maps(0x20), "the fixture maps the space");

        let narrowed = read.narrowed(&|_| true).expect("kept");
        let again = ToUnicode::parse(&narrowed).expect("burrow reads its own output");
        assert_eq!(again.len(), read.len(), "the round trip changed the count");
        assert!(again.maps(0x20), "and maps the same codes");
    }

    #[test]
    fn a_nul_inside_a_hex_code_is_white_space_like_any_other() {
        // The companion: the same shape with the byte the fuzzer actually used. If
        // `decode_string` ever stopped treating NUL as white space, the test above would keep
        // passing over a mechanism that had moved.
        let mut program = Vec::from(b"1 beginbfrange\n<".as_slice());
        program.extend(std::iter::repeat_n(0u8, 26));
        program.extend_from_slice(b"20> <7E> <0020>\nendbfrange\n");
        let read = ToUnicode::parse(&program).expect("parses");
        assert!(read.maps(0x20), "NUL is white space inside a hex string");

        let narrowed = read.narrowed(&|_| true).expect("kept");
        let again = ToUnicode::parse(&narrowed).expect("burrow reads its own output");
        assert_eq!(again.len(), read.len());
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
    fn a_cmap_that_buys_work_by_re_mapping_is_refused_rather_than_paid_for() {
        // `MAX_ENTRIES` bounds the MAP and bounds nothing about the WORK: a range that re-maps
        // codes already present passes the size check every time. Measured before this cap: a
        // 752 kB program took 61.8 s, and that text Flate-compresses 343:1, so a 30 kB stream
        // in a file is about fourteen minutes inside one engine call.
        //
        // Eight ranges of 256 codes each is 2,048 mappings, well under the cap; the same eight
        // at full width is 524,288, which is past it. The pair is what makes this a cap rather
        // than a refusal of `bfrange`.
        let narrow: Vec<u8> = "1 beginbfrange\n<00> <FF> <0000>\nendbfrange\n"
            .repeat(8)
            .into_bytes();
        assert_eq!(
            ToUnicode::parse(&narrow).expect("eight small ranges").len(),
            256
        );

        let wide: Vec<u8> = "1 beginbfrange\n<000000> <00FFFF> <0000>\nendbfrange\n"
            .repeat(8)
            .into_bytes();
        let error = ToUnicode::parse(&wide).expect_err("past MAX_INSERTS");
        assert!(
            format!("{error}").contains("more mappings than burrow will perform"),
            "got: {error}"
        );
    }

    #[test]
    fn a_single_range_wider_than_the_budget_is_refused_before_it_is_walked() {
        // Checking inside the loop would still pay for the first `MAX_INSERTS` of a range
        // declaring four billion codes. Four-byte codes, so the span is declarable.
        let program = b"1 beginbfrange\n<00000000> <FFFFFFFF> <0000>\nendbfrange\n";
        let started = std::time::Instant::now();
        let error = ToUnicode::parse(program).expect_err("a range wider than the budget");
        assert!(
            format!("{error}").contains("wider than burrow will expand"),
            "got: {error}"
        );
        // THE ASSERTION IS THE CLOCK, because a refusal reached after doing the work is not the
        // thing this cap is for.
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "the refusal took {:?}; it is being reached after the expansion rather than before",
            started.elapsed()
        );
    }

    #[test]
    fn a_character_code_wider_than_four_bytes_is_refused_rather_than_truncated() {
        // `code_of` did `take(4)`, keeping the FIRST four bytes -- so `<0000000041>` came out
        // as 0 and two distinct long codes collided onto one key, which is the folding its own
        // comment said it avoided.
        let program = b"1 beginbfchar\n<0000000041> <0041>\nendbfchar\n";
        let error = ToUnicode::parse(program).expect_err("a five-byte code");
        assert!(
            format!("{error}").contains("wider than burrow will read"),
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

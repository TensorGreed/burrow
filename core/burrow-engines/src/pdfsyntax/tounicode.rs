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

/// The most destinations one code may be given before burrow stops rewriting the CMap.
///
/// A CMap may name the same code twice, and burrow keeps **every** destination for a kept code
/// so that narrowing cannot change what that code decodes to — see [`ToUnicode::insert`]. That
/// makes what the map holds grow with the duplicates rather than with the distinct codes, so it
/// needs a ceiling of its own: without one the bound would be [`MAX_INSERTS`], four times
/// [`MAX_ENTRIES`], all of it reachable for a single code.
///
/// Eight, because a producer re-stating a code a handful of times is a thing that happens and a
/// producer stating it nine times is not. Past it the CMap is refused rather than rewritten
/// lossily, which is the direction that cannot silently alter text.
const MAX_DESTINATIONS_PER_CODE: usize = 8;

/// The most destination bytes one CMap may hold, across every code and every duplicate.
///
/// # The count was bounded and the bytes were not
///
/// [`MAX_ENTRIES`], [`MAX_INSERTS`] and [`MAX_DESTINATIONS_PER_CODE`] all bound *how many*
/// mappings there are. None of them bounds how long one destination is, and a `bfrange` clones
/// its destination once per code — so `<0000> <FFFF> <…5,000 UTF-16 units…>` is 65,536 copies
/// of 10 kB from one line of source text.
///
/// Measured by a security review, release build: a program of four such ranges is 80 kB, which
/// Flate-compresses to **161 bytes**, and drove `redact::page` to a peak RSS of **2,525 MB**
/// with a `narrowed()` output of 5.25 GB — returning `Ok`, with `Limits::DEFAULT`'s 1 GiB
/// `max_memory_bytes` never firing, from a 1,038-byte PDF. An OOM kill of the tab with no
/// signal at all.
///
/// Four mebibytes, because a real `/ToUnicode` destination is one to four UTF-16 units and a
/// legitimate CMap mapping every one of [`MAX_ENTRIES`] codes to four of them is 512 kB. This
/// leaves eight times that and refuses the program that is buying memory with a range.
const MAX_DESTINATION_BYTES: usize = 4 << 20;

/// A `/ToUnicode` CMap, read.
#[derive(Debug, Default)]
pub struct ToUnicode {
    /// The declared code ranges, as their raw hex bytes, so the narrowed program can re-declare
    /// exactly what the original did. The byte width of the source codes lives here and nowhere
    /// else, and getting it wrong re-frames every code in the stream.
    codespace: Vec<(Vec<u8>, Vec<u8>)>,
    /// Code to its UTF-16BE destinations, each with the byte width its source was written at.
    ///
    /// **Every** destination, in the order the program gave them, not the last one to arrive.
    /// [`ToUnicode::insert`] has the measurement that made this a list.
    map: BTreeMap<u32, Vec<(usize, Vec<u8>)>>,
    /// How many mappings have been performed, which is not how many are held. See
    /// [`MAX_INSERTS`].
    inserts: usize,
    /// How many destination bytes are held, against `MAX_DESTINATION_BYTES`.
    destination_bytes: usize,
}

impl ToUnicode {
    /// How many **codes** the CMap maps.
    ///
    /// Not how many entries it carries: a code named twice is one code. The distinction is the
    /// subject of `MAX_DESTINATIONS_PER_CODE`, which is private.
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
        // EVERY DESTINATION IS KEPT, IN ORDER, and the alternative was measured rather than
        // reasoned about. A `/ToUnicode` CMap may name the same code more than once —
        // `21-cid-lying-tounicode.pdf` names `<0003>` three times and `<0008>` twice — and
        // §9.10.3 does not say which one a consumer takes.
        //
        // This was `insert`, so the last one won, and narrowing such a CMap changed what two
        // **kept** codes decoded to: `-` became `L` at two origins the region never reached.
        // Replacing it with first-wins moved the damage rather than removing it — three other
        // codes changed instead. PDFium's answers on that fixture are not one rule: `<0003>`
        // and `<0006>` read as their last entry, `<0008>` as its first.
        //
        // So burrow does not pick. It keeps the whole sequence for a code and re-emits it in
        // the same order, and whatever rule the reader applies it applies to the same input.
        // Fidelity by construction beats matching a guess at someone else's precedence — and a
        // redaction silently rewriting text it was asked to leave alone is the failure
        // `narrow_to_unicode`'s own doc comment is about, one level further in: keeping the
        // entry is not enough, the entry has to still mean what it meant.
        let destinations = self.map.entry(code).or_default();
        if destinations.len() >= MAX_DESTINATIONS_PER_CODE {
            return Err(Error::Unsupported(
                "a /ToUnicode CMap gives one code more destinations than burrow will carry"
                    .to_owned(),
            ));
        }
        // THE BYTES, NOT JUST THE COUNT. See `MAX_DESTINATION_BYTES`: every other ceiling here
        // bounds how many mappings there are, and a `bfrange` buys memory with how long one is.
        self.destination_bytes = self.destination_bytes.saturating_add(destination.len());
        if self.destination_bytes > MAX_DESTINATION_BYTES {
            return Err(Error::Unsupported(
                "a /ToUnicode CMap holds more destination text than burrow will carry".to_owned(),
            ));
        }
        destinations.push((digits, destination));
        Ok(())
    }

    /// The program that maps exactly the codes in `keep`, and nothing else.
    ///
    /// Returns `None` when nothing is left to map: a CMap with no `bfchar` section is not a
    /// CMap, and the caller removes the key instead — at which point no code is drawn through
    /// it, so nothing readable is lost.
    #[must_use]
    pub fn narrowed(&self, keep: &dyn Fn(u32) -> bool) -> Option<Vec<u8>> {
        // FLATTENED, so a code named twice is written twice. `chunk`ing the codes instead
        // would put a code's own entries in one section and is not what changes here; what
        // changes is that both entries survive at all.
        let kept: Vec<(&u32, &(usize, Vec<u8>))> = self
            .map
            .iter()
            .filter(|(code, _)| keep(**code))
            .flat_map(|(code, destinations)| destinations.iter().map(move |entry| (code, entry)))
            .collect();
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
    /// The one destination `code` maps to, asserting there is exactly one.
    ///
    /// The map holds a **list** per code so a CMap naming a code twice survives narrowing
    /// unchanged. Every case below is written against a CMap that names each code once, so
    /// taking the last would hide a duplicate these tests never meant to create; this refuses
    /// instead.
    fn sole_destination(read: &super::ToUnicode, code: u32) -> Option<Vec<u8>> {
        let destinations = read.map.get(&code)?;
        assert_eq!(
            destinations.len(),
            1,
            "code {code} has {} destinations; this fixture states it once",
            destinations.len()
        );
        destinations.first().map(|(_, text)| text.clone())
    }

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
        sole_destination(&read, code)
    }

    #[test]
    fn a_producers_bfchar_section_reads_back_entry_for_entry() {
        let read = ToUnicode::parse(SUBSET).expect("parses");
        assert_eq!(read.len(), 4);
        assert_eq!(sole_destination(&read, 1), Some(vec![0x00, 0x53]));
        assert_eq!(sole_destination(&read, 4), Some(vec![0x00, 0x72]));
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
        assert_eq!(sole_destination(&read, 0x20), Some(vec![0, 0x41]));
        assert_eq!(sole_destination(&read, 0x23), Some(vec![0, 0x44]));
    }

    #[test]
    fn a_bfrange_with_an_array_takes_each_destination_in_turn() {
        let program = b"1 beginbfrange\n<10> <12> [<0041> <00C6> <0042>]\nendbfrange\n";
        let read = ToUnicode::parse(program).expect("parses");
        assert_eq!(sole_destination(&read, 0x11), Some(vec![0x00, 0xC6]));
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
        assert_eq!(
            read.len(),
            256,
            "the second range re-maps rather than adds codes"
        );
        // BOTH DESTINATIONS, IN ORDER. `len` counts codes, and a re-mapped code is still one
        // code -- which is what this test was written to pin. What it must *also* pin now is
        // that the re-mapping did not throw the first destination away: burrow does not decide
        // which of them a reader takes, it re-emits the sequence and lets the reader decide.
        assert_eq!(
            read.map.get(&0).map(|destinations| destinations
                .iter()
                .map(|(_, text)| text.clone())
                .collect::<Vec<_>>()),
            Some(vec![vec![0x00, 0x00], vec![0x01, 0x00]]),
            "both destinations survive, in the order the program gave them"
        );
    }

    #[test]
    fn a_code_named_twice_is_narrowed_to_both_of_its_destinations() {
        // THE FIDELITY CASE, from `21-cid-lying-tounicode.pdf`. Narrowing must not decide
        // which duplicate wins, because burrow and PDFium do not agree on one rule and the
        // fixture proves there is not one to agree on: it reads `<0003>` as its last entry and
        // `<0008>` as its first. Emitting one entry per code changed what a kept code decoded
        // to -- text the region never reached.
        let program = b"3 beginbfchar
<0008> <002D>
<0009> <0046>
<0008> <004C>
endbfchar
";
        let read = ToUnicode::parse(program).expect("parses");
        let narrowed = read.narrowed(&|code| code == 0x0008).expect("keeps a code");
        let text = String::from_utf8_lossy(&narrowed);
        assert!(
            text.contains("<0008> <002D>") && text.contains("<0008> <004C>"),
            "both destinations for the kept code must be re-emitted: {text}"
        );
        assert!(
            !text.contains("<0009>"),
            "the removed code must not survive: {text}"
        );
        assert!(
            text.contains("2 beginbfchar"),
            "the section must count the entries it writes, not the codes: {text}"
        );
    }

    #[test]
    fn a_range_buying_memory_with_a_long_destination_is_refused() {
        use super::MAX_DESTINATION_BYTES;

        // THE 161-BYTE FILE. A security review measured this shape at 2,525 MB peak RSS and a
        // 5.25 GB narrowed output, returning `Ok` under the default ceilings. The per-code and
        // per-entry ceilings all passed it: it is 65,536 codes with one destination each, and
        // the whole cost is in how long that destination is.
        let long = "0041".repeat(5_000);
        let program = format!("1 beginbfrange\n<0000> <FFFF> <{long}>\nendbfrange\n");
        let error = ToUnicode::parse(program.as_bytes()).expect_err("past the byte ceiling");
        assert!(
            format!("{error}").contains("more destination text than burrow will carry"),
            "got: {error}"
        );

        // AND THE NEAR-MISS, so the ceiling is not simply "refuse every wide range". The same
        // range with an ordinary one-unit destination maps the whole two-byte space -- the
        // largest legitimate CMap there is -- and must still parse.
        //
        // The destination has to be `<0000>`, and that is a fact about `bfrange` rather than
        // about this ceiling: the destination's last unit is incremented per code, so anything
        // higher runs past U+FFFF over a range this wide and is refused for **that** reason.
        // Two earlier spellings of this near-miss were refused by that rule instead, which
        // would have left this test passing while measuring nothing about the byte ceiling.
        let real = "1 beginbfrange\n<0000> <FFFF> <0000>\nendbfrange\n";
        let read = ToUnicode::parse(real.as_bytes()).expect("the whole two-byte space is ordinary");
        assert_eq!(read.len(), 65_536);
        assert!(
            read.destination_bytes <= MAX_DESTINATION_BYTES,
            "{} bytes held, against a ceiling of {MAX_DESTINATION_BYTES}",
            read.destination_bytes
        );
    }

    #[test]
    fn a_code_given_more_destinations_than_the_ceiling_is_a_refusal() {
        // Keeping every duplicate makes what the map holds grow with the duplicates, so the
        // growth needs a ceiling of its own rather than inheriting MAX_INSERTS.
        use super::MAX_DESTINATIONS_PER_CODE;
        let mut program = format!("{} beginbfchar\n", MAX_DESTINATIONS_PER_CODE + 1);
        for at in 0..=MAX_DESTINATIONS_PER_CODE {
            program.push_str(&format!("<0001> <{:04X}>\n", 0x41 + at));
        }
        program.push_str("endbfchar\n");
        let error = ToUnicode::parse(program.as_bytes()).expect_err("past the ceiling");
        assert!(
            format!("{error}").contains("more destinations than burrow will carry"),
            "got: {error}"
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

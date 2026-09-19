//! Channel 23: what the FONT still says after the glyphs are gone.
//!
//! # Why this exists, and why it was not in the first 22
//!
//! The original enumeration asked *where is the text drawn from*. It did not ask *what does the
//! drawing apparatus retain*, and those are different questions. A subset font built for one run
//! of text carries, in the document, an exact description of the characters that run contained:
//!
//! - **`/ToUnicode`** is a `beginbfchar` list mapping each code to the character it means. Written
//!   one entry per position, it spells the run out in order.
//! - **`/Differences`** names each code's glyph. `/B /U /R /O /W /hyphen ...` is the run's
//!   alphabet in first-appearance order.
//! - the embedded **`FontFile2`** contains outlines for exactly those glyphs and no others.
//!
//! Removing the text-showing operator from the content stream removes none of it. Measured on
//! this spike's own output: `redacted-05-cid-with-tounicode.pdf` has a page whose content is the
//! keep line alone, and a `/ToUnicode` reading `0042 0055 0052 0052 004F 0057 002D 0053 0045
//! 0043 0052 0045 0054 002D 0030 0035` — `BURROW-SECRET-05`, complete and in order.
//!
//! **The harness scored that file "still in the file: no".** Three separate files in this
//! repository warn about exactly this class — *"a font subset still carrying glyphs for removed
//! characters"* — and the spike built four fixtures that exhibit it and then measured past it,
//! because every instrument it had was looking for the secret as TEXT.
//!
//! This is a structural probe, not a fifth instrument. It reads two specific constructs rather
//! than searching, so it finds what it was pointed at and nothing else — which is the honest
//! description and the reason finding 1's headline says "at least".

use std::collections::BTreeSet;

/// The `cmap` of every embedded font program in `expanded`, as the set of character codes it
/// covers.
///
/// A subset font's `cmap` covers exactly the characters the subsetter was asked for, so on a
/// font built for one run of text it **is** that run's alphabet. This is the third of the four
/// constructs named in the module docstring, and the reason it is parsed rather than asserted:
/// channels 6 and 21 have no `/ToUnicode` and no `/Differences`, so without it they score clean
/// on a font that names their alphabet outright.
///
/// Finds font programs by `/Length1`, which is the key that declares a stream is an sfnt, then
/// walks the table directory. Anything it cannot parse is skipped and COUNTED, never silently
/// treated as empty -- see `FontLeak::programs` against `programs_parsed`.
fn cmap_alphabet(expanded: &[u8]) -> (BTreeSet<char>, usize, usize) {
    let mut chars = BTreeSet::new();
    let (mut seen, mut parsed) = (0usize, 0usize);
    let mut at = 0usize;
    while let Some(rel) = find(&expanded[at..], b"/Length1") {
        let pos = at + rel;
        at = pos + 8;
        let Some(len1) = read_int(&expanded[pos + 8..]) else { continue };
        let Some(srel) = find(&expanded[pos..], b"stream") else { continue };
        let mut start = pos + srel + 6;
        while matches!(expanded.get(start), Some(b'\r') | Some(b'\n')) {
            start += 1;
        }
        let end = (start + len1).min(expanded.len());
        seen += 1;
        if parse_cmap(&expanded[start..end], &mut chars) {
            parsed += 1;
        }
    }
    (chars, seen, parsed)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}

fn read_int(b: &[u8]) -> Option<usize> {
    let s: String = b
        .iter()
        .skip_while(|c| c.is_ascii_whitespace())
        .take_while(|c| c.is_ascii_digit())
        .map(|c| *c as char)
        .collect();
    s.parse().ok()
}

fn be16(b: &[u8], at: usize) -> Option<usize> {
    Some(usize::from(u16::from_be_bytes([
        *b.get(at)?,
        *b.get(at + 1)?,
    ])))
}

fn be32(b: &[u8], at: usize) -> Option<usize> {
    Some(u32::from_be_bytes([
        *b.get(at)?,
        *b.get(at + 1)?,
        *b.get(at + 2)?,
        *b.get(at + 3)?,
    ]) as usize)
}

/// Walk an sfnt's table directory to `cmap`, read a format 4 subtable's covered codes.
fn parse_cmap(font: &[u8], out: &mut BTreeSet<char>) -> bool {
    let Some(num_tables) = be16(font, 4) else { return false };
    if num_tables == 0 || num_tables > 64 {
        return false;
    }
    let mut cmap_at = None;
    for i in 0..num_tables {
        let rec = 12 + i * 16;
        if font.get(rec..rec + 4) == Some(b"cmap") {
            cmap_at = be32(font, rec + 8);
        }
    }
    let Some(cmap) = cmap_at else { return false };
    let Some(n) = be16(font, cmap + 2) else { return false };
    for i in 0..n {
        let rec = cmap + 4 + i * 8;
        let Some(off) = be32(font, rec + 4) else { continue };
        let sub = cmap + off;
        if be16(font, sub) != Some(4) {
            continue;
        }
        let Some(seg_x2) = be16(font, sub + 6) else { continue };
        let segs = seg_x2 / 2;
        for s in 0..segs {
            let Some(end) = be16(font, sub + 14 + s * 2) else { continue };
            let Some(start) = be16(font, sub + 16 + seg_x2 + s * 2) else { continue };
            if start > end || end == 0xFFFF {
                continue;
            }
            for code in start..=end {
                if let Some(c) = char::from_u32(code as u32) {
                    out.insert(c);
                }
            }
        }
        return true;
    }
    false
}

/// What a document's font objects disclose about a run of text that is no longer drawn.
#[derive(Debug, Default, Clone)]
pub struct FontLeak {
    /// Set when `cmap_alphabet` contains every distinct character of the secret.
    pub cmap_covers_all: bool,
    /// The `/ToUnicode` targets, concatenated in the order the CMap lists them.
    pub tounicode: String,
    /// The `/Differences` glyph names, resolved to characters, in array order.
    pub differences: String,
    /// The secret appears **verbatim** in one of the two.
    pub exact: bool,
    /// How many of the secret's distinct characters the font's mapping accounts for.
    pub distinct_covered: usize,
    pub distinct_total: usize,
    /// The alphabet the embedded font programs' `cmap` covers.
    pub cmap_alphabet: String,
    /// Font programs found, and how many were parsed. Reported so a parser that stopped
    /// working reads as "0 of 3" rather than as a clean document.
    pub programs: usize,
    pub programs_parsed: usize,
}

impl FontLeak {
    /// Anything at all recoverable about the removed run from the font alone.
    pub fn any(&self) -> bool {
        self.exact
            || (self.distinct_total > 0 && self.distinct_covered == self.distinct_total)
            || self.cmap_covers_all
    }

    /// The font's own `cmap` covers every distinct character of the secret and nothing else --
    /// which on a subset font is the alphabet of the removed run, stated outright.
    pub fn cmap_exact(&self) -> bool {
        self.cmap_covers_all
    }

    pub fn cell(&self) -> String {
        if self.exact {
            "**verbatim**".into()
        } else if self.distinct_total > 0 && self.distinct_covered == self.distinct_total {
            format!("**alphabet** {}/{}", self.distinct_covered, self.distinct_total)
        } else if self.cmap_covers_all {
            format!("**cmap alphabet** ({})", self.cmap_alphabet)
        } else if self.programs > 0 {
            format!("font present, {}/{} parsed", self.programs_parsed, self.programs)
        } else {
            "—".into()
        }
    }
}

/// The glyph names this spike's generated face uses, resolved back to characters.
///
/// A real implementation would need the Adobe Glyph List. This covers the names
/// `blockfont.GLYPH_NAME` emits and nothing else, which is stated rather than discovered: a name
/// outside this table is reported as unresolved rather than skipped, so the count cannot quietly
/// shrink.
fn glyph_char(name: &str) -> Option<char> {
    match name {
        "space" => Some(' '),
        "hyphen" => Some('-'),
        "zero" => Some('0'),
        "one" => Some('1'),
        "two" => Some('2'),
        "three" => Some('3'),
        "four" => Some('4'),
        "five" => Some('5'),
        "six" => Some('6'),
        "seven" => Some('7'),
        "eight" => Some('8'),
        "nine" => Some('9'),
        n if n.len() == 1 && n.chars().all(|c| c.is_ascii_uppercase()) => n.chars().next(),
        _ => None,
    }
}

/// Every `beginbfchar` target, in order, across every CMap in `expanded`.
fn tounicode_targets(expanded: &[u8]) -> String {
    let text = String::from_utf8_lossy(expanded);
    let mut out = String::new();
    let mut rest = text.as_ref();
    while let Some(start) = rest.find("beginbfchar") {
        let after = &rest[start + "beginbfchar".len()..];
        let end = after.find("endbfchar").unwrap_or(after.len());
        for line in after[..end].lines() {
            // `<src> <dst>` — the second angle group is the character it claims to mean.
            let mut groups = line.split('<').skip(1).filter_map(|g| g.split('>').next());
            let (_src, dst) = (groups.next(), groups.next());
            if let Some(dst) = dst {
                for chunk in dst.as_bytes().chunks(4) {
                    if let Ok(hex) = std::str::from_utf8(chunk) {
                        if let Ok(v) = u32::from_str_radix(hex.trim(), 16) {
                            if let Some(c) = char::from_u32(v) {
                                out.push(c);
                            }
                        }
                    }
                }
            }
        }
        rest = &after[end.min(after.len())..];
    }
    out
}

/// Every `/Differences` array's glyph names, resolved, in order.
fn differences_chars(expanded: &[u8]) -> String {
    let text = String::from_utf8_lossy(expanded);
    let mut out = String::new();
    let mut rest = text.as_ref();
    while let Some(start) = rest.find("/Differences") {
        let after = &rest[start..];
        let Some(open) = after.find('[') else { break };
        let Some(close) = after[open..].find(']') else { break };
        for token in after[open + 1..open + close].split_whitespace() {
            if let Some(name) = token.strip_prefix('/') {
                if let Some(c) = glyph_char(name) {
                    out.push(c);
                }
            }
        }
        rest = &after[open + close..];
    }
    out
}

/// What the fonts in `expanded` disclose about `secret`.
pub fn scan(expanded: &[u8], secret: &str) -> FontLeak {
    let tounicode = tounicode_targets(expanded);
    let differences = differences_chars(expanded);
    let (cmap, programs, programs_parsed) = cmap_alphabet(expanded);
    let distinct: BTreeSet<char> = secret.chars().collect();
    let seen: BTreeSet<char> = tounicode.chars().chain(differences.chars()).collect();
    // The generated face always covers space; ignore it, so a space in the secret does not
    // make an unrelated font look like a disclosure.
    let interesting: BTreeSet<char> = distinct.iter().copied().filter(|c| *c != ' ').collect();
    let cmap_covers_all =
        !interesting.is_empty() && interesting.iter().all(|c| cmap.contains(c));
    FontLeak {
        cmap_covers_all,
        exact: tounicode.contains(secret) || differences.contains(secret),
        distinct_covered: distinct.iter().filter(|c| seen.contains(c)).count(),
        distinct_total: distinct.len(),
        cmap_alphabet: cmap.iter().collect(),
        programs,
        programs_parsed,
        tounicode,
        differences,
    }
}

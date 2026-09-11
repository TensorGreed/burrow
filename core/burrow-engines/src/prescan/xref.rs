//! Reading what the cross-reference declares, without materialising any of it.
//!
//! Two forms exist and both are handled:
//!
//! - a **classic table**, `xref` followed by `start count` subsection headers;
//! - a **cross-reference stream**, an ordinary object whose dictionary carries `/Size`,
//!   `/W` and optionally `/Index`.
//!
//! For each, the question is the same: **how many entries does it declare?** That is what
//! drives an engine's cost — not `/W`, which is only how compactly the *file* encodes
//! them. See `super::ENGINE_BYTES_PER_XREF_ENTRY`.
//!
//! Nothing here decompresses, resolves a reference, or recurses. The dictionary is read by
//! scanning forward for the keys it wants, which is not a PDF parser and must not become
//! one — an engine's job is to understand the file, and this module's job is to decide
//! whether an engine should be allowed to try.

use super::Declared;

/// Bytes per entry in a classic cross-reference table. Fixed by the specification:
/// ten digits, a space, five digits, a space, a type character, and a two-byte ending.
///
/// Used to skip over a subsection's entries when looking for the next header, not to
/// estimate cost.
const CLASSIC_ENTRY_BYTES: u64 = 20;

/// How many subsection headers to read from one classic table.
///
/// A real file has one, or a handful after incremental updates. This bounds the inner
/// loop against a file that is nothing but subsection headers.
const MAX_SUBSECTIONS: usize = 1024;

/// How far past an xref offset to look for the keys we want.
///
/// A cross-reference stream's dictionary sits immediately at the offset and is normally a
/// few hundred bytes. 64 KiB is far beyond any real one, and bounds the work per chain hop
/// regardless of file size.
///
/// Raised from 4 KiB after review: padding the dictionary so `/Size` fell past the old
/// window made the scan read zero entries and pass a 2.5 GB bomb. Widening alone is not a
/// fix -- a file can always pad further -- which is why [`super::describe`] also has a
/// whole-buffer backstop for when this path finds nothing.
const DICT_WINDOW: usize = 64 * 1024;

/// Read the cross-reference chain's declarations.
///
/// `window` is how far back from the end to look for `startxref`; `max_chain` caps the
/// `/Prev` walk. Both come from the caller so the constants stay in one place.
pub(super) fn describe(bytes: &[u8], window: usize, max_chain: usize) -> Declared {
    let mut declared = Declared::default();

    let Some(mut offset) = startxref_offset(bytes, window) else {
        return declared;
    };

    // Visited offsets, so a `/Prev` pointing at itself -- or a cycle of any length --
    // terminates instead of spinning. Bounded by `max_chain`, so this allocation is O(1).
    let mut visited: Vec<usize> = Vec::with_capacity(max_chain);

    for _ in 0..max_chain {
        if offset >= bytes.len() || visited.contains(&offset) {
            break;
        }
        visited.push(offset);
        declared.xref_sections = declared.xref_sections.saturating_add(1);

        let Some(section) = bytes.get(offset..) else {
            break;
        };
        let window_end = section.len().min(DICT_WINDOW);
        let Some(head) = section.get(..window_end) else {
            break;
        };

        let (entries, prev) = if starts_with_keyword(section, b"xref") {
            (classic_table(section), find_int(head, b"/Prev"))
        } else {
            // A cross-reference stream. `/Size` is the entry count unless `/Index`
            // narrows it to specific ranges.
            let size = find_int(head, b"/Size").unwrap_or(0);
            let entries = index_entries(head).unwrap_or(size);
            (entries, find_int(head, b"/Prev"))
        };

        // MAX, not sum. An engine materialises one merged table, and a cross-reference
        // stream without `/Index` repeats the whole document's `/Size` in every section --
        // so summing a file with twenty incremental updates would over-estimate
        // twentyfold and reject a document nothing is wrong with.
        declared.xref_entries = declared.xref_entries.max(entries);

        match prev.and_then(|p| usize::try_from(p).ok()) {
            Some(next) => offset = next,
            None => break,
        }
    }

    declared
}

/// The largest `/Size` declared anywhere in the buffer.
///
/// The backstop for when the directed walk finds nothing. See [`super::describe`] for why
/// it exists and why it is deliberately not the primary path.
///
/// O(n) in time, O(1) in allocation, like everything else here.
pub(super) fn largest_declared_size(bytes: &[u8]) -> u64 {
    find_int(bytes, b"/Size").unwrap_or(0)
}

/// The offset named by the last `startxref` in the final `window` bytes.
fn startxref_offset(bytes: &[u8], window: usize) -> Option<usize> {
    let start = bytes.len().saturating_sub(window);
    let tail = bytes.get(start..)?;
    let at = rfind(tail, b"startxref")?;
    let after = tail.get(at.checked_add(b"startxref".len())?..)?;
    usize::try_from(read_int(after)?).ok()
}

/// Entry count for a classic `xref` table.
///
/// Sums the `count` of each `start count` subsection header. Stops at `trailer`, at the
/// subsection cap, or as soon as a line is not two integers -- which is what the entries
/// themselves look like, so this naturally stops at the first entry of each subsection
/// without needing to understand them.
fn classic_table(section: &[u8]) -> u64 {
    let mut entries: u64 = 0;
    let mut cursor = b"xref".len();

    for _ in 0..MAX_SUBSECTIONS {
        let Some(rest) = section.get(cursor..) else {
            break;
        };
        let leading = leading_space(rest);
        let Some(rest) = rest.get(leading..) else {
            break;
        };
        if starts_with_keyword(rest, b"trailer") {
            break;
        }

        // `start count`
        let Some((_start, after_start)) = read_int_at(rest) else {
            break;
        };
        let gap = leading_space(after_start);
        let Some(after_gap) = after_start.get(gap..) else {
            break;
        };
        let Some((count, after_count)) = read_int_at(after_gap) else {
            break;
        };

        entries = entries.saturating_add(count);

        // Skip the entries themselves: `count` lines of exactly twenty bytes.
        let skip = usize::try_from(count.saturating_mul(CLASSIC_ENTRY_BYTES)).unwrap_or(usize::MAX);
        let consumed = section.len().saturating_sub(after_count.len());
        cursor = consumed
            .saturating_add(leading_space(after_count))
            .saturating_add(skip);
        if cursor >= section.len() {
            break;
        }
    }

    entries
}

/// Entry count from an `/Index [first count first count ...]` array, if present.
///
/// `/Index` narrows a cross-reference stream to specific ranges, so when it is there it is
/// a better entry count than `/Size`.
fn index_entries(head: &[u8]) -> Option<u64> {
    let at = find(head, b"/Index")?;
    let rest = head.get(at.saturating_add(b"/Index".len())..)?;
    let open = leading_space(rest);
    if rest.get(open) != Some(&b'[') {
        return None;
    }
    let mut cursor = rest.get(open.saturating_add(1)..)?;

    let mut total: u64 = 0;
    let mut seen = 0usize;
    // Pairs of (first, count); take the count of each. Capped so a long array cannot make
    // this loop expensive.
    for _ in 0..256 {
        let gap = leading_space(cursor);
        let Some(next) = cursor.get(gap..) else {
            break;
        };
        if next.first() == Some(&b']') {
            break;
        }
        let Some((value, rest)) = read_int_at(next) else {
            break;
        };
        if seen % 2 == 1 {
            total = total.saturating_add(value);
        }
        seen = seen.saturating_add(1);
        cursor = rest;
    }
    (seen > 0).then_some(total)
}

/// The **largest** integer following any occurrence of `key` in `head`.
///
/// Three things here are deliberate, and each of them was a bypass before review found it:
///
/// - **Every occurrence is examined, not the first.** Taking the first means a decoy
///   earlier in the dictionary hides the real value.
/// - **The largest value wins.** A `/Size 1` planted before the real `/Size 20000000`
///   would otherwise be the one read.
/// - **The key must be followed by a delimiter.** `/Sizes 1` contains `/Size`, so a
///   nine-byte insertion of `/Sizes 1 ` made a literal search read nothing at all and the
///   scan report zero entries -- which is indistinguishable from "no cross-reference
///   here", and is exactly what waved a 2.5 GB bomb through.
fn find_int(head: &[u8], key: &[u8]) -> Option<u64> {
    let mut best: Option<u64> = None;
    let mut cursor = 0usize;

    while let Some(rest) = head.get(cursor..) {
        let Some(at) = find(rest, key) else {
            break;
        };
        let after = cursor.saturating_add(at).saturating_add(key.len());
        cursor = after;

        // A PDF name ends at whitespace or a delimiter. Anything else means this is a
        // longer name that merely starts with the key.
        let Some(tail) = head.get(after..) else {
            break;
        };
        if !tail.first().is_some_and(|b| is_name_terminator(*b)) {
            continue;
        }
        if let Some(value) = read_int(tail) {
            best = Some(best.map_or(value, |b: u64| b.max(value)));
        }
    }

    best
}

/// Whether `b` ends a PDF name: whitespace, or one of the delimiter characters.
const fn is_name_terminator(b: u8) -> bool {
    b.is_ascii_whitespace()
        || matches!(
            b,
            b'[' | b']' | b'<' | b'>' | b'(' | b')' | b'/' | b'%' | b'{' | b'}'
        )
}

/// Whether `haystack` begins with `needle` followed by a delimiter.
///
/// Guards against `xref` matching the start of `xrefstm`, which is a real key.
fn starts_with_keyword(haystack: &[u8], needle: &[u8]) -> bool {
    if !haystack.starts_with(needle) {
        return false;
    }
    match haystack.get(needle.len()) {
        None => true,
        Some(&b) => b.is_ascii_whitespace() || b == b'<' || b == b'[',
    }
}

/// First index of `needle` in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Last index of `needle` in `haystack`.
fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).rposition(|w| w == needle)
}

/// Count of leading ASCII whitespace.
fn leading_space(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len())
}

/// Read the first unsigned integer in `bytes`, skipping leading whitespace.
fn read_int(bytes: &[u8]) -> Option<u64> {
    let skip = leading_space(bytes);
    read_int_at(bytes.get(skip..)?).map(|(value, _)| value)
}

/// Read an unsigned integer at the very start of `bytes`, returning it and the remainder.
///
/// Saturating rather than wrapping or failing: a 400-digit number is a declaration of
/// something absurd, and the caller's job is to reject absurd declarations, not to be
/// unable to represent them.
fn read_int_at(bytes: &[u8]) -> Option<(u64, &[u8])> {
    let digits = bytes
        .iter()
        .position(|b| !b.is_ascii_digit())
        .unwrap_or(bytes.len());
    if digits == 0 {
        return None;
    }
    let mut value: u64 = 0;
    for &b in bytes.get(..digits)? {
        value = value
            .saturating_mul(10)
            .saturating_add(u64::from(b.saturating_sub(b'0')));
    }
    Some((value, bytes.get(digits..)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn describe_default(bytes: &[u8]) -> Declared {
        describe(bytes, 2048, 32)
    }

    #[test]
    fn a_cross_reference_stream_declares_its_size() {
        let pdf = b"%PDF-1.5\n4 0 obj\n<< /Type /XRef /Size 20000000 /W [1 8 8] /Root 1 0 R >>\nstream\nx\nendstream\nendobj\nstartxref\n9\n%%EOF\n";
        assert_eq!(describe_default(pdf).xref_entries, 20_000_000);
    }

    #[test]
    fn index_narrows_the_entry_count_when_present() {
        let pdf = b"%PDF-1.5\n4 0 obj\n<< /Type /XRef /Size 20000000 /Index [0 10] /W [1 2 1] >>\nstream\nx\nendstream\nendobj\nstartxref\n9\n%%EOF\n";
        let declared = describe_default(pdf);
        // /Index says only ten entries are present, so ten -- not twenty million.
        assert_eq!(declared.xref_entries, 10);
    }

    #[test]
    fn a_classic_table_counts_its_subsection_entries() {
        let pdf = b"%PDF-1.7\nxref\n0 4\n0000000000 65535 f \n0000000009 00000 n \n0000000050 00000 n \n0000000100 00000 n \ntrailer\n<< /Size 4 >>\nstartxref\n9\n%%EOF\n";
        assert_eq!(describe_default(pdf).xref_entries, 4);
    }

    #[test]
    fn a_prev_chain_pointing_at_itself_terminates() {
        // The offset of `startxref`'s target is the `xref` keyword, whose trailer's /Prev
        // points straight back at it. Without the visited set this never returns.
        let pdf = b"%PDF-1.7\nxref\n0 1\n0000000000 65535 f \ntrailer\n<< /Size 1 /Prev 9 >>\nstartxref\n9\n%%EOF\n";
        let declared = describe_default(pdf);
        assert_eq!(
            declared.xref_sections, 1,
            "the self-reference was followed twice"
        );
    }

    #[test]
    fn a_chain_longer_than_the_cap_stops_at_the_cap() {
        // Every section points at the next; with a cap of 3 only three are visited.
        let pdf = b"%PDF-1.7\nxref\n0 1\ntrailer\n<< /Size 1 /Prev 40 >>\nxref\n0 1\ntrailer\n<< /Size 1 /Prev 9 >>\nstartxref\n9\n%%EOF\n";
        let declared = describe(pdf, 2048, 3);
        assert!(declared.xref_sections <= 3, "{declared:?}");
    }

    #[test]
    fn absurd_declarations_saturate_instead_of_wrapping() {
        let pdf = b"%PDF-1.5\n4 0 obj\n<< /Type /XRef /Size 99999999999999999999999999 >>\nstartxref\n9\n%%EOF\n";
        assert_eq!(
            describe_default(pdf).xref_entries,
            u64::MAX,
            "a 26-digit /Size should saturate, not wrap to something small"
        );
    }

    /// The `/Prev` chain takes the **maximum** across sections, not the sum.
    ///
    /// A cross-reference stream without `/Index` repeats the whole document's `/Size` in
    /// every section, so a file with several incremental updates would otherwise estimate
    /// several times what an engine actually materialises -- and be rejected for it.
    #[test]
    fn a_chain_takes_the_largest_section_not_their_sum() {
        // Two sections, each declaring 1000. The merged table an engine builds is 1000
        // entries, not 2000.
        let pdf = b"%PDF-1.7\nxref\n0 1\ntrailer\n<< /Size 1000 /Prev 46 >>\nxref\n0 1\ntrailer\n<< /Size 1000 >>\nstartxref\n9\n%%EOF\n";
        let declared = describe_default(pdf);
        assert!(declared.xref_sections >= 1);
        assert_eq!(
            declared.xref_entries, 1000,
            "the chain summed its sections instead of taking the largest"
        );
    }

    #[test]
    fn startxref_pointing_past_the_end_is_ignored() {
        let pdf = b"%PDF-1.7\nstartxref\n999999\n%%EOF\n";
        assert_eq!(describe_default(pdf).xref_sections, 0);
    }

    #[test]
    fn xref_does_not_match_the_start_of_a_longer_keyword() {
        // `/XRefStm` and `xrefstm` both begin with the classic table's keyword.
        assert!(!starts_with_keyword(b"xrefstm 123", b"xref"));
        assert!(starts_with_keyword(b"xref\n0 1", b"xref"));
        assert!(starts_with_keyword(b"xref", b"xref"));
    }

    #[test]
    fn reading_an_integer_saturates_rather_than_overflowing() {
        assert_eq!(read_int(b"  42 rest"), Some(42));
        assert_eq!(read_int(b"not a number"), None);
        assert_eq!(read_int(b""), None);
        let (value, _) = read_int_at(b"99999999999999999999999999").expect("digits");
        assert_eq!(value, u64::MAX);
    }
}

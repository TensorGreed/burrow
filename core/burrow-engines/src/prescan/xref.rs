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
    // THE ENTRY BYTES ALREADY JUDGED, as spans, shared by the whole chain. Sections may overlap --
    // a declared count can land every section's cursor on one shared, large subsection -- and
    // judging each section's entries afresh read the same bytes once per section: 32 times over a
    // 480 MB file, 3.7 s with no deadline consulted (second security review). Judging each entry
    // byte once keeps the walk linear in the file, whatever the chain. SPANS, NOT ONE MARK: an
    // older section usually sits earlier in the file than the newer one read first, so a single
    // high-water mark skipped entries nothing had read.
    let mut judged = Judged::default();

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

        // White space before the keyword is skipped, as readers skip it: a `startxref` one byte
        // early, at the line ending before `xref`, sent this down the stream branch -- which
        // read the trailer's `/Size` and none of the table's entries.
        let table = section.get(leading_space(section)..).unwrap_or(section);
        let (entries, prev) = if starts_with_keyword(table, b"xref") {
            let base = bytes.len().saturating_sub(table.len());
            let read = classic_table(table, base, bytes.len(), &mut judged);
            declared.entry_past_end |= read.past_end;
            declared.entries_judged = declared.entries_judged.saturating_add(read.judged);
            let entries = read.entries;
            (entries, find_int(head, b"/Prev"))
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
///
/// Also whether any in-use entry places its object at or past `file_len` -- see
/// [`super::Declared::entry_past_end`]. Each entry is read only where it is written exactly as
/// the specification lays it out; the first that is not ends the judging of its subsection,
/// so the count above is unaffected and an oddly written table is never refused for it.
///
/// `base` is where `section` begins in the file, and `judged` the spans of entry bytes any
/// section has already read: an entry inside one is skipped, not re-read.
fn classic_table(section: &[u8], base: usize, file_len: usize, judged: &mut Judged) -> Table {
    let mut entries: u64 = 0;
    let mut past_end = false;
    let mut read: u64 = 0;
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
        // Read each one on the way past, bounded by the bytes actually present: a declared
        // count larger than the table stops at the end of the section, not at the count.
        let first = consumed.saturating_add(leading_space(after_count));
        let mut at = first;
        while let Some(entry) = section.get(at..at.saturating_add(20)) {
            if at.saturating_sub(first) >= skip {
                break;
            }
            // Already read by another section: jump past that span, on this subsection's own
            // 20-byte stride.
            if let Some(end) = judged.covering(base.saturating_add(at)) {
                let past = end.saturating_sub(base).saturating_sub(first);
                at = first.saturating_add(past.div_ceil(20).saturating_mul(20));
                continue;
            }
            read = read.saturating_add(1);
            match in_use_offset(entry) {
                Some(Some(offset)) => {
                    if usize::try_from(offset).map_or(true, |offset| offset >= file_len) {
                        past_end = true;
                    }
                }
                Some(None) => {}
                None => break,
            }
            judged.add(
                base.saturating_add(at),
                base.saturating_add(at).saturating_add(20),
            );
            at = at.saturating_add(20);
        }
        cursor = consumed
            .saturating_add(leading_space(after_count))
            .saturating_add(skip);
        if cursor >= section.len() {
            break;
        }
    }

    Table {
        entries,
        past_end,
        judged: read,
    }
}

/// Spans of the file whose entries have been read, sorted and disjoint.
///
/// Bounded: a span grows by merging, and new spans come one per run of unread entries, at most
/// one per subsection read -- `MAX_SUBSECTIONS` per section, across at most the chain's cap of
/// sections. A constant, however large the file.
#[derive(Default)]
struct Judged {
    spans: Vec<(usize, usize)>,
}

impl Judged {
    /// The end of the span holding `at`, if one does.
    fn covering(&self, at: usize) -> Option<usize> {
        let index = self.spans.partition_point(|&(start, _)| start <= at);
        let &(start, end) = self.spans.get(index.checked_sub(1)?)?;
        (start <= at && at < end).then_some(end)
    }

    /// Record `[start, end)` as read, merging with any span it touches.
    fn add(&mut self, start: usize, end: usize) {
        let index = self.spans.partition_point(|&(s, _)| s < start);
        let mut merged = (start, end);
        let mut from = index;
        if let Some(&(s, e)) = index.checked_sub(1).and_then(|i| self.spans.get(i))
            && e >= start
        {
            merged = (s, e.max(end));
            from = index - 1;
        }
        let mut to = from;
        while let Some(&(s, e)) = self.spans.get(to) {
            if s > merged.1 {
                break;
            }
            merged.1 = merged.1.max(e);
            merged.0 = merged.0.min(s);
            to += 1;
        }
        self.spans.splice(from..to, [merged]);
    }
}

/// What one classic table declared, and how much of it was read.
struct Table {
    /// Entries its subsection headers declare.
    entries: u64,
    /// Whether an in-use entry placed its object at or past the end of the file.
    past_end: bool,
    /// Entries actually read to answer that -- each entry byte once across the whole chain.
    judged: u64,
}

/// A classic entry's offset if it is in use: `Some(Some(offset))` for `dddddddddd ddddd n` and a
/// two-byte line ending, `Some(None)` for a free entry (`… f`), and `None` for anything not
/// written exactly that way -- all twenty bytes.
fn in_use_offset(entry: &[u8]) -> Option<Option<u64>> {
    let digits = |range: core::ops::Range<usize>| {
        entry
            .get(range)
            .filter(|run| run.iter().all(u8::is_ascii_digit))
    };
    let offset = digits(0..10)?;
    digits(11..16)?;
    if entry.get(10) != Some(&b' ') || entry.get(16) != Some(&b' ') {
        return None;
    }
    // THE WHOLE TWENTY BYTES, line ending included: one of the specification's three two-byte
    // endings. Reading only the first eighteen judged the first entry of a table written with
    // one-byte line endings, which is not "written exactly as the specification lays it out"
    // (fourth code review).
    if !matches!(entry.get(18..20), Some(b" \n" | b" \r" | b"\r\n")) {
        return None;
    }
    match entry.get(17) {
        Some(b'n') => Some(read_int(offset)),
        Some(b'f') => Some(None),
        _ => None,
    }
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

    /// A one-table file whose entries are `entries`, with `startxref` `early` bytes before the
    /// `xref` keyword (0 = exactly at it).
    fn with_entries(entries: &[&str], early: usize) -> Vec<u8> {
        let mut pdf = b"%PDF-1.7\n1 0 obj\n<< >>\nendobj\n".to_vec();
        let at = pdf.len();
        pdf.extend_from_slice(format!("xref\n0 {}\n", entries.len()).as_bytes());
        for entry in entries {
            pdf.extend_from_slice(entry.as_bytes());
        }
        pdf.extend_from_slice(
            format!(
                "trailer\n<< /Size {} >>\nstartxref\n{}\n%%EOF\n",
                entries.len(),
                at - early
            )
            .as_bytes(),
        );
        pdf
    }

    #[test]
    fn an_in_use_entry_past_the_end_of_the_file_is_read_as_one() {
        let past = with_entries(&["0000000000 65535 f \n", "0000099999 00000 n \n"], 0);
        assert!(describe_default(&past).entry_past_end);
        // THE NEAR-MISS: the same table with the object where the file has bytes.
        let inside = with_entries(&["0000000000 65535 f \n", "0000000009 00000 n \n"], 0);
        assert!(!describe_default(&inside).entry_past_end);
    }

    #[test]
    fn the_boundary_is_the_file_length_itself() {
        // An object cannot begin AT the end either: no byte is there. One before it can.
        let probe = with_entries(&["0000000000 65535 f \n", "0000000009 00000 n \n"], 0);
        let len = probe.len();
        let at_end = with_entries(
            &["0000000000 65535 f \n", &format!("{len:010} 00000 n \n")],
            0,
        );
        assert_eq!(
            at_end.len(),
            len,
            "the fixture's length does not depend on the offset"
        );
        assert!(describe_default(&at_end).entry_past_end);
        let before_end = with_entries(
            &[
                "0000000000 65535 f \n",
                &format!("{:010} 00000 n \n", len - 1),
            ],
            0,
        );
        assert!(!describe_default(&before_end).entry_past_end);
    }

    #[test]
    fn a_free_entry_is_not_an_object_and_is_not_judged() {
        let free = with_entries(&["0000099999 65535 f \n", "0000000009 00000 n \n"], 0);
        assert!(!describe_default(&free).entry_past_end);
    }

    #[test]
    fn a_table_not_written_to_the_specification_is_not_judged() {
        // Nineteen-byte lines put every later entry one byte off; a reader that judged them
        // anyway would read digits from the wrong columns. It stops at the first that does not
        // match, so the misaligned one is never read.
        let short = with_entries(
            &[
                "0000000000 65535 f\n",
                "0000000009 00000 n\n",
                "0000099999 00000 n\n",
            ],
            0,
        );
        assert!(!describe_default(&short).entry_past_end);
        // THE FIRST MISSHAPEN ENTRY ENDS THE JUDGING of its subsection: a well-formed entry
        // after it, past the end, is not read -- what follows a broken line is not known to
        // line up.
        let after_garbage = with_entries(
            &[
                "0000000000 65535 f \n",
                "xxxxxxxxxx 00000 n \n",
                "0000099999 00000 n \n",
            ],
            0,
        );
        assert!(!describe_default(&after_garbage).entry_past_end);
        // AND THE SEPARATORS ARE PART OF THE SHAPE: the right digits with a `-` where a space
        // belongs are not an entry this reads.
        let dashed = with_entries(&["0000000000 65535 f \n", "0000099999-00000 n \n"], 0);
        assert!(!describe_default(&dashed).entry_past_end);
    }

    /// A file with two classic sections joined by `/Prev`; `bad_in` names which section holds
    /// an in-use entry past the end ("newer", "older" or "neither").
    fn two_sections(bad_in: &str) -> Vec<u8> {
        let entry = |bad: bool| {
            if bad {
                "0000099999 00000 n \n"
            } else {
                "0000000009 00000 n \n"
            }
        };
        let mut pdf = b"%PDF-1.7\n1 0 obj\n<< >>\nendobj\n".to_vec();
        let older = pdf.len();
        pdf.extend_from_slice(
            format!(
                "xref\n0 2\n0000000000 65535 f \n{}trailer\n<< /Size 2 >>\n",
                entry(bad_in == "older")
            )
            .as_bytes(),
        );
        let newer = pdf.len();
        pdf.extend_from_slice(
            format!(
                "xref\n1 1\n{}trailer\n<< /Size 2 /Prev {older} >>\nstartxref\n{newer}\n%%EOF\n",
                entry(bad_in == "newer")
            )
            .as_bytes(),
        );
        pdf
    }

    #[test]
    fn an_entry_past_the_end_in_any_section_of_the_chain_is_read_as_one() {
        // THE FLAG ACCUMULATES ACROSS SECTIONS: assigning it instead let a clean older section
        // overwrite a bad newer one (second code review). Both positions, and the near-miss.
        assert!(describe_default(&two_sections("newer")).entry_past_end);
        assert!(describe_default(&two_sections("older")).entry_past_end);
        assert!(!describe_default(&two_sections("neither")).entry_past_end);
        assert_eq!(describe_default(&two_sections("neither")).xref_sections, 2);
    }

    #[test]
    fn the_separator_before_the_type_is_part_of_the_shape_too() {
        // Column 16, as the dashed near-miss above is column 10.
        let dashed = with_entries(&["0000000000 65535 f \n", "0000099999 00000-n \n"], 0);
        assert!(!describe_default(&dashed).entry_past_end);
    }

    #[test]
    fn overlapping_sections_judge_each_entry_byte_once() {
        // THE WALK STAYS LINEAR: a second section reaching the same entries reads none of them
        // again. Re-reading them per section was 3.7 s over a 480 MB file with a 32-section
        // chain (second security review).
        let entries: Vec<String> = (0..10)
            .map(|n| format!("{:010} 00000 n \n", 9 + n))
            .collect();
        let refs: Vec<&str> = entries.iter().map(String::as_str).collect();
        let pdf = with_entries(&refs, 0);
        let at = pdf
            .windows(4)
            .position(|w| w == b"xref")
            .expect("the table");
        let table = &pdf[at..];
        let mut judged = Judged::default();
        let once = classic_table(table, at, pdf.len(), &mut judged);
        let again = classic_table(table, at, pdf.len(), &mut judged);
        assert_eq!(once.judged, 10);
        assert_eq!(again.judged, 0, "the same entries were read twice");
        assert_eq!(again.entries, 10, "the declared count is still read");
    }

    #[test]
    fn an_entry_beyond_the_declared_count_is_not_judged() {
        // The subsection says one entry; a second, past the end, follows before `trailer`. It is
        // outside what the table declares, so it is not this table's to judge.
        let body = "%PDF-1.7\n1 0 obj\n<< >>\nendobj\n";
        let pdf = format!(
            "{body}xref\n0 1\n0000000000 65535 f \n0000099999 00000 n \ntrailer\n<< /Size 1 >>\nstartxref\n{}\n%%EOF\n",
            body.len()
        );
        assert!(!describe_default(pdf.as_bytes()).entry_past_end);
    }

    #[test]
    fn a_generation_that_is_not_five_digits_is_not_an_entry_this_reads() {
        let lettered = with_entries(&["0000000000 65535 f \n", "0000099999 0000x n \n"], 0);
        assert!(!describe_default(&lettered).entry_past_end);
    }

    #[test]
    fn describe_shares_one_span_set_across_the_chain() {
        // WIRED IN, not only working: two sections whose declared counts land on one shared
        // subsection of ten entries. Shared, the ten are read once -- 10 + one misshapen line per
        // section = 12. A span set per section reads them twice: 22 (third code review).
        let mut pdf = b"%PDF-1.7\n1 0 obj\n<< >>\nendobj\n".to_vec();
        let older = pdf.len();
        pdf.extend_from_slice(b"xref\n0 2\n");
        pdf.extend_from_slice(&[b'x'; 11]);
        let newer = pdf.len();
        pdf.extend_from_slice(b"xref\n0 1\n");
        pdf.extend_from_slice(&[b'y'; 20]);
        pdf.extend_from_slice(b"0 10\n");
        for n in 0..10 {
            pdf.extend_from_slice(format!("{:010} 00000 n \n", 9 + n).as_bytes());
        }
        pdf.extend_from_slice(
            format!("trailer\n<< /Size 11 /Prev {older} >>\nstartxref\n{newer}\n%%EOF\n")
                .as_bytes(),
        );
        let declared = describe_default(&pdf);
        assert_eq!(declared.xref_sections, 2);
        assert_eq!(declared.entries_judged, 12);
        assert!(!declared.entry_past_end);
    }

    #[test]
    fn judged_spans_agree_with_a_byte_map() {
        // A MODEL: every `add` is mirrored into a plain byte map, and `covering` must agree with it
        // at every position. Deterministic pseudo-random sequences, so a failure reproduces.
        let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = |bound: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            usize::try_from(seed % u64::try_from(bound).unwrap()).unwrap()
        };
        for _ in 0..2_000 {
            let mut judged = Judged::default();
            let mut map = [false; 400];
            for _ in 0..next(12) + 1 {
                let start = next(380);
                let end = start + next(20) + 1;
                judged.add(start, end);
                map[start..end].iter_mut().for_each(|b| *b = true);
            }
            for at in 0..400 {
                assert_eq!(
                    judged.covering(at).is_some(),
                    map[at],
                    "at {at}: {:?}",
                    judged.spans
                );
                if let Some(end) = judged.covering(at) {
                    assert!(map[at..end].iter().all(|b| *b) && map.get(end) != Some(&true));
                }
            }
            assert!(
                judged.spans.windows(2).all(|w| w[0].1 < w[1].0),
                "{:?}",
                judged.spans
            );
        }
    }

    #[test]
    fn a_first_entry_with_a_one_byte_line_ending_is_not_judged() {
        // The first entry of a nineteen-byte table lines up with the stride even though the table
        // is not written to the specification; its line ending is what says so.
        let first_short = with_entries(&["0000099999 00000 n\n", "0000000009 00000 n\n"], 0);
        assert!(!describe_default(&first_short).entry_past_end);
        assert_eq!(
            describe_default(&first_short).entries_judged,
            1,
            "read, and not judged"
        );
        // THE NEAR-MISSES: each of the three endings the specification allows is read.
        for ending in [" \n", " \r", "\r\n"] {
            let entry = format!("0000099999 00000 n{ending}");
            let table = with_entries(&["0000000000 65535 f \n", &entry], 0);
            assert!(describe_default(&table).entry_past_end, "{ending:?}");
        }
    }

    #[test]
    fn a_startxref_one_byte_early_still_reads_the_table() {
        // At the line ending before `xref`: read as a stream,
        // it gave the trailer's `/Size` and none of the entries.
        let early = with_entries(&["0000000000 65535 f \n", "0000099999 00000 n \n"], 1);
        assert!(describe_default(&early).entry_past_end);
        assert_eq!(describe_default(&early).xref_entries, 2);
    }
}

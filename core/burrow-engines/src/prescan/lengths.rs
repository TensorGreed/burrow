//! Every `/Length` the file declares, found in one bounded pass.
//!
//! A stream's `/Length` is what an engine will trust when it reads the stream's bytes, so
//! a `/Length` larger than the whole file is a declaration that cannot be honest. Finding
//! the largest one is cheap and catches the crudest form of amplification outright.
//!
//! This is a **byte scan**, not a parse. It does not know which `/Length` belongs to which
//! object, does not resolve the indirect form (`/Length 5 0 R`, which it simply skips),
//! and does not care whether the stream exists. It is looking for declarations, and a
//! declaration is exactly what it finds.
//!
//! Cost is O(n) in time and O(1) in allocation. That is the invariant that matters: the
//! module this belongs to exists to be un-blowable-up, and a scan that allocated in
//! proportion to its input would defeat that.

/// Cap on how many `/Length` keys are read.
///
/// A file that is nothing but `/Length 1` repeated is a way to make this loop long. The
/// scan itself is linear either way, but stopping early bounds the work on a file whose
/// only content is this token. Real documents have one per stream.
const MAX_LENGTHS: usize = 1 << 20;

/// The largest `/Length` declared anywhere in `bytes`.
///
/// Only the largest: summing them was tried and cut. The sum approximates the file's own
/// size, so comparing it against `max_memory_bytes` rejected legitimate large documents
/// whenever a caller set a memory ceiling below their input size -- a false positive on a
/// real file, which is the one outcome this module must not produce.
pub(super) fn largest(bytes: &[u8]) -> u64 {
    const KEY: &[u8] = b"/Length";

    let mut largest: u64 = 0;
    if bytes.len() < KEY.len() {
        return largest;
    }

    let mut found = 0usize;
    let mut cursor = 0usize;

    while let Some(rest) = bytes.get(cursor..) {
        if rest.len() < KEY.len() {
            break;
        }
        let Some(at) = rest.windows(KEY.len()).position(|w| w == KEY) else {
            break;
        };

        let after = cursor.saturating_add(at).saturating_add(KEY.len());
        cursor = after;

        // `/Lengths` is a different key, and `/Length1` is a real one in embedded font
        // streams. Only accept the key when what follows it cannot be part of a longer
        // name -- which for a number means whitespace has to come first.
        let Some(tail) = bytes.get(after..) else {
            break;
        };
        if !matches!(tail.first(), Some(b) if b.is_ascii_whitespace()) {
            continue;
        }

        if let Some(value) = read_uint(tail) {
            largest = largest.max(value);
            found = found.saturating_add(1);
            if found >= MAX_LENGTHS {
                break;
            }
        }
    }

    largest
}

/// Read an unsigned integer after leading whitespace.
///
/// `None` when what follows is not a direct integer -- most often the indirect form
/// `/Length 5 0 R`, which cannot be resolved without the object graph and is therefore not
/// this module's business. Skipping it is the right answer: an unread declaration is one
/// this scan makes no claim about, and the measured post-open check still applies.
///
/// Saturating, so an absurd run of digits becomes an absurd number rather than wrapping
/// into a plausible one.
fn read_uint(bytes: &[u8]) -> Option<u64> {
    let skip = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let rest = bytes.get(skip..)?;

    let digits = rest
        .iter()
        .position(|b| !b.is_ascii_digit())
        .unwrap_or(rest.len());
    if digits == 0 {
        return None;
    }

    let mut value: u64 = 0;
    for &b in rest.get(..digits)? {
        value = value
            .saturating_mul(10)
            .saturating_add(u64::from(b.saturating_sub(b'0')));
    }

    // The indirect form is `<num> <num> R`. Detect it and decline, rather than reporting
    // the object number as if it were a byte count.
    let after = rest.get(digits..).unwrap_or_default();
    if is_indirect_reference(after) {
        return None;
    }

    Some(value)
}

/// Whether what follows a number makes it the first half of `<num> <gen> R`.
fn is_indirect_reference(after: &[u8]) -> bool {
    let skip = after
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(after.len());
    if skip == 0 {
        return false;
    }
    let Some(rest) = after.get(skip..) else {
        return false;
    };
    let digits = rest
        .iter()
        .position(|b| !b.is_ascii_digit())
        .unwrap_or(rest.len());
    if digits == 0 {
        return false;
    }
    let Some(tail) = rest.get(digits..) else {
        return false;
    };
    let gap = tail
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(tail.len());
    gap > 0 && tail.get(gap) == Some(&b'R')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_direct_length_is_read() {
        assert_eq!(largest(b"<< /Length 1234 >>"), 1234);
    }

    #[test]
    fn the_largest_of_several_wins() {
        assert_eq!(largest(b"/Length 10 ... /Length 500 ... /Length 7"), 500);
    }

    #[test]
    fn the_indirect_form_is_skipped_rather_than_misread() {
        // `/Length 5 0 R` means "see object 5". Reporting 5 as a byte count would be
        // wrong in the harmless direction, but it would still be wrong.
        assert_eq!(largest(b"<< /Length 5 0 R >>"), 0);
    }

    #[test]
    fn length1_and_lengths_are_not_mistaken_for_length() {
        // `/Length1` is a real key in embedded font streams and carries a large number.
        // Reading it as a stream length would be a false positive on legitimate files.
        assert_eq!(largest(b"<< /Length1 999999 >>"), 0);
        assert_eq!(largest(b"<< /Lengths 42 >>"), 0);
    }

    #[test]
    fn an_absurd_run_of_digits_saturates() {
        assert_eq!(largest(b"/Length 999999999999999999999999999999"), u64::MAX);
    }

    #[test]
    fn input_without_the_key_scans_clean() {
        assert_eq!(largest(b""), 0);
        assert_eq!(largest(b"/Len"), 0);
        assert_eq!(largest(&[0xFF; 4096]), 0);
    }

    #[test]
    fn a_key_at_the_very_end_does_not_run_off_the_buffer() {
        assert_eq!(largest(b"/Length"), 0);
        assert_eq!(largest(b"/Length "), 0);
    }
}

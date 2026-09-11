//! Measuring how much memory an open actually cost.
//!
//! # Why this exists, and what it is worth
//!
//! [`crate::estimate`] predicts cost from the input's **length**, which is all it can see
//! before handing the buffer over. That misses an entire attack: a small file can declare
//! an enormous structure, and the engine will materialise it. A 330 KB PDF whose
//! cross-reference stream declares twenty million entries drives PDFium to allocate about
//! 1.2 GB — roughly 3,900 times the pre-check's estimate — and the pre-check cannot see it
//! coming, because every declared quantity in a PDF is invisible to a byte count.
//!
//! So the size estimate is a floor, not a ceiling, and this module is the other half: read
//! the process's resident set before and after the load, and refuse to hand back a
//! document that has already cost more than the caller allowed.
//!
//! # What this does NOT do
//!
//! It is an **after-the-fact** check. The memory has already been allocated by the time it
//! fires. It stops a document that broke the limit from being *used* — the same rule
//! `max_pages` already follows — and it turns a silent success into a typed
//! `LimitExceeded`. It does **not** prevent the allocation, and on a device with a hard
//! ceiling the engine can still be killed before this code runs. See
//! `docs/adr/0007-limit-enforcement-per-platform.md` and ADR 0011's consequences; the real
//! fix is a structural pre-scan of the declared sizes, which needs qpdf (M1 PR 3).
//!
//! It is also **best-effort in a second way**: resident set size is process-wide, so
//! another thread allocating during the measurement window is attributed here. Every
//! PDFium call is serialised onto one thread, which makes the window small and the
//! attribution usually right, but "usually" is the honest word. Because of that, this
//! check only ever fires on an overshoot far larger than any plausible noise.

/// Resident set size of this process, in bytes.
///
/// `None` when it cannot be read, which is not an error: the caller degrades to the
/// size-based estimate alone rather than failing an operation over a missing procfs.
pub(crate) fn resident_bytes() -> Option<u64> {
    // Field 2 of /proc/self/statm is the resident set, in pages. `statm` rather than
    // `status` because it is a single short line of integers -- no parsing of units, no
    // locale, and one read.
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    pages.checked_mul(page_size())
}

/// Bytes per page.
fn page_size() -> u64 {
    // SAFETY: `sysconf` is a pure lookup of a process-wide constant. It takes an `int` and
    // returns a `long`, touches no pointer, and has no failure mode here beyond returning
    // a negative value, which is handled below.
    let raw = unsafe { sysconf(SC_PAGESIZE) };
    // Falls back to 4 KiB rather than propagating a failure: getting this wrong scales the
    // measurement, and a wrong scale is better than no limit at all. Every platform this
    // module compiles for uses 4 KiB or more, so the fallback under-reports rather than
    // over-reports, which is the safe direction for a check that rejects.
    u64::try_from(raw).unwrap_or(4096)
}

const SC_PAGESIZE: core::ffi::c_int = 30;

unsafe extern "C" {
    /// `long sysconf(int name)` — POSIX.
    fn sysconf(name: core::ffi::c_int) -> core::ffi::c_long;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_size_is_plausible() {
        let size = page_size();
        assert!(size >= 4096, "page size {size} is below 4 KiB");
        assert!(
            size.is_power_of_two(),
            "page size {size} is not a power of two"
        );
    }

    #[test]
    fn resident_bytes_is_readable_and_nonzero_on_linux() {
        let rss = resident_bytes().expect("/proc/self/statm should be readable on Linux");
        assert!(rss > 0, "a running process has a non-zero resident set");
    }

    #[test]
    fn resident_bytes_grows_when_memory_is_actually_touched() {
        let before = resident_bytes().expect("readable");
        // Touched, not just allocated: untouched zero pages are never faulted in and would
        // not move the resident set -- which is exactly why the 2 GiB input test does not
        // use any real memory.
        let mut block = vec![0u8; 64 * 1024 * 1024];
        for page in block.chunks_mut(4096) {
            page[0] = 1;
        }
        let after = resident_bytes().expect("readable");
        assert!(
            after > before,
            "resident set did not grow after touching 64 MiB: {before} -> {after}"
        );
        drop(block);
    }
}

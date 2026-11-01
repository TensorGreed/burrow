//! The resident-set reader really does move when memory is touched.
//!
//! This test lives in its own integration-test binary, so cargo runs it in its own process,
//! apart from the lib suite; only libtest's threads and rss.rs's two unit tests share it. `resident_bytes` reads the WHOLE process's resident
//! set, so under the parallel lib test harness another test freeing memory between the
//! two reads made the growth assertion fail (merge-group run 41233708531, "497094656 ->
//! 321171456"). `rss.rs` is included by path, without widening any visibility; its own two
//! small unit tests compile into this binary too and allocate nothing worth noting.

#![cfg(target_os = "linux")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "../src/rss.rs"]
mod rss;

#[cfg(test)]
mod growth {
    use super::rss::resident_bytes;

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

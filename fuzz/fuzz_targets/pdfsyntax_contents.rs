//! Fuzz the `/Contents` span map — the thing that keeps a page's content-stream array an array.
//!
//! ```bash
//! # from fuzz/
//! LD_LIBRARY_PATH=../engines/vendor/native-$(uname -m)/lib ASAN_OPTIONS=detect_leaks=0 \
//!   cargo +nightly fuzz run pdfsyntax_contents -- \
//!     -max_total_time=60 -timeout=10 -rss_limit_mb=2048
//! ```
//!
//! # The input is one content stream, cut into elements at a byte the fuzzer picks
//!
//! A page's `/Contents` may be an array, and its elements divide **only at token boundaries** —
//! but a hostile document is under no obligation to honour that, and the divisions are exactly
//! where this module can be wrong. So the seed is a carved content stream, and the first byte of
//! the input chooses the delimiter the rest is cut on. That gives libFuzzer a way to steer the
//! boundaries onto a token, into the middle of one, into a string, either side of an inline
//! image's binary data — which is the interesting axis and is not reachable by mutating bytes
//! alone.
//!
//! # The claims being tested
//!
//! 1. **No panic or hang**, on any cut of any bytes.
//! 2. **The identity holds.** `apply` with no edits returns every element exactly as it went in.
//!    This is the property every later assertion rests on: if a page nothing was removed from
//!    does not come back byte-identical, the round-trip test measures this function's noise.
//! 3. **A no-op edit is the identity, or is refused — and the document decides which.** Replacing
//!    each operation's span with *its own bytes* goes through the whole splice-and-cut path; where
//!    no operation straddles an element boundary the input must come back unchanged, and where one
//!    does the replacement must be refused rather than applied. The bi-conditional is the point: an
//!    "ok or err" assertion accepts everything. This is the property that found the defect the
//!    module's `apply` rustdoc records, and it needs no oracle.
//! 4. **`locate` agrees with the concatenation.** Every offset it claims for an element really
//!    holds that element's byte.
//! 5. **Deleting really deletes, by exactly the right number of bytes.** With every operation
//!    removed, the elements together must be shorter by precisely the count of covered bytes
//!    that some element owned. The first version asserted only that no element came back
//!    *longer*, which passes for an `apply` that removes nothing — the vacuity `CLAUDE.md` names,
//!    written into the check meant to catch it.

#![no_main]

use burrow_engines::pdfsyntax::contents::{Contents, Edit, MAX_ELEMENTS};
use burrow_engines::pdfsyntax::ops::operations;
use libfuzzer_sys::fuzz_target;

/// The most elements one `/Contents` array is cut into here.
///
/// It was 64, chosen "about this target's own allocation" — and code review pointed out that a
/// harness inventing a limit the library does not have is the library's missing limit written in
/// the wrong place, and that it made everything past 64 elements unreachable. The library has
/// [`MAX_ELEMENTS`] now, so this follows it: the target explores every shape the library accepts,
/// and one past it, rather than a ceiling of its own.
const MAX_PARTS: usize = MAX_ELEMENTS + 1;

fuzz_target!(|data: &[u8]| {
    let Some((&delimiter, body)) = data.split_first() else {
        return;
    };
    let mut parts: Vec<&[u8]> = body.split(|&b| b == delimiter).take(MAX_PARTS).collect();
    if parts.is_empty() {
        parts.push(body);
    }

    let Ok(contents) = Contents::concatenate(&parts) else {
        // Past `MAX_ELEMENTS`, which is a refusal rather than a truncation.
        assert!(
            parts.len() > MAX_ELEMENTS,
            "a /Contents array of {} elements was refused",
            parts.len()
        );
        return;
    };
    assert_eq!(contents.len(), parts.len(), "an element went missing");

    // WHERE EACH ELEMENT STARTS, computed from the parts this target built rather than asked of
    // `locate`. The straddle check below needs an oracle, and an oracle that calls the code under
    // test is the same arithmetic written twice -- which is what the first version did, copying
    // `apply`'s own expression verbatim.
    let mut starts = Vec::with_capacity(parts.len());
    let mut offset = 0usize;
    for part in &parts {
        starts.push((offset, offset + part.len()));
        offset += part.len() + 1;
    }

    // (2) THE IDENTITY. Nothing removed, so nothing may change.
    let untouched = contents.apply(&[]).expect("applying no edits cannot fail");
    assert_eq!(untouched.len(), parts.len());
    for (got, want) in untouched.iter().zip(&parts) {
        assert_eq!(
            got.as_slice(),
            *want,
            "applying no edits changed an element"
        );
    }

    // (4) `locate` agrees with the bytes. Checked against the concatenation rather than against
    // an independently computed offset, because an independent computation here would be the
    // same arithmetic written twice.
    let joined = contents.bytes();
    // Every element edge, plus a stride through the middle. Checking all of `joined` cost this
    // target 15x its sibling's executions per second for the same evidence: the interesting
    // offsets are the boundaries, and the stride keeps the interior honest.
    let stride = 1 + joined.len() / 512;
    let edges = starts.iter().flat_map(|&(start, end)| {
        [start, end.saturating_sub(1), end, end + 1].into_iter()
    });
    for offset in edges.chain((0..joined.len()).step_by(stride)) {
        let Some(&byte) = joined.get(offset) else {
            continue;
        };
        if let Some((index, local)) = contents.locate(offset) {
            let part = parts
                .get(index)
                .expect("locate named an element that is there");
            assert_eq!(
                part.get(local).copied(),
                Some(byte),
                "locate({offset}) said element {index} byte {local}, which is not that byte"
            );
            // And it agrees with the offsets this target computed for itself.
            assert_eq!(
                starts.get(index).map(|&(start, _)| offset - start),
                Some(local),
                "locate({offset}) disagrees with where element {index} actually starts"
            );
        }
    }

    let Ok(read) = operations(joined) else {
        return;
    };

    // (3) A NO-OP EDIT IS THE IDENTITY, OR IS REFUSED -- and which of the two is decided by
    // the document rather than by chance. An operation that straddles an element boundary cannot
    // be REPLACED, because nothing says which element the new bytes belong to; one that does not
    // straddle must come back byte-identical. Asserting the bi-conditional rather than "ok or
    // err" is what stops this degenerating into a test that accepts everything.
    //
    // This is the property that found the original defect: the first `apply` put a straddling
    // replacement wholly in the earlier element, so replacing every operation with its own bytes
    // MOVED content between streams while the concatenation stayed identical.
    // An edit straddles unless SOME element holds the whole of it. Stated as coverage over the
    // independently computed `starts`, so it shares no expression with the implementation.
    let straddles = |(span_start, span_end): (usize, usize)| -> bool {
        !starts
            .iter()
            .any(|&(start, end)| span_start >= start && span_end <= end)
    };
    let same: Vec<Edit> = read
        .iter()
        .map(|operation| Edit {
            span: operation.span,
            replacement: joined
                .get(operation.span.0..operation.span.1)
                .expect("an operation's span is inside the stream")
                .to_vec(),
        })
        .filter(|edit| !edit.replacement.is_empty())
        .collect();
    let any_straddles = same.iter().any(|edit| straddles(edit.span));
    match contents.apply(&same) {
        Ok(rewritten) => {
            assert!(
                !any_straddles,
                "a replacement across an element boundary was applied instead of refused"
            );
            for (got, want) in rewritten.iter().zip(&parts) {
                assert_eq!(
                    got.as_slice(),
                    *want,
                    "replacing every operation with its own bytes changed an element"
                );
            }
        }
        Err(_) => assert!(
            any_straddles,
            "a replacement entirely inside its element was refused"
        ),
    }

    // (5) Deleting really deletes.
    let removals: Vec<Edit> = read
        .iter()
        .map(|operation| Edit {
            span: operation.span,
            replacement: Vec::new(),
        })
        .collect();
    let removed_bytes: usize = removals
        .iter()
        .map(|edit| edit.span.1.saturating_sub(edit.span.0))
        .sum();
    if let Ok(emptied) = contents.apply(&removals) {
        let before: usize = parts.iter().map(|p| p.len()).sum();
        let after: usize = emptied.iter().map(std::vec::Vec::len).sum();
        // THE BYTES ARE GONE, not merely "no longer". `got.len() <= want.len()` was the first
        // version of this and it passes for an `apply` that removes nothing at all -- the exact
        // vacuity `CLAUDE.md` names. The separators are synthetic, so what must disappear is
        // every covered byte that some element owned.
        let owned_and_removed = removals
            .iter()
            .map(|edit| {
                (edit.span.0..edit.span.1)
                    .filter(|offset| contents.locate(*offset).is_some())
                    .count()
            })
            .sum::<usize>();
        assert_eq!(
            after,
            before - owned_and_removed,
            "removing {removed_bytes} bytes left the elements {after} bytes instead of {}",
            before - owned_and_removed
        );
    }
});

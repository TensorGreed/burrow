//! A split output carries nothing derived from the pages it excluded.
//!
//! **The required test for [ADR 0019](../../../docs/adr/0019-how-split-builds-its-outputs.md)
//! §2**, which states the rule for every operation whose output is a subset of its input --
//! split, extract, and above all redact:
//!
//! > The emitted bytes must contain no data derived from the excluded content. Not the
//! > content, and not anything computed from it: titles, names, destinations, counts,
//! > thumbnails, or index entries that describe what was taken away.
//!
//! It is the property redaction depends on, stated for redaction in ADR 0006's R8-R10: what
//! matters is the bytes that are **emitted**, not what the operation meant to emit.
//!
//! # Why it is a scan and not a structural walk
//!
//! Walking the output's catalog would mean writing a second PDF parser and trusting it, and a
//! leak that arrived through a key that walk did not know about would be invisible. The
//! fixture gives every feature on every page a canary naming its own page, so the question
//! "did anything from page 4 come along" is a search over bytes.
//!
//! # Why the output is decompressed first
//!
//! `qpdf --qdf --object-streams=disable`. This is a NEW technique here, not a borrowed one:
//! `core/burrow-engines/tests/secret_leak.rs` asks a different question and never invokes the
//! CLI. Saying otherwise would lend this expansion authority a sibling test does not give it —
//! and it is the part that has already gone silently wrong once. qpdf flates streams and packs objects into
//! object streams on write, so a canary sitting in a compressed stream is **not in the output
//! bytes at all** -- and a scan of the raw bytes would report silence for the wrong reason.
//! That failure has already happened once in this repository, to `merge`'s first order test.
//!
//! # The control
//!
//! Two of them, because the scan can be wrong in two directions:
//!
//!   * `the_scan_finds_a_canary_that_is_really_there` -- run the identical scan over the
//!     SOURCE, where every canary is present by construction, and require it to report a leak.
//!     Without this, a scan that had stopped finding anything reports the same green as one
//!     that works.
//!   * `the_included_pages_canaries_do_survive` -- the output must still contain the canaries
//!     of the pages it kept. Otherwise "no excluded canary" would be satisfied perfectly by an
//!     operation that emitted an empty document.

#![cfg(all(feature = "native-engines", target_os = "linux"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::process::{Command, Stdio};
use std::sync::Arc;

use burrow_engines::OpenOptions;
use burrow_engines::qpdf::Qpdf;
use burrow_ops::{Cuts, split};
use burrow_types::{Clock, Limits, ManualClock};

/// Pages in the generated fixture.
const PAGES: u64 = 5;

fn options() -> OpenOptions<'static> {
    OpenOptions::new(
        Limits::DEFAULT,
        Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
    )
}

/// One of the generated fixtures, by name.
///
/// Included by running the script, not by reimplementing it: `core/CLAUDE.md`'s rule that two
/// generators agree until the day they do not applies to fixtures as much as to code.
fn fixture(name: &str) -> Vec<u8> {
    // ONCE PER PROCESS, and that is not an optimisation. The first version generated into a
    // directory keyed by process id and ran the generator per test; cargo runs the tests in
    // this file in parallel threads of ONE process, so three of them wrote the same path while
    // reading it, and a test read a half-written file and reported the fixture as damaged.
    // A shared mutable path is a shared mutable path even when it is only a fixture.
    static DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    let dir = DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("burrow-split-canary-{}", std::process::id()));
        let status = Command::new("python3")
            .arg(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../tools/make-split-fidelity-fixtures.py"),
            )
            .arg(&dir)
            .stdout(Stdio::null())
            .status()
            .expect("python3 is needed to generate the split fidelity fixtures");
        assert!(status.success(), "the fixture generator failed");
        dir
    });
    std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("fixture {name}: {e}"))
}

fn canary_fixture() -> Vec<u8> {
    fixture("canary-per-page.pdf")
}

/// `qpdf --qdf` the bytes so a canary inside a compressed stream is findable.
///
/// **Fails loudly rather than falling back.** An earlier harness returned the raw bytes when
/// the CLI was missing, which would make this test report "no leak" for a document it could
/// not read into. A leak test that degrades to silence is worse than no leak test.
fn expanded(bytes: &[u8]) -> Vec<u8> {
    // VIA TEMP FILES, not stdin. `qpdf --qdf - -` looks like the obvious spelling and this
    // qpdf does not accept it: it reports `open -: No such file or directory` and writes
    // nothing. The first version of this helper used it, and because it also had a fallback
    // to the raw bytes, it silently scanned COMPRESSED output and reported no leak.
    let dir = std::env::temp_dir();
    let tag = format!(
        "burrow-split-expand-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    );
    let input = dir.join(format!("{tag}-in.pdf"));
    let output = dir.join(format!("{tag}-out.pdf"));
    std::fs::write(&input, bytes).expect("write the document to expand");

    let status = Command::new("qpdf")
        .args(["--qdf", "--object-streams=disable"])
        .arg(&input)
        .arg(&output)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect(
            "the `qpdf` CLI is needed to decompress split output before scanning it for \
             canaries; without it this test cannot see inside a flated stream",
        );
    // `--qdf` exits 3 on warnings, which our own fixtures do not produce but a future one
    // might; 0 and 3 both mean a file was written.
    assert!(
        matches!(status.code(), Some(0 | 3)),
        "qpdf could not expand the document ({status:?}), so nothing could be scanned"
    );

    let expanded = std::fs::read(&output).expect("read the expanded document");
    // NO FALLBACK. A leak test that degrades to scanning compressed bytes reports silence for
    // the wrong reason, which is the failure this whole file exists to prevent one layer down.
    assert!(
        !expanded.is_empty(),
        "qpdf produced an empty expansion, so nothing could be scanned"
    );
    let _ = std::fs::remove_file(&input);
    let _ = std::fs::remove_file(&output);
    expanded
}

/// Every canary belonging to a page that is NOT in `kept`, found in `bytes`.
fn leaked_canaries(bytes: &[u8], kept: &[u64]) -> Vec<String> {
    let text = expanded(bytes);
    let mut found = Vec::new();
    for page in 1..=PAGES {
        if kept.contains(&page) {
            continue;
        }
        // Every category the fixture plants, so a leak through any one of them is caught.
        for kind in ["outline", "field", "annot", "attach", "bytes"] {
            let canary = format!("BURROWCANARY-{kind}-{page}");
            if contains(&text, &canary) {
                found.push(canary);
            }
        }
    }
    found
}

/// Whether `text` contains `needle`, **in either encoding a producer might have written it**.
///
/// A byte-literal ASCII search is not enough. Real outline titles and field names are very
/// often UTF-16BE text strings, which qpdf round-trips as hex — so a scan looking only for
/// ASCII would report silence about a document that says the thing in plain sight. Found by
/// security review; `shared-objects.pdf` plants one canary in UTF-16BE precisely so a
/// regression here fails rather than passing quietly.
fn contains(text: &[u8], needle: &str) -> bool {
    if text.windows(needle.len()).any(|w| w == needle.as_bytes()) {
        return true;
    }
    let hex: String = needle
        .chars()
        .map(|c| format!("{:04X}", c as u32))
        .collect();
    for form in [hex.clone(), format!("FEFF{hex}")] {
        for candidate in [form.to_uppercase(), form.to_lowercase()] {
            if text
                .windows(candidate.len())
                .any(|w| w == candidate.as_bytes())
            {
                return true;
            }
        }
    }
    false
}

fn split_fixture(cuts: &[u64]) -> Vec<Vec<u8>> {
    split(
        &Qpdf::new(),
        canary_fixture().into_boxed_slice(),
        Cuts::after_pages(cuts),
        &options(),
    )
    .expect("the canary fixture must split")
}

#[test]
fn no_output_carries_a_canary_from_a_page_it_excluded() {
    // Three parts: pages 1-2, 3, 4-5. Every part excludes something, and the middle one
    // excludes pages on both sides of itself.
    let outputs = split_fixture(&[2, 3]);
    assert_eq!(outputs.len(), 3);

    let kept: [&[u64]; 3] = [&[1, 2], &[3], &[4, 5]];
    for (index, (output, keeps)) in outputs.iter().zip(kept).enumerate() {
        let leaks = leaked_canaries(output, keeps);
        assert!(
            leaks.is_empty(),
            "output {index} keeps pages {keeps:?} and carries data derived from pages it \
             excluded: {leaks:?}. ADR 0019 §2 -- this is the property redaction depends on."
        );
    }
}

/// Canaries in the shared-object fixture that name a page this output does not contain.
///
/// Separate from `leaked_canaries` because this fixture names its canaries by CHANNEL rather
/// than by category-and-page: each one says which page it belongs to in its own text, and the
/// channels are what the enumeration is about.
fn shared_object_leaks(bytes: &[u8]) -> Vec<&'static str> {
    let text = expanded(bytes);
    [
        "LEAKCANARY-resource-only-page-4-draws-it",
        "LEAKCANARY-fieldgroup-spans-1-and-4",
        "LEAKCANARY-utf16-typed-on-page-4",
        "LEAKCANARY-widget-page-4",
        "LEAKCANARY-annot-page-4",
        "LEAKCANARY-thread-covers-page-4",
    ]
    .into_iter()
    .filter(|canary| contains(&text, canary))
    .collect()
}

#[test]
#[ignore = "ADR 0019 §2a: six measured channels still carry data from excluded pages. \
            This is the test that says so; it is ignored rather than deleted so the gap is \
            visible in the suite rather than only in a document. See issue #54."]
fn no_output_carries_anything_from_a_page_it_excluded_even_when_objects_are_shared() {
    // THE TEST THE ORIGINAL ONE SHOULD HAVE BEEN. `canary-per-page.pdf` gives every page its
    // own annotations, its own resources and its own everything -- precisely the structure in
    // which a subsetting leak CANNOT occur -- so its twenty canaries confirmed a case that was
    // never at risk. This fixture shares objects between page 1 (kept) and page 4 (excluded),
    // which is what documents real software emits look like.
    //
    // It is `#[ignore]`d and it is expected to FAIL when run. That is deliberate: ADR 0019 §2
    // states a rule the implementation does not yet meet, and a suite that simply omitted the
    // failing case would report the rule as satisfied.
    let outputs = split(
        &Qpdf::new(),
        fixture("shared-objects.pdf").into_boxed_slice(),
        Cuts::after_pages(&[1]),
        &options(),
    )
    .expect("the shared-object fixture must split");

    let leaks = shared_object_leaks(&outputs[0]);
    assert!(
        leaks.is_empty(),
        "the page-1-only output carries data derived from excluded pages: {leaks:?}"
    );
}

#[test]
fn the_measured_leak_channels_are_exactly_the_ones_recorded() {
    // THE GAP, PINNED. ADR 0019 §2a lists the channels that still carry excluded data. If a
    // future change closes one, this fails and the ADR must be updated; if a change OPENS a
    // new one, the ignored test above is the one that catches it. Neither direction is allowed
    // to happen quietly, which is the only honest way to ship a known-incomplete rule.
    let outputs = split(
        &Qpdf::new(),
        fixture("shared-objects.pdf").into_boxed_slice(),
        Cuts::after_pages(&[1]),
        &options(),
    )
    .expect("split");

    let leaks = shared_object_leaks(&outputs[0]);
    assert!(
        !leaks.is_empty(),
        "no leak was found in the shared-object fixture -- either the channels have been \
         closed (update ADR 0019 §2a and un-ignore the test above) or the scan has gone \
         blind, and those two look identical from here"
    );
    // Named individually, so closing one is visible rather than absorbed into a count.
    for expected in [
        "LEAKCANARY-resource-only-page-4-draws-it",
        "LEAKCANARY-fieldgroup-spans-1-and-4",
        "LEAKCANARY-utf16-typed-on-page-4",
    ] {
        assert!(
            leaks.contains(&expected),
            "ADR 0019 §2a records {expected} as a live channel and it did not appear; the \
             record and the code disagree"
        );
    }
}

#[test]
fn the_included_pages_canaries_do_survive() {
    // THE FIRST CONTROL. Without it, "no excluded canary" is satisfied perfectly by an
    // operation that emits an empty document, which is the degenerate way to pass a
    // negative assertion.
    //
    // Only the per-page ANNOTATION is asserted, and that is deliberate rather than lazy:
    // ADR 0019 records that this route drops outlines and catalog-level attachments on
    // purpose, so asserting those survive would be asserting the opposite of the decision.
    // What must survive is what travels with a page.
    let outputs = split_fixture(&[2, 3]);
    let text = expanded(&outputs[1]);
    let canary = "BURROWCANARY-annot-3";
    assert!(
        text.windows(canary.len()).any(|w| w == canary.as_bytes()),
        "the output that keeps page 3 lost page 3's own annotation, so the leak assertion \
         above could be passing for the wrong reason"
    );
}

#[test]
fn the_scan_finds_a_canary_that_is_really_there() {
    // THE SECOND CONTROL, and the one ADR 0019 §3 requires: the identical scan, over bytes
    // that definitely contain excluded canaries, must report a leak. A scan that had stopped
    // finding anything -- a changed prefix, a broken expansion, a fixture whose markers moved
    // -- reports the same green as a scan that works.
    //
    // The source document is the honest leaker: it contains every page's canary by
    // construction, so scanning it while claiming to keep only page 3 must find the other
    // four pages' worth.
    let source = canary_fixture();
    let leaks = leaked_canaries(&source, &[3]);
    assert!(
        !leaks.is_empty(),
        "the scan found no canary in a document that contains all of them, so every \
         no-leak assertion in this file is measuring nothing"
    );
    // Every category, not merely one: a scan that had lost its attachment prefix would still
    // find outlines and look healthy.
    for kind in ["outline", "field", "annot", "attach", "bytes"] {
        assert!(
            leaks.iter().any(|c| c.contains(kind)),
            "the scan cannot see {kind} canaries, so a leak through one would be invisible"
        );
    }
}

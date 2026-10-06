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
//! # The controls
//!
//! Four, because a scan like this can be wrong in more directions than it can be right in, and
//! three of the four exist because the thing they guard against has already happened here:
//!
//!   * `the_scan_finds_a_canary_that_is_really_there` -- run the identical scan over the SOURCE,
//!     where every canary is present by construction, and require it to report a leak. Without
//!     this, a scan that had stopped finding anything reports the same green as one that works.
//!   * `the_included_pages_canaries_do_survive` -- the output must still contain the canaries of
//!     the pages it kept. Otherwise "no excluded canary" would be satisfied perfectly by an
//!     operation that emitted an empty document.
//!   * `the_scan_can_still_see_every_channel_it_is_the_gate_for` -- each of ADR 0019 §2a's
//!     channels, found by name in a document that has them all. It replaces
//!     `the_measured_leak_channels_are_exactly_the_ones_recorded`, which asserted the channels
//!     still fired while the rule was unmet. That assertion had to invert when #54 closed, not
//!     disappear: the state it was distinguishing -- a scan gone blind -- is exactly as reachable
//!     now as it was then, and now it looks like success rather than like failure.
//!   * `a_document_without_layers_is_not_caught_by_the_layer_refusal` -- the near-miss for the
//!     optional-content refusal. A refusal that fired on every document would close §2a row 6
//!     perfectly, make `split` useless, and be indistinguishable from a correct one by the test
//!     that only checks the refusal fires.
//!
//! # Two layers, and neither is sufficient
//!
//! This file is the canary layer. `tests/subset_closure.rs` is the structural one, and ADR 0019
//! §3 requires both. The structural harness asserts a property over every object in the source,
//! so it fails for categories nobody named -- but it cannot see a leak **inside** an object that
//! legitimately survives, which is what §2a rows 2 and 5 are: a form field owned by a kept and an
//! excluded page carrying the value typed on the excluded one, and a named destination sitting in
//! a kept page's own link. Those are what the canaries here are for.

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
use burrow_types::{Clock, Error, Limits, ManualClock};

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

/// How many times `key` appears as a dictionary key in the expanded bytes.
///
/// `qpdf --qdf` writes one key per line, indented, so a key is `\n` + spaces + the name. Matching
/// that rather than the bare name is what stops `/Dest` matching inside `/Destination` or inside a
/// string somebody wrote.
fn part_key_count(text: &[u8], key: &[u8]) -> usize {
    let mut found = 0;
    let mut at = 0;
    while let Some(next) = text.get(at..).and_then(|rest| {
        rest.windows(key.len())
            .position(|window| window == key)
            .map(|found_at| at + found_at)
    }) {
        // Preceded only by white space back to a newline, and followed by a delimiter or space.
        let before_is_indent = text
            .get(..next)
            .and_then(|head| head.iter().rposition(|byte| *byte == b'\n'))
            .is_some_and(|line| {
                text.get(line + 1..next)
                    .is_some_and(|gap| gap.iter().all(|byte| *byte == b' '))
            });
        let after = text.get(next + key.len()).copied();
        let after_ends_it = after.is_none_or(|byte| byte == b' ' || byte == b'[' || byte == b'\n');
        if before_is_indent && after_ends_it {
            found += 1;
        }
        at = next + key.len();
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

/// Every channel `shared-objects.pdf` plants, in one place.
///
/// **One list, read by both the scan and the control that proves the scan works.** They were two
/// copies, and the count assertion built on the second was a tautology: it re-listed these names,
/// checked each, then asserted the total was seven. Code review measured that it could not fail
/// independently of the checks above it.
const CHANNELS: [&str; 8] = [
    "LEAKCANARY-resource-only-page-4-draws-it",
    "LEAKCANARY-fieldgroup-spans-1-and-4",
    "LEAKCANARY-utf16-typed-on-page-4",
    "LEAKCANARY-widget-page-4",
    "LEAKCANARY-annot-page-4",
    "LEAKCANARY-thread-covers-page-4",
    // ADR 0019 §2a ROW 5, which had no fixture and no test until #54 closed. It rides INSIDE an
    // object the output is allowed to keep -- page 1's own link -- so the structural closure
    // harness cannot see it by construction. This is what the named canaries are for.
    "LEAKCANARY-namedest-page-4",
    // Reachable only through `/Stash`, a key on the inherited `/Resources` that is not one of
    // the seven resource categories. The filter iterated the categories and filtered inside
    // each, so anything else came through whole -- ADR 0019 §2a row 1 leaking through a key
    // nobody enumerated, which is what the page-key rule is an allowlist to avoid. Found by
    // security review; the resource filter is an allowlist now too.
    "LEAKCANARY-stash-belongs-to-page-4",
];

/// Canaries in the shared-object fixture that name a page this output does not contain.
///
/// Separate from `leaked_canaries` because this fixture names its canaries by CHANNEL rather
/// than by category-and-page: each one says which page it belongs to in its own text, and the
/// channels are what the enumeration is about.
fn shared_object_leaks(bytes: &[u8]) -> Vec<&'static str> {
    let text = expanded(bytes);
    CHANNELS
        .into_iter()
        .filter(|canary| contains(&text, canary))
        .collect()
}

#[test]
fn no_output_carries_anything_from_a_page_it_excluded_even_when_objects_are_shared() {
    // THE TEST THE ORIGINAL ONE SHOULD HAVE BEEN. `canary-per-page.pdf` gives every page its
    // own annotations, its own resources and its own everything -- precisely the structure in
    // which a subsetting leak CANNOT occur -- so its twenty canaries confirmed a case that was
    // never at risk. This fixture shares objects between page 1 (kept) and page 4 (excluded),
    // which is what documents real software emits look like.
    //
    // It was `#[ignore]`d and expected to FAIL for the whole of M1, because ADR 0019 §2 stated a
    // rule the implementation did not meet. Issue #54 closed it: `qpdf/prune.rs` takes back out
    // what `qpdf_add_page`'s reachability closure dragged in.
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
fn the_scan_can_still_see_every_channel_it_is_the_gate_for() {
    // THE REPLACEMENT FOR `the_measured_leak_channels_are_exactly_the_ones_recorded`, and it is
    // a replacement rather than a deletion on purpose.
    //
    // That test asserted the §2a channels still FIRED. While the rule was unmet it was the only
    // thing separating two states that look identical from a green run: the channels closing,
    // and the scan going blind. Now that pruning has landed the first state is the goal, so the
    // assertion has to be inverted rather than dropped -- because the second state has not gone
    // anywhere. A changed canary prefix, a fixture whose markers moved, an expansion that
    // silently stopped decompressing, and `shared_object_leaks` returns an empty list over an
    // output that is full of leaks, and every no-leak assertion above passes.
    //
    // So: the identical scan, over the SOURCE, where every canary is present by construction.
    // Each of the six must be found BY NAME. That keeps "the scan can see this channel" asserted
    // per channel, which is the property the old test was really carrying.
    let source = fixture("shared-objects.pdf");
    let seen = shared_object_leaks(&source);

    // EVERY CHANNEL THE SCAN ENUMERATES, derived from the scan's own list rather than repeated
    // here. The first version listed the seven names again and then asserted `seen.len() == 7`
    // -- which, after seven `contains` checks, cannot fail independently of them and is not the
    // derived denominator its comment claimed it was. Found by code review. `CHANNELS` is now
    // the one place the list lives, and `shared_object_leaks` scans exactly it.
    for channel in CHANNELS {
        assert!(
            seen.contains(&channel),
            "the scan cannot see {channel} in a document that definitely contains it, so the \
             no-leak assertion for that channel is measuring nothing"
        );
    }
}

#[test]
fn a_document_that_uses_layers_is_refused_rather_than_split() {
    // ADR 0019 §2a ROW 6, which had no fixture and no test at all before #54.
    //
    // It is the channel a canary scan is the wrong instrument for: dropping the catalog's
    // `/OCProperties` while keeping the OCGs a page references does not move a string anywhere,
    // it makes content the source HID visible. §2b forbids dropping the configuration alone for
    // that reason, and carrying a pruned one needs the destination's catalog -- which ADR 0013's
    // caller rule puts out of reach, because `qpdf_get_root` is `QTC::TC(…); return
    // trap_oh_errors(…)` and is not on the trapped list.
    //
    // So the answer is a refusal, and this is the assertion that it fires rather than the split
    // quietly succeeding and revealing a watermark somebody turned off.
    let result = split(
        &Qpdf::new(),
        fixture("optional-content.pdf").into_boxed_slice(),
        Cuts::after_pages(&[1]),
        &options(),
    );
    assert!(
        matches!(result, Err(Error::Unsupported(_))),
        "a document whose page draws inside a layer that `/D /OFF` turns off was split; the \
         part would show content the source hid. Got {result:?}"
    );
}

#[test]
fn a_page_whose_content_cannot_be_tokenised_is_refused_rather_than_guessed_at() {
    // A BEHAVIOUR CHANGE #54 BROUGHT, pinned rather than left to be discovered from a bug
    // report. A document with a malformed content stream -- here a stray `)` -- opens and splits
    // on any other tool. burrow refuses it, because the resource filter has to know which names
    // the page uses and a PARTIAL answer deletes a resource the page draws with. There is no
    // third option: keeping every resource on a stream we could not read would carry the
    // excluded pages' fonts, which is the leak this whole file is about.
    //
    // The refusal is the safe direction and it costs something real. That is worth a test
    // stating it, because "split got stricter" is not visible from the signature.
    let result = split(
        &Qpdf::new(),
        fixture("unlexable-content.pdf").into_boxed_slice(),
        Cuts::after_pages(&[1]),
        &options(),
    );
    assert!(
        matches!(result, Err(Error::Malformed(_))),
        "a page whose content stream cannot be tokenised was split, so the resource filter ran \
         on a partial name set. Got {result:?}"
    );
}

#[test]
fn a_stream_that_cannot_be_decoded_is_refused_rather_than_scanned_compressed() {
    // THE OTHER REFUSAL, and the one that is easiest to get wrong quietly. qpdf reports whether
    // it actually decoded a stream, and at `qpdf_dl_specialized` it will not decode a lossy
    // filter -- so the bytes come back COMPRESSED. Lexing those for resource names finds a
    // handful of accidents, which is an under-approximation, which deletes resources the page
    // uses. A caller that treated "not decoded" as "empty" would produce a document that opens
    // and renders wrong.
    let result = split(
        &Qpdf::new(),
        fixture("undecodable-stream.pdf").into_boxed_slice(),
        Cuts::after_pages(&[1]),
        &options(),
    );
    assert!(
        matches!(result, Err(Error::Unsupported(_))),
        "a page reaching a stream qpdf could not decode was split, so its names were read from \
         compressed bytes. Got {result:?}"
    );
}

#[test]
fn a_link_survives_exactly_when_it_still_points_into_the_output() {
    // THE FOUR-WAY DECISION, all in one output, because each of the four is how one of the other
    // three could be wrong. "Drop everything" passes the two negative cases; "keep everything"
    // passes the two positive ones. Only all four together say the rule is the rule.
    //
    // `split` used to drop every `/A` and `/Dest`. Measured: `qpdf_add_page`'s copier maps a
    // destination to a page it copied and reserves a NULL for one it did not -- so a link into the
    // output survives and works, and an outward one is already inert. Dropping the first was a
    // fidelity loss with no privacy gain.
    let outputs = split(
        &Qpdf::new(),
        fixture("destinations.pdf").into_boxed_slice(),
        // Pages 1-2 | 3-5, so page 2 is in the same part as page 1 and page 4 is not.
        Cuts::after_pages(&[2]),
        &options(),
    )
    .expect("the destination fixture must split");
    let text = expanded(&outputs[0]);

    for (needle, why) in [
        (
            "BURROWMARK dest-into-part",
            "a link whose destination is a page in this very output was dropped",
        ),
        (
            "BURROWMARK-uri",
            "a web address was dropped; it describes nothing that was excluded",
        ),
    ] {
        assert!(
            contains(&text, needle),
            "{why} -- the rule is over-dropping, which is a fidelity loss with no privacy gain"
        );
    }

    // AND THE TWO THAT MAY NOT SURVIVE. The named destination is ADR 0019 §2a row 5; the outward
    // explicit one resolves to null and carries nothing, but a link that goes nowhere is not
    // something to keep either.
    assert!(
        !contains(&text, "LEAKCANARY-namedest-page-4"),
        "a named destination survived -- the name is the leak, and it names a page this output \
         does not contain"
    );
    // AND THE OUTWARD EXPLICIT ONE. Counted rather than searched for: the annotation itself stays
    // -- its `/Contents` is the kept page's own text and carries nothing -- so what has to be gone
    // is its `/Dest` key, and exactly one `/Dest` may remain in the whole output: the inward
    // link's. The first version of this assertion looked for the annotation's marker and found it,
    // which said nothing about the key.
    let remaining = part_key_count(&text, b"/Dest");
    assert_eq!(
        remaining, 1,
        "the output has {remaining} `/Dest` key(s); exactly one -- the link into this output -- \
         may survive, and the one pointing at an excluded page may not"
    );
}

#[test]
fn a_page_that_draws_images_is_split_rather_than_refused() {
    // THE WORST DEFECT SECURITY REVIEW FOUND, as a regression case. `/XObject` holds images as
    // well as forms, and the walk followed anything in it that was a stream -- so it tried to
    // DECODE them. A JPEG cannot be decoded at `qpdf_dl_specialized`, which the walk turned into
    // a refusal, so **every document containing a JPEG** was refused. A flate image decoded and
    // then failed to lex as PDF syntax whenever its pixels held an unbalanced `(`.
    //
    // This fixture has one of each. An image names no resources, so following one was never
    // useful -- only expensive, and then fatal.
    let outputs = split(
        &Qpdf::new(),
        fixture("images.pdf").into_boxed_slice(),
        Cuts::after_pages(&[1]),
        &options(),
    );
    assert!(
        outputs.is_ok(),
        "a page drawing a JPEG and a flate image was refused: {outputs:?}"
    );
}

#[test]
fn a_font_named_two_levels_down_is_not_pruned_off_the_page() {
    // THE OVER-PRUNE DIRECTION, which is the one that produces a file that opens and renders
    // wrong. A form draws a form which draws text in `/F1`, and the inner form has no
    // `/Resources` of its own — so `/F1` resolves against the page's. The walk resolved names
    // against the page's categories only, never reached the inner form, and pruned the font off
    // the page while the form still asked for it. Security review measured the font object
    // leaving the file entirely.
    let outputs = split(
        &Qpdf::new(),
        fixture("nested-forms.pdf").into_boxed_slice(),
        Cuts::after_pages(&[1]),
        &options(),
    )
    .expect("a document with nested forms must split");
    let text = expanded(&outputs[0]);
    assert!(
        text.windows(9).any(|w| w == b"Helvetica"),
        "the font the inner form draws with was pruned off the page; the part still asks for \
         `/F1` and the file no longer contains it"
    );
}

#[test]
fn a_layer_one_level_down_is_refused_like_one_on_the_page() {
    // ADR 0019 §2a ROW 6, at depth. The refusal read the page's own `/Resources /Properties`, so
    // an OCG inside a form XObject's own resources was past it — and security review measured
    // that document splitting happily, with the hidden text visible in the output and the
    // layer's name still in it. It is the shape Illustrator and InDesign emit, not an
    // adversarial one.
    let result = split(
        &Qpdf::new(),
        fixture("oc-nested.pdf").into_boxed_slice(),
        Cuts::after_pages(&[1]),
        &options(),
    );
    assert!(
        matches!(result, Err(Error::Unsupported(_))),
        "a hidden layer one level down was split rather than refused, so content the source hid \
         is visible in the part. Got {result:?}"
    );
}

#[test]
fn an_ordinary_document_is_not_caught_by_either_refusal() {
    // THE NEAR-MISS FOR BOTH. A tokeniser that refused everything, or a decode check that read
    // every stream as unfiltered, would satisfy the two assertions above perfectly and make
    // `split` refuse every document in the world.
    let outputs = split(
        &Qpdf::new(),
        canary_fixture().into_boxed_slice(),
        Cuts::after_pages(&[2]),
        &options(),
    );
    assert!(
        outputs.is_ok(),
        "an ordinary five-page document was refused: {outputs:?}"
    );
}

#[test]
fn a_document_without_layers_is_not_caught_by_the_layer_refusal() {
    // THE NEAR-MISS FOR THE RULE ABOVE. A refusal that fired on everything would close the
    // channel perfectly and make `split` useless, and it would look identical to a correct one
    // from the test above alone. `/Properties` also carries ordinary marked-content property
    // lists -- every tagged PDF has them -- so the detector keys on `/Type /OCG` and `/OCMD`
    // rather than on the category being present.
    let outputs = split(
        &Qpdf::new(),
        fixture("shared-objects.pdf").into_boxed_slice(),
        Cuts::after_pages(&[1]),
        &options(),
    );
    assert!(
        outputs.is_ok(),
        "the optional-content refusal fired on a document that has no layers: {outputs:?}"
    );
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

/// A document of `objects`, numbered from 1, object 1 the catalogue.
fn raw_pdf(objects: &[String]) -> Vec<u8> {
    let mut out = String::from("%PDF-1.7\n");
    let mut offsets = Vec::new();
    for (index, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.push_str(&format!("{} 0 obj\n{body}\nendobj\n", index + 1));
    }
    let xref_at = out.len();
    out.push_str(&format!(
        "xref\n0 {}\n0000000000 65535 f \n",
        objects.len() + 1
    ));
    for offset in &offsets {
        out.push_str(&format!("{offset:010} 00000 n \n"));
    }
    out.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
        objects.len() + 1
    ));
    out.into_bytes()
}

fn content(data: &str) -> String {
    format!("<< /Length {} >>\nstream\n{data}\nendstream", data.len())
}

/// Page 1 draws `/F1` after an inline image, filtered or not; `/F2` (Courier) is in its resources
/// and drawn by nothing on it.
///
/// The filtered image's `/L 1` ends its data after `8`, so a reader that trusted `/L` would read
/// on and take `/ImX Do` as page 1 drawing `/ImX` -- which, in the fixtures that have one, is an
/// image only the excluded page draws. That is what makes "pruning trusts `/L`" show up as the
/// excluded page's bytes in the output rather than as a missing refusal.
fn page_one(filtered: bool) -> String {
    let image = if filtered {
        "BI /W 1 /H 1 /BPC 8 /CS /G /F /AHx /L 1 ID 8 EI /ImX Do EI"
    } else {
        "BI /W 1 /H 1 /BPC 8 /CS /G ID x EI"
    };
    content(&format!(
        "q 10 0 0 10 50 50 cm {image} Q BT /F1 12 Tf 10 10 Td (ONE) Tj ET"
    ))
}

const HELVETICA: &str = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>";
const COURIER: &str = "<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>";
/// An image only the excluded page draws; its bytes are the canary.
const EXCLUDED_IMAGE: &str = "<< /Type /XObject /Subtype /Image /Width 18 /Height 1 /ColorSpace \
                              /DeviceGray /BitsPerComponent 8 /Length 18 >>\nstream\nEXCLUDEDIMAGEBYTES\nendstream";

fn split_first(bytes: Vec<u8>, after: &[u64]) -> burrow_types::Result<Vec<Vec<u8>>> {
    split(
        &Qpdf::new(),
        bytes.into_boxed_slice(),
        Cuts::after_pages(after),
        &options(),
    )
}

/// #228: a page whose read stops at a filtered inline image refuses the split, and the reason
/// names the image. Where an `Ok` would carry `leak` -- the excluded page's bytes, or a layer the
/// source hid -- into the first output, that is asserted first, so a regression reads as the leak
/// it is rather than as a missing refusal.
fn assert_refused_naming_the_image(
    result: burrow_types::Result<Vec<Vec<u8>>>,
    leak: Option<&str>,
    why: &str,
) {
    match result {
        Err(error) => {
            let reason = format!("{error:?}");
            assert!(
                reason.contains("[split-inline-image-filtered]")
                    && reason.contains("filtered inline image"),
                "{why}: refused, but not for the image: {reason}"
            );
        }
        Ok(outputs) => {
            if let Some(leak) = leak {
                assert!(
                    !contains(&expanded(&outputs[0]), leak),
                    "{why}: {leak} reached the first output"
                );
            }
            panic!("{why}: returned Ok, where a filtered inline image must refuse the split");
        }
    }
}

#[test]
fn a_filtered_inline_image_on_a_page_with_its_own_resources_refuses() {
    // (a) #228: a filtered inline image ends where its filter's data ends, which burrow cannot
    // find, so the names after it are unread. Even with resources nobody else reaches, the split
    // refuses: "nobody else reaches them" was measured to be unanswerable by dictionary identity
    // (owner, 2026-10-04).
    let bytes = raw_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 5 0 R \
         /Resources << /Font << /F1 6 0 R /F2 7 0 R >> >> >>"
            .to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 8 0 R \
         /Resources << /Font << /F1 6 0 R >> >> >>"
            .to_owned(),
        page_one(true),
        HELVETICA.to_owned(),
        COURIER.to_owned(),
        content("BT /F1 12 Tf 10 10 Td (TWO) Tj ET"),
    ]);
    assert_refused_naming_the_image(split_first(bytes, &[1]), None, "(a) own resources");
}

#[test]
fn a_filtered_inline_image_on_pages_that_share_resources_only_with_each_other_refuses() {
    // (b) Pages 1 and 2 share one `/Resources`, and both are in the first output; page 3 is not.
    let bytes = raw_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 3 /Kids [3 0 R 4 0 R 5 0 R] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 6 0 R /Resources 9 0 R >>"
            .to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 7 0 R /Resources 9 0 R >>"
            .to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 7 0 R \
         /Resources << /Font << /F1 10 0 R >> >> >>"
            .to_owned(),
        page_one(true),
        content("BT /F1 12 Tf 10 10 Td (TWO) Tj ET"),
        "null".to_owned(),
        "<< /Font << /F1 10 0 R /F2 11 0 R >> >>".to_owned(),
        HELVETICA.to_owned(),
        COURIER.to_owned(),
    ]);
    assert_refused_naming_the_image(split_first(bytes, &[2]), None, "(b) shared within");
}

/// Two pages under one `/Pages` node carrying `/Resources` with an image only page 2 draws; page
/// 1's inline image filtered or not. The first output is page 1 alone.
fn inherited(filtered: bool) -> Vec<u8> {
    raw_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] \
         /Resources << /Font << /F1 7 0 R >> /XObject << /ImX 8 0 R >> >> >>"
            .to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 5 0 R >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 6 0 R >>".to_owned(),
        page_one(filtered),
        content("q 100 0 0 10 10 10 cm /ImX Do Q"),
        HELVETICA.to_owned(),
        EXCLUDED_IMAGE.to_owned(),
    ])
}

#[test]
fn a_filtered_inline_image_under_resources_an_excluded_page_inherits_refuses() {
    // (c) THE §2b CASE: page 1's `/Resources` -- inherited from the `/Pages` node -- is page 2's
    // too, which this output excludes. Kept whole, page 2's image would travel with page 1; read
    // past the image by its `/L`, `/ImX` would be "used" and kept.
    assert_refused_naming_the_image(
        split_first(inherited(true), &[1]),
        Some("EXCLUDEDIMAGEBYTES"),
        "(c) inherited",
    );
}

#[test]
fn the_same_inheritance_with_an_unfiltered_image_is_pruned_normally() {
    // (d) THE TWIN: an unfiltered inline image's extent is computed, every name is read, and the
    // inherited dictionary is pruned as before -- the excluded page's image does not travel.
    let outputs = split_first(inherited(false), &[1]).expect("an unfiltered image splits");
    let text = expanded(&outputs[0]);
    assert!(
        !contains(&text, "EXCLUDEDIMAGEBYTES"),
        "the excluded page's image was pruned"
    );
    assert!(
        contains(&text, "Helvetica"),
        "the font page 1 draws with stays"
    );
}

#[test]
fn a_filtered_inline_image_beside_an_entry_only_an_excluded_page_draws_refuses() {
    // THE SPEC REVIEW'S S1: page 1's `/Resources` is its own and shared with nobody, but its own
    // `/XObject` lists an image only page 2 draws. Keeping it whole -- the fallback this PR first
    // shipped -- carried page 2's image into page 1's output.
    let bytes = raw_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 5 0 R \
         /Resources << /Font << /F1 7 0 R >> /XObject << /ImB 8 0 R >> >> >>"
            .to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 6 0 R \
         /Resources << /XObject << /ImB 8 0 R >> >> >>"
            .to_owned(),
        page_one(true),
        content("q 100 0 0 10 10 10 cm /ImB Do Q"),
        HELVETICA.to_owned(),
        EXCLUDED_IMAGE.to_owned(),
    ]);
    assert_refused_naming_the_image(
        split_first(bytes, &[1]),
        Some("EXCLUDEDIMAGEBYTES"),
        "an entry only page 2 draws",
    );
}

#[test]
fn a_category_dictionary_an_excluded_page_shares_refuses_too() {
    // Page 1's `/Resources` is its own, but its `/XObject` is the same object as page 2's.
    let bytes = raw_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 5 0 R \
         /Resources << /Font << /F1 7 0 R >> /XObject 9 0 R >> >>"
            .to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 6 0 R \
         /Resources << /XObject 9 0 R >> >>"
            .to_owned(),
        page_one(true),
        content("q 100 0 0 10 10 10 cm /ImX Do Q"),
        HELVETICA.to_owned(),
        EXCLUDED_IMAGE.to_owned(),
        "<< /ImX 8 0 R >>".to_owned(),
    ]);
    assert_refused_naming_the_image(
        split_first(bytes, &[1]),
        Some("EXCLUDEDIMAGEBYTES"),
        "a shared /XObject",
    );
}

#[test]
fn a_hidden_layer_in_a_form_drawn_after_a_filtered_inline_image_refuses() {
    // THE REGRESSION BOTH REVIEWS FOUND. ADR 0019 §2a row 6 refuses a hidden layer one level
    // down, but that check runs on what the walk visits -- and a form drawn after a filtered
    // image is never visited, because its name is never read. Kept whole, the part had the
    // hidden text in it and no `/OCProperties` to hide it: 1,681 dark pixels from a page that
    // rendered none.
    let form_data = "/OC /L1 BDC BT /F1 12 Tf 10 10 Td (HIDDENLAYERTEXT) Tj ET EMC";
    let bytes = raw_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R \
         /OCProperties << /OCGs [9 0 R] /D << /OFF [9 0 R] >> >> >>"
            .to_owned(),
        "<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 5 0 R \
         /Resources << /Font << /F1 7 0 R >> /XObject << /Fm 8 0 R >> >> >>"
            .to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 6 0 R \
         /Resources << /Font << /F1 7 0 R >> >> >>"
            .to_owned(),
        content(
            "q 10 0 0 10 50 50 cm BI /W 1 /H 1 /BPC 8 /CS /G /F /AHx ID 80> EI Q /Fm Do \
             BT /F1 12 Tf 10 10 Td (ONE) Tj ET",
        ),
        content("BT /F1 12 Tf 10 10 Td (TWO) Tj ET"),
        HELVETICA.to_owned(),
        format!(
            "<< /Type /XObject /Subtype /Form /BBox [0 0 200 200] \
             /Resources << /Font << /F1 7 0 R >> /Properties << /L1 9 0 R >> >> \
             /Length {} >>\nstream\n{form_data}\nendstream",
            form_data.len()
        ),
        "<< /Type /OCG /Name (Hidden) >>".to_owned(),
    ]);
    assert_refused_naming_the_image(
        split_first(bytes, &[1]),
        Some("HIDDENLAYERTEXT"),
        "a hidden layer after the image",
    );
}

#[test]
fn a_filtered_inline_image_inside_a_form_refuses_like_one_on_the_page() {
    // A PARTIAL READ ONE LEVEL DOWN. The page's own content reads to the end; the form it draws
    // holds the filtered image and then `/F2 Tf`. Pruning by that read would take Courier off a
    // page whose form draws with it.
    let data = "BI /W 1 /H 1 /BPC 8 /CS /G /F /AHx ID 80> EI BT /F2 9 Tf (F) Tj ET";
    let form = format!(
        "<< /Type /XObject /Subtype /Form /BBox [0 0 200 200] /Length {} >>\nstream\n{data}\nendstream",
        data.len()
    );
    let bytes = raw_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 5 0 R \
         /Resources << /XObject << /Fm 6 0 R >> /Font << /F2 7 0 R >> >> >>"
            .to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 8 0 R \
         /Resources << >> >>"
            .to_owned(),
        content("/Fm Do"),
        form,
        COURIER.to_owned(),
        content(""),
    ]);
    match split_first(bytes, &[1]) {
        Err(error) => assert!(
            format!("{error:?}").contains("[split-inline-image-filtered]"),
            "refused, but not for the image: {error:?}"
        ),
        Ok(outputs) => {
            assert!(
                contains(&expanded(&outputs[0]), "Courier"),
                "the font the form draws after its image was pruned off the page"
            );
            panic!("returned Ok, where a filtered inline image in a form must refuse the split");
        }
    }
}

#[test]
fn a_form_whose_names_the_page_already_read_still_refuses_on_its_filtered_image() {
    // ONE PASS, NO CACHE HIT (#228, second code review). The page names every word of the form's
    // image dictionary itself, so the form adds nothing new, the name walk ends after one pass,
    // and the form is never met in the cache. Only the fresh read's partial mark refuses here;
    // without it the split returned `Ok` with Courier, which the form draws with, pruned away.
    let data = "BI /W 1 /H 1 /BPC 8 /CS /G /F /AHx ID 80> EI BT /F2 9 Tf (F) Tj ET";
    let form = format!(
        "<< /Type /XObject /Subtype /Form /BBox [0 0 200 200] /Length {} >>\nstream\n{data}\nendstream",
        data.len()
    );
    let bytes = raw_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 5 0 R \
         /Resources << /XObject << /Fm 6 0 R >> /Font << /F2 7 0 R >> >> >>"
            .to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 8 0 R \
         /Resources << >> >>"
            .to_owned(),
        content(
            "/W BMC EMC /H BMC EMC /BPC BMC EMC /CS BMC EMC /G BMC EMC /F BMC EMC \
             /AHx BMC EMC /Fm Do",
        ),
        form,
        COURIER.to_owned(),
        content(""),
    ]);
    match split_first(bytes, &[1]) {
        Err(error) => assert!(
            format!("{error:?}").contains("[split-inline-image-filtered]"),
            "refused, but not for the image: {error:?}"
        ),
        Ok(outputs) => {
            assert!(
                contains(&expanded(&outputs[0]), "Courier"),
                "the font the form draws after its image was pruned off the page"
            );
            panic!("returned Ok, where a filtered inline image in a form must refuse the split");
        }
    }
}

#[test]
fn a_filtered_inline_image_in_an_annotation_appearance_refuses() {
    // The appearance stream of a kept annotation is read for names like the page's own content,
    // so a filtered image there stops the read the same way (#228, second code review: nothing
    // tested this path).
    let appearance = "BI /W 1 /H 1 /BPC 8 /CS /G /F /AHx ID 80> EI BT /F2 9 Tf (A) Tj ET";
    let bytes = raw_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 5 0 R \
         /Resources << /Font << /F1 6 0 R /F2 7 0 R >> >> /Annots [8 0 R] >>"
            .to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 10 0 R \
         /Resources << >> >>"
            .to_owned(),
        content("BT /F1 12 Tf 10 10 Td (ONE) Tj ET"),
        HELVETICA.to_owned(),
        COURIER.to_owned(),
        "<< /Type /Annot /Subtype /Square /Rect [10 10 100 100] /AP << /N 9 0 R >> >>".to_owned(),
        format!(
            "<< /Type /XObject /Subtype /Form /BBox [0 0 90 90] /Length {} >>\nstream\n{appearance}\nendstream",
            appearance.len()
        ),
        content(""),
    ]);
    assert_refused_naming_the_image(split_first(bytes, &[1]), None, "an appearance stream");
}

/// #257 fixtures: a name token longer than a renderer reads is refused, so split cannot carry a
/// hidden layer a renderer would draw from the truncated name. The active discriminator is the
/// `[name-token-too-long]` refusal marker. The hidden layer is really drawn (via `/Fm`), but that
/// is a LATENT guard: today, removing the lexer check does not leak -- split still refuses, via the
/// optional-content path -- so the `HIDDENLAYERTEXT`-absent branch is a backstop against a future
/// regression that removed both, not something these fixtures exercise now. The over-long name is
/// the refusal trigger, standing in for the draw operand PDFium would cut to a real resource.
fn hidden_layer_pdf_drawing(page_one_content: &str) -> Vec<u8> {
    let form_data = "/OC /L1 BDC BT /F1 12 Tf 10 10 Td (HIDDENLAYERTEXT) Tj ET EMC";
    raw_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R \
         /OCProperties << /OCGs [9 0 R] /D << /OFF [9 0 R] >> >> >>"
            .to_owned(),
        "<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 5 0 R \
         /Resources << /Font << /F1 7 0 R >> /XObject << /Fm 8 0 R >> >> >>"
            .to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 6 0 R \
         /Resources << /Font << /F1 7 0 R >> >> >>"
            .to_owned(),
        content(page_one_content),
        content("BT /F1 12 Tf 10 10 Td (TWO) Tj ET"),
        HELVETICA.to_owned(),
        format!(
            "<< /Type /XObject /Subtype /Form /BBox [0 0 200 200] \
             /Resources << /Font << /F1 7 0 R >> /Properties << /L1 9 0 R >> >> \
             /Length {} >>\nstream\n{form_data}\nendstream",
            form_data.len()
        ),
        "<< /Type /OCG /Name (Hidden) >>".to_owned(),
    ])
}

fn assert_refused_for_long_name(result: burrow_types::Result<Vec<Vec<u8>>>, why: &str) {
    match result {
        Err(error) => assert!(
            format!("{error:?}").contains("[name-token-too-long]"),
            "{why}: refused, but not for the over-long name: {error:?}"
        ),
        Ok(outputs) => {
            assert!(
                !contains(&expanded(&outputs[0]), "HIDDENLAYERTEXT"),
                "{why}: split carried the hidden layer into the part"
            );
            panic!("{why}: returned Ok, where an over-long name must refuse the split");
        }
    }
}

#[test]
fn a_name_token_longer_than_a_renderer_reads_refuses_the_split() {
    // 256 raw bytes (`/` + 255) -- one past PDFium's 255-byte cut, which would truncate it to a
    // 255-raw name and resolve a different resource. The hidden-layer form is drawn by `/Fm`; the
    // over-long name is the operand that carries the leak, refused before the walk resolves it.
    let long = format!("/{} Do", "A".repeat(255));
    assert_refused_for_long_name(
        split_first(hidden_layer_pdf_drawing(&format!("/Fm Do {long}")), &[1]),
        "a 256-raw-byte name",
    );
}

#[test]
fn a_name_long_in_raw_escapes_refuses_even_though_it_decodes_short() {
    // The #257 case the OLD decoded-length check missed: `/` + 100 x `#41` is 301 raw bytes and
    // decodes to "A" x 100 (100 bytes), so a decoded cap passed it while PDFium cut it to a
    // different resource. The RAW check refuses it.
    let escapes = "#41".repeat(100);
    let long = format!("/{escapes} Do");
    assert_refused_for_long_name(
        split_first(hidden_layer_pdf_drawing(&format!("/Fm Do {long}")), &[1]),
        "a 301-raw-byte escape name",
    );
}

#[test]
fn a_long_but_valid_name_at_the_ceiling_still_splits() {
    // THE NEAR-MISS. 255 raw bytes (`/` + 254) -- exactly the cut, which both readers read whole --
    // is a legitimate resource name and must NOT be refused, or the ceiling is a wall one byte low.
    // A real form keyed by that name, no hidden layer, drawn on page 1.
    let key = "A".repeat(254);
    let bytes = raw_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] >>".to_owned(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 5 0 R \
             /Resources << /Font << /F1 7 0 R >> /XObject << /{key} 6 0 R >> >> >>"
        ),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 8 0 R \
         /Resources << /Font << /F1 7 0 R >> >> >>"
            .to_owned(),
        content(&format!("/{key} Do BT /F1 12 Tf 10 10 Td (ONE) Tj ET")),
        "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Length 0 >>\nstream\n\nendstream"
            .to_owned(),
        HELVETICA.to_owned(),
        content("BT /F1 12 Tf 10 10 Td (TWO) Tj ET"),
    ]);
    let outputs = split_first(bytes, &[1]).expect("a 255-raw name is at the ceiling, not past it");
    assert_eq!(
        outputs.len(),
        2,
        "split after page 1 of a 2-page doc yields two parts"
    );
}

/// Build a 2-page doc whose page 1 draws a chain of `levels` nested Form XObjects; the deepest
/// form draws a hidden-OCG layer (if `hidden`) or a plain font (if not). Object numbering:
/// 1 catalog, 2 pages, 3 page1, 4 page2, 5 page1-content, 6 page2-content, 7 OCG, 8 font,
/// 9.. forms Fm1..FmN.
fn nested_form_chain(levels: usize, hidden: bool) -> Vec<u8> {
    let first_form = 9; // object number of Fm1
    let catalog = if hidden {
        "<< /Type /Catalog /Pages 2 0 R \
         /OCProperties << /OCGs [7 0 R] /D << /OFF [7 0 R] >> >> >>"
            .to_owned()
    } else {
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned()
    };
    let mut objects = vec![
        catalog,
        "<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] >>".to_owned(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 5 0 R \
             /Resources << /Font << /F1 8 0 R >> /XObject << /Fm1 {first_form} 0 R >> >> >>"
        ),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 6 0 R \
         /Resources << /Font << /F1 8 0 R >> >> >>"
            .to_owned(),
        content("/Fm1 Do BT /F1 12 Tf 10 10 Td (ONE) Tj ET"),
        content("BT /F1 12 Tf 10 10 Td (TWO) Tj ET"),
        "<< /Type /OCG /Name (Hidden) >>".to_owned(),
        HELVETICA.to_owned(),
    ];
    for level in 1..=levels {
        let obj = first_form + level - 1;
        let body = if level < levels {
            let next = obj + 1;
            format!(
                "<< /Type /XObject /Subtype /Form /BBox [0 0 200 200] \
                 /Resources << /XObject << /Fm{} {next} 0 R >> >> /Length {{LEN}} >>\nstream\n/Fm{} Do\nendstream",
                level + 1,
                level + 1
            )
        } else if hidden {
            let data = "/OC /L1 BDC BT /F1 12 Tf 10 10 Td (HIDDENLAYERTEXT) Tj ET EMC";
            format!(
                "<< /Type /XObject /Subtype /Form /BBox [0 0 200 200] \
                 /Resources << /Font << /F1 8 0 R >> /Properties << /L1 7 0 R >> >> \
                 /Length {} >>\nstream\n{data}\nendstream",
                data.len()
            )
        } else {
            // A vector with no resources of its own, so the clean near-miss does not descend one
            // level further into a font and refuse for depth that way -- the form level maps to
            // the walk depth cleanly.
            let data = "10 10 20 20 re f";
            format!(
                "<< /Type /XObject /Subtype /Form /BBox [0 0 200 200] /Length {} >>\nstream\n{data}\nendstream",
                data.len()
            )
        };
        // Fix up {LEN} for the intermediate forms (their stream is `/Fm{next} Do`).
        let body = if body.contains("{LEN}") {
            let stream = format!("/Fm{} Do", level + 1);
            body.replace("{LEN}", &stream.len().to_string())
        } else {
            body
        };
        objects.push(body);
    }
    raw_pdf(&objects)
}

#[test]
fn a_hidden_layer_past_the_form_nesting_ceiling_is_refused() {
    // #253. The walk follows form XObjects to a bounded depth; past it, it used to return
    // Ok(()) silently, so the optional-content refusal (ADR 0019 §2a row 6) never ran on the
    // deepest forms and a hidden layer nine forms down was carried into the part. Nine levels is
    // the MEASURED leaking depth: with the guard restored to Ok(()), this fixture splits Ok with
    // HIDDENLAYERTEXT in the part (confirmed by mutation); chains of eight and shallower are
    // caught by the optional-content refusal itself. The guard now refuses instead.
    let result = split_first(nested_form_chain(9, true), &[1]);
    match result {
        Err(error) => assert!(
            format!("{error:?}").contains("[split-nested-form-too-deep]"),
            "refused, but not for the nesting depth: {error:?}"
        ),
        Ok(outputs) => {
            assert!(
                !contains(&expanded(&outputs[0]), "HIDDENLAYERTEXT"),
                "split carried a layer hidden nine forms down into the part"
            );
            panic!("returned Ok, where a form chain past the depth ceiling must refuse the split");
        }
    }
}

#[test]
fn a_form_chain_at_the_ceiling_still_splits() {
    // THE NEAR-MISS. A legitimate eight-deep chain (the deepest form draws a plain vector, no
    // hidden layer) is exactly at the ceiling the walk follows, so it must still split -- the
    // ceiling is a ceiling, not a wall one level below it. (The census found real documents nest
    // forms at most two deep, so refusing at nine newly refuses zero of them.)
    let outputs = split_first(nested_form_chain(8, false), &[1])
        .expect("an eight-deep form chain is at the ceiling, not past it");
    assert_eq!(
        outputs.len(),
        2,
        "split after page 1 of a 2-page doc yields two parts"
    );
}

/// A 2-page doc whose page 1 carries a widget annotation whose `/AP /N` appearance is a chain of
/// `levels` nested Form XObjects, the deepest holding a hidden OCG. The appearance is absorbed at
/// EVEN depth, so this drives the `follow_resources` guard parity the form-chain fixtures do not.
/// Objects: 1 catalog, 2 pages, 3 page1, 4 page2, 5 page1-content, 6 page2-content, 7 OCG, 8 font,
/// 9 widget, 10.. appearance forms Ap1..ApN.
fn annotation_appearance_chain(levels: usize, hidden: bool) -> Vec<u8> {
    let first = 10;
    let catalog = if hidden {
        "<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [7 0 R] /D << /OFF [7 0 R] >> >> >>"
            .to_owned()
    } else {
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned()
    };
    let mut objects = vec![
        catalog,
        "<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 5 0 R \
         /Annots [9 0 R] /Resources << /Font << /F1 8 0 R >> >> >>"
            .to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 6 0 R \
         /Resources << /Font << /F1 8 0 R >> >> >>"
            .to_owned(),
        content("BT /F1 12 Tf 10 10 Td (ONE) Tj ET"),
        content("BT /F1 12 Tf 10 10 Td (TWO) Tj ET"),
        "<< /Type /OCG /Name (Hidden) >>".to_owned(),
        HELVETICA.to_owned(),
        format!(
            "<< /Type /Annot /Subtype /Widget /Rect [0 0 100 100] /FT /Btn /AP << /N {first} 0 R >> >>"
        ),
    ];
    for level in 1..=levels {
        let obj = first + level - 1;
        let body = if level < levels {
            let next = obj + 1;
            let stream = format!("/Ap{} Do", level + 1);
            format!(
                "<< /Type /XObject /Subtype /Form /BBox [0 0 100 100] \
                 /Resources << /XObject << /Ap{} {next} 0 R >> >> /Length {} >>\nstream\n{stream}\nendstream",
                level + 1,
                stream.len()
            )
        } else if hidden {
            let data = "/OC /L1 BDC BT /F1 12 Tf 10 10 Td (HIDDENLAYERTEXT) Tj ET EMC";
            format!(
                "<< /Type /XObject /Subtype /Form /BBox [0 0 100 100] \
                 /Resources << /Font << /F1 8 0 R >> /Properties << /L1 7 0 R >> >> /Length {} >>\nstream\n{data}\nendstream",
                data.len()
            )
        } else {
            let data = "10 10 20 20 re f";
            format!(
                "<< /Type /XObject /Subtype /Form /BBox [0 0 100 100] /Length {} >>\nstream\n{data}\nendstream",
                data.len()
            )
        };
        objects.push(body);
    }
    raw_pdf(&objects)
}

#[test]
fn a_hidden_layer_in_an_annotation_appearance_past_the_ceiling_is_refused() {
    // #253, the EVEN-PARITY path. An annotation `/AP` appearance is absorbed at depth 0, not via a
    // page form at depth 1, so a deep appearance chain trips the `follow_resources` guard (the one
    // the form-chain fixtures above never reach). Nine appearance levels is the measured leaking
    // depth here: with the guards restored to Ok(()) this splits with HIDDENLAYERTEXT in the part
    // (confirmed by mutation); eight is caught by the optional-content refusal. Annotation
    // appearances are a resource split prunes, so this is a redaction-reachable path.
    let result = split_first(annotation_appearance_chain(9, true), &[1]);
    match result {
        Err(error) => assert!(
            format!("{error:?}").contains("[split-nested-form-too-deep]"),
            "refused, but not for the nesting depth: {error:?}"
        ),
        Ok(outputs) => {
            assert!(
                !contains(&expanded(&outputs[0]), "HIDDENLAYERTEXT"),
                "split carried a layer hidden deep in an annotation appearance into the part"
            );
            panic!("returned Ok, where a deep annotation-appearance chain must refuse the split");
        }
    }
}

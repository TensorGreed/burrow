//! Every document in the redaction corpus, through the real qpdf steps.
//!
//! # Why the whole corpus, and not the fixtures written for this change
//!
//! Every test on the `Steps` seam before this one ran against a fake implementation, and three
//! fakes on this milestone turned out more forgiving than the engine they stood in for. A
//! fixture written alongside an implementation is a fixture the implementation can pass; the
//! corpus was written against spike 0006's survival channels, months before any of this, and
//! it does not know what this code finds convenient.
//!
//! # What it asserts, and what it only reports
//!
//! Asserted, for every document: the outcome is either a **named** refusal or a PDF, no glyph
//! appears that was not drawn before, and — where the region reaches ink — the glyphs it
//! reaches are gone. Reported without assertion: which documents refuse, with which rule, and
//! how many glyphs each removal took beyond the ones the region reached. The refusal set is a
//! measurement of where this operation currently stands, and pinning it would turn every
//! future improvement into a test failure.
//!
//! The count of documents examined is printed and gated against the directory listing, so a
//! glob that silently matched four files cannot read as a sweep of forty-three.

#![cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "a test reading committed fixtures it lists from disk; the workspace lints are for \
              attacker-controlled input in library code, where they stay denied. The sibling \
              test binaries carry the same allowance"
)]

mod support;

use std::collections::BTreeSet;

use burrow_engines::pdfsyntax::region::Region;
use support::char_box_oracle::{OracleChar, chars_on_page, ink_overlaps, origins_close, page_size};

/// Where the corpus lives, relative to this crate.
const CORPUS: &[&str] = &[
    "../../tests/redaction/generated",
    "../../tests/redaction/fixtures",
];

/// One document's outcome.
enum Outcome {
    /// Refused, by this rule.
    Refused(String),
    /// Redacted: glyphs the region reached, glyphs removed beyond them.
    Redacted { reached: usize, extra: usize },
    /// The region reached nothing the oracle could see, so nothing was asked of it.
    NothingToRemove,
}

#[test]
fn every_document_in_the_redaction_corpus_either_redacts_or_refuses_by_name() {
    let mut examined = 0usize;
    let mut expected = 0usize;
    let mut refusals: Vec<(String, String)> = Vec::new();
    let mut redactions: Vec<(String, usize, usize)> = Vec::new();
    let mut quiet: Vec<String> = Vec::new();

    for directory in CORPUS {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(directory);
        let mut entries: Vec<std::path::PathBuf> = std::fs::read_dir(&path)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "pdf"))
            .collect();
        entries.sort();
        expected += entries.len();

        for file in entries {
            let name = file
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let pdf = std::fs::read(&file).expect("a committed corpus document");
            examined += 1;
            match outcome_for(&name, &pdf) {
                Outcome::Refused(rule) => refusals.push((name, rule)),
                Outcome::Redacted { reached, extra } => redactions.push((name, reached, extra)),
                Outcome::NothingToRemove => quiet.push(name),
            }
        }
    }

    eprintln!("\n  redacted ({}):", redactions.len());
    for (name, reached, extra) in &redactions {
        eprintln!("    {name:<48} reached {reached}, removed {extra} more");
    }
    eprintln!("\n  refused ({}):", refusals.len());
    for (name, rule) in &refusals {
        eprintln!("    {name:<48} {rule}");
    }
    eprintln!(
        "\n  region reached no glyph the oracle sees ({}):",
        quiet.len()
    );
    for name in &quiet {
        eprintln!("    {name}");
    }
    eprintln!(
        "\n  {examined} of {expected} documents examined: {} redacted, {} refused, {} quiet\n",
        redactions.len(),
        refusals.len(),
        quiet.len()
    );

    // THE COUNT, against the listing rather than against a number written here. A sweep that
    // examined four of forty-three prints `OK` just as loudly as one that examined all of them.
    assert_eq!(
        examined, expected,
        "the sweep examined {examined} of the {expected} documents on disk"
    );
    assert!(
        expected >= 40,
        "the corpus has shrunk to {expected} documents, which is not the corpus this was \
         written against"
    );

    // A DOCUMENT THE ORACLE READS NOTHING FROM MEASURES NOTHING, and is reported here as
    // `quiet` rather than as a failure. That is right for the report and wrong as a resting
    // state: every fixture in this corpus draws at least the keep line, so a quiet one is a
    // broken one.
    //
    // Measured on my own mistake: an evasion fixture written with one `>>` too many closed the
    // page's `/Resources` early and detached `/Contents`. `qpdf --check` passed it, the
    // generator reported it written, and this sweep counted it quiet and stayed green -- a
    // fixture whose whole purpose is to probe a leak, asserting nothing, in a test that said OK.
    // THE MESSAGE NAMES BOTH CAUSES, because `Outcome::NothingToRemove` has two: the oracle
    // reading no text at all, and a region that reached no glyph after one. Blaming the oracle
    // for the second would send the next reader to the wrong place.
    assert!(
        quiet.is_empty(),
        "{} document(s) asserted nothing -- either the oracle read no text, or the region \
         reached no glyph: {:?}. Every fixture here draws at least the keep line and puts its \
         canary under the region, so a quiet one is malformed rather than uninteresting",
        quiet.len(),
        quiet
    );

    // AND THE OUTCOME DISTRIBUTION, not just the count examined. A mutation that made
    // `QpdfRedaction::new` refuse unconditionally left this test green — 43 of 43 examined, 0
    // redacted, 43 refused — because every per-document assertion is inside the `Ok` arm.
    // `CLAUDE.md`: gate on the expected count where that count is knowable.
    //
    // It was 4, with a comment saying the other 39 were held back by the standard-14 refusal.
    // That refusal is now closed — the metrics are bundled — and 39 documents redact. Leaving
    // the floor at 4 would have left this test passing over a regression that took 39 back to
    // 5, which is the whole failure mode the floor exists to catch: "4 of 43" reads as success.
    //
    // A floor rather than an equality, because the refusals that remain are ones this milestone
    // intends to close, and closing one must not be a test failure.
    //
    // IT WENT DOWN ONCE, 39 to 38, and the reason is worth keeping: `evade-image-in-type3-glyph`
    // was **redacting** while its own manifest entry declares `verdict = "refuse"` and
    // `expect_after = "refused"`. Refusing a Type 3 procedure that draws — not only one that
    // shows text — closed that evasion and took the count with it. A floor that may only rise
    // would have read that as a regression.
    //
    // It also names a gap: the manifest assigns every fixture a verdict and nothing compares an
    // outcome against it, so a fixture can disagree with its own declared decision in silence.
    // That is the `expect_after` half of ADR 0029 §8, which `check-redaction-corpus.py` says it
    // does not check.
    assert!(
        redactions.len() >= 39,
        "{} documents redacted, and 39 did when this floor was last measured: a refusal widened \
         far enough to cover them would take every per-document assertion here to zero while \
         this test still printed a pass",
        redactions.len()
    );
}

/// Run one document and classify what came back.
fn outcome_for(name: &str, pdf: &[u8]) -> Outcome {
    let before = chars_on_page(pdf, 0);
    let drawn_before: Vec<&OracleChar> = before.iter().filter(|char| !char.generated).collect();
    let (_, height) = page_size(pdf, 0);

    let Some(target) = drawn_before
        .iter()
        .max_by(|a, b| a.ink.top.total_cmp(&b.ink.top))
    else {
        return Outcome::NothingToRemove;
    };
    let region = Region {
        left: target.ink.left - 1.0,
        top: height - target.ink.top - 1.0,
        width: (target.ink.right - target.ink.left) + 2.0,
        height: (target.ink.top - target.ink.bottom) + 2.0,
    };

    let redacted: BTreeSet<usize> = [0].into_iter().collect();
    let out = match support::redact_page(pdf, 0, redacted, region) {
        Ok((out, report)) => {
            // THE DISCLOSURE COUNT, bounded by something that is not the sharing walk, and
            // bounded by THIS DOCUMENT'S page count rather than by a claim about the corpus.
            //
            // This said "every document in this corpus has one page" and asserted zero. That
            // was false when it was written -- `evade-widget-on-another-page.pdf` has two, and
            // says so in its name -- and it never fired, because that document refused at the
            // door for a missing width table until the standard-14 metrics landed. The first
            // run that reached the assertion failed it, on a report that was **correct**: the
            // font is used by page 2, page 2 is outside the redacted set, so `also_used_by` is
            // 1 and the font is retained rather than cut.
            //
            // A ceiling derived from the document is the check that survives the next fixture:
            // a font cannot be used by more pages than the document has, outside the one page
            // this redacts. `tests/redaction_disclosure.rs` carries the exact multi-page case.
            let pages = page_count_of(pdf);
            let outside = pages.saturating_sub(1);
            for font in &report.fonts {
                assert!(
                    font.also_used_by <= outside,
                    "{name}: a {pages}-page document redacting one page reports a font used by \
                     {} other pages, and there are only {outside}: {font:?}",
                    font.also_used_by
                );
            }
            // AND THE TWO ANSWERS MUST AGREE. A retained font is exactly a font used outside
            // the redacted set, so a document with nowhere else to use one must not disclose,
            // and one that reports a nonzero count must.
            assert_eq!(
                report.discloses_a_retained_font(),
                report.fonts.iter().any(|font| font.also_used_by > 0),
                "{name}: the disclosure and the counts disagree: {:?}",
                report.fonts
            );
            out
        }
        Err(error) => {
            // A REFUSAL IS AN OUTCOME AND MUST NAME ITS RULE. An unnamed one cannot be told
            // apart from a redaction that quietly did nothing.
            let text = format!("{error:?}");
            let rule = text
                .split_once('[')
                .and_then(|(_, rest)| rest.split_once(']'))
                .map(|(rule, _)| rule.to_owned());
            return Outcome::Refused(rule.unwrap_or_else(|| {
                panic!("{name}: refused without naming a rule: {text}");
            }));
        }
    };

    assert!(out.starts_with(b"%PDF"), "{name}: the output must be a PDF");
    let after = chars_on_page(&out, 0);
    let drawn_after: Vec<&OracleChar> = after.iter().filter(|char| !char.generated).collect();

    let mut reached = 0usize;
    for want in &drawn_before {
        if !ink_overlaps(
            &want.ink,
            region.left,
            region.top,
            region.width,
            region.height,
            height,
        ) {
            continue;
        }
        reached += 1;
        assert!(
            !drawn_after
                .iter()
                .any(|got| got.unicode == want.unicode && origins_close(got.origin, want.origin)),
            "{name}: U+{:04X} at {:?} has ink inside the region and is still drawn",
            want.unicode,
            want.origin
        );
    }
    for got in &drawn_after {
        assert!(
            drawn_before
                .iter()
                .any(|want| want.unicode == got.unicode && origins_close(got.origin, want.origin)),
            "{name}: U+{:04X} appears at {:?} and was not drawn before -- the page reflowed",
            got.unicode,
            got.origin
        );
    }
    if reached == 0 {
        return Outcome::NothingToRemove;
    }
    Outcome::Redacted {
        reached,
        extra: drawn_before
            .len()
            .saturating_sub(reached + drawn_after.len()),
    }
}

/// How many pages the document has.
///
/// Read from the document rather than assumed, because the assumption that every corpus
/// document has one page was already false — see the disclosure check in `outcome_for`.
fn page_count_of(pdf: &[u8]) -> usize {
    let document = support::open(pdf.to_vec()).expect("a committed corpus document opens");
    let count = support::page_count(&document).expect("its page count is readable");
    usize::try_from(count).expect("a corpus document's page count fits in a usize")
}

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

    // AND THE OUTCOME DISTRIBUTION, not just the count examined. A mutation that made
    // `QpdfRedaction::new` refuse unconditionally left this test green — 43 of 43 examined, 0
    // redacted, 43 refused — because every per-document assertion is inside the `Ok` arm.
    // `CLAUDE.md`: gate on the expected count where that count is knowable, and this one is:
    // the four real-producer documents redact, and the reason the other 39 do not is the
    // standard-14 refusal ADR 0029 records.
    //
    // A floor rather than an equality, because closing that refusal should not turn an
    // improvement into a test failure — the number can only go up.
    assert!(
        redactions.len() >= 4,
        "{} documents redacted, and the four real-producer documents must: a refusal widened \
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
    let out = match burrow_engines::redact_probe::redact_page(pdf, 0, redacted, region) {
        Ok((out, report)) => {
            // THE DISCLOSURE COUNT, bounded by something that is not the sharing walk. Every
            // document in this corpus has one page and that page is the redacted one, so no
            // font can be used outside the redacted set and every count must be zero. It is a
            // weak claim, and it is the strongest one a single-page corpus supports --
            // `tests/redaction_disclosure.rs` carries the multi-page case, which the corpus
            // does not contain.
            for font in &report.fonts {
                assert_eq!(
                    font.also_used_by, 0,
                    "{name}: a one-page document reports a font used by                      {} other pages: {font:?}",
                    font.also_used_by
                );
            }
            assert!(
                !report.discloses_a_retained_font(),
                "{name}: a one-page document has nothing to retain a font for: {:?}",
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

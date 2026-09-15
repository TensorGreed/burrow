//! The damaged documents qpdf counts and PDFium refuses: does the count survive a write?
//!
//! # Why this file exists
//!
//! #26 moves `page_count` from PDFium to qpdf, which makes the two engines' answers on the
//! damaged corpus a shipped difference rather than an internal one. Four conformance fixtures
//! disagree, and two of them are the direction that matters: **qpdf opens and counts a
//! document PDFium refuses.** `truncated-mid-object.pdf` counts 1 page, `trailer-removed.pdf`
//! counts 3.
//!
//! An engine difference is fine. The one that is not fine is [issue
//! #61](https://github.com/TensorGreed/burrow/issues/61)'s shape: an optimistic count, then a
//! write that quietly comes back short, so a person is handed a document missing a page and
//! told nothing. `tests/damaged/page-loss-on-write.pdf` is that shape — it opens as five
//! pages and writes as four.
//!
//! **So the difference cannot be recorded as accepted until each of these two has been asked
//! the same question**, which is what this file does. It is deliberately not a page-count
//! test: the count is already in the conformance corpus. What is asserted here is what
//! happens *after* the count — whether the operation delivers, or refuses, and never that it
//! delivers something short.

#![cfg(all(feature = "native-engines", target_os = "linux"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use burrow_engines::OpenOptions;
use burrow_engines::qpdf::Qpdf;
use burrow_types::{Clock, Error, Limits, SystemClock};

fn options() -> OpenOptions<'static> {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock::new());
    // NO RECOVERY FLAG HERE, and that is the finding rather than an omission:
    // `attempt_recovery` lives on `CheckOptions` and not on `OpenOptions`, so **no operation
    // opens with recovery**. The optimistic count in the corpus comes from `structure_check`
    // with recovery ON; every write path runs with it off.
    OpenOptions::new(Limits::DEFAULT, clock)
}

/// A structural check at a chosen recovery posture, which is where the optimistic count
/// comes from.
fn count_with_recovery(bytes: &[u8], recovery: bool) -> burrow_types::Result<u64> {
    use burrow_engines::{CheckOptions, StructureEngine};
    let clock: Arc<dyn Clock> = Arc::new(SystemClock::new());
    let mut options = CheckOptions::new(Limits::DEFAULT, clock);
    options.attempt_recovery = recovery;
    Qpdf::new()
        .check(bytes.to_vec().into_boxed_slice(), &options)
        .map(|report| report.pages)
}

fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/conformance/fixtures")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Rotate by zero and see what comes back.
///
/// **Zero degrees on purpose.** A rotation that changes nothing is the thinnest possible
/// write: whatever the output loses, the operation did not ask for. #61 was found exactly
/// this way — "a rotation by zero degrees on the same bytes loses the same page", which is
/// what showed it belonged to the shared write step rather than to `reorder`.
fn rotate_everything_by_zero(bytes: Vec<u8>) -> burrow_types::Result<(u64, Vec<u8>)> {
    let engine = Qpdf::new();
    let opts = options();
    // The count first, through the same engine, so the assertion below compares the number a
    // caller would have been shown against the number they were handed.
    let source =
        burrow_engines::PageRotator::open(&engine, bytes.clone().into_boxed_slice(), &opts)?;
    let pages = burrow_engines::PageRotator::pages(&engine, &source)?;
    drop(source);

    let all: Vec<u64> = (1..=pages).collect();
    let out = burrow_ops::rotate(
        &engine,
        bytes.into_boxed_slice(),
        burrow_ops::Pages::numbered(&all),
        0,
        &opts,
    )?;
    Ok((pages, out))
}

/// What each posture answers, printed so the corpus entry can quote a measurement.
#[test]
fn the_recovery_posture_is_what_produces_the_optimistic_count() {
    for name in ["truncated-mid-object.pdf", "trailer-removed.pdf"] {
        let bytes = fixture(name);
        let on = count_with_recovery(&bytes, true);
        let off = count_with_recovery(&bytes, false);
        println!("{name}: recovery on -> {on:?}   recovery off -> {off:?}");
        // BOTH SIDES ASSERTED. `off` was computed, printed and never checked -- the write-only
        // defect this batch has now produced four times, in the test whose name is a claim
        // about the DIFFERENCE between the two postures. If recovery-off started counting, the
        // name would be false and nothing would have said so. Found by code review.
        assert!(
            on.is_ok(),
            "{name}: recovery-on must count, or the corpus is wrong"
        );
        assert!(
            off.is_err(),
            "{name}: recovery-off counted {off:?}, so the posture is no longer what produces \
             the optimistic count and the #61 question has to be asked again"
        );
    }
}

/// Neither fixture reaches a caller with a page missing, because neither reaches a write.
///
/// **The measured answer to "is this the #61 shape?" is no, for a reason that also removes
/// the difference.** `attempt_recovery` lives on `CheckOptions` and not on `OpenOptions`, so
/// every operation opens with recovery OFF — and with it off qpdf refuses both of these
/// outright, exactly as PDFium does. The optimistic count exists only under the posture a
/// structural check can ask for and no operation uses.
///
/// So there is no path on which a person is shown a page count and then handed a document
/// short of a page. That is what the corpus entry has to carry, and it is stronger than
/// "engines differ": the engines **agree** once the posture is equal.
#[test]
fn an_optimistic_count_never_reaches_a_write() {
    for (name, counted) in [
        ("truncated-mid-object.pdf", 1_u64),
        ("trailer-removed.pdf", 3),
    ] {
        let bytes = fixture(name);

        assert_eq!(
            count_with_recovery(&bytes, true).ok(),
            Some(counted),
            "{name}: the corpus records a count this open did not produce"
        );
        // AND THE SAME ENGINE REFUSES IT WITHOUT RECOVERY, which is every operation's posture.
        assert!(
            matches!(count_with_recovery(&bytes, false), Err(Error::Malformed(_))),
            "{name}: recovery-off no longer refuses, so an operation could now open it and \
             the #61 question has to be asked again"
        );

        match rotate_everything_by_zero(bytes) {
            Err(Error::Malformed(_)) => {}
            Ok((pages, output)) => {
                // NOT A PASS BY DEFAULT. If an operation ever does open one of these, every
                // page counted must be in the output -- read back through a FRESH engine,
                // because the writing engine's opinion of its own output is not evidence.
                let engine = Qpdf::new();
                let reread = burrow_engines::PageRotator::open(
                    &engine,
                    output.into_boxed_slice(),
                    &options(),
                )
                .unwrap_or_else(|e| panic!("{name}: the output does not reopen: {e:?}"));
                let out_pages = burrow_engines::PageRotator::pages(&engine, &reread).unwrap();
                assert_eq!(
                    out_pages, pages,
                    "{name}: THE #61 SHAPE. Counted {pages} pages and delivered {out_pages}."
                );
            }
            Err(Error::OutputRejected(why)) => {
                // ADR 0022 caught it, which is #61's own stated acceptable outcome: "a refusal
                // would not block; losing the page quietly does".
                println!("{name}: refused on write-back -- {why}");
            }
            Err(other) => panic!("{name}: unexpected {other:?}"),
        }
    }
}

/// The bomb, which is the fifth difference and was missed in the first enumeration.
///
/// `objstm-bomb-tight-ceiling` is the case where PDFium refuses at the **measured** stage and
/// qpdf counts 1 page — an optimistic-count direction, and not among the four this file
/// originally asked the #61 question of. Found by code review, which also noticed that the
/// spike's "the remaining four" read as an exhaustive enumeration and was not one.
///
/// It is safe for two independent reasons, and both are asserted rather than argued:
/// the write path refuses it, and on the **web** — the platform the `page_count` move affects
/// — `expectations.json` already carries a `platform_expectations` entry making
/// `structure_check` refuse at `measured` too, because WASM linear memory never shrinks so the
/// web reading sees the peak qpdf inflates and frees.
#[test]
fn the_object_stream_bomb_is_not_delivered_short_either() {
    let bytes = fixture("objstm-bomb.pdf");
    // The tight ceiling the corpus case uses, so this asks the corpus's question and not a
    // different one.
    let tight = 96 * 1024 * 1024;

    let counted = {
        use burrow_engines::{CheckOptions, StructureEngine};
        let clock: Arc<dyn Clock> = Arc::new(SystemClock::new());
        let limits = Limits::with(|l| l.max_memory_bytes = tight);
        Qpdf::new()
            .check(
                bytes.clone().into_boxed_slice(),
                &CheckOptions::new(limits, clock),
            )
            .map(|report| report.pages)
    };
    // Whatever it answers, the write must not hand back fewer pages than it promised.
    let Ok(pages) = counted else {
        return; // Refused at the count. Nothing optimistic to follow up.
    };

    let clock: Arc<dyn Clock> = Arc::new(SystemClock::new());
    let opts = OpenOptions::new(Limits::with(|l| l.max_memory_bytes = tight), clock);
    let engine = Qpdf::new();
    let all: Vec<u64> = (1..=pages).collect();
    match burrow_ops::rotate(
        &engine,
        bytes.into_boxed_slice(),
        burrow_ops::Pages::numbered(&all),
        0,
        &opts,
    ) {
        Ok(output) => {
            let witness = Qpdf::new();
            let reread =
                burrow_engines::PageRotator::open(&witness, output.into_boxed_slice(), &opts)
                    .unwrap_or_else(|e| panic!("the output does not reopen: {e:?}"));
            let out_pages = burrow_engines::PageRotator::pages(&witness, &reread).unwrap();
            assert_eq!(
                out_pages, pages,
                "THE #61 SHAPE. Counted {pages} pages and delivered {out_pages}."
            );
        }
        Err(error) => println!("objstm-bomb.pdf: refused on write -- {error:?}"),
    }
}

/// The other two differences run the safe way, and this pins the direction.
///
/// `object-number-above-int-max` and `canary` are the pair PDFium accepts and qpdf refuses,
/// at equal recovery posture (both declare `attempt_recovery: false`). Moving `page_count` to
/// qpdf therefore makes the web **stricter** on these, never more optimistic — and a refusal
/// cannot deliver a document short of a page, so #61's shape is not reachable through them.
///
/// **BOTH HALVES ARE ASSERTED HERE, and only qpdf's was until a code review asked.** The name
/// promises a comparison: "where the engines DIFFER". With only the qpdf half, the day PDFium
/// also started refusing these files the fixtures would no longer differ, the premise would
/// have no subject, and this test would go on passing while measuring nothing — which is the
/// failure the root `CLAUDE.md` names, and which had already happened once in this change
/// (`e2e/engines.spec.ts`'s "both engines are live in one worker"). It is also the entry that
/// `tests/conformance/expectations.json`'s `platform_expectations` cite as their guard, so a
/// guard that watches one side is a citation to something weaker than claimed.
#[test]
fn where_the_engines_differ_at_equal_posture_qpdf_is_the_stricter_one() {
    use burrow_engines::{DocumentEngine, pdfium::Pdfium};

    let pdfium = Pdfium::new();
    for name in ["object-number-above-int-max.pdf", "canary.pdf"] {
        let bytes = fixture(name);

        let refused = count_with_recovery(&bytes, false);
        assert!(
            matches!(refused, Err(Error::Malformed(_))),
            "{name}: qpdf no longer refuses, so this is no longer the safe direction: {refused:?}"
        );

        // THE OTHER HALF. Same posture: PDFium has no recovery switch, so this IS its
        // `attempt_recovery: false`, and it is the posture `page_count` used before the move.
        let clock: Arc<dyn Clock> = Arc::new(SystemClock::new());
        let options = OpenOptions::new(Limits::DEFAULT, clock);
        let accepted = pdfium.open(bytes.clone().into_boxed_slice(), &options);
        match accepted {
            Ok(document) => {
                let pages = pdfium.page_count(&document);
                assert!(
                    matches!(pages, Ok(count) if count > 0),
                    "{name}: PDFium opened it but reports no pages, so the pair no longer \
                     differs in the direction this test is named for: {pages:?}"
                );
            }
            Err(error) => panic!(
                "{name}: PDFium now refuses this file too, so the engines no longer DIFFER \
                 here and this test has no subject. Either the fixture changed or a PDFium \
                 bump tightened it -- re-derive the pair rather than deleting the assertion, \
                 and update the `platform_expectations` entries that cite this test: {error:?}"
            ),
        }
    }
}

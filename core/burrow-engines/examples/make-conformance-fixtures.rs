//! Regenerate `tests/conformance/fixtures/` and `tests/conformance/expectations.json`.
//!
//! ```bash
//! cargo run -p burrow-engines --example make-conformance-fixtures
//! ```
//!
//! Every fixture is generated here rather than downloaded. That is a provenance decision,
//! not a convenience: a committed corpus file needs a licence, a source, and a
//! redistribution answer, and generated output needs none of those — it is this
//! repository's own work, under the project's own licence. It also means the expected
//! outcomes are derived from how the file was *built*, not from what an engine happened to
//! say about it.
//!
//! Two fixtures are **not** written here and are listed anyway, because the expectations file
//! has to cover them: `xref-bomb.pdf` and `objstm-bomb.pdf` both need a real deflate
//! compressor to get their expansion ratios, so they come from `tools/make-xref-bomb.py` and
//! `tools/make-objstm-bomb.py`. See `tests/conformance/PROVENANCE.md`.
//!
//! # The expected outcomes are written by hand, and that is the point
//!
//! Recording whatever the engine currently returns would turn the conformance suite into a
//! snapshot of today's behaviour, and it would keep passing through the exact regression it
//! exists to catch. Every entry below is a claim about what burrow *should* do, with the
//! reasoning beside it wherever the answer is not obvious.
//!
//! One entry is a claim about what burrow *does* and should not: see `objstm-bomb`, whose
//! `known_gap` names the issue tracking it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../testsupport/expectations.rs"]
mod expectations;
#[path = "../testsupport/minimal_pdf.rs"]
mod minimal_pdf;

use expectations::{
    Case, CaseInput, CaseLimits, ErrorKind, Expectations, Failure, KnownGap, Operation, Outcome,
    Outcomes, Platform, PlatformExpectation, conformance_dir, sha256_hex,
};

/// One fixture to write, and what every operation on it must produce.
struct Fixture {
    name: &'static str,
    filename: &'static str,
    /// `None` for a fixture generated elsewhere — it is read from disk and only listed here.
    bytes: Option<Vec<u8>>,
    password: Option<&'static str>,
    limits: Option<CaseLimits>,
    attempt_recovery: bool,
    expect: Outcomes,
    platform_expectations: Vec<PlatformExpectation>,
    known_gap: Option<KnownGap>,
}

const GENERATED_BY: &str = "cargo run -p burrow-engines --example make-conformance-fixtures";

/// The milestone burrow is working in, which is what makes a `known_gap`'s target enforceable.
///
/// Bumping this is the act that calls in every outstanding gap: `conformance.rs` fails on any
/// whose milestone has been reached or passed. Bump it when the milestone closes, and expect
/// to fix or deliberately re-target the gaps it surfaces in the same change.
const CURRENT_MILESTONE: &str = "M1";

/// The declared cross-reference bombs all cost the same, because they declare the same thing.
///
/// 20,000,000 entries × 64 bytes per entry, the measured per-entry cost recorded in
/// `prescan::ENGINE_BYTES_PER_XREF_ENTRY`. The three hidden variants are one-line edits to the
/// same declaration, so an expectation that differed between them would mean the pre-scan was
/// reading something different — which is the regression they exist to catch.
const BOMB_REQUESTED: u64 = 20_000_000 * 64;
/// `Limits::DEFAULT.max_memory_bytes`, spelled out because the expectation records the number
/// the caller would see, not a reference to a constant JavaScript cannot read.
const DEFAULT_MEMORY: u64 = 1024 * 1024 * 1024;

/// The pre-scan's refusal, which every declared-size bomb must produce identically.
fn refused_by_prescan() -> Outcome {
    Outcome::Err(Failure {
        kind: ErrorKind::LimitExceeded,
        limit: Some("max_memory_bytes".to_owned()),
        // THE STAGE IS THE POINT. `size_estimate` and `measured` report the same `limit`;
        // only this says the file was refused *before anything parsed it*, which is the whole
        // value of the pre-scan and the difference between rejecting a bomb and surviving one.
        stage: Some("prescan".to_owned()),
        requested: Some(BOMB_REQUESTED),
        allowed: Some(DEFAULT_MEMORY),
    })
}

fn malformed() -> Outcome {
    Outcome::Err(Failure::of(ErrorKind::Malformed))
}

fn password_required() -> Outcome {
    Outcome::Err(Failure::of(ErrorKind::PasswordRequired))
}

fn opens(page_count: u64) -> Outcome {
    Outcome::Ok {
        page_count,
        rotations: None,
    }
}

/// Both single-document engines produce the same outcome.
///
/// **Does NOT include `merge`.** A single-document fixture says nothing about merging, and
/// schema 3 lets a case declare only the operations it is about rather than inventing an
/// expectation for every one. The merge cases are built separately, below.
fn both(outcome: Outcome) -> Outcomes {
    Outcomes::from([
        (Operation::PageCount, outcome.clone()),
        (Operation::StructureCheck, outcome),
    ])
}

/// The engines disagree, which several fixtures exist to pin.
fn differ(page_count: Outcome, structure_check: Outcome) -> Outcomes {
    Outcomes::from([
        (Operation::PageCount, page_count),
        (Operation::StructureCheck, structure_check),
    ])
}

/// A case that is only about merging.
fn merges(outcome: Outcome) -> Outcomes {
    Outcomes::from([(Operation::Merge, outcome)])
}

/// A rotate case: the page count, and every page's rotation after the operation.
///
/// **The rotations are the assertion.** A rotation cannot change the page count, so a case
/// declaring only that would pass against an implementation that did nothing — the failure
/// this corpus exists to catch is the two paths disagreeing, and the inheritance walk is
/// where they most plausibly would.
fn rotates(page_count: u64, rotations: Vec<i64>) -> Outcomes {
    Outcomes::from([(
        Operation::Rotate,
        Outcome::Ok {
            page_count,
            rotations: Some(rotations),
        },
    )])
}

/// A `reorder` outcome: the page count and the rotations after reversing every page.
///
/// The rotations are the observable because a page count cannot see a permutation and neither
/// side has another per-page readout. `Operation::Reorder` carries the argument.
fn reorders(page_count: u64, rotations: Vec<i64>) -> Outcomes {
    Outcomes::from([(
        Operation::Reorder,
        Outcome::Ok {
            page_count,
            rotations: Some(rotations),
        },
    )])
}

/// A `split` outcome: how many parts came out.
fn splits_into(parts: u64) -> Outcomes {
    Outcomes::from([(
        Operation::Split,
        Outcome::Ok {
            page_count: parts,
            rotations: None,
        },
    )])
}

/// A `split` outcome: the operation refuses this document.
fn split_refused() -> Outcomes {
    Outcomes::from([(
        Operation::Split,
        Outcome::Err(Failure::of(ErrorKind::Unsupported)),
    )])
}

/// A two-page document whose first page draws inside a layer the catalog turns OFF.
///
/// **The fixture that catches one platform failing to prune at all.** `split`'s pruning policy is
/// shared between the native and web implementations, so the corpus can no longer catch them
/// pruning *differently* -- there is only one pruning. What it has to catch instead is one path
/// never reaching it, and this is how.
///
/// The refusal for optional content lives inside the policy. A path that skips pruning does not
/// refuse; it splits happily and reports two parts. So the expectation is `Unsupported`, and a
/// dropped `prune_output` call on either side turns this case red with a plain outcome mismatch --
/// no new expectation shape, no byte comparison, nothing the corpus could not already express.
fn layered_document() -> Vec<u8> {
    // 1 catalog, 2 page tree, 3-4 pages, 5-6 contents, 7 the OCG.
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [7 0 R] \
/D << /OFF [7 0 R] >> >> >>"
            .to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] \
/Resources << /Properties << /MC0 7 0 R >> >> /Contents 5 0 R >>"
            .to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 6 0 R >>".to_vec(),
    ];
    for body in [
        &b"/OC /MC0 BDC BT (hidden by a layer) Tj ET EMC"[..],
        b"BT (page two) Tj ET",
    ] {
        let mut stream = format!("<< /Length {} >>\nstream\n", body.len()).into_bytes();
        stream.extend_from_slice(body);
        stream.extend_from_slice(b"\nendstream");
        objects.push(stream);
    }
    objects.push(b"<< /Type /OCG /Name (a layer that is off) >>".to_vec());

    let mut out = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (index, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    let count = objects.len() + 1;
    out.extend_from_slice(format!("xref\n0 {count}\n0000000000 65535 f \n").as_bytes());
    for at in &offsets {
        out.extend_from_slice(format!("{at:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {count} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    out
}

fn main() {
    // `--check` WRITES NOTHING AND REPORTS DRIFT.
    //
    // The corpus is committed and every case records its fixture's sha256, so the corpus is
    // self-consistent whatever this generator would produce -- and the two can therefore
    // diverge silently in one direction: change a fixture generator, do not re-run this, and
    // nothing anywhere notices. That is not hypothetical. `pdf_with_page_tree` gained distinct
    // `/MediaBox` widths in the reorder core PR so that page order would be observable, and
    // the committed `mixed-rotation-4page.pdf` and `inherited-rotation-6page.pdf` still had
    // the old 612-wide pages until reorder's bridge regenerated them. Nothing failed in
    // between; the corpus simply described a document this file no longer produces.
    //
    // So CI runs `--check`, and drift fails on the commit that introduces it rather than
    // surfacing whenever somebody next happens to regenerate.
    let check_only = std::env::args().any(|a| a == "--check");
    let mut drift: Vec<String> = Vec::new();
    let dir = conformance_dir();
    let fixtures = dir.join("fixtures");
    std::fs::create_dir_all(&fixtures).expect("tests/conformance/fixtures should be creatable");

    let files = vec![
        // ---- well-formed ------------------------------------------------------------
        Fixture {
            name: "blank-1page",
            filename: "blank-1page.pdf",
            bytes: Some(minimal_pdf::pdf_with_pages(1)),
            password: None,
            limits: None,
            attempt_recovery: false,
            expect: both(opens(1)),
            platform_expectations: Vec::new(),
            known_gap: None,
        },
        Fixture {
            name: "pages-10",
            filename: "pages-10.pdf",
            bytes: Some(minimal_pdf::pdf_with_pages(10)),
            password: None,
            limits: None,
            attempt_recovery: false,
            expect: both(opens(10)),
            platform_expectations: Vec::new(),
            known_gap: None,
        },
        Fixture {
            name: "pages-137",
            filename: "pages-137.pdf",
            // An awkward number on purpose: an off-by-one or a truncated count is visible
            // in a way it would not be for 1, 10, or a power of two.
            bytes: Some(minimal_pdf::pdf_with_pages(137)),
            password: None,
            limits: None,
            attempt_recovery: false,
            expect: both(opens(137)),
            platform_expectations: Vec::new(),
            known_gap: None,
        },
        // ---- damaged ----------------------------------------------------------------
        Fixture {
            name: "truncated",
            filename: "truncated.pdf",
            bytes: Some(minimal_pdf::truncated_pdf()),
            password: None,
            limits: None,
            attempt_recovery: false,
            expect: both(malformed()),
            platform_expectations: Vec::new(),
            known_gap: None,
        },
        Fixture {
            name: "not-a-pdf",
            filename: "not-a-pdf.bin",
            bytes: Some(minimal_pdf::not_a_pdf()),
            password: None,
            limits: None,
            attempt_recovery: false,
            expect: both(malformed()),
            platform_expectations: Vec::new(),
            known_gap: None,
        },
        Fixture {
            name: "no-pages",
            filename: "no-pages.pdf",
            bytes: Some(minimal_pdf::pdf_with_no_pages()),
            password: None,
            limits: None,
            attempt_recovery: false,
            // Not `Ok { page_count: 0 }`. A document with no pages is not a document that
            // opened successfully, and the whole of ADR 0006 requirement 6 is about not
            // letting "zero pages" be a success.
            //
            // REORDER JOINS THE EXISTING CASE rather than getting a fixture of its own. A
            // separate `reorder-refuses-a-document-with-no-pages` was written first and code
            // review was right that it earned nothing: the refusal happens at open, which
            // every operation shares, so it passed against any implementation and cost a case
            // and a comparison to say what `page_count` already said. Declared here, it costs
            // one comparison and still records that reorder refuses rather than, say, hanging.
            expect: {
                let mut expect = both(malformed());
                expect.insert(Operation::Reorder, malformed());
                expect
            },
            platform_expectations: Vec::new(),
            known_gap: None,
        },
        // ---- encrypted --------------------------------------------------------------
        Fixture {
            name: "encrypted-no-password",
            filename: "encrypted.pdf",
            bytes: Some(minimal_pdf::pdf_encrypted_with_unusable_credentials()),
            password: None,
            limits: None,
            attempt_recovery: false,
            expect: both(password_required()),
            platform_expectations: Vec::new(),
            known_gap: None,
        },
        Fixture {
            name: "encrypted-wrong-password",
            filename: "encrypted.pdf",
            bytes: Some(minimal_pdf::pdf_encrypted_with_unusable_credentials()),
            password: Some("not the password"),
            limits: None,
            attempt_recovery: false,
            expect: both(password_required()),
            platform_expectations: Vec::new(),
            known_gap: None,
        },
        // ---- declared-size bombs, all refused before anything parses them ------------
        //
        // The three "hidden" variants are the bypasses security review found in M1 PR 3, each
        // a one-line edit that made the pre-scan read *nothing* and treat "nothing declared"
        // as "nothing to declare". They are in the corpus rather than in one native test
        // because the web path runs the same pre-scan and had never been shown to.
        Fixture {
            name: "xref-bomb",
            filename: "xref-bomb.pdf",
            bytes: None,
            password: None,
            limits: None,
            attempt_recovery: false,
            expect: both(refused_by_prescan()),
            platform_expectations: Vec::new(),
            known_gap: None,
        },
        Fixture {
            name: "bomb-hidden-by-decoy-key",
            filename: "bomb-hidden-decoy-key.pdf",
            bytes: Some(minimal_pdf::xref_bomb_hidden_by_decoy_key()),
            password: None,
            limits: None,
            attempt_recovery: false,
            expect: both(refused_by_prescan()),
            platform_expectations: Vec::new(),
            known_gap: None,
        },
        Fixture {
            name: "bomb-hidden-by-dictionary-padding",
            filename: "bomb-hidden-padded-dictionary.pdf",
            bytes: Some(minimal_pdf::xref_bomb_hidden_by_dictionary_padding()),
            password: None,
            limits: None,
            attempt_recovery: false,
            expect: both(refused_by_prescan()),
            platform_expectations: Vec::new(),
            known_gap: None,
        },
        Fixture {
            name: "bomb-hidden-by-trailing-junk",
            filename: "bomb-hidden-trailing-junk.pdf",
            bytes: Some(minimal_pdf::xref_bomb_hidden_by_trailing_junk()),
            password: None,
            limits: None,
            attempt_recovery: false,
            expect: both(refused_by_prescan()),
            platform_expectations: Vec::new(),
            known_gap: None,
        },
        // ---- rotate ------------------------------------------------------------------
        //
        // Every page turned 90 degrees, which is what `Operation::Rotate` fixes. Two cases,
        // because they ask different questions.
        Fixture {
            // A flat page tree with no `/Rotate` anywhere: every page ends at 90. This is
            // the case a no-op fails -- it would report ten zeroes.
            name: "rotate-a-flat-document",
            filename: "pages-10.pdf",
            bytes: Some(minimal_pdf::pdf_with_pages(10)),
            password: None,
            limits: None,
            attempt_recovery: false,
            expect: rotates(10, vec![90; 10]),
            platform_expectations: Vec::new(),
            known_gap: None,
        },
        Fixture {
            // A TWO-LEVEL page tree whose root carries `/Rotate 90`, so no page has one of
            // its own and every page inherits. Turning each by 90 must give 180 -- an
            // implementation that read the page dictionary and stopped would see nothing,
            // write 90, and silently UNDO the inherited quarter turn, reporting six 90s.
            //
            // This is the case the differential harness is really for. The two paths walk
            // `/Parent` separately, on purpose, and this is where they would diverge.
            name: "rotate-a-document-that-inherits",
            filename: "inherited-rotation-6page.pdf",
            bytes: Some(minimal_pdf::pdf_with_page_tree(
                6,
                minimal_pdf::RotationPlacement::OnTheRoot(90),
            )),
            password: None,
            limits: None,
            attempt_recovery: false,
            expect: rotates(6, vec![180; 6]),
            platform_expectations: Vec::new(),
            known_gap: None,
        },
        Fixture {
            // THE CASE THAT CAN TELL A PER-PAGE WRITE FROM AN ANCESTOR WRITE, and the reason
            // the two cases above cannot.
            //
            // `Operation::Rotate` turns EVERY page, so on a document where all pages inherit
            // the same value, writing `/Rotate` to the shared `/Pages` root produces exactly
            // the vector a correct implementation produces. That is hazard #2 in both rotate
            // modules -- the one that turns a whole document while reporting success on the
            // page that was asked for -- and the corpus was blind to it until this fixture.
            // Found by security review.
            //
            // Here page 1 starts at 270 and the rest inherit 90. Turning every page by 90
            // gives [0, 180, 180, 180]; an ancestor write gives [180, 180, 180, 180].
            name: "rotate-a-document-where-one-page-differs",
            filename: "mixed-rotation-4page.pdf",
            bytes: Some(minimal_pdf::pdf_with_page_tree(
                4,
                minimal_pdf::RotationPlacement::OnTheRootAndTheFirstPage {
                    root: 90,
                    first_page: 270,
                },
            )),
            password: None,
            limits: None,
            attempt_recovery: false,
            expect: rotates(4, vec![0, 180, 180, 180]),
            platform_expectations: Vec::new(),
            known_gap: None,
        },
        // ---- split -------------------------------------------------------------------
        //
        // Cut after the first page, fixed like the other two operations' parameters.
        //
        // TWO CASES, AND THE SECOND IS THE ONE THAT MATTERS. The first says a split of an
        // ordinary document produces two parts on both platforms, which is the ordinary
        // agreement this corpus is for. The second is the replacement for something the corpus
        // gave up: `split`'s pruning policy is SHARED between the two implementations, so this
        // harness can no longer catch them pruning differently. What it catches instead is one
        // of them never reaching the policy -- see `layered_document`.
        Fixture {
            name: "split-an-ordinary-document",
            filename: "pages-10.pdf",
            bytes: None,
            password: None,
            limits: None,
            attempt_recovery: false,
            expect: splits_into(2),
            platform_expectations: Vec::new(),
            known_gap: None,
        },
        Fixture {
            name: "split-refuses-a-layered-document",
            filename: "layered.pdf",
            bytes: Some(layered_document()),
            password: None,
            limits: None,
            attempt_recovery: false,
            expect: split_refused(),
            platform_expectations: Vec::new(),
            known_gap: None,
        },
        // ---- reorder -----------------------------------------------------------------
        //
        // Every page reversed, which is what `Operation::Reorder` fixes. THREE CASES, AND
        // THEY ARE NOT EQUALLY STRONG -- said here rather than left to be assumed, because
        // two of them cannot fail for a wrong permutation.
        //
        // The fixtures are the same three `rotate` uses, deliberately: the observable is the
        // rotations vector, so a fixture that distinguishes pages by rotation for one
        // operation distinguishes them for the other, and reusing them means no new bytes
        // enter the corpus for a second operation to assert about.
        Fixture {
            // Every page 0, so reversing gives ten zeroes. CANNOT fail for a wrong
            // permutation, or for a no-op -- it asserts the page count and that nothing was
            // lost, which is worth having and is not more than that.
            name: "reorder-a-flat-document",
            filename: "pages-10.pdf",
            bytes: None,
            password: None,
            limits: None,
            attempt_recovery: false,
            expect: reorders(10, vec![0; 10]),
            platform_expectations: Vec::new(),
            known_gap: None,
        },
        Fixture {
            // A two-level tree whose ROOT carries `/Rotate 90`. Reversing is a real
            // permutation, so qpdf flattens the tree and pushes the inherited value onto
            // every page first (ADR 0021) -- and the assertion is that all six still display
            // 90 afterwards.
            //
            // So this case is not the weak one the vector suggests. It cannot fail for a
            // wrong permutation, and it CAN fail for a flattening that drops the inheritance,
            // which is the failure that would silently unrotate a scanned document.
            name: "reorder-a-document-that-inherits",
            filename: "inherited-rotation-6page.pdf",
            bytes: None,
            password: None,
            limits: None,
            attempt_recovery: false,
            expect: reorders(6, vec![90; 6]),
            platform_expectations: Vec::new(),
            known_gap: None,
        },
        Fixture {
            // THE ONLY ONE OF THE THREE THAT A WRONG PERMUTATION FAILS.
            //
            // Page 1 starts at 270 and the rest inherit 90, so the fixture reads
            // [270, 90, 90, 90] and a reversal must read [90, 90, 90, 270]. A no-op fails it,
            // and so does any permutation that does not put page 1 last -- 18 of the 24.
            // The six that do put page 1 last pass, which is the honest limit of asserting a
            // permutation through a vector with only two distinct values in it.
            name: "reorder-a-document-where-one-page-differs",
            filename: "mixed-rotation-4page.pdf",
            bytes: None,
            password: None,
            limits: None,
            attempt_recovery: false,
            expect: reorders(4, vec![90, 90, 90, 270]),
            platform_expectations: Vec::new(),
            known_gap: None,
        },
        // ---- the length-based estimate, which nothing else in the corpus reaches ------
        Fixture {
            name: "size-estimate-refuses-an-ordinary-file",
            filename: "pages-10.pdf",
            bytes: Some(minimal_pdf::pdf_with_pages(10)),
            password: None,
            // Below `BASE_OVERHEAD_BYTES` (16 MiB), so the estimate exceeds it for ANY input.
            // That is the point: this case is not about the file, it is about the one stage
            // otherwise absent from the corpus — and `size_estimate` is precisely the stage
            // most easily confused with `prescan`, since both run before an engine and both
            // report `max_memory_bytes`.
            limits: Some(CaseLimits {
                max_memory_bytes: Some(8 * 1024 * 1024),
                ..CaseLimits::default()
            }),
            attempt_recovery: false,
            // `both`, not `differ`, since #26 --- and the flip is the whole point of the
            // case. It was `differ` because `check_open_memory` was called from both PDFium
            // paths and NEITHER qpdf path, so a ceiling below the estimate refused a file
            // through one engine and not the other. Consistent across native and web, so an
            // engine difference rather than a divergence, and recorded as a known gap.
            //
            // Both engines now run it, through `crate::estimate::before_open`, and
            // `tests/limits.rs::both_engines_refuse_at_the_same_estimate_with_the_same_reason`
            // walks the boundary from both sides: one byte below, both refuse with the same
            // four numbers; at exactly the estimate, both accept.
            //
            // The constants were measured against PDFium, and `examples/measure-open-cost.rs`
            // shows that is not the direction of risk --- qpdf is the hungrier engine and its
            // cost tracks PAGE COUNT where the estimate tracks length. On a 1 MB document of
            // 9,000 pages it peaks at 19.30 MB against an 18.15 MB estimate, EXCEEDING it,
            // where PDFium uses 3.39 MB. Too lenient for qpdf rather than too strict, which
            // is #26's successor and not an argument for leaving it out.
            expect: both(Outcome::Err(Failure {
                kind: ErrorKind::LimitExceeded,
                limit: Some("max_memory_bytes".to_owned()),
                stage: Some("size_estimate".to_owned()),
                // A pure function of the input's length, and written out rather than
                // computed: `estimated_open_bytes` is `len + len/4 + BASE_OVERHEAD_BYTES`,
                // which for this 1388-byte file is 1388 + 347 + 16,777,216. Spelled as one
                // number because an expectation that recomputed the implementation's own
                // formula would agree with it by construction, however wrong the formula
                // became. Deterministic and identical on both platforms, which is what
                // makes it comparable where the measured stage is not.
                requested: Some(16_778_951),
                allowed: Some(8 * 1024 * 1024),
            })),
            platform_expectations: Vec::new(),
            // NO KNOWN GAP ANY MORE. #26 is closed; the reason it recorded is above.
            known_gap: None,
        },
        // ---- the bomb the pre-scan does NOT see --------------------------------------
        Fixture {
            name: "objstm-bomb-default-limits",
            filename: "objstm-bomb.pdf",
            bytes: None,
            password: None,
            limits: None,
            attempt_recovery: false,
            // `Ok`. This is what burrow does, and it is wrong: a 200 KB file inflates an
            // object stream to 200 MiB and every check passes it. The measured post-open
            // check compares the resident set before and after, so a peak that occurs
            // *during* is invisible to it.
            expect: both(opens(1)),
            platform_expectations: Vec::new(),
            known_gap: Some(KnownGap {
                issue: "https://github.com/TensorGreed/burrow/issues/24".to_owned(),
                reason: "the pre-scan does not model object-stream inflation, and the \
                         measured check compares resident set before and after so it cannot \
                         see a transient peak. Measured on a 1.4 MB variant: PDFium peaks at \
                         2,437 MB and the file still returns Ok under the default 1 GiB \
                         ceiling."
                    .to_owned(),
                // M2: teaching the pre-scan to bound stream inflation is the fix, and it
                // lands with the rest of the memory-enforcement work the pre-M2 gate settles.
                // Redaction is where a file reaching 2.4 GB stops being merely wasteful.
                milestone: "M2".to_owned(),
            }),
        },
        Fixture {
            name: "objstm-bomb-tight-ceiling",
            filename: "objstm-bomb.pdf",
            bytes: None,
            password: None,
            // 96 MiB, pinned from three directions at once. Above the ~16 MiB fixed
            // `BASE_OVERHEAD_BYTES`, so the *size estimate* does not fire first and the case
            // asserts nothing about the check it is here for. Above
            // `MIN_CONVERGING_MEMORY_BYTES` (64 MiB), below which a worker recycles on every
            // operation and this would be exercising that documented pathology instead. And
            // far enough below the fixture's 200 MiB inflation that the measured check fires
            // with room to spare after its own 64 MiB noise margin.
            limits: Some(CaseLimits {
                max_memory_bytes: Some(96 * 1024 * 1024),
                ..CaseLimits::default()
            }),
            attempt_recovery: false,
            expect: differ(
                Outcome::Err(Failure {
                    kind: ErrorKind::LimitExceeded,
                    limit: Some("max_memory_bytes".to_owned()),
                    stage: Some("measured".to_owned()),
                    // No `requested`: it is the process resident set on native and the engine
                    // module's `HEAPU8.byteLength` on the web (ADR 0007), so the two cannot
                    // agree and neither number is a fact about the file.
                    requested: None,
                    allowed: Some(96 * 1024 * 1024),
                }),
                // NATIVELY, qpdf pays nothing measurable for this file. It does inflate the
                // object stream, but the memory is released before the post-open reading, and
                // the native counter is the process resident set sampled before and after --
                // so the delta is ~0 and the file passes. See the platform expectation below
                // for what the same file does on the web.
                opens(1),
            ),
            platform_expectations: vec![PlatformExpectation {
                platform: Platform::Web,
                operation: Operation::StructureCheck,
                expect: Outcome::Err(Failure {
                    kind: ErrorKind::LimitExceeded,
                    limit: Some("max_memory_bytes".to_owned()),
                    stage: Some("measured".to_owned()),
                    requested: None,
                    allowed: Some(96 * 1024 * 1024),
                }),
                reason: "the measured check reads a different counter on each platform, \
                         which ADR 0007 records deliberately: the process resident set on \
                         native, and the engine module's HEAPU8.byteLength on the web. WASM \
                         linear memory NEVER SHRINKS, so the web reading includes the peak \
                         this file causes while the native one sees only what was still held \
                         afterwards -- qpdf inflates 200 MiB and frees it before the native \
                         sampling. The web is therefore strictly stronger here, and this \
                         entry records that rather than hiding it behind a skipped case."
                    .to_owned(),
            }],
            known_gap: None,
        },
        // ---- files the two engines genuinely disagree about ---------------------------
        Fixture {
            name: "object-number-above-int-max",
            filename: "object-number-above-int-max.pdf",
            bytes: Some(minimal_pdf::pdf_with_object_number_above_int_max()),
            password: None,
            limits: None,
            attempt_recovery: false,
            // The file that used to abort the process: `qpdf_is_linearized` is not routed
            // through qpdf's `trap_errors`, and `QIntC::to_int` throws above `INT_MAX`
            // (ADR 0013 §1). burrow no longer calls it, so this is now an ordinary damaged
            // file to qpdf and an ordinary readable one to PDFium. It stays in the corpus so
            // that reintroducing an untrapped call aborts the suite rather than passing it.
            expect: differ(opens(1), malformed()),
            platform_expectations: Vec::new(),
            known_gap: None,
        },
        Fixture {
            name: "truncated-mid-object",
            filename: "truncated-mid-object.pdf",
            bytes: Some(minimal_pdf::truncated_mid_object()),
            password: None,
            limits: None,
            attempt_recovery: false,
            expect: both(malformed()),
            platform_expectations: Vec::new(),
            known_gap: None,
        },
        Fixture {
            name: "truncated-mid-object-recovered",
            filename: "truncated-mid-object.pdf",
            bytes: Some(minimal_pdf::truncated_mid_object()),
            password: None,
            limits: None,
            attempt_recovery: true,
            // ADR 0013's *The repair question, answered*: qpdf reads a file PDFium refuses
            // outright. This pair is the evidence for keeping qpdf at all, and it had never
            // been run through the web path before this PR.
            expect: differ(malformed(), opens(1)),
            platform_expectations: Vec::new(),
            known_gap: None,
        },
        Fixture {
            name: "trailer-removed",
            filename: "trailer-removed.pdf",
            bytes: Some(minimal_pdf::trailer_removed()),
            password: None,
            limits: None,
            attempt_recovery: false,
            expect: both(malformed()),
            platform_expectations: Vec::new(),
            known_gap: None,
        },
        Fixture {
            name: "trailer-removed-recovered",
            filename: "trailer-removed.pdf",
            bytes: Some(minimal_pdf::trailer_removed()),
            password: None,
            limits: None,
            attempt_recovery: true,
            // The other half of the pair, and the one pinned exactly: unlike the truncation,
            // this file's recovered page count does not depend on where a cut landed.
            expect: differ(malformed(), opens(3)),
            platform_expectations: Vec::new(),
            known_gap: None,
        },
        // ---- the canary ---------------------------------------------------------------
        Fixture {
            name: "canary",
            filename: "canary.pdf",
            bytes: Some(minimal_pdf::pdf_with_fixed_canary()),
            password: None,
            limits: None,
            attempt_recovery: false,
            // Deliberately damaged, so it reaches a failure path: a file that parses cleanly
            // proves nothing about what failure messages contain. PDFium reconstructs enough
            // to find one page; qpdf refuses without recovery.
            expect: differ(opens(1), malformed()),
            platform_expectations: Vec::new(),
            known_gap: None,
        },
        Fixture {
            name: "canary-recovered",
            filename: "canary.pdf",
            bytes: Some(minimal_pdf::pdf_with_fixed_canary()),
            password: None,
            limits: None,
            attempt_recovery: true,
            expect: differ(opens(1), opens(1)),
            platform_expectations: Vec::new(),
            known_gap: None,
        },
    ];

    let mut cases = Vec::new();
    let mut files_compared = 0usize;
    for fixture in files {
        let path = fixtures.join(fixture.filename);
        let bytes = match fixture.bytes {
            Some(bytes) => {
                if check_only {
                    files_compared += 1;
                    match std::fs::read(&path) {
                        Ok(committed) if committed == bytes => {}
                        // BY DIGEST, NOT BY LENGTH. The first version reported the two
                        // lengths -- and the real drift it was written for changed
                        // `/MediaBox [0 0 612 792]` to `[0 0 101 792]`, which is the same
                        // number of bytes. "committed 1400 bytes, this generator produces
                        // 1400" is a message that makes a reader doubt the tool.
                        Ok(committed) => drift.push(format!(
                            "{}: committed sha256 {}, this generator produces {}",
                            fixture.filename,
                            &sha256_hex(&committed)[..16],
                            &sha256_hex(&bytes)[..16]
                        )),
                        Err(e) => drift.push(format!("{}: cannot read: {e}", fixture.filename)),
                    }
                } else {
                    std::fs::write(&path, &bytes)
                        .unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
                    println!("wrote {} ({} bytes)", path.display(), bytes.len());
                }
                bytes
            }
            None => {
                // Generated by a Python script, because it needs a real deflate compressor.
                // Read rather than written, so its digest is still recorded here and an
                // accidental edit still fails the suite.
                let bytes = std::fs::read(&path).unwrap_or_else(|e| {
                    panic!(
                        "reading {}: {e}. This fixture is generated by tools/, not by this \
                         example -- see tests/conformance/PROVENANCE.md",
                        path.display()
                    )
                });
                println!(
                    "listed {} ({} bytes, generated by tools/)",
                    path.display(),
                    bytes.len()
                );
                bytes
            }
        };
        cases.push(Case {
            name: fixture.name.to_owned(),
            inputs: vec![CaseInput {
                file: format!("fixtures/{}", fixture.filename),
                sha256: sha256_hex(&bytes),
            }],
            password: fixture.password.map(str::to_owned),
            limits: fixture.limits,
            attempt_recovery: fixture.attempt_recovery,
            expect: fixture.expect,
            platform_expectations: fixture.platform_expectations,
            known_gap: fixture.known_gap,
        });
    }

    // MERGE CASES, added with schema 3. Built here rather than in the `files` table because
    // they name fixtures that table has already written, in combinations -- a merge case has
    // no fixture of its own, which is exactly why the schema needed an input LIST.
    //
    // Expected page counts are written by hand, like every other outcome in this file. The
    // module docs are emphatic about it and merge is the case where the temptation is
    // strongest: summing the inputs' counts would be recording what the engine does rather
    // than what anyone decided it should do.
    let digest_of = |name: &str| {
        let path = fixtures.join(name);
        sha256_hex(&std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
    };
    let merge_input = |name: &str| CaseInput {
        file: format!("fixtures/{name}"),
        sha256: digest_of(name),
    };

    for (name, files, expect) in [
        (
            "merge-two-ordinary",
            vec!["pages-10.pdf", "pages-137.pdf"],
            merges(opens(147)),
        ),
        (
            "merge-one-document-is-the-identity",
            vec!["pages-10.pdf"],
            merges(opens(10)),
        ),
        (
            "merge-three-including-a-single-page",
            vec!["blank-1page.pdf", "pages-10.pdf", "blank-1page.pdf"],
            merges(opens(12)),
        ),
        (
            // ALL-OR-NOTHING, in the corpus rather than only in a unit test. The second
            // input is fine; the first is not, and the whole operation fails. ADR 0017 §2.
            "merge-refuses-when-any-input-is-malformed",
            vec!["not-a-pdf.bin", "pages-10.pdf"],
            merges(Outcome::Err(Failure::of(ErrorKind::InputFailed))),
        ),
        (
            // An encrypted input with no password fails the whole merge, naming its index.
            "merge-refuses-an-encrypted-input",
            vec!["pages-10.pdf", "encrypted.pdf"],
            merges(Outcome::Err(Failure::of(ErrorKind::InputFailed))),
        ),
        (
            // ADR 0017's correction to ADR 0013: qpdf COUNTS three pages in this file and
            // cannot extract the first. Reading a damaged file is not merging it.
            "merge-refuses-a-trailerless-input-qpdf-can-count",
            vec!["trailer-removed.pdf", "pages-10.pdf"],
            merges(Outcome::Err(Failure::of(ErrorKind::InputFailed))),
        ),
    ] {
        cases.push(Case {
            name: name.to_owned(),
            inputs: files.iter().map(|f| merge_input(f)).collect(),
            password: None,
            limits: None,
            attempt_recovery: false,
            expect,
            platform_expectations: Vec::new(),
            known_gap: None,
        });
    }

    // THE AGGREGATE CEILINGS, which are the whole reason `merge` checks a limit twice.
    //
    // These two were hand-written into `expectations.json` and never added here -- so the
    // first regeneration deleted them, and nothing failed: the corpus simply got smaller and
    // the run still printed OK. Found by code review on rotate's branch, and it is the shape
    // `CLAUDE.md` names as worse than no check at all. `the_corpus_is_not_shrinking` is the
    // gate that makes it impossible to repeat; this is the fix for the loss itself.
    for (name, limits, expect) in [
        (
            "merge-refuses-when-the-total-passes-max-input-bytes",
            CaseLimits {
                max_input_bytes: Some(2000),
                ..CaseLimits::default()
            },
            // Per input both are under the ceiling; together they are not. The point of the
            // aggregate check: a hundred inputs each just under it is a hundred times over.
            Outcome::Err(Failure {
                kind: ErrorKind::LimitExceeded,
                limit: Some("max_input_bytes".to_owned()),
                stage: Some("input_size".to_owned()),
                requested: Some(2776),
                allowed: Some(2000),
            }),
        ),
        (
            "merge-refuses-when-the-total-passes-max-pages",
            CaseLimits {
                max_pages: Some(11),
                ..CaseLimits::default()
            },
            // `InputFailed`, not a bare `LimitExceeded`: the ceiling is reached while opening
            // an input, so it arrives wrapped -- and the wrapper carries the detail, which is
            // what ADR 0017's correction was about.
            Outcome::Err(Failure {
                kind: ErrorKind::InputFailed,
                limit: Some("max_pages".to_owned()),
                stage: Some("page_count".to_owned()),
                requested: Some(20),
                allowed: Some(11),
            }),
        ),
    ] {
        cases.push(Case {
            name: name.to_owned(),
            inputs: vec![merge_input("pages-10.pdf"), merge_input("pages-10.pdf")],
            password: None,
            limits: Some(limits),
            attempt_recovery: false,
            expect: merges(expect),
            platform_expectations: Vec::new(),
            known_gap: None,
        });
    }

    let expectations = Expectations {
        schema: 3,
        generated_by: GENERATED_BY.to_owned(),
        current_milestone: CURRENT_MILESTONE.to_owned(),
        cases,
    };
    let json = serde_json::to_string_pretty(&expectations).expect("the schema should serialise");
    let out = dir.join("expectations.json");
    let rendered = format!("{json}\n");

    if check_only {
        match std::fs::read_to_string(&out) {
            Ok(committed) if committed == rendered => {}
            Ok(_) => drift.push(
                "expectations.json: the committed file is not what this generator produces"
                    .to_owned(),
            ),
            Err(e) => drift.push(format!("expectations.json: cannot read: {e}")),
        }
        // REPORTS WHAT IT EXAMINED, not merely a verdict. A comparison that silently stopped
        // covering fixtures would otherwise print the same "OK" as a real one.
        println!(
            "--check: compared {} fixture(s) and expectations.json against this generator",
            files_compared
        );
        if drift.is_empty() {
            println!("OK -- the committed corpus is what this generator produces.");
            return;
        }
        eprintln!("\nFAILED -- {} file(s) have drifted:", drift.len());
        for item in &drift {
            eprintln!("  - {item}");
        }
        eprintln!(
            "\n  Run `cargo run -p burrow-engines --all-features \\\n    --example              make-conformance-fixtures M1` and commit the result.\n  A fixture generator              changed without the corpus being regenerated -- the corpus is self-consistent\n               either way, which is why nothing else notices."
        );
        std::process::exit(1);
    }

    std::fs::write(&out, rendered).unwrap_or_else(|e| panic!("writing {}: {e}", out.display()));
    println!("wrote {}", out.display());
}

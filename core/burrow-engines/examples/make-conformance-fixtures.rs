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
    Case, CaseLimits, ErrorKind, Expectations, Failure, KnownGap, Operation, Outcome, Outcomes,
    Platform, PlatformExpectation, conformance_dir, sha256_hex,
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
    Outcome::Ok { page_count }
}

/// Both engines produce the same outcome.
fn both(outcome: Outcome) -> Outcomes {
    Outcomes {
        page_count: outcome.clone(),
        structure_check: outcome,
    }
}

/// The engines disagree, which several fixtures exist to pin.
fn differ(page_count: Outcome, structure_check: Outcome) -> Outcomes {
    Outcomes {
        page_count,
        structure_check,
    }
}

fn main() {
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
            expect: both(malformed()),
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
            expect: differ(
                Outcome::Err(Failure {
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
                }),
                // **qpdf does not run this check at all**, on either platform. Found by adding
                // this case: `check_open_memory` is called from both PDFium paths and neither
                // qpdf path, so a ceiling below the estimate refuses a file through one engine
                // and not the other. It is consistent across native and web, so it is an
                // engine difference rather than a divergence -- and it is why this entry is
                // `differ` rather than `both`. Issue #26.
                opens(10),
            ),
            platform_expectations: Vec::new(),
            known_gap: Some(KnownGap {
                issue: "https://github.com/TensorGreed/burrow/issues/26".to_owned(),
                reason: "the length-based memory estimate runs on the PDFium path and not the \
                         qpdf path, so `max_memory_bytes` does not get its cheapest pre-check \
                         when a caller asks for a structure check. Consistent across native \
                         and web, so not a divergence -- but undocumented until this case \
                         surfaced it."
                    .to_owned(),
                // M2: this is one of the numbers ADR 0015 §7 deferred to a pre-M2 decision,
                // and #25's remedy is the same decision. Fixing it before then would mean
                // giving qpdf its own cost model on a guess.
                milestone: "M2".to_owned(),
            }),
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
    for fixture in files {
        let path = fixtures.join(fixture.filename);
        let bytes = match fixture.bytes {
            Some(bytes) => {
                std::fs::write(&path, &bytes)
                    .unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
                println!("wrote {} ({} bytes)", path.display(), bytes.len());
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
            file: format!("fixtures/{}", fixture.filename),
            sha256: sha256_hex(&bytes),
            password: fixture.password.map(str::to_owned),
            limits: fixture.limits,
            attempt_recovery: fixture.attempt_recovery,
            expect: fixture.expect,
            platform_expectations: fixture.platform_expectations,
            known_gap: fixture.known_gap,
        });
    }

    let expectations = Expectations {
        schema: 2,
        generated_by: GENERATED_BY.to_owned(),
        current_milestone: CURRENT_MILESTONE.to_owned(),
        cases,
    };
    let json = serde_json::to_string_pretty(&expectations).expect("the schema should serialise");
    let out = dir.join("expectations.json");
    std::fs::write(&out, format!("{json}\n"))
        .unwrap_or_else(|e| panic!("writing {}: {e}", out.display()));
    println!("wrote {}", out.display());
}

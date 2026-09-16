//! The schema of `tests/conformance/expectations.json`.
//!
//! Included by the conformance test and by the fixture generator, so the writer and the
//! reader cannot disagree about the format.
//!
//! # Why this file is data and not a list of `assert_eq!`s
//!
//! M1 PR 4b is a **differential conformance harness**: the same corpus run through the
//! native `DocumentEngine`/`StructureEngine` and through the web ones, asserting identical
//! typed outcomes (ROADMAP M1 item 12). If the expected outcomes lived in Rust `assert_eq!`s,
//! the web side would have to restate every one of them in TypeScript, and the two lists
//! would drift — which is the failure this file exists to prevent. The JSON is the contract;
//! neither implementation owns it.
//!
//! So the schema is deliberately dull and language-neutral: no Rust types leak into it, the
//! outcome is a tag rather than a message, and every field is something JavaScript can read
//! without a parser of its own.
//!
//! # What schema 2 added, and why each part was forced
//!
//! Schema 1 modelled **one engine** under **one set of limits** producing **one error kind**.
//! Each of those was a real limitation, not a simplification:
//!
//! - **Per-operation outcomes.** `page_count` is PDFium and `structure_check` is qpdf, and
//!   they legitimately disagree: ADR 0013 records two files qpdf reads and PDFium refuses.
//!   Under schema 1 those files could not be in the corpus at all.
//! - **Per-case limits.** `xref-bomb.pdf` has been on disk since PR 3 and was deliberately
//!   *absent* from the expectations, because its outcome is a function of the ceiling it is
//!   opened with and the schema could not say so.
//! - **The stage.** Three different checks produce `LimitExceeded { limit: "max_memory_bytes" }`
//!   — the length-based estimate, the structural pre-scan, and the measured post-open check.
//!   Comparing outcomes without the stage would read "the same error reached by a completely
//!   different route" as agreement, and the difference between refusing a file and refusing it
//!   *after* allocating a gigabyte is the entire value of the pre-scan.
//! - **`platform_expectations`.** Some differences are by design and must be recorded rather
//!   than skipped — a skipped case records "untested", which is the wrong memory to leave for
//!   M2. See [`PlatformExpectation`].
//! - **`known_gap`.** A case that documents a defect we have not fixed yet, with the issue
//!   that tracks it. See [`KnownGap`].

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

/// The whole file.
#[derive(Debug, Deserialize, Serialize)]
pub struct Expectations {
    /// Bumped when the shape below changes incompatibly. A reader that does not recognise
    /// the version must fail, not guess.
    pub schema: u32,
    /// The command that produced this file and the fixtures beside it.
    pub generated_by: String,
    /// The milestone burrow is currently working in, e.g. `"M1"`.
    ///
    /// This exists so [`KnownGap::milestone`] can mean something a check can enforce. A gap
    /// targeted at a milestone we have reached or passed is a gap whose deadline went by,
    /// and the conformance suite fails on it -- so bumping this field is the act that forces
    /// every outstanding gap to be fixed or deliberately re-targeted, in a diff a reviewer
    /// sees.
    ///
    /// A declared value rather than a date or a GitHub query: a date gate fails on an idle
    /// branch, for reasons that have nothing to do with the code, and a query puts a network
    /// call and a token inside a check that has to run offline.
    pub current_milestone: String,
    /// One entry per (fixture, limits) combination worth asserting.
    pub cases: Vec<Case>,
}

/// One fixture, the conditions it is opened under, and what every operation must produce.
#[derive(Debug, Deserialize, Serialize)]
pub struct Case {
    /// Short identifier, for test output. Unique across the file.
    pub name: String,
    /// The fixtures this case runs, in order.
    ///
    /// **A list since schema 3.** `merge` takes several documents and the order is the
    /// operation's entire meaning, so a single `file` could not express a case at all.
    /// Single-input cases carry a one-element list rather than keeping a shorthand: two
    /// ways of saying the same thing is two code paths in every reader, and the readers are
    /// the part that must not drift.
    ///
    /// Several cases may name the same file: `encrypted.pdf` appears with and without a
    /// password, and `objstm-bomb.pdf` appears under two different ceilings.
    pub inputs: Vec<CaseInput>,
    /// Password to open with, or `null` for none.
    ///
    /// A string, not bytes: every fixture's password is ASCII, and a JSON string is what the
    /// web harness can pass straight through. A fixture needing a non-UTF-8 password would
    /// need this field to grow a byte-array form, and that is a schema bump.
    pub password: Option<String>,
    /// Ceilings to open under. Omitted fields keep `Limits::DEFAULT`.
    ///
    /// **A case's outcome is a function of these**, which is why they are here rather than
    /// assumed. A bomb is `Ok` under a generous ceiling and `LimitExceeded` under a tight
    /// one, and both are worth asserting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<CaseLimits>,
    /// Whether qpdf runs with recovery enabled for this case.
    ///
    /// Off by default, matching `CheckOptions`: a structural check that silently repairs the
    /// structure it is checking answers a question the caller did not ask.
    #[serde(default)]
    pub attempt_recovery: bool,
    /// What each operation must produce.
    pub expect: Outcomes,
    /// Differences that are **by design**, with a mandatory reason.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub platform_expectations: Vec<PlatformExpectation>,
    /// A defect this case documents rather than asserts as correct.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known_gap: Option<KnownGap>,
}

/// One fixture in a case's input list.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CaseInput {
    /// Path to the fixture, relative to the directory holding `expectations.json`.
    pub file: String,
    /// sha256 of the fixture, so an edited file fails instead of quietly changing what is
    /// asserted.
    pub sha256: String,
}

/// Ceilings for one case. Every field optional; omitted means `Limits::DEFAULT`.
#[derive(Debug, Default, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
pub struct CaseLimits {
    /// See [`burrow_types::Limits::max_input_bytes`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_input_bytes: Option<u64>,
    /// See [`burrow_types::Limits::max_memory_bytes`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_memory_bytes: Option<u64>,
    /// See [`burrow_types::Limits::max_duration_ms`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duration_ms: Option<u64>,
    /// See [`burrow_types::Limits::max_pages`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_pages: Option<u64>,
}

impl CaseLimits {
    /// The `Limits` this describes.
    #[must_use]
    pub fn to_limits(self) -> burrow_types::Limits {
        burrow_types::Limits::with(|l| {
            if let Some(v) = self.max_input_bytes {
                l.max_input_bytes = v;
            }
            if let Some(v) = self.max_memory_bytes {
                l.max_memory_bytes = v;
            }
            if let Some(v) = self.max_duration_ms {
                l.max_duration_ms = v;
            }
            if let Some(v) = self.max_pages {
                l.max_pages = v;
            }
        })
    }
}

/// The operations a case is run through, and what each must produce.
///
/// **A map since schema 3, and it had to become one.** Schema 2 was a struct with one field
/// per operation, which said "every case runs every operation" in the type — true while the
/// two operations both took a single document and answered with a page count. `merge` takes
/// SEVERAL documents, so requiring every case to declare a merge outcome would have meant
/// inventing one for twenty-one single-document fixtures, and `split` and `reorder` produce
/// several outputs and will not fit the old shape either.
///
/// A case now declares the operations it is about. Two rules keep that from becoming a way
/// to quietly stop testing something, and the harness asserts both:
///
/// - every case declares at least one operation, so a fixture cannot sit in the corpus
///   asserting nothing;
/// - every operation in [`Operation::ALL`] appears in at least one case, so an operation
///   cannot fall out of the corpus entirely and leave a green run behind.
pub type Outcomes = std::collections::BTreeMap<Operation, Outcome>;

/// Which operation an outcome belongs to.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    /// PDFium.
    PageCount,
    /// qpdf.
    StructureCheck,
    /// `burrow_ops::merge`, over qpdf's assembler. Takes every input in order.
    Merge,
    /// `burrow_ops::rotate`, over qpdf. **Every page, by 90 degrees.**
    ///
    /// Fixed rather than declared per case: a case shape that carried a page list and an
    /// angle would be a second thing to keep in step across three readers, and the one
    /// question this corpus exists to answer is whether the two implementations agree --
    /// which every-page-by-90 asks as well as any other selection, on a fixture whose pages
    /// start at different rotations.
    Rotate,
    /// `burrow_ops::reorder`, over qpdf. **Reverse every page.**
    ///
    /// Fixed, like `Rotate`, and for the same reason.
    ///
    /// # The observable is the ROTATIONS, and that is not a workaround
    ///
    /// A reorder changes which page is where, and a page count cannot see it -- so a case
    /// asserting only `page_count` would pass against an implementation that did nothing.
    /// Neither side has a way to say "this is page 3" without a per-page readout, and the
    /// corpus already has one: the effective rotation, which `Rotate` uses and both
    /// implementations compute from emitted bytes by walking the page tree separately.
    ///
    /// So a reversal is checked by reversing the rotations vector. On a fixture whose pages
    /// have different rotations that is a strong assertion -- it fails for any permutation
    /// that is not the one asked for, not merely for a no-op -- and it needs no new entry
    /// point on either side. On a fixture where every page shares a rotation it degrades to
    /// "nothing was lost", which is worth having and is not pretended to be more.
    Reorder,
    /// `burrow_ops::split`, over qpdf. **Cut after the first page.**
    ///
    /// Fixed, like `Rotate` and `Reorder`, and for the same reason.
    ///
    /// # This case exists to catch ONE path failing to prune at all
    ///
    /// `split`'s pruning policy is shared between the native and web implementations
    /// (`crate::prune`), which is deliberate and is argued there: a divergence between two
    /// prunings would be a leak on one platform and not the other, and this corpus compares
    /// typed outcomes and page counts -- neither of which can see an object that should not
    /// have travelled.
    ///
    /// **What that trades away is the thing this corpus is for**, so the risk it leaves has to
    /// be caught by something. The remaining failure is not two policies disagreeing, it is one
    /// path never reaching the policy: a `prune_output` call dropped from `qpdf/extract.rs` or
    /// from `web/extract.rs` while the other keeps it.
    ///
    /// That is observable through a typed outcome and needs no new expectation shape. The
    /// refusal for optional content lives INSIDE the policy, so a path that skips pruning does
    /// not refuse -- it succeeds. On a layered fixture the expectation is `Unsupported`, and a
    /// path that stopped pruning reports `Ok` and diverges. Verified by mutation on both sides;
    /// see `tools/test-prune-is-reached.sh`.
    Split,
    /// `burrow_ops::compress`, over qpdf. **The whole document; there is no selection.**
    ///
    /// # What this case can and cannot catch, because the difference is not obvious
    ///
    /// The observable is the page count, the rotation vector, and **whether the output came
    /// back smaller**. The first two are ADR 0022's witness and catch what they catch
    /// everywhere: a page lost, a page reattributed, a document one path refuses and the other
    /// does not.
    ///
    /// **The third is here because without it this case would be nearly blind.** Compression's
    /// specific divergence risk is one path setting `qpdf_o_generate` and the other not — and
    /// that produces a valid document with an identical page count and an identical rotation
    /// vector on both sides. A case comparing only those would pass against a web path that had
    /// silently become a plain write, which is the exact failure this corpus exists to find.
    ///
    /// **It is a BOOLEAN rather than a byte count, deliberately.** Comparing sizes would assert
    /// that native libqpdf and the same source compiled to wasm emit byte-identical output — a
    /// much stronger claim than this corpus needs, and one that would fail the whole operation
    /// over a single byte of difference in a build detail. "Did it get smaller" is the property
    /// the operation actually promises, and it differs between a path that compresses and one
    /// that does not.
    ///
    /// What it still cannot see: two paths that both compress but by different amounts, and
    /// anything about what compression did to a page's *content* — which is
    /// `compress_keeps_everything.rs`'s job and cannot be a runtime comparison (ADR 0022).
    Compress,
}

impl Operation {
    /// Every operation, in a stable order.
    pub const ALL: [Self; 7] = [
        Self::PageCount,
        Self::StructureCheck,
        Self::Merge,
        Self::Rotate,
        Self::Reorder,
        Self::Split,
        Self::Compress,
    ];

    /// The name used in the JSON and in the harness's records.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PageCount => "page_count",
            Self::StructureCheck => "structure_check",
            Self::Merge => "merge",
            Self::Rotate => "rotate",
            Self::Reorder => "reorder",
            Self::Split => "split",
            Self::Compress => "compress",
        }
    }
}

/// Which implementation an outcome came from.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq, Clone, Copy, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    /// `burrow-engines`' PDFium and qpdf, linked natively.
    Native,
    /// The same Rust over the JS bridge, in a browser worker.
    Web,
}

/// The required outcome: a value, or a specific typed failure.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq, Clone)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The operation succeeds, and reports this many pages.
    Ok {
        /// Pages the engine must report.
        page_count: u64,
        /// Every page's effective rotation, in page order. `rotate` and `reorder` cases.
        ///
        /// **Because a page count cannot fail for a rotation.** Rotate does not change how
        /// many pages there are -- that is one of its invariants -- so a case that asserted
        /// only `page_count` would pass against an implementation that did nothing at all.
        /// The rotations are the readout that distinguishes a rotation from a no-op, and
        /// they are comparable across implementations because they are what both paths
        /// compute from the emitted bytes.
        ///
        /// They also carry the part most likely to DIVERGE between native and web: the
        /// effective rotation is the nearest `/Rotate` up the page tree, and the two
        /// implementations walk that tree separately, on purpose.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rotations: Option<Vec<i64>>,
        /// Whether the output came back smaller than the input. `compress` cases.
        ///
        /// **Because a page count and a rotation vector cannot see compression happening.** A
        /// path that stopped setting the object stream mode would emit a valid document with
        /// both of those identical, and the case would pass — see [`Operation::Compress`].
        ///
        /// A boolean rather than a size: comparing byte counts across native libqpdf and the
        /// same source compiled to wasm asserts far more than this corpus needs.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        compressed: Option<bool>,
    },
    /// The operation fails, exactly this way.
    Err(Failure),
}

/// A typed failure, in as much detail as is comparable across implementations.
///
/// The **variant**, never the message. Messages are ours to reword; a variant is the
/// contract, and it is what both implementations have to agree on.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq, Clone)]
pub struct Failure {
    /// The `burrow_types::Error` variant.
    pub kind: ErrorKind,
    /// For `LimitExceeded`: the field the caller set, e.g. `"max_memory_bytes"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<String>,
    /// For `LimitExceeded`: which check fired, from `burrow_types::Stage::as_str`.
    ///
    /// **Required whenever `kind` is `LimitExceeded`**, and
    /// `the_schema_records_a_route_for_every_limit_failure` enforces it. An expectation that
    /// omitted it would let a rejection move between the pre-scan and the measured check
    /// without anything noticing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    /// What the input asked for.
    ///
    /// Omitted for `stage: "measured"`, and **only** there: that number is the process
    /// resident set on native and the engine module's `HEAPU8.byteLength` on the web, a
    /// difference ADR 0007 records deliberately, so the two can never be equal. Every other
    /// stage computes it from the file and must match exactly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested: Option<u64>,
    /// What the configured limit permitted. Always comparable: it is the caller's own number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed: Option<u64>,
}

impl Failure {
    /// A failure with no limit detail.
    #[must_use]
    pub fn of(kind: ErrorKind) -> Self {
        Self {
            kind,
            limit: None,
            stage: None,
            requested: None,
            allowed: None,
        }
    }
}

/// A difference between implementations that is **expected**, with the reason it exists.
///
/// # Not the same thing as the allowlist
///
/// This records a difference we **designed**. `tests/conformance/divergences.toml` records a
/// difference we have **accepted but not designed**, and it needs an issue link. Conflating
/// them would turn "we know why these differ" into "we have not got round to this", which is
/// exactly the distinction a reader six months later needs.
///
/// The archetype is a hang fixture: the native path cannot interrupt a single engine call
/// (ADR 0007), while the web path's watchdog terminates the worker (ADR 0015 §2). Skipping
/// such a case would record it as untested; this records what it actually does, and fails if
/// that stops being true.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq, Clone)]
pub struct PlatformExpectation {
    /// The implementation this applies to.
    pub platform: Platform,
    /// The operation this applies to.
    pub operation: Operation,
    /// What that platform produces instead of [`Case::expect`].
    pub expect: Outcome,
    /// Why. **Mandatory**, and a mechanism rather than a restatement of the difference.
    pub reason: String,
}

/// A defect a case documents rather than endorses.
///
/// The outcome recorded for such a case is what burrow **does**, not what it **should** do.
/// CI stays green, the harness prints it in its summary, and — the part that makes this
/// honest — fixing the defect changes the outcome, which breaks the recorded expectation and
/// forces the fixture and the issue to be closed together.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq, Clone)]
pub struct KnownGap {
    /// The issue tracking it. **Mandatory.**
    pub issue: String,
    /// What is wrong, in one sentence. **Mandatory.**
    pub reason: String,
    /// The milestone this gap must be closed by, e.g. `"M2"`. **Mandatory.**
    ///
    /// A `known_gap` is green CI and an open defect at the same time. That trade is
    /// reasonable -- the alternative is a skipped test, which records "untested", the wrong
    /// memory to leave for M2 -- but it needs a deadline, or `known_gap` becomes where
    /// inconvenient failures go and the corpus quietly stops asserting anything.
    ///
    /// Enforced against [`Expectations::current_milestone`]: a gap targeted at a milestone
    /// that has been reached or passed fails the suite. It is not a soft warning, because a
    /// warning on a green run is a thing people stop reading.
    pub milestone: String,
}

/// The project's milestones, in order. Used to decide whether a [`KnownGap`] is overdue.
///
/// A fixed list rather than a parsed number: it is short, it is the same list
/// `docs/ROADMAP.md` is organised around, and a typo in a milestone name should fail loudly
/// rather than sort as zero.
pub const MILESTONES: [&str; 7] = ["M0", "M1", "M2", "M3", "M4", "M5", "M6"];

/// Position of `milestone` in [`MILESTONES`], or `None` if it is not a milestone we know.
pub fn milestone_index(milestone: &str) -> Option<usize> {
    MILESTONES.iter().position(|m| *m == milestone)
}

/// A `burrow_types::Error` variant, by name.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq, Clone, Copy)]
pub enum ErrorKind {
    /// [`Error::Malformed`](burrow_types::Error::Malformed).
    Malformed,
    /// [`Error::Unsupported`](burrow_types::Error::Unsupported).
    Unsupported,
    /// [`Error::PasswordRequired`](burrow_types::Error::PasswordRequired).
    PasswordRequired,
    /// [`Error::LimitExceeded`](burrow_types::Error::LimitExceeded).
    LimitExceeded,
    /// [`Error::InvalidArgument`](burrow_types::Error::InvalidArgument).
    InvalidArgument,
    /// [`Error::Io`](burrow_types::Error::Io).
    Io,
    /// [`Error::Internal`](burrow_types::Error::Internal).
    Internal,
    /// [`Error::InputFailed`](burrow_types::Error::InputFailed).
    ///
    /// Multi-input operations only. The INDEX is deliberately not recorded: it is in the
    /// error a caller sees, and putting it here would make the corpus assert which input
    /// the engine happened to reach first rather than that the operation refused. ADR 0017
    /// §2 is about the refusal.
    InputFailed,
}

impl ErrorKind {
    /// Classify an error by variant.
    ///
    /// Returns `None` for a variant added since this was written — `Error` is
    /// `#[non_exhaustive]`, and a case that silently matched the wildcard would pass while
    /// asserting nothing.
    pub fn of(error: &burrow_types::Error) -> Option<Self> {
        use burrow_types::Error as E;
        Some(match error {
            E::Malformed(_) => Self::Malformed,
            E::Unsupported(_) => Self::Unsupported,
            E::PasswordRequired => Self::PasswordRequired,
            E::LimitExceeded { .. } => Self::LimitExceeded,
            E::InvalidArgument(_) => Self::InvalidArgument,
            E::Io(_) => Self::Io,
            E::Internal(_) => Self::Internal,
            E::InputFailed { .. } => Self::InputFailed,
            _ => return None,
        })
    }
}

/// Turn a real result into the shape the schema records.
///
/// The single place a live outcome becomes a comparable one, so the native harness and the
/// generator cannot describe the same result differently.
///
/// `requested` is **dropped for `Stage::Measured`** — see [`Failure::requested`]. Dropping it
/// here rather than at the comparison is deliberate: the recorded outcome should not contain
/// a number that is not a fact about the file, or someone will eventually compare it.
pub fn outcome_of(result: &burrow_types::Result<u64>) -> Outcome {
    outcome_with_rotations(result, None)
}

/// The same, carrying the rotations AND whether the output got smaller — `compress` cases.
///
/// A third entry point rather than more arguments on the others, for the reason
/// `outcome_with_rotations` gives about itself: an operation that has no compression verdict
/// should not be passing `None` for one.
#[must_use]
pub fn outcome_with_compression(
    result: &burrow_types::Result<u64>,
    rotations: Option<Vec<i64>>,
    compressed: Option<bool>,
) -> Outcome {
    if let Ok(pages) = result {
        return Outcome::Ok {
            page_count: *pages,
            rotations,
            compressed,
        };
    }
    outcome_of_failure(result)
}

/// The same, carrying the rotations a `rotate` case compares.
///
/// Separate entry point rather than a fourth argument everywhere: three of the four
/// operations have no rotations to report, and a `None` at every call site would read as an
/// omission rather than as "this operation does not have them".
#[must_use]
pub fn outcome_with_rotations(
    result: &burrow_types::Result<u64>,
    rotations: Option<Vec<i64>>,
) -> Outcome {
    if let Ok(pages) = result {
        return Outcome::Ok {
            page_count: *pages,
            rotations,
            compressed: None,
        };
    }
    outcome_of_failure(result)
}

fn outcome_of_failure(result: &burrow_types::Result<u64>) -> Outcome {
    match result {
        Ok(pages) => Outcome::Ok {
            page_count: *pages,
            rotations: None,
            compressed: None,
        },
        Err(error) => {
            let kind = ErrorKind::of(error).unwrap_or_else(|| {
                // NOT `{error:?}`. Several variants carry a `String` payload, and on a damaged
                // file that payload is the one place something input-derived could be. This is
                // reachable only by adding an `Error` variant, so the message does not need the
                // value to be actionable -- it needs the reader to go and add the variant here.
                panic!(
                    "an Error variant the conformance schema does not know about. Add it to \
                     ErrorKind and decide what both implementations must report."
                )
            });
            // THROUGH `InputFailed`, which names WHICH input and never WHAT. A per-input
            // ceiling arrives wrapped, so matching only the outer variant recorded a bare
            // `InputFailed` and threw away the limit, the stage and both numbers -- the exact
            // detail this schema exists to hold the two implementations to. The KIND stays the
            // wrapper on both sides (see `kind_of`'s note); only the detail is unwrapped.
            let detail = match error {
                burrow_types::Error::InputFailed { source, .. } => source.as_ref(),
                other => other,
            };
            match detail {
                burrow_types::Error::LimitExceeded {
                    limit,
                    stage,
                    requested,
                    allowed,
                } => Outcome::Err(Failure {
                    kind,
                    limit: Some((*limit).to_owned()),
                    stage: Some(stage.as_str().to_owned()),
                    requested: if *stage == burrow_types::Stage::Measured {
                        None
                    } else {
                        Some(*requested)
                    },
                    allowed: Some(*allowed),
                }),
                _ => Outcome::Err(Failure::of(kind)),
            }
        }
    }
}

/// Lowercase hex of a sha256 digest.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// `tests/conformance/`, found from this crate's manifest directory.
///
/// A repository-root path on purpose: the directory is shared with the web harness, so it
/// belongs to neither crate. See its `PROVENANCE.md`.
pub fn conformance_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/conformance")
        .canonicalize()
        .unwrap_or_else(|_| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/conformance")
        })
}

// ---------------------------------------------------------------------------------
// The record the two implementations produce, and the harness compares.
// ---------------------------------------------------------------------------------

/// One implementation's answers for the whole corpus.
///
/// Written by the native conformance test and by the web Playwright spec, then diffed. The
/// shared expectations file already catches "both paths are wrong the same way"; this catches
/// what transitivity cannot — a divergence in something nobody thought to put in the schema.
#[derive(Debug, Deserialize, Serialize)]
pub struct OutcomeRecord {
    /// Which implementation produced it.
    pub platform: Platform,
    /// A label for the run — the browser name on the web, `"native"` otherwise.
    pub runner: String,
    /// sha256 of `expectations.json` as this run read it.
    ///
    /// **The freshness stamp.** A stale record from a previous corpus would otherwise compare
    /// cleanly against a corpus it never saw, and report agreement about files it never ran.
    pub expectations_sha256: String,
    /// One entry per case × operation. Every one is required: a missing entry is a divergence,
    /// never agreement.
    pub results: Vec<RecordedOutcome>,
}

/// One case × operation, as one implementation actually answered it.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq, Clone)]
pub struct RecordedOutcome {
    /// The case's `name`.
    pub case: String,
    /// The operation.
    pub operation: Operation,
    /// What happened.
    pub outcome: Outcome,
}

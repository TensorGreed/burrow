//! Which check produced a [`LimitExceeded`](crate::Error::LimitExceeded).
//!
//! # Why a limit failure needs to say where it came from
//!
//! `max_memory_bytes` is consulted by **three** different mechanisms, and until M1 PR 4b all
//! three reported the same thing. *Consulted*, not enforced: every one of them **detects** an
//! overrun rather than bounding it, which is the distinction
//! [`Limits`](crate::Limits)' own documentation turns on. What differs between them is how
//! early they notice, and that is worth a great deal even when none of them prevents.
//!
//! | mechanism | when it runs | what it knows |
//! |---|---|---|
//! | [`Stage::SizeEstimate`] | before the engine | the input's **length** |
//! | [`Stage::Prescan`] | before the engine | what the file **declares** |
//! | [`Stage::Measured`] | after the open | what the engine **actually used** |
//!
//! Those are not variations on one check. They fail for different reasons, they catch
//! different attacks, and a file that trips one has a completely different shape from a file
//! that trips another. Reporting them identically meant a regression that moved a rejection
//! from the pre-scan to the measured check — losing the protection that matters, since the
//! measured one fires only after the allocation has happened — was indistinguishable from no
//! regression at all.
//!
//! ROADMAP item 12's differential harness is what forced the issue: it compares typed outcomes
//! between the native and web implementations, and *"the same error kind reached by a different
//! route"* has to read as a divergence. It could not, because the route was not in the outcome.
//!
//! # Why an enum rather than a second string
//!
//! [`Limits::check`](crate::Limits::check) would otherwise take two adjacent `&'static str`
//! arguments — `check("prescan", "max_memory_bytes", …)` — which is the transposition hazard
//! `WebLimits` was introduced in PR 4a-i to remove, in a function called from every limit in
//! the codebase. An enum cannot be transposed with a `&'static str`, and it makes the compiler
//! ask every call site where it is.

/// The check that rejected an operation.
///
/// `#[non_exhaustive]` for the same reason [`Error`](crate::Error) is: a mechanism added later
/// must not be a breaking change, and a caller matching on this needs a wildcard arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Stage {
    /// The input was larger than `max_input_bytes`, before anything read it.
    InputSize,
    /// The **length-based** memory estimate, before the input reached an engine.
    ///
    /// Blind to what a file declares: a 330 KB PDF declaring twenty million cross-reference
    /// entries passes this and then costs 1.2 GB. That gap is [`Stage::Prescan`]'s job.
    SizeEstimate,
    /// The **structural pre-scan**, before the input reached an engine.
    ///
    /// Reads declared quantities only — never decompresses, resolves or recurses. This is the
    /// check that stops the declared-size bomb, and the one whose absence is most expensive,
    /// because everything after it fires only once the memory is already committed.
    Prescan,
    /// The page count an engine reported, against `max_pages`.
    PageCount,
    /// The decoded raster size, against `max_pixels`.
    ///
    /// **Reserved.** Nothing constructs this yet: no operation decodes a raster, so there is no
    /// `max_pixels` check to attribute. It is here so the enum describes the limit set rather
    /// than today's subset of it, and so adding the check later is not a `#[non_exhaustive]`
    /// change for every matcher.
    Pixels,
    /// The **measured** cost of an open, after it happened.
    ///
    /// The last line, and the weakest: the allocation has already been made. What it buys is
    /// that the operation fails rather than returning a handle that is already over budget.
    ///
    /// **Its number is not comparable across platforms.** The counter is the process resident
    /// set on native and the engine module's `HEAPU8.byteLength` on the web — a deliberate
    /// difference recorded in ADR 0007, and the reason a differential case that lands here
    /// needs an explicit platform expectation rather than an exact match.
    Measured,
    /// The operation's time budget, against `max_duration_ms`.
    Deadline,
}

impl Stage {
    /// A stable, lowercase name.
    ///
    /// Stable because it crosses two boundaries that outlive any refactor: the wasm binding's
    /// `Reply`, and `tests/conformance/expectations.json`, which is read by both the Rust and
    /// the TypeScript halves of the differential harness. Renaming one of these is a schema
    /// change, not a tidy-up.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InputSize => "input_size",
            Self::SizeEstimate => "size_estimate",
            Self::Prescan => "prescan",
            Self::PageCount => "page_count",
            Self::Pixels => "pixels",
            Self::Measured => "measured",
            Self::Deadline => "deadline",
        }
    }
}

impl core::fmt::Display for Stage {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two stages sharing a name would silently merge in the conformance schema, and the
    /// harness would report agreement between two different checks — the exact failure this
    /// type exists to prevent.
    #[test]
    fn every_stage_has_a_distinct_name() {
        let all = [
            Stage::InputSize,
            Stage::SizeEstimate,
            Stage::Prescan,
            Stage::PageCount,
            Stage::Pixels,
            Stage::Measured,
            Stage::Deadline,
        ];
        let mut seen = std::collections::BTreeSet::new();
        for stage in all {
            assert!(
                seen.insert(stage.as_str()),
                "{stage:?} reuses the name {:?}",
                stage.as_str()
            );
        }
        assert_eq!(seen.len(), all.len());
    }

    /// The names are a wire format. A rename is a schema change, and this is what makes that
    /// a deliberate act rather than a rename refactor nobody noticed crossing a boundary.
    #[test]
    fn the_names_are_the_ones_the_conformance_schema_records() {
        assert_eq!(Stage::Prescan.as_str(), "prescan");
        assert_eq!(Stage::SizeEstimate.as_str(), "size_estimate");
        assert_eq!(Stage::Measured.as_str(), "measured");
        assert_eq!(Stage::InputSize.as_str(), "input_size");
        assert_eq!(Stage::PageCount.as_str(), "page_count");
        assert_eq!(Stage::Pixels.as_str(), "pixels");
        assert_eq!(Stage::Deadline.as_str(), "deadline");
    }

    #[test]
    fn display_matches_the_stable_name() {
        assert_eq!(Stage::Measured.to_string(), Stage::Measured.as_str());
    }
}

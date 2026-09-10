# 0001. Record architecture decisions in ADRs

Date: 2026-09-10

## Status

Accepted

## Context

burrow spans a Rust core, two FFI layers, three frontends, and a set of vendored C/C++
engines. Decisions in that space are frequently irreversible in practice: an engine
choice shapes the API, a licensing rule constrains every future dependency, and a
binding strategy is expensive to unwind once three apps depend on it.

The project also expects contributors — human and agentic — who were not present when a
decision was made. Without a written record, the reasoning lives in commit messages,
pull request threads, and memory, and the same arguments get re-litigated at intervals.
Worse, a constraint whose rationale has been forgotten looks arbitrary and gets quietly
violated.

## Decision

We will record every architecturally significant decision as an ADR in `docs/adr/`,
using the format in [`template.md`](template.md), based on
[Michael Nygard's ADR pattern](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions).

A decision is architecturally significant if it is expensive to reverse, constrains
future work, or is the kind of thing someone would reasonably ask about twice. Concrete
triggers: adding or replacing a native engine; changing the licensing policy; changing a
binding or FFI strategy; changing the crate boundaries; anything affecting the privacy
guarantee; choosing a frontend framework.

Files are named `NNNN-kebab-case-title.md`, numbered sequentially from 0001. **ADRs are
append-only**: an accepted ADR is never edited to change its decision. To change course,
write a new ADR and mark the old one `Superseded by NNNN`. Correcting a typo is fine;
rewriting history is not.

An ADR may be `Proposed` before the work is done — that is the useful case, since it
gets the reasoning reviewed before the code exists.

## Consequences

Decisions become reviewable. A pull request that violates a documented constraint can be
answered with a link instead of an argument, and the constraint can be challenged on its
recorded merits rather than on whether anyone remembers it.

The cost is discipline: an ADR is only useful if it is written at the time, and there is
no way to enforce that mechanically. Retrofitted ADRs describe what we did, not what we
decided, which is much less useful. There is also a judgement call in "architecturally
significant"; we would rather have a few too many ADRs than too few.

The index in [`README.md`](README.md) must be updated by hand when an ADR is added.

## Alternatives considered

**Nothing written down.** The default, and the reason this ADR exists.

**A single long design document.** Cheap to start, but it accretes and gets edited in
place, so the reasoning behind superseded decisions is lost — exactly the information
that is most valuable later.

**Wiki or issue tracker.** Not versioned with the code, not reviewable in a pull request,
and not visible to a contributor who has only cloned the repository.

**Comments in the code.** Right for local invariants, wrong for cross-cutting decisions:
there is no single file where "we do not accept GPL dependencies" belongs.

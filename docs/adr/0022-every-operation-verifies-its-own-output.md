# 0022. Every operation verifies its own output before returning it

Date: 2026-09-14

## Status

Accepted.

Generalises #65, which was written as "merge verifies its own output". It is here rather than
in that issue because it changes what an *operation* is in this codebase, it is the mechanism
[ADR 0006](0006-wasm-linking-strategy.md)'s R8–R10 already require and nothing implements, and
it is the fix in reach for #61.

## Context

Three things arrived at the same place within a day of each other.

**#61 — a damaged-but-openable document loses a page.** Characterised: qpdf's two readings of
the same document disagree — 5 pages without cross-reference reconstruction, 4 with it — and
`open` reports the first while the writer emits the second. The output is a valid PDF, opens
happily, and is short a page with nothing said. It reproduces through `rotate`, through a plain
write, and through `split`'s build route.

**#62 — memory-unsafety in the pinned qpdf**, on the document-open path and in the foreign-object
copier. Measured on every path: a fault natively, a hang on wasm, **no silent wrong output
observed anywhere**. The conditional block placed on `/merge-pdf` was withdrawn because the
argument behind it — *"not observed is not cannot happen"* — applies to every operation and
therefore blocks everything, indefinitely, on an unpatched upstream bug. The right answer to
"cannot be excluded" is a detector, not a hold.

**ADR 0006's R8, R9 and R10** are three conditions on M2 redaction, each naming a check, and
that ADR says plainly: *"None of the three is satisfied until its named check exists and has
been shown to fail without the property."* None exists. All three are about the same thing —
output is emitted only after it has been verified, and the verification runs on the bytes
actually emitted.

And `CLAUDE.md`'s fourth non-negotiable has said from the beginning that redaction output is
verified automatically after every run. Redaction is the operation where a wrong output that
looks right is a leak; it is not the only one where it is data loss.

## Decision

**Every operation verifies its own output before returning it, through one shared step.**

```
core/burrow-ops/src/verify.rs
```

An operation does not return bytes. It returns bytes *that have been checked*, and the check is
the last thing between the engine and the caller.

### The shape

```rust
/// What an operation promises about the document it produced.
pub enum Expected {
    /// The same pages came out as went in. rotate, reorder, compress.
    PagesUnchanged(u64),
    /// Every input's pages, once each. merge.
    PagesTotalling(u64),
    /// One part of a partition; the operation checks the parts sum. split.
    PagesExactly(u64),
}

/// Reopen `output` and refuse it if it is not what the operation promised.
pub fn output<E: DocumentEngine>(
    engine: &E,
    output: &[u8],
    expected: Expected,
    options: &OpenOptions<'_>,
) -> Result<()>;
```

Three properties are load-bearing and each exists for a reason already paid for elsewhere:

- **It runs on the emitted bytes**, not on a source, a copy, or the engine's in-memory state.
  That is R10 exactly, and it is why the signature takes `&[u8]` rather than a handle.
- **It runs before the caller has the bytes.** The operation returns `Err` and drops the
  buffer; nothing downstream ever sees an unverified document. That is R9's property at the
  core layer, where it holds for the native paths too rather than only in a worker.
- **It reopens through the engine seam**, never a second parser. A verifier with its own parser
  is a second implementation that can agree with the first for the wrong reason.

### What an operation promises

| operation | expectation | why it is checkable |
|---|---|---|
| `merge` | `PagesTotalling(sum of inputs)` | ROADMAP invariant |
| `rotate` | `PagesUnchanged(opened)` | a rotation changes an attribute |
| `reorder` | `PagesUnchanged(opened)` | a permutation moves pages, it does not remove them |
| `split` | `PagesExactly(part)`, and the parts sum to the input | ROADMAP invariant |
| `compress` | `PagesUnchanged(opened)` | not yet written; the row is here so it is not invented later |
| redaction | the above **plus its own content verifier** | see below |

### Failure is a typed error, and a new one

Not `Malformed` — the input was fine. Not `InvalidArgument` — the caller asked for something
reasonable. `Error::OutputRejected { expected, produced }`, carrying the two page counts, which
are a count the engine reported and a count this crate computed. **Neither is derived from file
content**, so the message may name them.

The variant is new because the message a person needs is new: *we produced something and would
not give it to you*, which is a different sentence from *your file is broken* and a different
one from *that is not a thing you can ask for*.

## What it can detect

- The output is **not a document** — truncated, empty, or unopenable.
- The output has **the wrong number of pages**. That is #61 exactly: opened 5, emitted 4,
  refused. It is also what a copier defect that dropped or duplicated an object would most
  plausibly produce, and what a partition bug in `split` produces.
- An operation whose engine **silently did nothing** where the page count was supposed to
  change — a merge that returned one input.

## What it cannot detect, stated rather than discovered

This is the half that matters, because a verifier nobody has bounded is a verifier people
over-trust.

- **Wrong content on a right-numbered page.** A page displaying the wrong thing, a `/MediaBox`
  from another document, an inherited attribute resolved wrongly — all invisible to a page
  count. The copier defect in #62, if it ever produced a silent wrong output rather than the
  crash and hang that were measured, could land here.
- **A wrong permutation.** `reorder`'s output has the right number of pages whatever order they
  are in. Only a fixture with distinguishable pages can catch that, which is a test's job and
  not a runtime check's — `pdf_reading.rs` and the conformance corpus do it.
- **Sub-object leaks.** An object shared by an included and an excluded page carries the
  excluded page's data legitimately, and the page count says nothing. That is #54, and
  `object_closure.rs` is the harness for it.
- **Anything present only in transformed form.** A font subset still carrying glyphs for removed
  characters, an image's pixels. `qpdf --qdf` does not decode `/DCTDecode`, `/JPXDecode` or
  `/JBIG2Decode`, and neither does this. **For redaction that transformed form *is* the leak**,
  so M2 must not inherit this step as though it were sufficient — the same warning
  `object_closure.rs` already carries, repeated here because this is the more visible place.
- **Anything at all about whether the operation did what the user meant.** It checks a promise
  the operation made, not the user's intent.

## Consequences

**Cost.** One reopen per operation, on a path that is already engine calls; `split` pays one per
output. To be measured against `apps/web/e2e/measure.spec.ts` rather than assumed, and the
measurement recorded — a verification step whose cost nobody measured is the kind of thing that
gets removed later by someone who assumes it is free.

**It discharges #61 for shipping.** Silent page loss becomes a typed refusal, which is that
issue's own stated bar: *"a refusal would not block; losing the page quietly does."* It does not
fix the underlying disagreement between qpdf's two readings — that is a separate decision about
recovery posture, recorded on #61 and deliberately not taken here.

**It does not discharge #62, and must not be described as doing so.** It converts one class of
hypothetical outcome — a wrong page count — into a refusal. The crash, the hang, and any
content-level corruption are untouched. #62 continues to block M3/M4 on the fault.

**Redaction gets a frame rather than an answer.** R8, R9 and R10 are satisfied *in shape* by
this step: one value, emitted only on success, verified on the bytes emitted. What redaction
adds is its own predicate — did the content actually go — and that predicate is M2's whole
problem. Building the frame now means M2 argues about the predicate rather than about where the
check goes.

**Every operation gains a failure mode it did not have.** A caller that used to get a document
can now get `OutputRejected`. On a correct engine and an undamaged file that never fires, which
is exactly why it needs a test that makes it fire — a fake engine returning a document with the
wrong page count, and the mutation that deletes the check failing that test.

## Alternatives considered

**Verify only in `merge`,** as #65 was originally written. Rejected once #62's scope was
reconciled: the defect it was guarding against is reachable from every operation, and the same
silent-loss class was already demonstrated in `rotate` and `split` by #61. A guard on one
operation would have been aimed at the one place the evidence did *not* single out.

**Verify in the bindings, or in the worker.** Rejected: there are two bindings and one of them
does not exist yet, so the check would be implemented twice and missing once. The core is where
`CLAUDE.md` says there must be exactly one implementation of every rule.

**Compare the output against the input structurally** — object counts, a digest of page
content. Rejected for now as a runtime check: it is what `object_closure.rs` does as a *test*,
where it can afford `qpdf --qdf` and a manifest. Doing it on every operation would mean
decompressing every output in production, and the ceiling it would run under is the one
`max_memory_bytes` cannot enforce (ADR 0007).

**Make the page count agree at the source** — have `open` report the reconstructed count.
Rejected *here* rather than rejected outright: it is the deeper fix for #61, it is a posture
decision about `attempt_recovery`, and it is orthogonal to whether an operation checks its own
output. Recorded on #61.

# 0025. What `compress` does, and what it refuses to do

Date: 2026-09-16

## Status

Accepted. Rests on [spike 0005](../spikes/0005-what-qpdf-alone-compresses.md), which measured
everything this decides.

## Context

`compress` is M1's last operation and the only one where *what it does* was a design question
rather than an implementation one. The other four had an obvious meaning and a hard
implementation; this one had the reverse. "Make the PDF smaller" spans everything from
rewriting the object structure — lossless, cheap, and something qpdf can do — to resampling
every image, which is lossy, needs engines burrow does not ship, and is where the visible
savings are.

Three questions had to be settled before any code, and none could be decided by a function
signature:

1. **How much is qpdf alone worth**, and is that enough to ship?
2. **Does v1 do anything lossy?**
3. **What happens when the output is bigger than the input?**

Spike 0005 answered the first by measurement. This records what follows from it.

## Decision

### 1. `compress` ships, qpdf-only, and there is exactly one lever

Of the five compression levers qpdf's C API exposes, **four are already the writer's own
defaults** (`QPDFWriter_private.hh:296-315`): streams are compressed, unreferenced objects are
dropped, the decode level is `generalized`, and output is not linearized. `merge`, `rotate`,
`reorder` and `split` have been emitting all four since they shipped.

So `compress`'s entire contribution over every other operation on the site is one call:
`qpdf_set_object_stream_mode(qpdf_o_generate)`.

**The other four are deliberately not re-stated as configuration.** A default spelled out as a
setting is an invitation to tune what every other operation already emits, and the tuning would
be invisible to anyone reading those operations.

**One item on the original brief does not exist.** qpdf has **no duplicate-object removal** at
any setting; it garbage-collects unreferenced objects and never merges identical ones. Recorded
because it was asked for and is not a qpdf capability, so nobody re-derives it.

**The C++-only route is rejected on evidence.** `QPDFWriter::setRecompressFlate` and
`Pl_Flate::setCompressionLevel` have no C API (`grep -c recompress qpdf-c.h` is 0), and
ADR 0013 rejected a C++ shim. Measured, that restriction costs **between −0.32% and +1.30%**,
and it is a loss on more fixtures than it is a win. `qpdfjob-c.h` would reach the flags and is
refused: a new audit surface of argv parsing and file I/O, nothing on
`engines/qpdf-trapped-functions.txt`, to buy at most 1.3% on one fixture in seven.

### 2. v1 is lossless, by construction rather than by policy

No image is re-encoded, no font is subsetted, no content stream's meaning changes. The engine
sets one storage lever and touches nothing a page contains.

Three reasons, in order of weight:

- It is the only posture under which `docs/ROADMAP.md`'s *text remains extractable* holds
  **by construction** rather than by a test having to establish it.
- It is the only one under which `assert_nothing_lost` is a claim this operation can make.
- **It costs nothing that was on the table.** The measurement showed the lossy levers are
  precisely the ones qpdf does not have, so "lossless" here is a description of what is
  reachable rather than a restraint.

**The page must say so before the upload, not after**, and that is this decision's obligation
on the UI rather than a nicety — see §5.

### 3. Never worse, and it returns a typed outcome rather than the input bytes

Measured: **3.4%** of qpdf's own 618-file corpus grows strictly under the lever, with a further
**5.3%** left byte-identical. An object stream carries a fixed overhead, so it loses on
documents with little structure to pack. This is not an edge case.

`compress` therefore returns `Outcome`:

```rust
pub enum Outcome {
    Smaller { document: Vec<u8>, original_bytes: u64 },
    NotSmaller { original_bytes: u64, produced_bytes: u64 },
}
```

**The obvious alternative — hand back the caller's own input bytes — was considered and
rejected**, for two reasons pointing the same way and one that decides it:

- **It costs a copy of the input on every call, for nothing.** The engine seam takes its bytes
  by value, and must: the web path copies them into a separate heap and has nothing to borrow.
  Returning them means holding a second copy across the whole operation — including the
  overwhelming majority of calls that *do* get smaller — against a `max_memory_bytes` that only
  *detects* (ADR 0007).
- **It leaves the caller unable to say anything true.** A person handed a file of exactly the
  size they supplied has been told nothing. Both numbers are what lets a page say *"this file is
  already efficiently stored — we produced 1.2 MB against your 1.1 MB, so we kept yours"*.
- **It would make ADR 0022's read-back verify bytes burrow did not author**, with nothing saying
  so. That is the one that makes it wrong rather than merely expensive.

**Every caller already holds the input.** On the web this is explicit: ADR 0015 requires the
page to send a `Blob` rather than a transferred buffer precisely so the caller keeps a usable
handle after its worker is killed. A page offering the original file needs no bytes from here.

**`>=`, not `>`.** A re-encoding that lands on exactly the input's size achieved nothing, and
returning it would be reporting a saving of zero bytes as a result. Equality is 5.3% of that
corpus, so this is a common case rather than a boundary curiosity.

**`Outcome` is deliberately NOT `#[non_exhaustive]`**, which is the opposite of what `Error`
does, and the reasoning does not carry over. An unknown `Error` variant degrades safely — it is
still an error. An unknown `Outcome` variant does not: every arm decides what a person is told
and whether they are offered a file, so a third outcome falling silently into a `_` arm is a
page saying the wrong thing about somebody's document. Adding a variant here is a compile error
at every call site, on purpose.

### 4. ADR 0022 verification: a new `Expected::Compressed`, and it runs only on what is returned

ADR 0022 pre-declared that compress "promises what `rotate` does" — the same page count, the
same rotation vector — and as a *predicate* that is right. `Expected::Compressed` is
nevertheless a new variant rather than a reuse of `Rotated`, because ADR 0022's own step 3 asks
for one exactly when no existing variant states what yours leaves undetectable, and `Rotated`'s
rustdoc states rotate's residue.

**The residue is much larger here, and the variant says so.** A rotation changes one attribute
of a page dictionary and cannot touch a content stream. Compression re-encodes how every object
in the document is stored, so the set of wrong outputs that pass a page-count-and-rotation check
is far larger: a content stream re-encoded, an image resampled, a font dropped — every one
leaves the witness identical.

**So the runtime check carries the least of any operation's, and the tests carry the rest.**
`compress_keeps_everything.rs` uses the object-closure harness's `assert_nothing_lost` entry
point — **not** `assert_closed`, which is mathematically vacuous when every page is kept — and
asserts each page's operator run survives in the decompressed output.

**Verification runs after the size comparison, not before.** ADR 0022 puts verification between
the engine and the **caller**; a document that is not going to the caller has no caller to
protect. Verifying a candidate about to be dropped would pay roughly the cost of the operation
again to check bytes nobody will see. `a_document_that_did_not_shrink_is_never_verified` pins
the ordering.

**Rendered-output comparison was costed and refused.** It would need PDFium, which left the web
payload in spike 0004; returning it costs **1,904,807 brotli** — 460,894 → ~2.39 MB, a 5×
increase — to compare rasterisations that legitimately differ, of pixels v1 never touches.

### 5. What the page is obliged to say

This is part of the decision rather than a note for later, because the measurement is what makes
the tool defensible and the page is where that reaches a person.

- **Show the real result**: original size, new size, percentage. This is the one tool whose
  output a person judges instantly.
- **Say before the upload that a scan or a photo-heavy document will not shrink**, because that
  is measured — 0.15% and 0.12% — and a person arriving at a "compress PDF" page with a 12 MB
  scan is the modal visitor. A tool that says *"this file is already efficiently stored"* is
  worth more than one that claims 2%.
- **State that nothing is re-encoded.** Since v1 is lossless, that is the more reassuring true
  sentence, and it needs no hedge about quality.
- **Never present `NotSmaller` as a failure.** Nothing went wrong; the file was already
  efficiently stored, which is a fact about the file.

## Consequences

**A fuzz target for qpdf's object-stream writer, and it must be run seeded.** `compress` adds no
new *parser* entry point, so the definition of done's fuzz bullet does not require one on its
letter. It has one anyway: `qpdf_o_generate` reaches `QPDFWriter`'s object-stream assembly, its
eligibility walk and its cross-reference-stream emission — several hundred lines of qpdf C++ that
every other target avoids by writing with `qpdf_o_preserve`. Seeded fuzzing has already found two
memory-unsafety defects in this same pinned qpdf (#62), and the writer is the part of it burrow
has run least.

**An unseeded run of that target is evidence about a different function**, and more so than for
any other target here: libFuzzer does not invent a valid PDF, and an invalid one is refused at
`open` and never reaches the writer at all.

**And the seeded run reproduces #62, which is the reason the PR gate is unseeded.** The first
seeded run was 59,438 executions in 61 seconds, clean. The second, against a corpus libFuzzer
had grown further, found a **heap-use-after-free**:

```
qpdf::BaseDictionary::getKeys()
QPDF::Doc::Pages::pushInheritedAttributesToPageInternal(...)
QPDF::Doc::Pages::flattenPagesTree()  ->  Pages::cache()  ->  Objects::parse()
QPDF::processMemoryFile(...)          ->  qpdf_read_memory
burrow_engines::qpdf::Document::open  ->  burrow_ops::compress  (compress/mod.rs, `engine.open`)
```

**It is in `open`, not in the writer**, and the same input crashes `qpdf_check` — one of the
three paths #62 records reproducing through. So it is the known defect arriving by a fifth
route rather than anything this operation introduced, and it confirms the ROADMAP's
characterisation: *"it is reached by opening a document, so every target and every operation
can hit it"*.

Two things follow, and neither is a change of posture:

- **The PR fuzz gate runs `compress` unseeded** (`ci.yml`, with `rm -rf corpus/$target`), which
  is the #62 quarantine already documented there: while that defect is open, no seeded target
  can gate a pull request, because quarantining them one at a time is whack-a-mole against
  something upstream of all of them. So compress's PR gate is a rejection-path test until #62
  closes, and this ADR says so rather than leaving it to be reconstructed.
- **The seeded ten-minute run lives in `fuzz-nightly.yml`**, where a crash is a report with an
  artifact upload rather than a red pull request. `compress` is in that matrix.

The crash artifact is **not committed and cannot be**: `/fuzz/artifacts/` is gitignored, and
#62's own entry records that no reproducer for it is in this repository.

**`max_duration_ms` is weaker on this operation than on any other.** The write is a single engine
call and the most expensive one in the crate; qpdf offers no timeout, cancellation or abort hook
(ADR 0007), so a document that takes ten minutes to re-encode takes ten minutes and is refused
afterwards. On the web the worker watchdog bounds it. **Natively nothing does**, and that is
stated rather than smoothed over.

**`compress` is single-use against one engine `Source`.** `QPDFWriter::generateObjectStreams`
calls `newIndirectNull` once per object stream on every write, so compressing the same source
twice grows the in-memory document. The objects are unreferenced and dropped on write, so no
output is affected, and no caller does it — `burrow_ops::compress` opens, compresses once and
drops. Recorded because the growth is C++-side and the handle-release test cannot see it.

**The web does not have this operation yet.** `qpdf_set_object_stream_mode` is declared natively
and deliberately not exported, argued in `engines/qpdf-not-exported.toml` with its owner named:
the change that gives `WebQpdf` an `impl DocumentCompressor`. Exporting it now would widen the
worker's reachable surface for an operation with no web caller.

**The percentage a person sees will vary by two orders of magnitude between documents**, and
no single figure describes it. Any future claim about "how much burrow compresses" has to be
per category or it is an average across incomparable things.

## Alternatives considered

**Do not ship it.** Genuinely arguable, and spike 0005 was written to make refusing possible: if
qpdf-only compression were not worth a person's time, the honest outcome was to say so. It is
worth shipping on the form case (85.6%) and the median (16.0%), and the argument against —
that the modal visitor gets nothing — is answered by the page saying so first rather than by
withholding the tool from everyone it does help.

**Add hb-subset.** The largest remaining lossless opportunity: on a text report with one
embedded face, the font is 38.3% of the compressed output and qpdf does not touch it. Rejected
for this batch. The payload figure is **unmeasured and not invented** — a number needs HarfBuzz
vendored and built for wasm, which is an engine-acquisition change and *stop and ask*. The 38.3%
is also an upper bound taken from a woff2 stand-in: a real uncompressed `/FontFile2` would flate
under the existing lever, so its residual share would be lower. And subsetting is the change
most likely to break the *text remains extractable* invariant this operation claims.

One correction it surfaces: ADR 0010's duplicate-symbol hazard — two definitions of every `hb_*`
symbol resolved first-wins by link order — was written when PDFium shipped to the web. Since
spike 0004 it does not, so that collision is now a **native and mobile problem only**. Smaller
than the ADR records, and not gone.

**Add jbig2enc.** It targets exactly the category this measurement found unreachable, which
makes it the tempting answer. Rejected, and the disqualifying argument is correctness rather
than licence: JBIG2's symbol-matching mode — the mode that produces the dramatic savings on
scanned text — has a documented history of **silently substituting characters between visually
similar symbols**, so a scanned document comes back readable, plausible, and wrong in its
digits. Against non-negotiable #4, in a tool whose output a person cannot easily check, that is
disqualifying. Generic lossless JBIG2 avoids the substitution and delivers a small fraction of
the saving, which does not justify a new engine, a licence audit and a payload. It is also not
pinned or audited; that is a separate reason and not the one that decides it.

**Offer a lossy mode behind a switch.** Deferred rather than rejected: there is no engine to do
it with, so it is not a choice available today. If it ever is, it must be explicitly opt-in and
say what it is doing before it does it — not a default with a warning afterwards.

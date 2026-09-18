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

**What that guarantees, precisely — added 2026-09-17 after it was measured not to be the
stronger thing this record read as:** *the bytes we produced match what we believed when we
produced them.* **Not** *the output matches the document you gave us.* The difference is the
input side: every promise below is computed from the operation's OWN reading of its input, so a
wrong reading is compared against itself. See the fifth property in *The shape*, the residue at
the top of *What it cannot detect*, and the [#111 amendment](#amendment-2026-09-17-111-the-check-reads-its-promise-from-the-source-it-is-checking).

```
core/burrow-ops/src/verify.rs
```

An operation does not return bytes. It returns bytes *that have been checked*, and the check is
the last thing between the engine and the caller.

### The shape

Implemented in `core/burrow-ops/src/verify.rs`. What follows is what it is, not what it was
proposed as — the two differ, and *What changed between the decision and the implementation*
below says where and why.

```rust
/// What an operation promises about the document it produced.
pub enum Expected {
    /// The same pages came out as went in, displaying as `rotations` says. rotate.
    Rotated { rotations: Vec<i64> },
    /// The same pages, permuted by the order that was asked for. reorder.
    Reordered { rotations: Vec<i64> },
    /// Every input contributed the pages it had. merge.
    Merged { contributions: Vec<u64> },
}

/// Reopen `bytes` through a FRESH engine and refuse them if they are not what was promised.
pub fn output<E: OutputReader>(
    engine: &E,
    bytes: &[u8],
    expected: &Expected,
    options: &OpenOptions<'_>,
) -> Result<()>;
```

Four properties are load-bearing and each exists for a reason already paid for elsewhere:

- **It runs on the emitted bytes**, not on a source, a copy, or the engine's in-memory state.
  That is R10 exactly, and it is why the signature takes `&[u8]` rather than a handle.
- **It runs before the caller has the bytes.** The operation returns `Err` and drops the
  buffer; nothing downstream ever sees an unverified document. That is R9's property at the
  core layer, where it holds for the native paths too rather than only in a worker.
- **It reopens through the engine seam**, never a second parser. A verifier with its own parser
  is a second implementation that can agree with the first for the wrong reason.
- **It reads through a fresh engine instance**, never the one that produced the bytes. A
  document handle that has just been edited is not a neutral witness to its own output, and a
  corrupted instance can agree with itself — which is precisely the #62 residue this step is
  aimed at. `OutputReader::fresh` is what each engine provides, and its rustdoc states what
  "fresh" does and does not mean per platform: natively a new `QPDF`; on the web a new
  `qpdf_data` **in the same WebAssembly module**, so it is a fresh parse and a fresh page tree
  over a shared heap, and a heap-wide corruption is not what it catches.

- **The PROMISE is read from the operation's own source, and that is the one property here
  that is not independent.** Every `Expected` value is computed from the input as the operation
  read it — `rotate` and `reorder` and `compress` from the source handle, `split` from a sweep
  over its own source, `merge` from the running totals of the assembly it is building. So the
  four properties above make the **output** side a neutral witness, and nothing makes the input
  side one. A reading that was wrong before the operation started is the reading the check
  compares against.

  This was not stated when the record landed, and the record read as the stronger guarantee for
  every operation until #111 measured it. `Expected::Merged`'s own rustdoc contains the argument
  — it declines to take a rotation vector from the assembly because *"that is the engine state
  that produced the output, so it is the instance agreeing with itself"* — and then takes its
  page counts from exactly there. The reasoning was present and applied to one field.

**The freshness is free and the measurement says so** — see *Consequences*.

### What an operation promises, concretely

A page count cannot see a permutation, so the witness is **per page**: each page's *effective
rotation* — the value it displays at, followed up the page tree. It is what both engine seams
already offer without a new bridge method and without decompressing anything.

It is a **weak identity and a real one**. Two pages sharing a rotation are indistinguishable to
it, so on a document where every page displays the same way the vector degrades to a page
count; on a document where they differ it catches a permutation that is not the one asked for
and a turn applied to a page nobody named.

| operation | asserts | where the promise comes from | what remains undetectable |
|---|---|---|---|
| `rotate` | page count, and **the full rotation vector** — every page's effective rotation, the named pages turned | the input's own rotations, read before the edit through the handle about to be edited; free | a rotation written to a shared ancestor rather than the page, on a document where every page inherits the same value; wrong *content* on a right-rotated page |
| `reorder` | page count, and **the rotation vector permuted by the requested order** | the same read, put through the permutation | a permutation of pages that all display the same way — there it degrades to the page count, which still catches #61's lost page and says nothing about order |
| `merge` | page count, and **every input contributed at least one page** | the assembly's running page total before and after each append; free | **the largest residue of the three**: two inputs' pages interleaved wrongly, or one input's pages substituted for another's, as long as the counts land the same |

`merge` gets a count per input rather than a rotation vector deliberately. The expected
rotations would have to come from the inputs independently, and the only way to get them is to
parse every input **a second time** — doubling the cost of the operation the check is attached
to. Reading them from the *assembly* instead would be free and worthless: that is the engine
state that produced the output, so it is the instance agreeing with itself, which is the thing
`fresh` exists to stop.

An input that contributed **zero** pages is refused, and that refusal cannot distinguish an
input whose pages were dropped from an input that genuinely had none. That is the direction to
be wrong in: a zero-page page tree is malformed by the specification anyway, and telling the
two apart costs the second parse this variant exists to avoid.

### The operations that do not have it yet

`split` is held and `compress` does not exist. **Both inherit this step on arrival, and neither
may ship without it.** That is a gate, not an intention:

- `.claude/skills/add-operation` carries it as a numbered step, so an operation cannot reach
  review without it.
- `CLAUDE.md`'s definition of done carries it as a checkbox.

What each will promise is written down now rather than invented later:

| operation | asserts |
|---|---|
| `split` | **each part's slice of the source's rotation vector**, read once before any part is extracted. Amended 2026-09-14 from "each part's page count, and the parts summing to the input's": the slice subsumes both — if each part's count equals its slice's length and the slices partition the source vector, the parts sum to the input by construction — and it adds *which* pages, which a count cannot see. One part being right still says nothing about the partition, so every part is verified |
| `compress` | page count and the full rotation vector, exactly as `rotate` does; compression may not move or reattribute a page |
| redaction (M2) | the above **plus its own content predicate** — see *Consequences* |

### Failure is a typed error, and a new one

Not `Malformed` — the input was fine. Not `InvalidArgument` — the caller asked for something
reasonable. `Error::OutputRejected(String)`, carrying a message built from the two page counts, or
the two rotation vectors, or the index of the input that contributed nothing. Every number in
it is a count the engine reported or a count this crate computed. **None is derived from file
content**, so the message may name them.

It is a `String` rather than the structured `{ expected, produced }` first proposed because
three operations promise three different shapes, and a pair of page counts cannot carry a
rotation vector or an input index. The strings are constants plus numbers, which is what
`ALLOWED_MESSAGES` requires.

The variant is new because the message a person needs is new: *we produced something and would
not give it to you*, which is a different sentence from *your file is broken* and a different
one from *that is not a thing you can ask for*.

## What it can detect

- The output is **not a document** — truncated, empty, or unopenable.
- The output has **a different number of pages than the operation read going in**. That is #61
  exactly: opened 5, emitted 4, refused. It is also what a copier defect that dropped or
  duplicated an object would most plausibly produce, and what a partition bug in `split`
  produces. **"Than the operation read going in" is load-bearing and was missing here**: opened
  5 and emitted 5 passes, on a document that has six — see the first residue below.
- An operation whose engine **silently did nothing** where the page count was supposed to
  change — a merge that returned one input.
- **A turn applied to a page nobody named**, and **a permutation that is not the one asked
  for** — on any document where the pages involved do not all display the same way. Both come
  from the rotation vector, which is why the witness is per page rather than a total.

## What it cannot detect, stated rather than discovered

This is the half that matters, because a verifier nobody has bounded is a verifier people
over-trust.

- **A page count that was already wrong when the operation read it.** THE LARGEST RESIDUE, and
  the one this record did not state for the whole of its life until #111. The promise is the
  operation's own reading; the check re-reads the output through a fresh engine and compares the
  two. If the input reading dropped a page, the output has the same missing page, the two agree,
  and the operation reports success. Measured: `five-pages-or-six.pdf` declares six pages, a
  textual walk and PDFium both read six, qpdf reads five, and a split emits 2 + 3 = five pages
  and passes this check. **Every operation carrying a page count inherits it**, which is all five.
  `a_split_keeps_every_page_the_document_declares` states the property and is `#[ignore]`d.
- **Wrong content on a right-numbered page.** A page displaying the wrong thing, a `/MediaBox`
  from another document, an inherited attribute resolved wrongly — all invisible to a page count
  and to a rotation vector alike. The copier defect in #62, if it ever produced a silent wrong output rather than the
  crash and hang that were measured, could land here.
- **A wrong permutation of pages that all display the same way.** The rotation vector catches a
  permutation only where it puts differently-rotated pages in different places; on a document
  whose pages all display at 0 it degrades to the page count. That residue is a test's job —
  `pdf_reading.rs`'s `/MediaBox` widths and the conformance corpus do it with distinguishable
  fixtures, which a runtime check cannot assume it has. `reorder`'s
  `a_reorder_of_pages_that_all_display_the_same_way_is_only_a_page_count` asserts the limit
  rather than leaving it as prose.
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

**Cost — measured, not assumed.** `cargo run -p burrow-ops --features native-engines
--release --example measure-verification`, medians of 11 runs, one page rotated:

| fixture | operation | the promise sweep | verification | the same through the **writing** instance | `fresh()` itself |
|---|--:|--:|--:|--:|--:|
| `pages-137.pdf` (137 pages, 16 kB) | 1.03 ms | 184 µs (18%) | 719 µs (**70%**) | 716 µs (69%) | **32 ns (0.0%)** |
| `pages-10.pdf` (10 pages) | 106 µs | 14 µs (13%) | 70 µs (**66%**) | 68 µs (64%) | **32 ns (0.0%)** |
| `--generated 10000` (10,000 pages, flat tree) | 33.6 ms | 5.5 ms (16%) | 25.0 ms (74%) | 25.3 ms (75%) | **32 ns (0.0%)** |
| `--deep 10000 60` (10,000 pages, 60-deep tree) | 175 ms | **147 ms (84%)** | 165 ms (94%) | 165 ms (94%) | **32 ns (0.0%)** |

Three things follow, and the fourth row is why the last two are separate rows.

1. **Verification costs roughly two-thirds to nine-tenths of the operation.** It is a second
   full parse of the output plus a second sweep over its pages, which is what R10 asks for and
   cannot be had for less.
2. **The freshness is free.** `fresh()` is 32 ns against a read-back three to six orders of
   magnitude larger, on every shape measured. Verifying through the instance that wrote the
   bytes would save 0.0% and give up the one property that makes the witness a witness. The
   instruction was to measure the cost before deciding otherwise; there is nothing to trade.
3. **The per-page sweep is the part that scales, and it scales with page count × page-tree
   depth — both attacker-chosen.** On the committed fixtures it is 13–18% and looks like a
   rounding error; on a 10,000-page document with a 60-deep tree it is 84% of the operation.
   The first three rows are why it nearly shipped with **no deadline checkpoint in it**, which
   security review found and measured: 144 ms of unchecked sweep against 12 ms of
   edit-and-write. Both sweeps and the read-back now checkpoint — per page inside `rotations`,
   and around each engine call in `verify::output`, against the **operation's own** deadline
   rather than a fresh one.

Measured natively. The web path's numbers belong in `apps/web/e2e/measure.spec.ts` and are
**not** recorded here yet; the qpdf inside the wasm module is the same qpdf, so the shape
should hold, but that sentence is a prediction and is labelled as one.

**Memory, which the first version of this section did not mention at all.** Verification holds
a second parsed document and, on both platforms, a copy of the output bytes — `open_output`
hands the engine its own buffer. Security review measured RSS 197 MB → 387 MB across the
read-back of a 200 MB output. `rotate` and `reorder` therefore `drop(source)` before
verifying, so the input document is not resident alongside the output's; `merge`'s assembly is
already consumed by `finish`. The residue is real and is the usual one: `max_memory_bytes`
**detects and does not bound** (ADR 0007), and the only true bound anywhere is the web
module's fixed 2 GiB.

**One budget covers producing the output and checking it**, and that took three attempts to
be true. The sweeps first had no checkpoint at all; then they had one against a `Deadline` they
started themselves — and `Deadline::start` resets the origin *and* the budget, so each sweep
handed the operation another full `max_duration_ms`. Security review measured 56 ms returned
against a 50 ms ceiling. The sweeps now take the caller's deadline as an argument, which is why
`PageRotator::rotations` and its two siblings have a parameter no other engine method has, and
`one_budget_covers_the_promise_sweep_and_the_read_back` fails if any of them starts its own
again.

A `LimitExceeded` from inside the read-back now propagates unchanged rather than being wrapped
in `OutputRejected`. Running out of time is the operation's outcome, not a verdict on the
document — the same distinction `merge` draws when it refuses to blame the input its clock
happened to stop on. Wrapping it told a person their file was broken when the work simply did
not fit in the budget.

**If this ever needs to be faster, the measurement is here rather than the change.** The
per-page sweep is linear in page-tree depth — 10,000 pages, one page rotated, promise sweep
only:

| tree depth | 1 | 2 | 4 | 8 | 16 | 60 |
|---|--:|--:|--:|--:|--:|--:|
| sweep | 5.5 ms | 8.1 ms | 13.0 ms | 23.2 ms | 41.6 ms | 149 ms |
| share of the operation | 16% | 23% | 31% | 45% | 60% | 84% |

Roughly 2.5 ms per level per 10,000 pages, because `effective_rotation` re-walks `/Parent` to
the root for every page and pages under one node share their whole ancestor chain.

**The obvious fix is a memo** keyed on ancestor *identity* — object number and generation, the
only thing that may be compared (`core/CLAUDE.md`) — which collapses the sweep to O(pages +
nodes) and would flatten that row to about 5.5 ms at every depth. **It was looked at and not
taken**, and the reason is the shape of the benefit: on the depth a real producer writes (2–4)
it saves 7–18% of one operation, and it is large only on the adversarial shapes the per-page
deadline checkpoint already bounds. Against that, it restructures the same page-tree walk in
**both** engines — the newest and least exercised code here — and a memo that returns a stale
value is a silently wrong witness, which is the failure class this whole ADR exists to catch.
Identity comparison is also precisely where this repository has been bitten before (ADR 0013's
handle-identity amendment).

So: recorded, not done. `split`, `compress` and M2's redaction inherit this step, and if a
large-document profile ever justifies the memo, the numbers to beat are in the table above and
`--deep <pages> <depth>` in `measure-verification` reproduces them.

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

## What changed between the decision and the implementation

Recorded rather than smoothed over, because the ADR was written a few hours before the code and
two of its claims did not survive contact.

| as decided | as implemented | why |
|---|---|---|
| `Expected` carries a page count (`PagesUnchanged`, `PagesTotalling`, `PagesExactly`) | it carries a rotation vector or a per-input contribution list | a page count cannot see a permutation, and `reorder` is one of the three operations. The ADR said so itself under *What it cannot detect* and then proposed a shape that could not do better |
| `Error::OutputRejected { expected, produced }` | `Error::OutputRejected(String)` | three promises of three shapes; a pair of counts cannot carry a vector or an input index |
| `pub fn output<E: DocumentEngine>` | `pub fn output<E: OutputReader>`, a new trait | `DocumentEngine` has no way to read bytes back and no way to make a fresh instance. The fresh-instance requirement is a *capability*, so it is a trait, and the method list is the audit surface |
| "reopens through the engine seam" | that, **and through a fresh instance** | a corrupted instance agreeing with itself is #62's residue, and it is the one thing the original three properties left open |
| the read-back runs under the operation's `OpenOptions` | it runs under those with **`max_input_bytes` raised to fit the output** | that ceiling governs what a *caller* may hand burrow. Applied unchanged it denied a supported operation: a merge of inputs totalling exactly the ceiling succeeded and then refused its own output, because qpdf's output is normally larger than the sum of its inputs. Security review measured 32,995 bytes rejected under a 32,408-byte ceiling |
| the witness is each page's *effective rotation*, normalised | it is each page's `/Rotate` **as written** | normalising made an out-of-spec `/Rotate 45` on a page the request never named fail the whole operation — before ADR 0022 such a document reordered fine. A witness only has to be **stable** between the read before and the read after, and 45 is stable. The engine seam now has two reads: `effective_rotation` judges (a *turn* of 45 is undefined, so naming that page is still refused) and `rotations` records. Found by security review; `a_page_displaying_out_of_spec_does_not_fail_the_reordering` covers it against real qpdf |

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

---

## Amendment, 2026-09-17 (#111): the check reads its promise from the source it is checking

**This record's verification compares an operation's output against what the operation promised,
and the promised value comes from the same reading the operation used.** When that reading is
already wrong, the output is verified against a wrong number, the two agree, and the promise is
satisfied by the error.

### Measured

`tests/conformance/fixtures/five-pages-or-six.pdf`, 1,838 bytes. Six pages, `/Count 6`, six
`/Kids`; page three's `/Resources` points its `/XObject` above INT_MAX.

| who is asked | pages |
|---|--:|
| a textual walk of the page tree | **6** |
| PDFium (`page_count`, native) | **6** |
| qpdf (`page_count`, web bundle) | **5** |

Split after page 2: parts of 2 and 3 pages, **five in total**, reported as success. The read-back
compared five against five. **A page is gone and nothing says so.**

### Every operation that promises a page count inherits this

`split` (the parts partition the source), `rotate` and `reorder` and `compress` (the output has
the input's pages), `merge` (the total is the sum). Each takes the count from the engine it is
already using, and hands that number to the check.

**[#61](https://github.com/TensorGreed/burrow/issues/61)'s discharge is narrower than it reads.**
This verification was built to close "a damaged-but-openable document silently loses a page", and
it does close the half where the WRITER loses a page the reader saw. It cannot see the half where
the READER lost one before the writer heard of it — which is the same sentence with the two
halves swapped.

### What would close it

**A second, independent reading of the property**, from a different engine or at minimum a
different code path than the operation used. The value of a read-back is entirely its
independence, and on this axis it currently has none.

| | catches this fixture | cost |
|---|:--:|---|
| a structural page-tree walk outside the operating engine (`/Count` against reachable `/Kids`) | yes | small — not a full parse |
| ask the other engine | yes | close to a third full open, and on the web PDFium is a second bundle nothing else fetches ([#107](https://github.com/TensorGreed/burrow/issues/107)) |
| accept and document that the count is the engine's opinion | no | none, and it leaves #61's discharge overstated |

**The cost is not nothing and is stated here rather than discovered:** the read-back this record
already requires costs about **1.7×**, because every output is reopened and re-read. An
independent reading is on top of that.

### This is settled before M2, not carried into it

**Redaction depends on exactly this property.** "The pages you asked for are the pages that were
changed, and none of them is missing" is what redaction lives or dies by, and it will be verified
by whatever this record says verification means. Taking a check that reads its promise from the
operation's own source into the one feature where a mistake leaks secrets is the wrong order.

`core/burrow-ops/tests/split.rs::a_split_keeps_every_page_the_document_declares` states the
property and is `#[ignore]`d with its reason, the way #61's own reproduction was: asserting five
would turn a known hole into recorded correct behaviour. The conformance case records what both
engines actually say, so the day either changes is loud.

### The fix was attempted, and what it ran into (2026-09-18)

**A second call to the same engine is not a second reading**, and this is the clearest
demonstration of it we have.

The design was: read the page tree's `/Count` — the document's own declaration — through the
object seam, compare it with the page list the engine resolved, and refuse on disagreement. It
was built and wired into `open_document`, the funnel `rotate`, `reorder`, `compress` and
`split` all pass through, so one check covered every operation. It ran on every operation and
**refused nothing**: on `five-pages-or-six.pdf` it read a declared **5** against a resolved
**5**, on a document whose file says six.

**qpdf repairs the page tree when it resolves it, `/Count` included** — and the only entry into
the object graph from that seam is `page(0)`, so *indexing a page is what triggers the repair*.
Reading the declaration first does not help, because reading it goes through the same door.
There is no ordering that gets in front of it; the two readings converge by construction.

So independence has to be **structural** — a different path to the data — rather than a second
call to the same engine, which returns the same repaired model however early it is made.

**The declaration is genuinely there.** `qpdf --show-object=2` on that fixture, which never
touches the page list, prints `<< /Count 6 /Kids [...six...] /Type /Pages >>`. It is reachable
only through the trailer, and that is where this stops.

#### Three routes, each blocked by a decision this project has deferred

| route | blocked by | the decision it needs |
|---|---|---|
| a structural walk of the page tree, outside the engine | in an ordinary 11 MB PDF 1.6 file, `/Type/Pages`, `/Type/Page` and `/Count` appear **zero** times in the raw bytes — the tree is inside 67 object streams | an inflater, [#24](https://github.com/TensorGreed/burrow/issues/24) |
| ask the other engine | a tab holding qpdf and PDFium at once loses the tab on WebKit | [#107](https://github.com/TensorGreed/burrow/issues/107) |
| read the trailer through qpdf | `qpdf_get_root` and `qpdf_get_trailer` are both untrapped, and both **resolve an object** | weakening [ADR 0013](0013-qpdf-error-trapping.md)'s bar |

**The third is refused rather than deferred.** `engines/qpdf-untrapped-accepted.toml` admits a
function only when it "never touches the PDF's bytes, never resolves an object"; returning an
object handle is exactly what these two do. That bar exists because a **356-byte PDF killed the
process** through an untrapped function. **A page-count check is not worth trading a crash
defence for**, and `ffi.rs` already records these two functions under *"Two functions burrow
wanted and may NOT have, recorded so nobody looks again"* — which was accurate.

So #111 stays open, and it is not "add a check": **closing it requires taking one of the three
decisions above**, none of which belongs to a verification change.

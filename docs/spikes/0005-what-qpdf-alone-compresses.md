# Spike 0005 — what qpdf alone compresses, and whether that is a tool

**Date:** 2026-09-15
**Status:** decided — `compress` ships, qpdf-only and lossless. Neither hb-subset nor jbig2enc
is added in this batch. The page must lead with what the tool cannot do.

## Why now

`compress` is M1's last operation and the one where *what it does* is a design question rather
than an implementation one. The `add-operation` checklist's §2a says to settle a missing engine
capability **before** planning the operation, and spike 0004 deferred the engine question to
here in as many words: *"Per-page loading stays on the shelf unless `compress` proves it needs
PDFium for image recompression."*

So this is the measurement that decides it, and it is a measurement rather than a survey
because the honest answer was not predictable from the documentation: four of the five levers
turned out to be defaults burrow has been getting for free since `merge` shipped.

## Method

`core/burrow-engines/examples/measure-compress.rs`, over fixtures from
`tools/make-compress-fixtures.py`. Three arms, because one number cannot separate the causes:

| arm | what it is | the question |
|---|---|---|
| **C** | `qpdf_write` calling **no write-parameter setter at all** | how much of any "saving" is the rewrite every burrow operation **already** does |
| **A** | C plus every compression lever the C API exposes | what a qpdf-only `compress` can ship |
| **B** | the pinned qpdf CLI, with and without `--recompress-flate --compression-level=9` | the size of the gap ADR 0013's no-C++-shim rule costs |

Arm C is what makes the table readable, and it was the right thing to build: without it the
headline figure on the text report is 72.9%, and **69.2 of those points are available by
rotating a page and rotating it back.**

Arm B is measured **both ways**, with `--deterministic-id` on both sides, so Finding 3 compares
like with like and is reproducible from the committed tool rather than taken by hand.

### The controls, and the one that had to be rebuilt

Two run on every invocation, and the harness exits non-zero without them:

- **inertness** — calling all five setters with qpdf's **own default values** must produce
  output byte-identical to calling none of them. If it does not, then "setting a lever to its
  default" is not the same as "leaving it alone", and the A-over-C column is measuring
  something other than the levers.
- **non-vacuity** — at least one fixture must save a byte, so a build in which the levers had
  quietly stopped being applied fails instead of reporting zeroes as a finding.

Both PASS: 7 of 7 fixtures compared, and 7 of 7 examined.

**The inertness control did not always mean that, and it is worth recording what it was.** In
the first version, arm C was itself "the setters called with default values", so the control
compared `rewrite(BASELINE)` against `rewrite(BASELINE)` — the same call with the same
argument. It proved `qpdf_write` is deterministic. It could not have detected the one
regression it exists to catch.

Code review demonstrated that rather than arguing it: with `BASELINE.compress_streams` mutated
`true → false` (the mutation asserted to have applied before the run, per `CLAUDE.md`), every
number in Finding 2 became garbage — the text report's A-over-C fell from 12.0% to 0.9%, the
already-optimised fixture reported −242% — and **both controls printed PASS and the harness
exited 0**, while the prose still asserted the column was "the levers and nothing else". That
is exactly *"a check that silently examines nothing is worse than no check — it reads as
coverage"*.

Arm C is now a run that calls no setters, which is what the table above claims it is, and the
control is capable of failing.

## Finding 1 — four of the five levers are already qpdf's defaults

Read out of `QPDFWriter_private.hh:296-315`, not inferred:

| lever | qpdf's default | is it a lever? |
|---|---|---|
| `compress_streams_` | `true` | no — already on |
| `preserve_unreferenced_` | `false` | no — garbage is already collected |
| `decode_level_` | `qpdf_dl_generalized` | no — already the useful level |
| `linearize_` | `false` | no — hint streams are already dropped |
| `object_streams_` | `qpdf_o_preserve` | **yes — the only one** |

**`compress`'s entire contribution over every other burrow operation is one function call:**
`qpdf_set_object_stream_mode(qpdf_o_generate)`. Everything else on the brief's list —
stream recompression, structural cleanup, metadata handling — `merge`, `rotate`, `reorder` and
`split` have been doing silently since they shipped.

**One item on the brief does not exist at any setting.** qpdf has **no duplicate-object
removal**. It garbage-collects unreferenced objects; it does not merge identical ones. Every
"duplicate" in `QPDFWriter.cc` and `QPDF_pages.cc` is about duplicate *page* entries in a page
tree. The capability was on the brief and is not a qpdf capability.

## Finding 2 — the per-category table, and it is not close

`A over C` is the honest column: what `compress` adds over what a person already gets from any
other tool on the site.

| fixture | input | C | C% | A | **A over C** | what it is |
|---|--:|--:|--:|--:|--:|---|
| `form.pdf` | 66,430 | 67,483 | **−1.6%** | 9,729 | **85.6%** | 640 small objects, one per form field |
| `text-report.pdf` | 178,718 | 55,055 | 69.2% | 48,461 | **12.0%** | 40 pages of text, one embedded face |
| `text-report-no-font.pdf` | 160,194 | 36,494 | 77.2% | 29,900 | **18.1%** | the same report, font removed |
| `linearized.pdf` | 55,675 | 55,055 | 1.1% | 48,467 | **12.0%** | the same report, linearized |
| `scanned.pdf` | 1,337,118 | 1,337,459 | **−0.0%** | 1,335,485 | **0.15%** | 12 pages, each one full-page DCT scan |
| `photo-heavy.pdf` | 1,033,436 | 1,033,543 | **−0.0%** | 1,032,298 | **0.12%** | 6 pages of photographs |
| `already-optimised.pdf` | 48,461 | 48,461 | 0.0% | 48,461 | **0.00%** | arm A's own output, fed back |

Per-lever attribution — bytes **added** when a lever is turned off, from full arm A, which is
the direction that catches a lever that has silently stopped being applied. **All five levers,
and all seven fixtures**, because an enumeration that quietly drops rows is a shape this
repository has been caught by before:

| fixture | object streams | compress streams | decode < gen | keep unreferenced | linearize **ON** |
|---|--:|--:|--:|--:|--:|
| `form.pdf` | **+57,754** | +49,820 | 0 | −11 | +7,665 |
| `text-report.pdf` | +6,594 | +117,386 | 0 | +1,338 | +6,052 |
| `text-report-no-font.pdf` | +6,594 | +117,423 | 0 | +1,337 | +6,040 |
| `linearized.pdf` | +6,588 | +117,419 | 0 | +279 | +6,049 |
| `scanned.pdf` | +1,974 | +1,148 | 0 | +7 | +2,255 |
| `photo-heavy.pdf` | +1,245 | +967 | 0 | +6 | +1,804 |
| `already-optimised.pdf` | +0 | +117,386 | 0 | +1,014 | +6,052 |

**The last column runs the other way and is labelled so.** Linearization is already *off* in
arm A, so there is no "turn it off" to measure; that column is bytes added when it is turned
**on**. Under the header the other four share it would read as a saving, which is the reverse
of what it means.

The `compress streams` column is large and is **not a saving `compress` delivers** — it is a
default, already banked by arm C. It is in the table because a reader who saw only the object
stream column would conclude stream compression does not matter, and it does; it is simply
already happening.

**Three of the seven C% values are negative** — `form.pdf`, `scanned.pdf` and
`photo-heavy.pdf`. The ordinary rewrite makes those files *slightly larger*. That is not a
defect; it is qpdf writing a well-formed cross-reference where the input had a hand-made one.

**The object-streams column equals the A-over-C delta on every row**, which is a cross-check
rather than a coincidence: it is the only lever, so the two must agree, and a divergence would
mean something else was moving bytes.

## Finding 3 — recompression at level 9 buys between −0.32% and +1.30%

The C++-only `QPDFWriter::setRecompressFlate` and `Pl_Flate::setCompressionLevel` are the one
capability ADR 0013's no-C++-shim rule actually costs us. `grep -c recompress qpdf-c.h` is
**0**.

Both columns come from the same CLI invocation with `--deterministic-id` on **both** sides, so
the only difference between them is the recompression. Positive means recompression made the
file smaller:

| fixture | arm A via CLI | + `--recompress-flate --compression-level=9` | worth |
|---|--:|--:|--:|
| `form.pdf` | 9,729 | 9,603 | **+1.30%** |
| `photo-heavy.pdf` | 1,032,298 | 1,032,298 | 0.00% |
| `scanned.pdf` | 1,335,485 | 1,335,485 | 0.00% |
| `text-report.pdf` | 48,461 | 48,551 | **−0.19%** |
| `already-optimised.pdf` | 48,461 | 48,551 | **−0.19%** |
| `linearized.pdf` | 48,467 | 48,558 | −0.19% |
| `text-report-no-font.pdf` | 29,900 | 29,995 | **−0.32%** |

**Best case +1.30%, worst case −0.32%, and it is a loss on more fixtures than it is a win.**

An earlier draft of this section quoted the form's win as "0.13%" — a lost decimal place, and
it happened to point in the direction that supports the recommendation below. Caught by code
review. The rejection holds on a 1.30% best case, but it has to hold on the real number.

**The `arm A via CLI` column equals the C-API arm A byte for byte on all seven fixtures.** That
is a free cross-check that the harness configures qpdf exactly as the CLI does, and it is now
produced by the harness itself rather than taken by hand.

So the `qpdfjob-c.h` route — `qpdfjob_run_from_argv`, which would reach these flags — is
rejected **on the evidence rather than on the argument**. It was going to be rejected anyway
(argv parsing and file I/O as a new audit surface, nothing on
`engines/qpdf-trapped-functions.txt`, a shape unlike every existing engine seam), and it is
better to reject it for costing a real audit to buy at most 1.3% on one fixture out of seven.

## Finding 4 — the distribution, over files nobody here chose

618 of the 716 PDFs in qpdf's own test suite (`engines/vendor/src/qpdf-12.4.1/`, Apache-2.0,
checksum-pinned by `engines/fetch.sh`, never committed).

**The 98 that were not examined are attributable rather than assumed.** The harness counts
three causes separately: 0 could not be read from disk, **98 failed arm C** — qpdf's own
defaults could not write them, which is what a deliberately damaged corpus looks like — and
**0 failed arm A where arm C succeeded**. That last counter is the one that matters: a file
object-stream generation alone cannot write would be a finding about the lever, and there are
none. A single `unreadable` counter, which is what this had until code review, could not have
told those apart and the report asserted the cause anyway.

Arm A's saving **over arm C**:

| | |
|---|--:|
| min | −48.3% |
| p25 | 6.9% |
| **median** | **16.0%** |
| p75 | 37.2% |
| max | 84.5% |
| files arm A made **strictly larger** | **21 of 618 (3.4%)** |
| files arm A left **exactly unchanged** | 33 of 618 (5.3%) |
| files saving under 1% | 60 of 618, of which 21 are negative |

By input size — because "files got larger" and "files got larger and they were all tiny" are
different findings, and only the second tells a person whether it will happen to them:

| input size | median saving | n | strictly larger | unchanged |
|---|--:|--:|--:|--:|
| under 2 kB | 7.3% | 268 | 15 | 20 |
| 2 kB – 16 kB | 33.8% | 221 | 5 | 7 |
| 16 kB – 128 kB | 32.2% | 113 | 1 | 3 |
| over 128 kB | 2.7% | 16 | 0 | 3 |

20 of the 21 that grew are under 16 kB and the largest is **49,639 bytes**, which matches the
prior — an object stream has a fixed overhead, so it loses on small files. "Refuse to make it
worse" is therefore a measured **3.4%**, concentrated in small files but not confined to them.

**An earlier draft of this section said 8.9%, and named a 2.4 MB file as the largest
regression.** The counter used `>=`, so files arm A left at *exactly* arm C's size were
counted as having grown — two thirds of the population. The 2.4 MB file
(`large-inline-image-ii-all.pdf`) is written identically by both arms; it never grew at all.
Found independently by code review and by re-deriving the figure through the CLI. `>=` remains
the correct predicate for the never-worse **guard**; it was never the correct one for this
**label**, and the difference is three quoted numbers.

The "over 128 kB" row's 2.7% is on n = 16 and is consistent with the image fixtures: qpdf's
large test files are mostly image payloads it cannot touch. Small n, stated rather than
smoothed.

## What these fixtures can and cannot support

`CLAUDE.md`: *a test harness that generates its own inputs is measuring what it can generate.*

- **The structural rows are honest.** Object counts, size distributions, table-versus-stream
  cross-references, whether content streams arrive compressed — these are things naive
  producers really do, reproduced faithfully, and they are exactly what every lever acts on.
- **The image rows are our encoder's output**, at a quality we chose, over imagery we drew.
  That happens not to matter, and the reason is worth stating rather than relying on: **qpdf
  does not touch DCT data at any setting**, so those two rows measure how little of such a file
  is reachable *at all*, which is a structural fact the fixtures can support.
- **The embedded font is a woff2 face standing in for a font program**, and the error is in a
  known direction — see below.
- **No claim is made about the real-world distribution of these five categories.** Five files
  are five files; the distribution arm is qpdf's suite, which at least nobody here chose, and
  which is itself small and qpdf-shaped.

## Finding 5 — the font is 38% of the compressed output and qpdf does not touch it

Measured by subtraction against a committed fixture rather than estimated:
`text-report-no-font.pdf` is the same generator with `embed_font=False`, so this row can be
re-derived rather than believed.

| | input | arm A |
|---|--:|--:|
| `text-report.pdf` | 178,718 | 48,461 |
| `text-report-no-font.pdf` | 160,194 | 29,900 |
| **the font's share of the output** | 18,524 | **18,561 bytes — 38.3%** |

The face is 18,520 bytes on disk and 18,561 in the output: it survives arm A **intact**, plus
41 bytes of object overhead.

Note the second column against the first: removing an incompressible 18.5 kB blob raises the
report's A-over-C saving from 12.0% to **18.1%**, because the blob was diluting a percentage
of a whole file. That is the same denominator effect a real document with several embedded
faces would show, in the opposite direction.

**The error is in a known direction and it flatters hb-subset.** A woff2 face is already
entropy-coded, so arm A's `compress_streams` gains nothing on it. A real `/FontFile2` TrueType
arrives *uncompressed*, so arm A would flate it and its residual share of a compressed output
would be materially lower than 38%. **This number is an upper bound on the font's share, not an
estimate of it**, and it should not be quoted as the size of the hb-subset opportunity.

## Finding 6 — a suppression bypass, found by running the harness rather than by looking for it

Running the distribution arm put qpdf prose on stderr with burrow's native suppression
installed:

```
Pages tree includes non-dictionary object; ignoring
```

**The first version of this section said "all three of burrow's native suppression layers" and
only two were running.** The harness declared `qpdflogger_set_info/warn/error` with three of
their four parameters and passed `0` — which is `qpdf_log_dest_default`, not
`qpdf_log_dest_discard` (3) — so the third layer was installing a logger behaviourally
identical to the default one. Found by security review. The defect was inherited from
`measure-merge.rs`, which had it too; both are fixed, and `core/burrow-engines/src/codes/qpdf.rs`
has always had the right value, so **production was never affected**.

**The conclusion survives, and is now actually evidenced.** With the discarding logger
correctly installed at `dest = 3`, the re-run still puts **7 lines** on stderr.
`BaseHandle::warn(std::string const&)` (`QPDFObjectHandle.cc:2346-2353`) writes to
`QPDFLogger::defaultLogger()` — the **process-global** logger — when the handle has no owning
`QPDF`, and `qpdf_set_logger` cannot reach that on any setting.

burrow already knows about *one* `defaultLogger` bypass (`qpdf/mod.rs:95-115`, the
`qpdf_cleanup` unhandled-error warning) and closes it by draining the error slot. **Draining
does not close this one**; it is a distinct path, and it fires during a successful write rather
than at cleanup.

The string that leaked is a **constant** — no object numbers, no offsets, no file content — so
this is not a leak as it stands. It is filed as
[#86](https://github.com/TensorGreed/burrow/issues/86) because other call sites of the same
function embed file-derived data, including one on the write path every operation uses, and
whether any is reachable with an unowned handle is **not established here**.

It also narrows a claim this repository has recorded: `docs/ROADMAP.md` item 7 says the two
suppression layers are *"each sufficient on their own"*. That was measured against one canary
fixture and is not true in general — on the web the Emscripten `printErr` stub is the only
layer catching this. Nothing about `compress` depends on it, and it is recorded here because it
was found here.

## The engine recommendation

### hb-subset — not in this batch, and the next thing worth measuring

- **Licence: settled.** ADR 0010 admitted `MIT-Modern-Variant` for HarfBuzz 14.3.1 and committed
  the notice text upstream omits.
- **Payload: unmeasured, and this document does not invent one.** A figure needs HarfBuzz
  vendored and built for wasm, which is an engine-acquisition change and *stop and ask* under
  `CLAUDE.md`. Spike 0004 refused to invent a number in this exact position and the refusal
  holds.
- **A correction to ADR 0010, surfaced by this spike.** Its duplicate-symbol hazard — two
  definitions of every `hb_*` symbol, resolved first-wins by link order — was written when
  PDFium shipped to the web. **Since spike 0004 it does not**, so on the web there is no second
  HarfBuzz and the collision is now a **native and mobile problem only**. ADR 0010 already
  notes the asymmetry would not reproduce in a browser; what has changed is that the browser is
  now the only place burrow ships. The hazard is smaller than the ADR records and is *not*
  gone — M3/M4 still link PDFium.
- **It would put `compress`'s own invariant at risk.** `docs/ROADMAP.md` states *text remains
  extractable* for this operation. Subsetting a font is the change most likely to break text
  extraction in some viewers, and v1's whole claim is that it does not alter content.

**Recommendation: no, and revisit with a measurement on real reports carrying real
`/FontFile2` programs.** It is the largest remaining lossless opportunity and it is the one to
look at next — but 38% is an upper bound taken from a stand-in, and a decision to vendor an
engine should not rest on that.

### jbig2enc — no, and the reason is correctness, not licence

jbig2enc targets exactly the category this measurement found qpdf cannot help with: scans, at
0.15%. That makes it the tempting answer and it should still be refused.

- **Not pinned, not licence-audited, not mentioned anywhere in `docs/`.** It and its Leptonica
  dependency are believed permissive; that is a finding to confirm under ADR 0008's admission
  test, not an assumption, and it needs an ADR before its size matters.
- **The disqualifying argument is not the licence.** JBIG2's symbol-matching mode — the mode
  that produces the dramatic savings on scanned text — has a documented history of **silently
  substituting characters between visually similar symbols**, so a scanned document comes back
  readable, plausible, and wrong in its digits. Against non-negotiable #4, *correctness before
  features*, and most of all in a tool whose output a person cannot easily check, that is
  disqualifying.
- Generic (lossless) JBIG2 mode avoids the substitution and delivers a small fraction of the
  saving, which does not justify a new engine, a new licence audit and a new payload.

**Recommendation: no — not now, and not later in symbol mode.** If the scanned case is ever
worth addressing, the honest routes are lossless re-encoding of the image data or an explicitly
lossy, explicitly opt-in path that says what it is doing; neither is this milestone's work.

## The decision

**`compress` ships, qpdf-only and entirely lossless.** The case rests on two of the seven
fixtures and a median, and it is worth stating that plainly rather than as an average:

- A **form** loses 85.6% of its bytes. That is a transformative result and it is not rare — it
  is the shape of every document assembled from many small objects, which is most documents
  that are not images.
- A **text report** loses 12.0% — 18.1% once its incompressible embedded face is set aside —
  and the qtest median is **16.0%**.
- A **scan** loses **0.15%**, and a **photograph-heavy document 0.12%**.

**The awkward part, stated rather than buried: the documents people most want to compress are
the ones this tool cannot help.** A person arriving at a "compress PDF" page with a 12 MB scan
is the modal visitor, and they will get 12 MB back. That is a real argument against shipping,
and it is overridden — but only because the page can say so *before* the upload rather than
after, which is the difference between a disappointing tool and a dishonest one. A tool that
reports "this file is already efficient, and here is why" is worth more than one that claims
2%.

**Lossless, decided.** v1 re-encodes no image, subsets no font and changes no content stream's
meaning — only how objects are stored. Three reasons, in order: it is the only posture under
which the ROADMAP's *text remains extractable* holds by construction; it is the only one under
which `assert_nothing_lost` is a testable claim rather than an assertion; and the measurement
above shows the lossy levers are the ones we do not have, so choosing lossless costs nothing
that was on the table.

**Never worse, and it is not a nicety.** 3.4% of qpdf's own corpus grows strictly under arm A
and a further 5.3% is left byte-identical. If the produced output is not smaller than the
input, the input is returned unchanged and the result says so.

## What this changes for the operation

Recorded here so the next phase argues about the right things. These belong in ADR 0025 with
the operation, not in this spike.

- The engine seam needs **one** new write setter, `qpdf_set_object_stream_mode`, plus its
  `engines/qpdf-untrapped-accepted.toml` argument and its wasm export. The other four levers
  are defaults and must not be written as configuration — spelling out a default as a setting
  invites someone to "tune" it later.
- `verify::Expected` gets a `Compressed` variant rather than reusing `Rotated`. ADR 0022
  pre-declared that compress "promises what rotate does" and the promise is right; what is
  wrong is the *residue*, which `Rotated`'s rustdoc does not state. A page count and a rotation
  vector cannot see a content stream re-encoded, an image resampled or a font dropped.
- **Verification cannot compare rendered output, and the cost of making it able to is
  1,904,807 brotli** — returning PDFium to the payload, 460,894 → ~2.39 MB, a 5× increase, to
  compare rasterisations that legitimately differ, of pixels v1 never touches. The residue is
  carried by tests: `assert_nothing_lost` plus the byte-identical content-stream assertion
  `rotate_keeps_everything.rs` already makes. ADR 0022 rejected decompressing every output as a
  *runtime* check; this keeps it a test, consistently.
- The already-optimised case is a **fixture**, not a hypothetical: arm A of arm A saves exactly
  zero bytes, so the never-worse path has an input that reaches it every run.

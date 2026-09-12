# 0016. The differential conformance harness, and what it found

Date: 2026-09-11

## Status

Accepted — amends [ADR 0007](0007-limit-enforcement-per-platform.md)'s description of what
`max_memory_bytes` guarantees on the web, and implements [ADR 0015](0015-web-worker-lifecycle.md)
§10's `platform_expectations` design.

Written as a separate record rather than an edit, because
[ADR 0001](0001-record-architecture-decisions.md) makes accepted ADRs append-only.

## Context

`burrow` has **two implementations of one trait**: `Pdfium`/`Qpdf` linked natively, and
`WebPdfium`/`WebQpdf` over the JS bridge. `core/burrow-engines/src/web/mod.rs` has said since
PR 4a-i why that matters:

> ROADMAP item 12 requires the native and web implementations to produce **identical typed
> outcomes**, and at M2 a divergence between them is a redaction bug rather than a test failure.

Nothing enforced it. The web path had Playwright tests proving it *worked*; nothing proved it
*agreed*. And the corpus it would have agreed on was much smaller than the risk:
`expectations.json` held eight cases, all well-formed or simply damaged, while every adversarial
file this project has found — the xref bomb, the three pre-scan bypasses security review found
in PR 3, the file whose object number aborted the process, the two files qpdf reads and PDFium
refuses — lived inside one native test each and had never been near a browser.

Two things blocked building the harness, and both had to be fixed first.

**Three different checks produced an identical typed error.** `check_open_memory` (from the
input's length), `prescan::check` (from what the file declares) and `check_measured_memory`
(from what the engine actually used) all returned
`LimitExceeded { limit: "max_memory_bytes" }`. A harness comparing typed outcomes could not
tell a pre-scan rejection from a post-open one — so "the same error kind reached by a completely
different route" would have read as agreement, and the difference between refusing a bomb and
surviving one would have been invisible.

**The schema modelled one engine, one limit set, one error kind.** `expect` was a single
outcome, implicitly PDFium's, under `Limits::default()`. It could not express "qpdf reads this
and PDFium refuses it", and it could not express `xref-bomb.pdf` at all — which is why that
fixture had been on disk since PR 3 and deliberately absent from the expectations.

## Decision

### 1. `Stage`: the route is part of the typed outcome

`burrow_types::Stage` is a small `#[non_exhaustive]` enum — `InputSize`, `SizeEstimate`,
`Prescan`, `PageCount`, `Pixels`, `Measured`, `Deadline` — and `Error::LimitExceeded` carries
one. `Limits::check` takes it as its first argument, so the compiler asks every call site where
it is.

An enum rather than a second `&'static str` because `check("prescan", "max_memory_bytes", …)`
is two adjacent string literals — the transposition hazard `WebLimits` was introduced in PR
4a-i to remove — in the function every limit in the codebase goes through.

`limit` and `stage` answer different questions and both are needed. `limit` is the field the
**caller set**, so they know which number to change. `stage` is the **check that fired**.

### 2. Schema 2

Four additions, each forced by something the old schema could not say:

| Addition | Forced by |
|---|---|
| per-operation `expect` | the engines legitimately disagree; ADR 0013 records two files qpdf reads and PDFium refuses |
| per-case `limits` | a bomb is `Ok` under a generous ceiling and `LimitExceeded` under a tight one, and both are worth asserting |
| `stage` in a failure | §1 |
| `platform_expectations` | ADR 0015 §10 — a by-design difference must be recorded, not skipped |
| `known_gap` | a case that documents a defect we have not fixed, with the issue that tracks it |

`requested` is compared exactly at every deterministic stage and **omitted at `Measured`**. That
number is the process resident set on native and the engine module's `HEAPU8.byteLength` on the
web — a difference ADR 0007 records deliberately — so recording it would guarantee a false
divergence on every run. `the_schema_records_a_route_for_every_limit_failure` enforces both
halves: a limit failure must name its stage, and a measured one must not name a number.

### 3. Two comparison mechanisms, because neither is sufficient

- **Both sides assert against `expectations.json`.** This catches both paths being wrong in the
  same way, which comparing them to each other never can.
- **The two records are diffed directly.** This catches a divergence in a field nobody thought
  to put in the schema, which transitivity cannot.

The native side writes `native-outcomes.json`; the `web` CI job downloads it and diffs it per
browser. Each record carries the digest of the `expectations.json` it was written against, so a
stale record cannot compare cleanly against a corpus it never ran.

### 4. Two lists, kept apart

- **`platform_expectations`** (in `expectations.json`) records a difference we **designed**,
  with a mandatory reason. It is asserted exactly.
- **`tests/conformance/divergences.toml`** records a difference we have **accepted but not
  designed**. Every entry needs a reason *and* an issue link; one missing either is a harness
  error, not a waiver. **It is empty.**

They age differently, which is why conflating them would be a mistake: one is a decision a
reader should learn from, the other is a job nobody has done. Both fail when the difference
**stops happening**, because a record that has silently gone stale is what the next reader will
believe.

### 5. The harness cannot pass while checking nothing

The comparison is a **pure function** — `compare(expectations, native, web, allowlist, digest)`
— with no browser, filesystem or network in it, so every defence is unit-tested against planted
inputs rather than only exercised end to end. A defence that is only exercised end to end is one
nobody has seen fail, and three of the ones below had no test until review said so.

| Defence | What it catches |
|---|---|
| coverage, both directions | a fixture nothing runs; a case naming nothing |
| comparison count | zero comparisons, or fewer than the corpus describes |
| both paths ran | a missing row read as agreement — including both sides missing the same row |
| freshness | a record written against a different corpus |
| schema | a file this reader does not understand, guessed at |
| planted divergence | one on the native side and one on the web side, because they take different branches |
| distinct platforms | the same record passed twice — which agrees with itself perfectly |
| duplicate rows | a wrong row followed by a right one for the same key, which `Map` silently last-wins |

A mutation sweep over all ten finds no survivors.

Failure output carries the case name, the fixture digest and the two **typed** outcomes. Never
fixture bytes, never an engine message, never the canary — `secret_leak.rs`'s discipline applies
to CI logs, and a conformance failure is exactly when someone pastes output into an issue. A
unit test asserts it.

### 6. Determinism

Native injects a `ManualClock` that never advances. The web cannot, so every case runs with
`max_duration_ms` at 600 s: nothing in this corpus is meant to hit a duration limit, and a slow
runner must not invent one. `retries: 0` stands.

**Each case gets a fresh worker**, and that is a correctness requirement rather than hygiene.
WASM linear memory never shrinks, so the web's measured check reads how far the heap *grew* — and
a heap already holding 200 MiB from a previous case can satisfy the next without growing at all.
Running the corpus in one worker made `objstm-bomb-tight-ceiling` return `Ok` because the case
before it had paid for the heap, and made its sibling operation return `LimitExceeded` because a
recycle in between had reset one engine. The outcome depended on the order of the corpus, which
is not an outcome. A spawn costs about 80 ms (ADR 0015 §6).

### 7. The cross-browser matrix runs on every PR

Measured before deciding, as asked. The full three-browser differential matrix — 21 cases × 2
operations × 3 browsers, each on a fresh worker — takes **22 seconds**; the whole e2e suite
takes 107 s. No split is needed and none is taken.

## What the harness found

### Finding 1 — a pre-scan gap, and a measured check that measures the wrong thing (#24)

`tools/make-objstm-bomb.py` builds a PDF whose catalogue, page tree and page live inside a
**compressed object stream** padded with spaces. PDFium must inflate it to find the catalogue.

Measured on a 1.4 MB variant against the **default** 1 GiB ceiling:

| | |
|---|---|
| pre-scan | **passes**, estimating 384 bytes |
| PDFium peak RSS | **2,437 MB** — roughly 1,750× the input |
| outcome | **`Ok(1 page)`** |

Two distinct defects, and the second is the more general:

- The pre-scan reads declarations, and this file declares nothing large: `/Size 6`, and a
  `/Length` that is the honest *compressed* length. ADR 0013 §6 already states the class —
  *"it sees declarations, not truth"* — and points at the measured check as the layer that
  catches it.
- **The measured check compares a counter before and after**, so a peak that occurs *during*
  and is released is invisible. It saw a 1,028 MB delta against a 1,024 MiB ceiling and passed
  the file.

qpdf is unaffected at this size: its `flate_max_memory` is 256 MiB (ADR 0013 §5), so it refuses
the file past that. PDFium has no equivalent ceiling configured. That asymmetry is the fixable
part, and it is why the committed fixture inflates to 200 MiB — large enough to trip a 96 MiB
ceiling with margin, deliberately under qpdf's.

The fixture is in the corpus with `known_gap` pointing at #24. The recorded outcome is what
burrow **does**, not what it should do, so fixing the pre-scan breaks the recorded expectation
and forces the fixture and the issue to be closed together.

### Finding 2 — `max_memory_bytes` is not a hard ceiling on the web (#25)

ADR 0007 says:

> **Web**: the real mechanism. The WASM instance is created with a `maximum` memory size derived
> from `max_memory_bytes` … This is a genuine hard ceiling, and it is why the web is the
> best-protected platform here — the inverse of the usual expectation.

**That mechanism was never built.** Read out of the compiled artifacts' own memory sections:

| module | initial | maximum |
|---|---|---|
| `pdfium.wasm` | 17 MiB | **2048 MiB** |
| `qpdf.wasm` | 16 MiB | **2048 MiB** |

Both are build-time constants. Neither is derived from `max_memory_bytes`, and burrow hands
Emscripten an already-compiled module through `instantiateWasm` (ADR 0014 §2), so there is no
point at which a per-operation maximum *could* be applied without rebuilding the engines with
imported memory.

So on the web, during an operation, **nothing bounds a spike**: the pre-scan runs before, the
measured check after, the watchdog covers duration, and the recycler acts on the next operation.
The only backstop is the module's fixed 2 GiB ceiling, at which `_malloc` returns 0 and burrow
reports `Io` — fatal, worker discarded. On iOS a tab dies well before that, and takes the page
with it.

`Limits`' rustdoc and this ADR now say so. The claim had been load-bearing in two later ADRs'
reasoning, which is why correcting it is part of this PR rather than deferred with the issue.

**This changes what ADR 0015 §7's mobile question is about.** It is not "choose a good
`max_memory_bytes` for phones", because that value does not bound anything during an operation.
It is "make something bound it".

### Finding 3 — the web's measured check is stronger than native's

The first real user of `platform_expectations`, and the inverse of Finding 2's direction.

`objstm-bomb-tight-ceiling` / `structure_check`: **native returns `Ok`, the web returns
`LimitExceeded`**. qpdf inflates the object stream on both paths, but native samples the resident
set before and after and the memory is freed in between, so the delta is ~0. WASM memory cannot
shrink, so the web reading includes the peak.

Recorded rather than skipped, with the mechanism as its reason. It is not a defect on either
side: it is ADR 0007's per-platform counter choice, visible for the first time because something
finally compared the two.

### Finding 4 — the size estimate runs on one engine and not the other (#26)

Surfaced by adding a case for the one `Stage` the corpus did not otherwise reach.
`estimate::check_open_memory` — the cheap length-based pre-check — is called from both PDFium
paths and **neither** qpdf path. With `max_memory_bytes` below the estimate, the same file is
refused through `page_count` and opens through `structure_check`.

It is consistent across native and web, so the differential harness is green: this is an engine
difference, not a divergence, and it is recorded as one. Arguably it is also correct — both
constants behind the estimate are PDFium measurements, and applying a PDFium cost model to qpdf
would predict the wrong number. What was wrong was that nothing said so. `estimate.rs` now
documents the scope and the corpus pins the behaviour, so a change to it breaks a recorded
expectation.

### Finding 5 — the engine baselines are build-time facts, not measurements

`MIN_CONVERGING_MEMORY_BYTES` (64 MiB) rested on a single measurement of "~18 MiB per engine",
taken once in PR 4a-ii and written into a doc comment. The modules' memory sections show where
that number actually comes from: 17 MiB and 16 MiB of **declared initial memory**.

So it is a property of the pinned artifacts, and a pinned-engine bump that changed it should be
noticed. `e2e/measure.spec.ts` now reads the floor from Rust — through
`burrow_wasm::min_converging_memory_bytes`, rather than a JavaScript literal free to drift — and
asserts the measured baselines sit below the recycling threshold at that floor, and not
absurdly below it either.

## Consequences

Adding a limit check now requires naming its stage. That is the intended cost: a check whose
route is not worth naming is a check whose route nobody will be able to reason about later.

The corpus grew from 8 cases to 21, and from one engine to two, so it is now 42 comparisons per
platform. Every adversarial file this project has found runs through the browser path on every
PR, which it never did before.

`expectations.json` has three readers — the Rust test, the fixture generator, and the TypeScript
spec — and schema 2's shape is what they agree on. A change to it is a change to all three, and
the version check means a partial change fails loudly rather than being guessed at. The comparator
duplicates one rule, `expectedFor`, because the two halves are in different languages; it is four
lines, tested on both sides, and the alternative was the web side not honouring recorded
differences at all.

Two issues are open against behaviour this PR measured rather than introduced (#24, #25), and one
corpus entry is green while documenting a defect. That is deliberate and it is bounded: the gap
is printed on every run, and fixing it breaks the record.

## Alternatives considered

## What the reviews changed

Both reviewers ran against the local commit, before any push — the rule this PR adds to
`CLAUDE.md`. They converged independently on the same hole, and it is worth recording because it
is the exact failure §5 claims the design rules out:

**`compare(expectations, webRecord, webRecord, [])` passed clean**, with a full comparison count
and no failures — a differential harness that had compared one implementation with itself.
`platform` was in the type, present in both records, and read only to render a message. The
freshness check had the same shape: it compared each record against `web.expectations_sha256`,
which made the `web` iteration a no-op and asserted only that the two agreed with *each other*,
so two records carrying the same wrong digest also passed. Both are now closed, `compare` takes
the corpus digest as an argument, and both have tests.

A mutation sweep over the comparator now finds no survivors across ten defences. Three of them
had no test at all before review: the native-side half of "both paths ran", the redundant-waiver
branch, and duplicate rows in a record.

## Alternatives considered

**Compare the full typed tuple instead of adding `stage`.** `requested` already distinguishes
the size estimate (a function of length) from the pre-scan (a function of declarations), both
deterministic and identical on both paths. Rejected: it cannot distinguish either from
`Measured`, whose number is incomparable across platforms by construction — so exactly the check
that fires *after* the damage is done would have been the one the harness could not name. It
also infers the route from a number rather than reading it, which is the sort of cleverness that
survives until someone changes a constant.

**Transitive comparison only — both sides assert against the golden file.** No artifact
plumbing, and each side fails in its own job. Rejected: the comparison would be only as complete
as the schema, and a field the schema does not record could diverge silently. That is the
failure this PR exists to close.

**Direct diff only, no golden file.** Rejected: both paths agreeing on the wrong answer would
pass, and the corpus would stop having a recorded contract — which is what `expectations.json`
is for.

**Leave the pre-scan gap out of the corpus until it is fixed.** Rejected: the corpus would
silently lack the one case that motivated the issue, and nothing would notice if the behaviour
got worse.

**Record the *correct* outcome for the gap and let CI stay red.** Honest and impossible to
ignore. Rejected because it means the fix has to be in this PR, and the fix is a pre-scan change
with its own design question (how to bound a declared `/Length` under a filter) that deserves
its own review rather than being rushed into a testing PR.

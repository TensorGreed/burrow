# 0026. How rendering loads, without returning to the old payload

Date: 2026-09-16

## Status

Accepted — amends [spike 0004](../spikes/0004-the-first-load-budget-before-compress.md)'s
decision that *"PDFium does not ship to the web"*.

The saving that spike measured is not given back. What changes is *who pays it*, and the
whole of this record is about making that a fact about the build rather than a claim about
the code.

## Context

[ADR 0020](0020-rotate-ships-without-thumbnails.md) deferred page thumbnails to
[#57](https://github.com/TensorGreed/burrow/issues/57), and measured that the build cost was
near zero: `engines/vendor/wasm/lib/pdfium.wasm` is a prebuilt with 429 `FPDF_*` exports and
no `EXPORTED_FUNCTIONS` allowlist, so every symbol a renderer needs is already there.

Then spike 0004 took PDFium out of the web payload entirely. It was **79.7% of everything a
person downloaded before their first operation could run**, reachable from one function —
`page_count` — which qpdf answers through `StructureEngine::check`. Measured: 2,390,698 →
462,146 brotli, **−80.7%**, and the first operation on Chrome's Slow 3G profile went from
47.8 s to 9.2 s.

Rendering needs PDFium back. The naive form of that change undoes the spike: stage
`pdfium.wasm` again, put its glue back in the worker bundle, and every visitor to
`/merge-pdf` downloads 1.9 MB they will never use.

**The requirement is stated as an assertion rather than an intention:** *a visitor who lands
on `/merge-pdf` and merges two files downloads no PDFium at all* — and that has to be
enforced by the build graph, not by a check somebody has to keep passing.

## Decision

### 1. A second worker bundle, and the base one never learns PDFium exists

| bundle | engine | Rust module | fetched |
|---|---|---|---|
| `burrow-worker.js` | qpdf | `burrow_wasm_bg.wasm` | on the first file, by every tool page |
| `burrow-render-worker.js` | PDFium | `burrow_wasm_render_bg.wasm` | on the first page picture |

`tools/stage-web-engines.mjs` builds each from its own source list and generates **each
bundle's own slice of the manifest into it**. The base bundle contains no PDFium glue, no
PDFium bridge globals, and no PDFium URL — and a worker fetches only what its own manifest
names, because `prelude.js` loops over that list and Emscripten's `locateFile` never runs
(ADR 0014 §1a).

### 2. The Rust binding is built twice, under mutually exclusive features

`bindings/burrow-wasm` gains `documents` (default) and `render`, and `src/lib.rs` refuses a
**`wasm32`** build that enables both or neither.

This is the half that makes the claim structural rather than careful. A `no-modules`
wasm-bindgen import resolves from the worker's **global scope**, so an import declared against
a module the worker does not load fails at *instantiation* — in the browser and nowhere else.
Spike 0004 recorded that as the hazard that forced the imports out with the artifact; here it
is the mechanism. With `render` off there is no `__burrow_pdfium_*` import to leave dangling,
because the code that declares them is not compiled.

Measured on the **staged release** modules — the ones that ship, and the ones
`tools/check-pdfium-is-render-only.sh` reads:

| | `__burrow_pdfium_*` | `__burrow_qpdf_*` |
|---|--:|--:|
| `burrow_wasm_bg.wasm` | **0** | 46 |
| `burrow_wasm_render_bg.wasm` | **7** | **0** |

Seven is the whole of `PdfiumBridge`, which is the cross-check worth having: the render module
imports exactly the bridge surface `core/burrow-engines/src/web/bridge.rs` declares and nothing
else. An earlier draft of this paragraph quoted 319 and 43 — counts taken from a **debug**
build, where a symbol name appears more times than it does in the artifact. Neither number was
wrong about the property; both were about the wrong binary.

**The BOTH-features refusal is scoped to `wasm32`, and that is not a loophole.** `cargo test
--workspace --all-features` is a gate CI runs, and it turns both features on. On a native
target neither bridge is reachable — the tests drive `web/fake.rs` — so an unscoped refusal
would mean deleting a CI gate to protect a property native builds cannot violate. It fires on
exactly the builds that produce a shipped artifact.

**The NEITHER-features refusal is not scoped, and the asymmetry is the point.** Nothing turns
both off deliberately, so there is no gate to protect — and `--no-default-features` without a
replacement failed with `E0425: cannot find function engine_heap_bytes`, which tells a reader
nothing. Code review raised this: the argument above justifies scoping one of the two
refusals, and an earlier draft of this section applied it to both.

### 3. Two payloads, both budgeted, and the measurement is derived

`tools/first-load.mjs` took the engine payload as *"everything under `engines/`"*. That was
right while there was one bundle and is now wrong in the direction that would have hidden the
point of this change: PDFium is staged, and it is not part of what a person who merges two
files downloads. Counting it in the base payload would have reported a 5× regression for a
change whose entire purpose is that nobody pays it.

So the payload is derived from the manifest each bundle **carries**, and
`size-budget.json` gains a `render` section:

| | brotli | budget |
|---|--:|--:|
| base — every tool page | 468,077 | 509,812 |
| with rendering | 2,436,971 | 2,678,375 |

**The property the old line had is kept rather than traded away.** `engineClosures` fails if
any staged artifact belongs to no bundle, so a third bundle nobody budgeted, or a stray file
staged beside them, is a failure rather than free. An artifact still cannot hide; it now has
to be in somebody's payload.

**This is the real gate.** The absence claim becomes arithmetic: PDFium cannot enter the base
payload without the base budget failing by roughly 400%.

### 4. One lifecycle, two instances

`createWorkerHost` is unchanged in substance and is used twice. There is no second state
machine, no second watchdog, and no second circuit-breaker implementation — a copy would start
without the fixes this one accumulated, each of which was a measured failure: the memoised
**promise** rather than a boolean (spike 0001's HIGH finding), crash-counting rather than
respawn-counting (ADR 0015 §3), the ack-based watchdog clock (§2), and the guarded
`drainReply` that turns a stale `pkg/` into an immediate typed failure rather than a watchdog
kill thirty seconds later.

The same is true one layer down: `worker-protocol.js` — the fail-closed CSP guard, the
memoised init promise, the ack, the reply flattening and every refusal shape — is
**byte-identical in both bundles**. What each bundle supplies is `init`, `KNOWN_OPS` and
`runOperation`.

Where the two hosts differ, and why each is configuration:

| | documents | render | mechanism |
|---|---|---|---|
| bundle | `BUNDLE_WORKER` | `BUNDLE_RENDER_WORKER` | a generated descriptor |
| modules counted at start-up | 2 | 2 | `expectedEngineModules`, generated |
| `initTimeoutMs` | 240 s | 240 s | **the same.** ADR 0018 derived it from `pdfium.wasm` at 5.3 MB; it is generous for qpdf and exactly right for the render bundle |
| watchdog budget | 12 s | measured when rendering lands | already per-call; ADR 0015 §12's number is derived from the slowest *succeeding* operation, and a render is a workload nobody has timed |
| circuit breaker | independent | independent | separate instances — **deliberate:** a document that kills the renderer must not take merging offline, and vice versa |
| recycling | `reply.recycle` from Rust | the same | unchanged (ADR 0015 §5) |

**A page can now hold two workers at once, and nothing bounds the sum of their heaps.** The
recycling threshold is half `max_memory_bytes` per heap, and it was chosen when a worker held
two engines — so it happens to buy the same property across two single-engine workers. That is
a coincidence and is recorded as one: `core/burrow-engines/src/web/recycle.rs` says so at the
constant, and it is a second input to ADR 0015 §7's deferred decision about mobile defaults,
alongside the fact that PDFium's 1.9 GiB reading of `xref-bomb.pdf` is reachable on the web
again.

**`EXPECTED_ENGINE_MODULES` stopped being a constant, and the reason is the coincidence.**
Both bundles fetch two modules today. A constant that still works by coincidence is one nobody
checks, and the day one bundle gained a third module the other bundle's host would have started
ignoring its second `starting` message — a start-up bound quietly halved, on the slow
connections it exists for, with every test green.

### 5. The reply's heap field is renamed, because it would otherwise lie

`Reply::qpdf_heap_bytes` becomes `engine_heap_bytes` (`reply.engineHeapBytes` in the worker
protocol). There was a `pdfium_heap_bytes` beside it until spike 0004, leaving one field named
after the one engine that remained — and the render artifact's reply would have reported a
PDFium heap under a field spelling `qpdf`. Every reader of it, including the recycler and
`measure.spec.ts`'s table, would have been quietly wrong about which engine it was looking at.

### 6. The credits page over-declares, and says so

`engines/licenses.toml` gets the `pdfium-wasm` artifact back, and `generate-credits.mjs`
scopes the web surface to **both** artifacts — so the page credits FreeType, HarfBuzz, ICU,
lcms, OpenJPEG and nine more to every reader, including one who only merged two files.

**That is the right direction to be wrong in, and the page says which components are
conditional rather than leaving a reader to work it out.** A page scoped only to qpdf would
owe FreeType's FTL §2 and HarfBuzz's MIT-Modern-Variant the moment somebody looked at a page
picture, and a build cannot know in advance whether they will. Crediting more than you
downloaded breaks no licence; crediting less would. `src/credits.test.ts` asserts both halves,
and the two tests that asserted the *absence* of FreeType and HarfBuzz were inverted rather
than deleted — each carried its own inversion condition verbatim (*"if PDFium is back in the
payload this test is the wrong way round"*), which is what made the inversion a decision
rather than a discovery.

## What this does not close

**The CSP is one policy per document, and it names PDFium on every page.** `connect-src` lists
the exact content-hashed engine URLs (ADR 0014 §1), and a document gets one policy — so the
page that merges two files is served a policy that *permits* fetching PDFium. What stops it is
that nothing on that page asks: the base bundle's manifest has no such URL, and
`e2e/zero-requests.spec.ts` reads the server's own accept log rather than the policy.

This is the one place the split is not structural, and it is recorded here rather than left
for a reader to notice, **because the policy is exactly where somebody would look for the
guarantee.**

**A bundle-per-origin CSP would close it and is not available**: one build serves every route.
A per-route policy would mean per-route `_headers` entries that the `<meta>` tag — which is the
primary mechanism, because it is enforced in every environment including `pnpm preview` and
Playwright (ADR 0014 §5) — cannot express differently per page without generating a different
layout per route.

**`page_count` is answered by two engines, and no conformance case compares them.** The base
artifact answers it with qpdf and the render one with PDFium. The corpus asks one question per
operation per platform, and a second web answer would mean deciding what a disagreement
between two *web* engines means — a real question, with five divergences already measured in
spike 0004, and not this change's question.

**The render bundle does not render yet.** It answers `page_count`, which is the first half of
the capability rather than a stand-in for it: rendering a page means opening the document
first. It is there because a loading boundary with nothing behind it is one no browser test can
drive, and an untested `blob:` worker whose fail-closed guard decides whether file bytes may be
touched is exactly the thing that goes wrong quietly. `PageRenderer`, the bitmap reply and the
pixel ceiling are #57's second piece, and ADR 0020 records why that ceiling must be designed
rather than discovered under pressure from a UI.

## Consequences

- **Nothing a person downloads today changes.** The render bundle is staged and fetched by
  nothing on any shipped route, so this deploys with the base payload moving by **+2,036
  brotli bytes (0.44%)**, from 466,041 — the worker bundle's share of the protocol split's
  comments. The budget is unchanged at 509,812, and the headroom it records goes from 9.4% to
  8.9%: growth is supposed to eat recorded headroom, and raising the budget back would be
  refunding the thing being measured (`size-budget.json`'s own rule).

  **These figures were wrong twice before they were right, both times for the same reason.**
  The first pair predated the review fixes. The second pair was measured against a
  `pkg-render/` built *before* `check_input_budget` moved into `lib.rs`, so the render module
  was 612 brotli bytes light — and the recording passed `pnpm test` because the test and the
  recording were made from the same stale artifact. `ci-local.py`'s `wasm-pack` job is what
  caught it, which is exactly what its own comment says it exists for: *"a local sweep
  measured the size budget against a binding from whenever anyone last ran wasm-pack by
  hand."* The numbers above are from a run where every job passed.
- **`tools/check-no-pdfium-on-the-web.sh` is renamed `check-pdfium-is-render-only.sh` and
  retargeted at the base set.** Its three needles and two positive controls are unchanged; what
  is new is a **partition control** — PDFium must be *present* in the render bundle — because a
  build that staged no PDFium at all satisfies every absence rule and would serve a thumbnail
  strip that cannot render. Its self-test runs 36 adversarial cases, 15 of them probe gates.
- **`src/worker/` is checked as two TypeScript projects**, one per bundle, with the engine
  declarations split to match. A declaration that outlives the module it describes type-checks
  and then fails in the browser and nowhere else — measured, when five `_FPDF_*` declarations
  stood in `globals.d.ts` for one build after spike 0004 stopped loading PDFium.
- **`ci-local.py`'s `wasm-pack` parity key gains the output directory.** Keyed on the crate
  alone the two builds collapse to one gate, and a local sweep running only the base build
  would have reported full coverage while staging whatever the render directory held — the
  same miss that put the `wasm-pack` pattern there in the first place, one argument along.
- **A workflow that named the renamed gate was not caught by anything**, and now is.
  `deploy.yml` still invoked `check-no-pdfium-on-the-web.sh` after `ci.yml` was updated — found
  by code review, and the symptom would have been the deploy job dying at its last gate before
  upload, after merge to `main`, with the gate itself silently gone. `tools/ci-local.py` reads
  `ci.yml` only and should keep doing so: `deploy.yml` holds a credential and has steps that
  can have no local counterpart. So the control is a much smaller question asked of *every*
  workflow — `tools/check-workflow-scripts.py`: does the file named here exist? That has one
  answer and needs no model of what a gate is.
- **`bindings/burrow-wasm/pkg-render/` is gitignored in the ROOT file**, not in the one
  wasm-pack writes inside it. An ignore rule that governs generated output and lives beside the
  output is absent from the working tree the moment you check out a branch that lacks it, which
  is the shape that put 24 files on `main` in PR #37.

## Alternatives considered

**One worker, PDFium's module fetched lazily.** The simplest lifecycle by far: one bundle, one
digest, one host, no second budget shape. Rejected because the bundle is one concatenated file
under one integrity digest (ADR 0014 §1a), so `pdfium.js`'s glue would ship to everybody —
23,640 brotli bytes, 5% of the base payload, for a capability most visitors never use. It also
makes the claim a size argument rather than an absence one, and an absence is checkable.

**Two bundles sharing one Rust module.** Saves the second `wasm-pack` build and ~55 KB brotli of
duplicated Rust in the render payload. Rejected: the base bundle would then concatenate
wasm-bindgen glue naming `__burrow_pdfium_*`, and the base module would carry dead PDFium
bridge code — so *"no PDFium at all"* weakens to *"no PDFium engine"*, and the absence rule
weakens with it from a symbol scan to an artifact-name check. The duplicated Rust is noise
against a 1.9 MB render payload; the weakened claim is not.

**Keep the base payload's claim as a size budget alone.** It is already enforced there, and
arithmetic is the strongest of the three layers. Rejected as *sufficient*: a budget says how
many bytes arrived, not which bytes, and the failure this is aimed at — glue or a bridge global
in the wrong bundle — is small enough to pass a budget while being exactly the thing that makes
the split untrue.

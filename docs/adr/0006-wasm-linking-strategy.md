# 0006. WASM linking strategy for the C/C++ engines

Date: 2026-09-10

## Status

Accepted — **option 1**, decided by [spike 0001](../spikes/0001-wasm-engines.md).

The seven conditions in *Requirements* below are part of the decision, not advice. Option
2 is recorded as the better architecture, to be re-evaluated at a named **pre-M2 gate**.

### Amendment, 2026-09-12: the gate returns to one question

The pre-M2 gate had acquired a **second** question it was never designed to carry.
[Issue #25](https://github.com/TensorGreed/burrow/issues/25) found that `max_memory_bytes`
bounds nothing during an operation on the web, and the obvious remedy — supplying our own
`WebAssembly.Memory` via `-sIMPORTED_MEMORY` — requires **relinking PDFium from source**
rather than shipping the published artifact. That made "can we bound engine memory?" look
like an option-1-versus-option-2 decision, and dragged an engine-pin change, a licence
re-audit and a rebuilt size budget along with it.

**[Spike 0002](../spikes/0002-wasm-memory-ceiling.md) removes that question from this gate.**
A real ceiling is reachable without relinking anything: the declared maximum in the prebuilt
`pdfium.wasm`'s memory section is a 3-byte LEB128 field, and lowering it is a **1–2 byte patch
that leaves the file length identical**, because LEB128 permits non-minimal encodings and the
new value pads to the existing width.

The two measurements that make it a finding rather than a hope:

- **The module's declared maximum is what binds, not Emscripten's glue.** Both shipped glues
  check their own compiled-in 2 GiB `maxHeapSize`, pass, and then `wasmMemory.grow()` throws
  because the module's maximum is lower; `growMemory` catches it and
  `_emscripten_resize_heap` returns `false`. Neither glue can `abort()` there.
- **The failure transition tracks the patched value.** PDFium's `xref-bomb` peak is
  1900.7 MiB and the bomb flips from `ok` to `Internal` between 1920 and 1856 MiB; qpdf's peak
  is 513 MiB and its transition sits between 576 and 512 MiB. Two engines with peaks two orders
  of magnitude apart, both bracketed — which is what distinguishes the patch binding from a
  bomb failing for some unrelated reason.

18/18 across Chromium, Firefox and WebKit through the shipping path, deterministic over nine
runs, zero bytes of artifact size change, and the failure lands on the instance-fatal path
[ADR 0009](0009-web-panic-contract-and-binding-boundary.md) already defines.

**So this gate returns to the question it was written for**: option 1 versus a **from-source
PDFium build** for redaction safety — the shared-glue-globals hazard in requirement 1, and
trap-versus-unwind semantics — together with the open question below of whether that build
works on a `linux-aarch64` host.

Two things the spike explicitly does **not** settle, recorded here so the gate is not read as
larger than it is:

- The ceiling is **per worker, not per operation**. A caller's own `max_memory_bytes` below it
  stays detect-only, so issue #25 is *mitigated, not resolved*, and
  [ADR 0007](0007-limit-enforcement-per-platform.md)'s 2026-09-12 amendment is unchanged.
- The **value** for that ceiling is not established. Desktop WebKit is not iOS, and
  [ADR 0015](0015-web-worker-lifecycle.md) §7 still refuses to propose mobile defaults from
  desktop readings.

**Nothing is adopted by this amendment.** The spike's recommendation carries six conditions and
belongs in its own ADR when someone takes it up.

### Amendment, 2026-09-12: the gate is closed. Option 1 is settled, not deferred again.

[Spike 0003](../spikes/0003-pdfium-source-build.md) answered the two inputs this gate was
waiting on, and the answers close it. **Option 1 is the accepted linking strategy for the web,
and this is a settled decision rather than a third deferral.** Option 2 remains the better
architecture in the abstract; it is no longer a scheduled re-evaluation.

Four reasons, in the order they changed the picture.

**1. Upstream PDFium has no WASM target at all, so option 2 is a fork rather than a build.**
Grepping the entire tree for Emscripten or WASM across `.gn` and `.gni` returns **one file**,
`third_party/harfbuzz/BUILD.gn`, and its hits are HarfBuzz's own `hb-wasm-api-*.hh` shaper
filenames — nothing to do with building PDFium for the web. `DEPS` mentions Emscripten **zero**
times. There is no `target_os = "emscripten"`, no wasm toolchain and no wasm config.

So "build PDFium from source for the web" does not mean "run `gn` with a wasm target". It means
adopting the patch set that makes that target exist at all — `pdfium-binaries`' patches to
Chromium's `BUILDCONFIG.gn` and `//build/toolchain/wasm` — and then carrying those patches
across every PDFium bump, forever, on a configuration upstream does not test. The recurring
cost is a third party's patch set, not a build.

**2. `depot_tools` and `gn` do work on `linux-aarch64`, and the near-miss is worth recording.**
The first run produced

```
Platform linux-arm64 is not supported by the CIPD client bootstrap:
there's no pinned SHA256 hash for it in the *.digests file.
```

That message is real and **it is not what it looks like**: `cipd_client_version.digests` lists
37 platforms including `linux-arm64`, `cipd_manifest.txt` declares it a `$VerifiedPlatform`,
and `gn/gn/linux-arm64` is in CIPD and built daily. The failure was a relative path resolving
against the wrong working directory. Run correctly, the CIPD client runs and the depot_tools
Python bootstraps.

This is recorded in the ADR rather than only in the spike because **a false blocker is an input
to an architectural decision.** "aarch64 is unsupported" would have been a clean, plausible,
checkable-sounding reason to close this gate, and it would have been wrong. The gate is closed
on reason 1, which is a property of upstream PDFium, not on a property of this machine.

**3. The shared-glue-globals hazard (requirement 1) is closed on both engines.** Spike 0001's
HIGH finding was that loading `pdfium.js` twice in one scope rebinds every glue global while
the bridge still holds the first instance, so calls read and write the other instance's linear
memory — the sandbox holds and parses come back confidently wrong. The two engines needed
different answers:

| engine | closed by | what would reopen it |
|---|---|---|
| PDFium | **construction.** The glue is not modularised, it begins instantiating as the bundle is parsed, the worker is one bundled file ([ADR 0014](0014-web-engine-loading-and-csp.md) §1a) and a worker scope evaluates it once. There is no second `importScripts` to make. | splitting the glue back out of the bundle — which fails production-build's *"ships the worker as ONE bundle, with no glue loose beside it"* and integrity's *"the worker bundle contains all worker code, so one digest covers it"* |
| qpdf | **the memoised promise, plus a test.** `qpdf.js` *is* `MODULARIZE`'d, so `createQpdfModule()` returns a fresh independent instance on every call; `ready ??= init()` in `apps/web/src/worker/main.js` is what prevents a second one. | reverting `ready` to a boolean flag set after the `await` — which now fails `apps/web/src/worker/init-memoisation.test.ts` |

**This narrows a claim spike 0001 made, and the narrowing is the point.** That spike wrote
"`qpdf.js` is immune: we built it with `-sMODULARIZE=1`, so each instance gets its own
closure." That is true **of glue globals**, which is the mechanism it was describing — a second
qpdf instance cannot rebind the first one's closure, and PDFium's asymmetry does come entirely
from the prebuilt artifact. It is *not* true of the **bridge's single attachment point**:
`__burrow_attach` assigns one `qpdfModule`, so a second instance rebinds what every subsequent
`qpdf()` call returns, while an in-flight operation still holds pointers into the first
instance's heap. Same outcome — a call reading and writing the wrong linear memory — reached by
a different route. "Immune" was scoped to the mechanism and read as scoped to the hazard.

When spike 0003 looked, the qpdf half was closed by one line that **no test covered**: the
revert would have been silent. `init-memoisation.test.ts` closes that, and it applies the
revert to a copy of `main.js` and asserts the invariant goes red, so the test has been shown
able to fail rather than merely passing.

**4. `catch_unwind` on the web is not required for M2.** This was the strongest argument for
option 2 — [ADR 0009](0009-web-panic-contract-and-binding-boundary.md) records that a panic on
`wasm32-unknown-unknown` is an uncatchable trap, so `burrow-ffi::guard` is inert on the web,
and redaction is where a panic mid-operation is most consequential. It is nonetheless not a
blocker, because **redaction safety does not depend on unwinding**:

- Output is never emitted unless verification passed (non-negotiable 4). A panic mid-operation
  produces no output, which is the safe outcome — the failure mode unwinding would prevent is
  *losing the worker*, not *leaking a redaction*.
- Panic payloads are **discarded by policy** in any case. ADR 0009 forbids echoing what the
  module threw, because that text is panic output and can carry input-derived bytes; every
  `catch` in the worker binds nothing and reports a fixed, content-free `Internal`. So
  unwinding would carry nothing we keep.

What happens today instead — trap, instance fatal, terminate the worker, spawn a fresh one — is
implemented, tested in three browsers, and measured at 73–102 ms per respawn
([ADR 0015](0015-web-worker-lifecycle.md) §6). That is a recovery story, not a gap.

#### Two triggers that would reopen this

Recorded as named conditions so this is closed rather than open-ended. Absent one of these,
option 1 stands and no re-evaluation is scheduled.

1. **A true per-operation memory bound becomes required** ([issue #25](https://github.com/TensorGreed/burrow/issues/25)).
   Spike 0002's ceiling is per *worker*, and `-sIMPORTED_MEMORY` would be too unless combined
   with a fresh instance per operation. If a per-operation bound becomes a requirement rather
   than a wish, the linking model is back in scope.
2. **`pdfium-binaries` stops publishing at a version we need.** It is the single external
   dependency option 1 rests on, and reason 1 above is precisely why: the patch set that makes
   a wasm build possible lives there and not upstream. Tracked as a supply-chain risk with no
   action now: [issue #44](https://github.com/TensorGreed/burrow/issues/44).

#### Three conditions this decision carries, and they are requirements rather than hopes

Reason 4(a) — "no output is emitted unless verification passed" — is a claim about code M2 has
not written. A security review of this amendment pointed out that it is **stronger** than
non-negotiable 4 as CLAUDE.md states it ("redaction output is verified automatically after
every run" permits output to exist and then be checked; the argument above requires emission to
be *gated* on the check), and that under a trap the difference is real in exactly two cases M2
has genuine pressure to introduce. So they are written down as conditions, because a
requirement can be tested and an argument cannot.

**Each names the check that has to exist**, because a condition with no check is the thing this
project keeps finding: a claim that reads as coverage. R10 is stated under *What this does not
decide* below, with the hazard it comes from; all three are collected in `docs/ROADMAP.md`'s M2
section so they are planned rather than only recorded here. **None of the three is satisfied
until its named check exists and has been shown to fail without the property.**

- **R8. Redaction output is a single value returned from one Rust call, and is posted only
  after that call returns success.** No chunked or progressive emission of partial output.
  `apps/web/CLAUDE.md` asks for progress reporting and cancellation, and with no per-operation
  memory bound, streaming a large redaction out in pieces is the obvious way to avoid a 2×
  heap — but a trap mid-stream leaves partially redacted, unverified bytes already in the
  page's possession, and terminating the worker does not un-send them. Progress may report
  *position*; it may not emit *content*.

  **Check: `redaction-emission.spec.ts`** — drive a redaction through the worker and record
  every `postMessage` it sends. Assert that no message before the final reply carries bytes,
  and that the operation produces **exactly one** message containing output. Shown to fail by
  emitting a chunk mid-operation in a copy of the worker and asserting the spec goes red.
- **R9. Nothing is written outside the wasm heap before verification passes.** No OPFS or File
  System Access write, and no `blob:` URL handed to the page, until the verifier has run.
  `worker.terminate()` reclaims linear memory; it does not reclaim a file handle the page
  already holds.

  **Check: `redaction-no-side-channel.spec.ts`** — stub `navigator.storage.getDirectory`,
  `showSaveFilePicker` and `URL.createObjectURL` in the worker scope before the operation runs,
  and assert none is called until after the reply carrying verified output. The same shape as
  `e2e/zero-requests.spec.ts`, which already asserts a negative about what an operation does.
  Shown to fail by calling `URL.createObjectURL` before verification in a copy.

Under an unwind both cases are recoverable — Rust returns `Err`, the buffer drops, a `Drop`
impl deletes the file. Under a trap they are not. **These two conditions are what make reason
4 true; without them, closing this gate on reason 4 would be wrong.**

#### What this does not decide

**The cross-engine two-heap divergence, which reason 3 does not touch.** Reason 3 closes the
*same-engine* route: two instances of one engine, with the bridge rebound between them. It says
nothing about the fact that PDFium and qpdf are **separate Emscripten modules with separate
linear memories** — which is structural to option 1 and cannot be closed by memoisation. An M2
redaction pass that analyses content in one engine's heap and emits output from the other holds
two copies of the document, and "verification passed" against one copy is not a statement about
the bytes emitted from the other. That is precisely *"a redaction pass that inspects one heap
and edits another"*, this ADR's own words for the failure M2 exists to prevent.

It is latent today — no current operation spans both engines; `page_count` is PDFium only and
`structure_check` is qpdf only — and it becomes live the moment M2 writes one that does. It is
**not** a reason to reopen the gate, because option 2 does not obviously fix it either (PDFium
and qpdf would still be distinct modules unless both were relinked into one). It is an M2
design requirement:

- **R10. Redaction verification runs on the exact byte sequence that is emitted**, in the heap
  it is emitted from — never on a sibling heap's copy of it.

  **Check: a `redact_verify_same_bytes` unit test in `core/burrow-ops`**, asserting the
  verifier is handed the identical buffer the operation returns — by pointer or by hash of the
  emitted bytes, not by re-reading the source. Plus a **conformance divergence case**: a
  document redacted through a path that spans both engines must produce the same verdict
  natively and on the web, which is what `tests/conformance/` exists to catch. Shown to fail by
  verifying a copy taken before the final write.

Nothing about the memory ceiling. Spike 0002 is still unadopted, issue #25 is still *mitigated,
not resolved*, and [ADR 0007](0007-limit-enforcement-per-platform.md)'s 2026-09-12 amendment is
unchanged. Closing this gate makes M2 plannable against option 1; it adopts nothing.

#### Two forward references this resolves

[ADR 0009](0009-web-panic-contract-and-binding-boundary.md) twice points at this gate as a
scheduled decision — it notes that `catch_unwind` *does* work on `wasm32-unknown-emscripten`
and schedules the gate, and its *Consequences* say "if the pre-M2 gate adopts option 2, most of
section 1 becomes unnecessary". Both pointers now lead to a decision that has been made.
**ADR 0009 §1 is permanent, not provisional.** Nothing in it is weakened by this closure; a
reader is simply no longer waiting on an answer.

## Context

[ADR 0002](0002-rust-core-and-bindings.md) chose wasm-bindgen and, implicitly, the
`wasm32-unknown-unknown` target. [ADR 0004](0004-native-engines.md) chose PDFium and qpdf
and left their acquisition open, noting prebuilt binaries as the likely route. Those two
decisions do not currently compose, and the reason is not a detail of build configuration.

**`wasm32-unknown-unknown` and Emscripten are different platforms.**
`wasm32-unknown-unknown` has no libc, no sysroot, and no defined C ABI beyond the bare
Wasm calling convention. Emscripten is a full platform: its own libc, its own `setjmp`
and C++ exception lowering, its own memory layout, and a JS runtime it expects to be
loaded alongside the module. An Emscripten `.a` cannot be linked into a
`wasm32-unknown-unknown` binary. There is no linker flag that reconciles them.

Every prebuilt PDFium WASM build we would want — including
[pdfium-binaries](https://github.com/bblanchon/pdfium-binaries) — is an Emscripten
build. So the plan implied by ADR 0004 ("use prebuilt PDFium, including for wasm")
cannot be executed as written.

Two further constraints narrow the options, and both are easy to overlook until late:

- **C++ exceptions.** qpdf uses them centrally; PDFium uses them in places. Emscripten
  supports exceptions (with a size and speed cost). Support on
  `wasm32-unknown-unknown` and on WASI is partial and, in practice, means building the
  engine with exceptions disabled and accepting that a throw becomes an abort — which
  for qpdf means losing the error reporting we depend on for repair.
- **Threads.** Both engines can be built single-threaded, so this is survivable, but
  the choice interacts with whether we can use a shared memory model later.

This is the single largest unknown in M1, it is upstream of a great deal of work, and it
is discovered late if we start by writing operations.

## Decision

We will **not** decide this from documentation. M1's first task is a spike that proves
Rust plus PDFium plus qpdf running in a browser Web Worker, end to end, before any
operation is implemented.

The spike's bar: **open a real PDF in a browser worker and report its page count**, with
qpdf also linked and callable, driven from Rust, running in CI. Not a demo in Node, and
not PDFium alone — qpdf is the harder half, because of exceptions.

The spike decides this ADR and, at the same time, ADR 0004's open acquisition question.
Those two cannot be settled independently: the acquisition route determines what can be
linked, and the linking strategy determines what is worth acquiring.

### The options it will evaluate

**1. Separate Emscripten engine modules, bridged through JS.** The engines stay
Emscripten builds in their own module(s); Rust stays on `wasm32-unknown-unknown`; JS
mediates. This is
[pdfium-render](https://github.com/ajrcarey/pdfium-render)'s approach on the web, so it
is known to work.

- *For:* prebuilt PDFium is usable as published. Rust keeps the well-supported target
  and wasm-bindgen. Exceptions work, because Emscripten handles them.
- *Against:* two heaps, so file bytes are copied across the JS boundary — a real cost on
  large documents, and a second copy of the user's file in memory. Orchestration logic
  drifts into JS, which cuts against ADR 0002's rule that bindings contain no logic. Two
  modules to load and version. Passing a PDFium handle back and forth through JS is
  awkward and easy to get wrong.

**2. `wasm32-unknown-emscripten` throughout.** Build Rust with Emscripten too, so
everything links into one module with one heap.

- *For:* direct linking, no JS bridge, no copying, C++ exceptions work, and the engine
  wrapper looks like the native one — which keeps `burrow-engines` honest across targets.
- *Against:* wasm-bindgen does not support the Emscripten target, so ADR 0002's binding
  choice would have to be revisited for the web. Emscripten becomes a required toolchain
  for every Rust build. Much of the Rust wasm ecosystem assumes
  `wasm32-unknown-unknown`. Emscripten ships its own JS glue, which we would have to
  audit for network access to satisfy non-negotiable #1.

**3. WASI (`wasm32-wasip1`) with a browser shim.** One Rust target with a real std,
engines built with `wasi-sdk`, and a shim providing the WASI imports in the browser.

- *For:* a clean, standard platform; the same artifact runs headlessly in the corpus
  runner and in the browser; no Emscripten.
- *Against:* browsers do not run WASI natively, so a shim is mandatory and is ours to
  maintain. wasm-bindgen does not target WASI. PDFium under `wasi-sdk` is not an upstream
  configuration, so this route almost certainly means **building PDFium from source** for
  wasm — which is exactly the expensive path ADR 0004 hoped to avoid. Exception support
  under wasi-sdk is the weakest of the three.

## Decision (resolved)

**Option 1: Emscripten engine modules bridged through JS.** Rust on
`wasm32-unknown-unknown` with wasm-bindgen, driving prebuilt PDFium and a source-built
qpdf as separate Emscripten modules, inside a classic Web Worker.

It is the only route that works with an obtainable PDFium, and it meets this ADR's bar in
full: 6/6 Playwright assertions green in headless Chromium, locally on aarch64 and in CI
on x86-64.

Options 2 and 3 were eliminated by evidence, not preference:

- **Option 2** needs a from-source PDFium wasm build for two independent reasons. There is
  no wasm `libpdfium.a` — pdfium-binaries stages only `pdfium.{html,js,wasm}` for
  Emscripten targets — *and* the whole C++ stack must be built with the same exception
  model as Rust's emscripten target. Linking `-fexceptions` qpdf against Rust failed on
  `__resumeException`/`llvm_eh_typeid_for`; `-fwasm-exceptions` was required. A
  third-party `.a` built by emsdk 3.1.72 would be very unlikely to match.
- **Option 3** does not run. wasi-sdk 34 does ship EH-enabled sysroot variants (an
  earlier reading that it had none was wrong), and it links — but Chromium 153 rejects the
  result: *"module uses a mix of legacy and new exception handling instructions."*

### Requirements

These are conditions of the decision. A web build that does not satisfy all seven is not
compliant with this ADR.

1. **Memoise the init promise, and use one engine instance per worker.** `pdfium.js` is
   not modularised, so its state lives in worker-global `var`s and `importScripts` does
   not dedupe. Loading it twice rebinds every global while the bridge still holds the
   first instance — an in-flight call then reads and writes the *other* instance's linear
   memory and dispatches through its function table. The sandbox holds, but parses return
   confidently wrong data. Non-negotiable given what redaction does in M2.
2. **Suppress engine logging at both layers before any real file is opened.** qpdf's
   default logger writes warnings containing object numbers and byte offsets to stderr,
   which reaches the devtools console — including for files that parse *successfully*.
   `setSuppressWarnings(true)` plus a discarding `QPDFLogger`, and `printErr`/`print`
   stubbed on both modules.
3. **Pass `Module.wasmBinary`, build `-sENVIRONMENT=web,worker`, and ship
   `connect-src 'none'`.** This makes non-negotiable #1 enforced by the browser rather
   than by re-auditing 240 KB of minified third-party JS on every PDFium bump.
4. **Assert qpdf's crypto flags in CI.** `USE_IMPLICIT_CRYPTO` defaults ON upstream and
   would link GnuTLS (LGPL-2.1-or-later) into a static artifact. See
   [ADR 0004](0004-native-engines.md).
5. **Pin emsdk and wasi-sdk** in the engine pin manifest. The audit caught wasi-sdk
   fetched with no pin at all.
6. **No sentinel that can alias success, and no error classification by string match.**
   `-FPDF_GetLastError()` yields `-0`, and `-0 === 0` in JS, so a failed load reads as
   "opened fine, zero pages". Password detection must use qpdf's error *code*, never its
   prose — the rule already in `core/CLAUDE.md`.
7. **Re-audit `pdfium.js` on every PDFium bump**, or make that unnecessary via (3).

### Amendment, 2026-09-11 (M1 PR 4a-i): requirement 3, and a service worker would break it

Requirement 3 above asks for `Module.wasmBinary` **and** `connect-src 'none'`, which cancel
out: supplying the bytes means fetching them, and fetch is what `connect-src` governs. It is
replaced by [ADR 0014](0014-web-engine-loading-and-csp.md), which narrows the policy instead
of loosening it and states the guarantee accurately.

**A service worker invalidates the guard ADR 0014 specifies, and adding one requires
re-evaluating it.** Recorded here, not only there, because the temptation arrives as an
offline-support or caching feature that has nothing obviously to do with the CSP.

The guard establishes that a policy is in force by making two requests and comparing them: an
allowlisted one must succeed, a non-allowlisted one must be refused. A service worker sits in
front of `fetch` and may answer either from its own cache or synthesise a response, without
the network or the policy being consulted at all — so:

- a synthesised answer to the **probe** makes an enforced policy look absent, and the worker
  refuses every operation;
- a synthesised answer to the **control** makes an unreachable network look healthy, which is
  the failure the dedicated `cache: "no-store"` control exists to prevent — and `no-store`
  constrains the HTTP cache, not a service worker's `fetch` handler;
- a service worker also changes what `connect-src` is even observing, since a request it
  handles locally may never become a network request.

None of that is hypothetical or subtle to fix; it is simply a different set of assumptions.
If burrow ever adds a service worker, ADR 0014 §1b must be re-derived against it before the
worker path is trusted, and `src/worker/guard.test.ts` needs a world that models one.

Not "someday". **Before M2 starts**, option 2 is re-evaluated against the same bar, because
redaction is exactly where option 1's two weaknesses bite hardest: the shared-glue-globals
hazard in requirement 1, and trap-versus-unwind semantics. A redaction pass that inspects
one heap and edits another is the failure mode M2 exists to prevent.

Option 2's measured advantages: `catch_unwind` **works** on the Emscripten target, so ADR
0002's guard rule holds on the web — which it cannot under option 1, where a panic is an
uncatchable trap. One heap, so no cross-heap copy. And slightly smaller.

**Open question for that gate:** whether PDFium's from-source `gn`/`ninja` wasm build
works on a `linux-aarch64` host, or has to run on x86-64 CI runners. The dev and corpus
machine is aarch64, so this determines whether the build is reproducible where the
regression runs happen.

## Consequences

M1 starts with a spike rather than a feature, which delays the first visible operation.
That is the intended trade: this question invalidates work if answered late, and all
three options have consequences that reach into the public API, the binding layer, and
the shape of `burrow-engines`.

Option 1 keeps ADR 0002 intact but pushes logic into JS. Option 2 preserves the
single-implementation principle at the cost of reopening ADR 0002's binding choice.
Option 3 is the cleanest platform and by far the most work. We should expect to accept
something we do not love.

Whichever wins, the wasm module will be large, and the copying behaviour of option 1
would make peak memory roughly twice the file size — both of which feed the wasm size
budget and the `max_memory_bytes` enforcement discussed in
[ADR 0007](0007-limit-enforcement-per-platform.md).

Until the spike concludes, `bindings/burrow-wasm` deliberately contains no engine code
and no wasm-bindgen dependency. Adding either now would pin a decision this ADR exists
to make.

## Alternatives considered

**Pure-Rust PDF crates on the web, native engines elsewhere.** Sidesteps the problem
entirely for the web target. Rejected for the same reason as in ADR 0004, and more
sharply: two engines means two behaviours on malformed files, and for redaction (M2) that
is unacceptable — the web would be the least-tested path handling the highest-risk
operation.

**PDF.js for the web.** Already rejected in ADR 0004; noted again because the linking
difficulty makes it tempting. It remains a fourth implementation.

**Decide now from documentation and upstream issue threads.** Cheap, and how this ADR
was originally going to be written. Rejected: the failure modes here — exception
lowering, `setjmp` in PDFium, shim gaps — are exactly the kind that only appear when
something is actually built and run.

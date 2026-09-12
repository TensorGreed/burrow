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

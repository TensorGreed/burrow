# 0006. WASM linking strategy for the C/C++ engines

Date: 2026-09-10

## Status

Proposed — decided by the M1 spike described below.

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

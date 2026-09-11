---
name: m1-web-engine-path
description: Measured facts about the web engine path (PR 4a-i) — what pdfium.js actually contains, which artifacts are integrity-pinned, and where the fake bridge's leak detector is blind.
metadata:
  type: project
---

Four things about the web engine path that cost real digging to establish, all verified
against the built artifacts in `engines/vendor/wasm/lib/` on 2026-09-11.

**The prebuilt `pdfium.js` is NOT built with burrow's flags.** `engines/build-wasm.sh`
builds qpdf with `-sENVIRONMENT=web,worker -sFILESYSTEM=0`, but PDFium is unpacked from
`pdfium-wasm.tgz` as-is. The shipped 160 KB glue still contains `ENVIRONMENT_IS_NODE`,
`require("fs")`, `require("crypto")`, a synchronous `XMLHttpRequest` path, the lazy-file
XHR machinery, and `fetch(binaryFile, {credentials:"same-origin"})` for `pdfium.wasm`.
None of it runs (the worker supplies `instantiateWasm`, and a browser worker is not Node),
but any comment saying "the modules are built `-sENVIRONMENT=web,worker`" is true of qpdf
only. The `https?://` / Asyncify / pthread greps in build-wasm.sh are likewise applied to
`qpdf.js` and not to `pdfium.js` — the guard is absent on the third-party artifact that
gets re-fetched on every bump. Measured: pdfium.js is currently clean of absolute URLs and
Asyncify, so the gap is in the guard, not the artifact.

**Only the `.wasm` files are integrity-pinned.** The worker uses
`fetch(url, {integrity})` for the three `.wasm` and plain `importScripts()` for the three
`.js` (SRI has no `importScripts` form, and a blob/`new Function` workaround is refused by
`script-src 'self'` with no `'unsafe-eval'`). `tools/stage-web-engines.mjs` computes SRI
digests for all six and `e2e/integrity.spec.ts` asserts the manifest carries them, which
reads as if all six are checked. The content hash in the filename is cache-busting, not
verification — nothing recomputes it at runtime.

**`FakeHeap::assert_empty` cannot see the qpdf logger.** `web/fake.rs`'s `logger_create`
returns a hard-coded constant and allocates nothing, so the leak detector the module docs
advertise is structurally blind to exactly one allocation — the one the web path leaks per
operation (native installs one process-global logger under a `Once`; web creates one per
`check()` and `qpdflogger_cleanup` is deliberately undeclared).

**The qpdf export cross-check filters exports by `/^(qpdf|malloc|free)/`.** 12 of the
module's 31 exports (`__cxa_*`, `_emscripten_stack_*`, `memory`, `__indirect_function_table`,
`setThrew`) are never examined. All benign today. The `sed` parse of `ffi.rs` on the other
side is fragile but fails closed in both directions.

Related: [[m1-qpdf-exception-boundary]], [[m1-limits-real-strength]],
[[m1-engine-supply-chain]].

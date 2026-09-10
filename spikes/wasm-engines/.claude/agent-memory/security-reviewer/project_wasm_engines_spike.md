---
name: project-wasm-engines-spike
description: The spikes/wasm-engines spike compares three WASM engine-linking options; audited network + memory findings for option 1
metadata:
  type: project
---

`spikes/wasm-engines/` (branch `spike/wasm-engines`) compares three ways to get PDFium and
qpdf into WebAssembly: option 1 = Emscripten engine modules bridged through JS from a
`wasm32-unknown-unknown` Rust module; option 2 = everything under Emscripten; option 3 =
WASI. The deciding criterion named in the code is C++ exception handling and error
reporting (an ADR 0006 that is referenced by the spike but not yet in `docs/adr/`).

**Why:** the outcome picks the production linking strategy for `burrow-engines`, so
security properties found here are hard to change later.

**How to apply:** two audit conclusions worth not re-deriving from 240 KB of minified glue:

- **Network:** neither `vendor/pdfium/lib/pdfium.js` nor the locally built `build/qpdf.js`
  can reach off-origin. Both use `fetch(..., {credentials:"same-origin"})` and a
  *synchronous* XHR fallback (`readBinary`, worker-only) solely for their own
  `<name>.wasm`, at a URL derived from the module's own script location. The
  `FS.createLazyFile` range-XHR machinery is present but `FS` is not exported on either
  `Module`, so it is unreachable. Neither `.wasm` imports any network primitive. The
  hardening lever is passing `Module.wasmBinary` (or `-sSINGLE_FILE`) plus a
  `connect-src 'none'` CSP, which removes the capability rather than auditing it.
- **Option 1's structural hazard:** `pdfium.js` is *not* modularised, so its `wasmMemory`,
  `HEAPU8`, `wasmTable` and `wasmImports` closures are worker-global. Any second
  `importScripts` of it creates a second instance that rebinds those globals under the
  first. `qpdf.js` is modularised and immune. This is a property of the option, not of the
  spike's glue.

See [[feedback-spike-review-scope]] for how the user wants spike reviews scoped.

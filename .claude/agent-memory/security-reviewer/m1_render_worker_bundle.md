---
name: m1-render-worker-bundle
description: ADR 0026's second worker bundle (PDFium on demand) — the measured onRuntimeInitialized ordering hang, what the split really enforces, and the markers that check prose instead of code.
metadata:
  type: project
---

Measured 2026-09-16 against the working tree of branch `pdfium-on-demand` (ADR 0026),
which brings PDFium back to the web in a second worker bundle fetched only on demand.

**`Module["onRuntimeInitialized"]?.()` is a ONE-SHOT OPTIONAL CALL, and the render bundle
assigns it too late.** `render-main.js`'s `init()` does
`await new Promise(r => { self.Module.onRuntimeInitialized = () => r(self.Module); })`, but
`init()` is only reached from `worker-protocol.js` **after `await POLICED`** — two
`cache:"no-store"` fetches. `prelude.js` started the `pdfium.wasm` fetch at parse time with
`integrity` (cacheable), and Emscripten's `doRun()` fires the callback the instant the
instance lands. Verified with the real `engines/vendor/wasm/lib/pdfium.js` in a `node:vm`
context (repro pattern: assign the callback N ms after evaluating the glue):

| init delayed | outcome |
|---|---|
| 0 ms | resolved in 11 ms |
| 300 ms, compile immediate | **never settles** (`calledRun === true`) |
| 300 ms, compile delayed 2 s | resolved |

`WebAssembly.compile` of the 5.3 MB `pdfium.wasm` measures **10 ms**. So on a warm HTTP
cache the local path wins whenever the control fetch costs more than a few tens of ms — the
normal case on any real network, and never on localhost, which is why the e2e is green. The
symptom is no reply to `init` at all: `worker-host.js` `discard("engine start-up stalled",
{crash:true})` after `initTimeoutMs` (240 s, re-armed per `{starting:true}`), then the
breaker. The fix is to assign the callback in `render-prelude.js`, before `pdfium.js` parses.

**The absence claim IS structural, and I checked the two places it could leak.** The
generated `BURROW_ENGINES` is sliced per bundle (`moduleIdsByBundle`), so the base bundle has
no PDFium URL to fetch. Measured on the real build: the base Rust module imports only
`__burrow_qpdf_*` + `__burrow_now_ms`, the render module only `__burrow_pdfium_*` +
`__burrow_now_ms`; and the only `/engines/` URL in any `dist/_astro/*.js` chunk is
`burrow-worker.<hash>.js`, so the named-export tree-shaking argument in `tool-host.ts` holds.
Every page's CSP names both bundles — known, documented in three places, and correct.

**`INHERITS_PAGE_CSP` is a DEAD IDENTIFIER used as a test marker.** `e2e/integrity.spec.ts`
asserts both bundles contain it, labelled "the fail-closed guard". It exists nowhere as code —
only in prose in `prelude.js`, `guard.test.ts` and `globals.d.ts`, describing a guard that was
renamed to `POLICED`. Both assertions pass on a bundle with the guard deleted.

**`check-pdfium-is-render-only.sh`: two soft spots.** `grep -oc` counts matching *lines*, so
the "exactly 1 PDFium URL" rule cannot see 2 (masked by the `-eq 4` wasm-URL rule). And
nothing asserts `pkg-render/` was built `--features render`: all three `RENDER_PRESENT`
markers come from `bridge-pdfium.js` and the manifest, not from the Rust module, so a stale
`pkg-render` holding a `documents` build passes the file-level checks and fails only at
instantiation in the browser.

**The protocol extraction lost nothing** — a comment-stripped diff of
`origin/main:apps/web/src/worker/main.js` against `worker-protocol.js + main.js` is a pure
reordering plus `qpdfHeapBytes`→`engineHeapBytes` and the `KNOWN_OPS` Set. The protocol region
of the two BUILT bundles hashes identically, which is the cheap way to re-check that.

**The render bundle runs no `check_input_budget`,** unlike `main.js`, which runs it for every
op and not only `merge`. `render-main.js`'s comment justifies the omission on merge-specific
grounds and is wrong about it.

Related: [[m1-web-engine-path]], [[m1-web-worker-lifecycle]], [[m1-engine-supply-chain]].

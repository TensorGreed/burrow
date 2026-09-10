# Spike 0001 — WASM engine linking

**Status: in progress.** Updated as work happens, not at the end.

Decides [ADR 0006](../adr/0006-wasm-linking-strategy.md) (WASM linking strategy) and
[ADR 0004](../adr/0004-native-engines.md)'s open acquisition question.

Spike code: branch `spike/wasm-engines`, under `spikes/wasm-engines/`. Never merged.

## The bar (from ADR 0006)

- [ ] A real PDF opens inside a browser Web Worker, driven from Rust, reporting page count
- [ ] qpdf also linked and callable in the same setup
- [ ] A malformed PDF through qpdf returns a **typed error, not an abort** (the C++ exceptions test)
- [ ] Runs headless in CI (Playwright + Chromium)

## Environment

Dev machine is the ARM64 regression box: NVIDIA GB10, `aarch64`, Ubuntu 24.04, 20 cores,
121 GB RAM.

| Fact | Value |
|---|---|
| emsdk on linux-aarch64 | **Native, no emulation.** `wasm-binaries-arm64.tar.xz` exists |
| emsdk install time | **17.2 s** (`emsdk install latest`, warm network) |
| emsdk version used | 6.0.9 |
| Rust | 1.98.1; `wasm32-unknown-emscripten` and `wasm32-wasip1` both available via rustup |

emsdk arm64 builds are published for both 6.0.9 and 3.1.72 (pdfium-binaries' pin) —
verified HTTP 200 on both. Docker/qemu is not needed on this machine.

## Finding 1 — there is no wasm `libpdfium.a`. Option 2 cannot use the prebuilt.

`steps/07-stage.sh` in pdfium-binaries stages, for `emscripten-*` targets only:

```
mv "$BUILD/pdfium.html" "$STAGING_LIB"
mv "$BUILD/pdfium.js"   "$STAGING_LIB"
mv "$BUILD/pdfium.wasm" "$STAGING_LIB"
rm "$STAGING/PDFiumConfig.cmake"
```

Every other platform gets `libpdfium.a` or a shared library. wasm gets a finished
Emscripten module and no CMake config. Confirmed against the downloaded artifact: the
package contains `pdfium.{html,js,wasm}`, `include/`, `LICENSE`, `args.gn`, and **no
`.a` or `.bc` anywhere**.

**Consequence:** option 2 (single module, statically linked) cannot consume the prebuilt.
It requires a from-source PDFium wasm build via `gn`/`ninja` + depot_tools. This is the
expensive path ADR 0004 hoped to avoid, and it lands squarely on ADR 0004's acquisition
question.

Also relevant: pdfium-binaries marks its WebAssembly build **experimental**
([issue #28](https://github.com/bblanchon/pdfium-binaries/issues/28)).

## Finding 2 — the prebuilt module does export the full API

| Measure | Value |
|---|---|
| `pdfium.wasm` | 5,315,922 B (5.32 MB) raw |
| `pdfium.js` | 164,487 B |
| `pdfium.html` | 19,520 B (demo shell, unused) |
| Total wasm exports | 492 |
| **`FPDF_*` exports** | **429** |
| Allocator exports | `malloc`, `free`, `memory`, `__indirect_function_table` |
| Environments supported | web, worker, node |
| Modularised? | **No** — global `var Module`, no `EXPORT_NAME` |

So option 1 can use the prebuilt as published. This was not a given: no
`EXPORTED_FUNCTIONS` appears in `patches/wasm/`, so it had to be checked.

### Workaround required: global-scope loading

The module's first statement is
`var Module = typeof Module != "undefined" ? Module : {}`. Under CommonJS `require`
that executes at *function* scope, where the hoisted local `Module` shadows any global
config — so `locateFile` and `onRuntimeInitialized` are silently discarded and the init
hook never fires. It must be loaded at true global scope, where `var` over an existing
global property is a no-op.

A browser worker gets this for free (`importScripts()` is global scope). In Node it
needs `vm.runInThisContext` plus `require` and `__dirname` shims, because the glue's
node branch uses both. **Node-only workaround; does not affect the browser path.**

## Finding 3 — C++ exceptions: confirmed, and confirmed to matter

Minimal em++ test, throw across a `extern "C"` boundary:

| Build | Behaviour on `throw` |
|---|---|
| `em++ -fexceptions` | Caught, `what()` readable, returns cleanly |
| `em++` (default) | **Aborts the module** |

Exactly the risk ADR 0006 identified. `-fexceptions` is mandatory for qpdf.

## Finding 4 — PDFium's error path is typed, not exceptional

Page counts via `FPDF_LoadMemDocument` + `FPDF_GetPageCount`, errors via
`FPDF_GetLastError` (Node, prebuilt module):

| File | Bytes | Result |
|---|---|---|
| `small-1page.pdf` | 603 | 1 page |
| `medium-100page.pdf` | 15,859 | 100 pages |
| `large-50mb.pdf` | 52,429,484 | 1 page |
| `malformed-truncated.pdf` | 402 | error `FORMAT`, no abort |
| `malformed-badxref.pdf` | 15,859 | **100 pages — PDFium reconstructed the xref itself** |
| `malformed-notpdf.bin` | 3,700 | error `FORMAT`, no abort |

Two things follow.

**PDFium reports failures as error codes, never as exceptions**, so the exceptions risk
is concentrated entirely in qpdf — as ADR 0006 predicted.

**PDFium already repairs a corrupted xref.** ADR 0004 justifies qpdf partly as the
repair path for files "PDFium refuses". That premise is weaker than assumed: PDFium
silently reconstructed a fully corrupted xref table. qpdf's value is still structure,
encryption, and object streams, but "repair" needs a harder test case before it is used
to justify the dependency.

Heap after opening the 52 MB file: **65.4 MB**, consistent with the input being copied
into the engine heap.

## Test corpus

Generated by `spikes/wasm-engines/common/make-corpus.py` — reproducible byte-for-byte,
no licensing question, and the malformed cases are precisely controlled. The 50 MB
filler is a deterministic LCG, not random, and incompressible so the file is genuinely
large rather than large-when-decompressed (a compression bomb tests something else).

Generation takes 4.8 s.

## Provenance and pinning

`spikes/wasm-engines/scripts/pins.env` pins every download by version and sha256;
`fetch-engines.sh` refuses to proceed on a mismatch, and refuses a `TBD` pin after
printing the observed hash, so a value is committed deliberately rather than trusted.

| Artifact | Version | Integrity |
|---|---|---|
| `pdfium-wasm.tgz` | `chromium/8044` | `2528dfb9…11fe1` — **upstream publishes no checksums**, so this is trust-on-first-use |
| `qpdf-12.4.1.tar.gz` | 12.4.1 | `f045aa27…873c` — **matches upstream's PGP-clearsigned `qpdf-12.4.1.sha256`** |

The asymmetry is worth recording: qpdf ships a signed checksum file and a sigstore
bundle; pdfium-binaries ships neither. If PDFium is adopted from prebuilts, our only
integrity anchor is a hash we captured ourselves.

## Still to do

- Build qpdf → wasm with `-fexceptions`; typed error on malformed input
- Option 1: Rust (`wasm32-unknown-unknown`) + both engines via JS bridge, in a worker
- Playwright/Chromium test in CI; panic-kills-worker + respawn
- Option 2: Rust `wasm32-unknown-emscripten` + qpdf `.a`, single module
- Option 3: qpdf under wasi-sdk, exceptions behaviour
- Full measurements table; `license-auditor` and `security-reviewer` passes

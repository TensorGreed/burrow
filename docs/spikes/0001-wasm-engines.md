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

## Finding 5 — qpdf builds for wasm easily, and its exceptions work

| Measure | Value |
|---|---|
| `libqpdf.a` (Emscripten static archive) | 8,081,240 B |
| qpdf configure + build | **6.4 s** (20 cores, aarch64) |
| Shim link | 1.2 s |
| `qpdf.wasm` | 917,305 B |
| `qpdf.js` | 74,780 B |
| Modularised? | Yes — `-sMODULARIZE=1 -sEXPORT_NAME=createQpdf` |

Configured with native crypto only (`USE_IMPLICIT_CRYPTO=OFF`,
`REQUIRE_CRYPTO_NATIVE=ON`): no OpenSSL, no GnuTLS. OpenSSL is on our `deny.toml` ban
list, and neither belongs in software that never opens a socket.

**The exceptions test — the one ADR 0006 says decides this.** A C shim
(`qpdf_shim.cpp`) converts every throw into a returned status code:

| File | Result |
|---|---|
| `small-1page.pdf` | 1 page |
| `medium-100page.pdf` | 100 pages |
| `large-50mb.pdf` | 1 page |
| `malformed-truncated.pdf` | `MALFORMED`, message *"unable to find trailer dictionary while recover…"* |
| `malformed-badxref.pdf` | **100 pages** — qpdf reconstructed the xref |
| `malformed-notpdf.bin` | `MALFORMED` |

And critically: **the module is still alive afterwards** — `qpdf_probe_version_ok()`
returns 1 after every malformed input. The `throw` was genuinely caught and converted,
not turned into an abort.

**C++ exceptions from qpdf work under Emscripten with `-fexceptions`.** This removes the
largest single technical risk ADR 0006 identified.

### Two build frictions worth recording

**qpdf's CMake finds the host zlib under `emcmake`.** Its pkg-config path located the
system zlib 1.3.2 — wrong ABI for wasm — and no host libjpeg, so configure failed.
Fixed by pointing `PKG_CONFIG_EXECUTABLE` at a nonexistent path, which forces qpdf's
`find_path`/`find_library` fallback to resolve against Emscripten's sysroot. A
workaround, and it would need to be a documented part of any real build.

**zlib and libjpeg come from Emscripten ports** (`embuilder build zlib libjpeg`, 1.0 s).
These are fetched and integrity-checked by emsdk, not pinned by us — a deviation from
our "pin every download" rule that would have to be closed by vendoring both with our
own checksums if this route is adopted.

## Finding 6 — qpdf's warnings leak input-derived text to the console

Unprompted, qpdf writes warnings to stderr, and they embed data derived from the input:

```
WARNING: spike-input (object 3 0, offset 9999999999): expected n n obj
WARNING: spike-input: Attempting to reconstruct cross-reference table
```

In a browser those land in the devtools console; in a native app, in the platform log.
Offsets and object numbers are file content by any reasonable reading, so this is a
non-negotiable #1 concern, not cosmetic. Any real qpdf integration must install a
`QPDFLogger` and route warnings somewhere deliberate — never let qpdf's default logger
reach the console.

The same applies to the shim's `qpdf_probe_last_message()`, which exists here only to
prove the exception payload was readable. It must not be forwarded to the host: this is
exactly what `burrow-ffi::guard` was changed to prevent in #2.

## Finding 7 — option 1 meets the bar in full

6/6 Playwright assertions pass in headless Chromium (`tests/option1.spec.mjs`), in
2.5 s:

- a real PDF opens in a Web Worker, driven from Rust, reporting **100 pages**
- qpdf is linked and callable in the same worker
- malformed input through **qpdf** is a typed error, worker survives, next call fine
- malformed input through **PDFium** is a typed error, worker survives
- the panic path behaves as measured (see Finding 8) and the respawn path works
- measurements collected

### Option 1 payload

| Artifact | raw | gzip | brotli |
|---|---|---|---|
| `pdfium.wasm` | 5,315,922 | 2,431,675 | 1,904,807 |
| `pdfium.js` | 164,487 | 30,193 | 26,174 |
| `qpdf.wasm` | 917,305 | 307,988 | 242,408 |
| `qpdf.js` | 74,780 | 18,972 | 16,848 |
| Rust `.wasm` | 15,266 | 7,060 | 6,008 |
| Rust glue + bridge + worker | 15,015 | 5,038 | 4,335 |
| **TOTAL** | **6,502,775** | **2,800,926** | **2,200,580** |

The Rust module is 15 KB: all the weight is the engines, and PDFium is 82% of it.

### Option 1 timings and memory (headless Chromium, aarch64)

Cold load, worker spawn → first page count: **57–75 ms**.

| File | PDFium | qpdf | engine heap after |
|---|---|---|---|
| `small-1page.pdf` | 0.0 ms | 5.6 ms | 35.8 MB |
| `medium-100page.pdf` | 0.0 ms | 3.1 ms | 35.8 MB |
| `large-50mb.pdf` | 35.6 ms | 17.8 ms | **82.4 MB / 118.0 MB** |

Peak engine heap reaches **118 MB for a 50 MB input** — the copy across the JS bridge
is visible and material. Two wasm modules cannot share linear memory, so this is
inherent to option 1, not an implementation shortcut.

## Finding 8 — a Rust panic is a trap, not a worker death. `apps/web/CLAUDE.md` is wrong.

Measured, and it contradicts what that file currently says (which I wrote in #2):

```
PANIC_REPLY {"ok":false,"threw":"RuntimeError: unreachable"}
DEATHS 0
AFTER_PANIC       {"pages":1,"isOk":true}
AFTER_PANIC_QPDF  {"pages":100,"isOk":true}
```

On `wasm32-unknown-unknown` a panic ends in the `unreachable` instruction. That is a
WebAssembly **trap**, and a trap surfaces in JS as a **catchable `RuntimeError`**. The
worker does not die, no `error` event fires, and subsequent calls still succeed.

They succeed only because this spike's Rust holds no state, and because in option 1 the
engines live in *separate* modules whose heaps the trap never touched. The Rust
instance's invariants are still broken after a trap, so the page must discard it
**deliberately** — nothing forces its hand. That is a materially different contract from
"a crashed worker surfaces as `Error::Internal` and a fresh worker is spawned", and
`apps/web/CLAUDE.md` needs correcting.

## Finding 9 — option 2 works, and `catch_unwind` works with it

Rust on `wasm32-unknown-emscripten`, statically linked against the same qpdf:

```
shim reachable: true
  small-1page.pdf                 603 B  Pages(1)
  medium-100page.pdf            15859 B  Pages(100)
  large-50mb.pdf             52429484 B  Pages(1)
  malformed-truncated.pdf         402 B  Malformed
  malformed-badxref.pdf         15859 B  Pages(100)
  malformed-notpdf.bin           3700 B  Malformed
catch_unwind result: CAUGHT (unwinding works)
shim still reachable after catch: true
open after caught panic: Pages(100)
```

**`catch_unwind` works on the Emscripten target.** ADR 0006 did not anticipate this, and
it matters more than the size numbers: ADR 0002's rule that no panic crosses a boundary,
enforced by `burrow-ffi::guard` mapping panics to `Error::Internal`, **holds on the web
under option 2 and cannot under option 1**. Under option 1 a panic is an uncatchable
trap; under option 2 it is a catchable unwind, and the module keeps working afterwards.

Also: qpdf reads **directly out of Rust's slice**. Same linear memory, no copy, no
second buffer.

### Option 2 payload, like-for-like against option 1's qpdf path

| | raw | gzip | brotli |
|---|---|---|---|
| Option 2 (Rust + qpdf, one module) | 876,710 | 332,438 | **261,633** |
| Option 1 (Rust + bridge + qpdf module) | 1,020,214 | 338,058 | 268,759 |

Option 2 is smaller *and* single-module — though the difference is small enough not to
be the deciding factor.

### The exception model must match, and that is the real blocker

The first link attempt **failed**: `undefined symbol: __resumeException`,
`llvm_eh_typeid_for`. Cause: qpdf was built with `-fexceptions` (Emscripten's JS-based
EH) while Rust 1.98's emscripten target links with `-mllvm -exception-model=wasm` and
`-lunwind-legacyexcept` — native wasm EH. Rebuilding qpdf and the shim with
**`-fwasm-exceptions`** made it link.

Two consequences:

1. Every C++ dependency in option 2 must be built with the *same* exception model as
   Rust's emscripten target. That is a whole-stack constraint, not a per-library flag.
2. It compounds Finding 1. Even if pdfium-binaries did publish a wasm `libpdfium.a`, it
   is built by emsdk 3.1.72 with its own flags and would be very unlikely to match
   Rust 1.98's exception model. **Option 2 requires building PDFium from source not just
   because no `.a` exists, but because a third-party `.a` almost certainly would not link.**

Also required: `-C link-arg=-lc++ -lc++abi`, because rustc drives the link through
`emcc` rather than `em++` and does not add the C++ runtime itself.

### PDFium under option 2 — assessed, not attempted

Per the session's decision, PDFium's from-source wasm build was not attempted. What the
evidence says about its cost:

- No prebuilt wasm `.a` exists (Finding 1), so it is `gn` + `ninja` + depot_tools.
- pdfium-binaries' own wasm build needs patches to Chromium's `BUILDCONFIG.gn` and
  `//build/toolchain/wasm` just to make `target_os = "emscripten"` work at all — it is
  not a supported upstream configuration.
- It would have to be rebuilt with `-fwasm-exceptions` to match Rust, which pdfium's
  gn build does not expose as a switch.
- depot_tools sync is ~15–20 GB before anything compiles.

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

# Roadmap

Milestones are sequential. Each one ships something usable before the next begins; a
milestone is not done until CI is green and its
[definition of done](../CLAUDE.md#definition-of-done) is met for every task in it.

Dates are deliberately absent. The order is the commitment.

| Milestone | Scope | State |
|---|---|---|
| [M0](#m0--project-setup) | Project setup | **complete** |
| [M1](#m1--core-operations-and-the-web-app) | Merge, split, rotate, reorder, compress — core + web | in progress |
| [M2](#m2--redaction-with-verification) | Redaction with verification | not started |
| [M3](#m3--android) | Android app | not started |
| [M4](#m4--ios) | iOS app | not started |
| [M5](#m5--office-to-pdf) | Office → PDF | not started |
| [M6](#m6--pdf-to-docx) | PDF → DOCX | not started |

---

## M0 — project setup

Repository scaffolding, engineering rules, and enforcement. No product logic.

- [x] Rust workspace with `burrow-types`, `burrow-engines`, `burrow-ops`, `burrow-core`
- [x] Binding crate scaffolds (`burrow-ffi`, `burrow-wasm`) with FFI panic guard
- [x] `Limits` and the typed `Error` taxonomy
- [x] Pinned toolchain, workspace lints denying panicking constructs in library code
- [x] `deny.toml` with the permissive-only allowlist, checked across all shipped targets
- [x] CI: fmt, clippy `-D warnings`, tests, cargo-deny, cargo-audit, wasm build, web build
- [x] Dual license, README with honest status, CONTRIBUTING, SECURITY, Code of Conduct
- [x] ADRs 0001–0005
- [x] Astro + Svelte web scaffold
- [x] Corpus manifest and headless tool stubs
- [x] Verify CI green on GitHub — all 8 checks pass ([#1](https://github.com/TensorGreed/burrow/pull/1))

**M0 is complete.** SBOM generation and signed releases moved to M1 as a pre-ship task:
a meaningful SBOM has to cover the native engines, and those do not exist until M1
vendors them.

---

## M1 — core operations and the web app

The first usable product: five PDF operations, in the core and on the web.

Each operation is one vertical slice and is **not** done until every layer below is
present. This is the `add-operation` checklist; it is not optional per-operation.

### Foundations — PR sequence

Agreed sequence. Each PR is squash-merged with CI green before the next starts.

| PR | Scope | Items |
|---|---|---|
| **1** | **Engine acquisition** — pinned fetch, native (linux-aarch64 + linux-x86_64) and wasm builds, licence manifest, crypto assertions, CI caching, proof of linkage | 1–3 |
| **2** ✅ | `DocumentEngine` trait + **native PDFium** implementation: injected clock, `Limits` enforcement, typed error mapping, `page_count` as the thinnest end-to-end slice with all four test kinds, document-open fuzz target | 4, 5, 8 |
| **3** | **qpdf native** behind its own trait, logging suppression, and the "a secret never reaches the console or an error" test | 6, 7 |
| **4** | **Web path**: wasm binding + worker harness, `wasmBinary`, `-sENVIRONMENT=web,worker`, CSP `connect-src 'none'`, worker recovery per ADR 0009, wasm size budget, and the **differential conformance harness** | 9–12 |
| **5+** | Operations, one at a time, starting with `merge` | — |

The linking strategy is **settled** by [spike 0001](spikes/0001-wasm-engines.md):
option 1, Emscripten engine modules bridged through JS, with Rust on
`wasm32-unknown-unknown`. See [ADR 0006](adr/0006-wasm-linking-strategy.md) for the
decision and its seven requirements, and [ADR 0004](adr/0004-native-engines.md) for
acquisition. The spike is not production code and nothing from it is reused directly.

1. **Engine acquisition, per ADR 0004.** Both **native** (linux-aarch64 and
   linux-x86_64) and **wasm** — `cargo test`, the fuzz targets and the corpus runs are
   all native, so wasm alone is not enough. Prebuilt PDFium `chromium/8044` for all three
   targets with trust-on-first-use hashes (upstream publishes no signature), and the
   **extracted `libpdfium.so` pinned separately** from its tarball; qpdf 12.4.1 from
   source, verified against upstream's PGP-signed checksum; zlib and libjpeg-turbo
   **vendored with our own checksums** for both native and wasm, not taken from
   Emscripten ports; emsdk pinned. Native linkage is dynamic with a pinned `DT_RPATH` —
   see ADR 0004.
   *Testable:* the fetch script fails closed on any checksum mismatch; `build.rs`
   re-verifies the library before linking it; a test reads `/proc/self/maps` and asserts
   the loaded PDFium is the pinned file; CI builds every engine from the pinned manifest
   alone, on both architectures.
2. **Engine licence manifest wired into the build.** Extend
   [`engines/licenses.toml`](../engines/licenses.toml) as each engine is vendored, and
   keep `tools/check-engine-licences.py` green. Add the two mandatory credit lines
   (FreeType FTL §2, IJG condition 2) to `THIRD_PARTY_NOTICES.md` **and** a website
   credits page, per [ADR 0008](adr/0008-widened-licence-allowlist.md).
   *Testable:* CI fails if a component is declared with a licence outside the allowlist;
   a test asserts both credit lines are present in the built site.
3. **qpdf crypto flags asserted in CI.** `USE_IMPLICIT_CRYPTO=OFF`,
   `REQUIRE_CRYPTO_NATIVE=ON`. The upstream default would link GnuTLS (LGPL-2.1+).
   *Testable:* CI fails if the crypto summary changes or `QPDFCrypto_gnutls` appears in
   the link.
4. ~~**`DocumentEngine` implementation over PDFium**~~ — **done, PR 2.** In `burrow-engines`, with every C
   error code mapped to a typed `Error` and no raw code escaping the crate. No sentinel
   that can alias success — `-FPDF_GetLastError()` yields `-0`, and `-0 === 0` in JS — and
   **no error classification by string matching**; use engine codes, never engine prose.
   *Testable:* every PDFium error path has a test asserting the mapped variant, including
   one that a zero error code cannot read as success.
5. ~~**Open and parse with limits enforced**~~ — **done, PR 2**, per
   [ADR 0007](adr/0007-limit-enforcement-per-platform.md): an injected clock rather than
   `Instant::now()`, which panics on wasm; WASM maximum memory as the web ceiling;
   estimate-based pre-checks on native. Reject oversized input **before** allocating, and
   take `size_t`, not `int` — a signed length turns a >2 GiB input into a huge
   out-of-bounds read.
   *Testable:* a 20k-page document and a 2 GB file both return `LimitExceeded`, not a
   crash or an OOM kill; timeout behaviour is tested with a fake clock, not by waiting.
6. **qpdf integration for structure, encryption, object streams, and linearisation.**
   Note the correction in ADR 0004: PDFium reconstructs a corrupted xref by itself, so
   "repair" is **not** currently a justification for qpdf. If it is to be claimed, it
   needs a corpus case PDFium actually fails.
   *Testable:* a corpus of structurally damaged files; each either succeeds or returns
   `Malformed`, and no case aborts.
7. **Engine logging suppressed at both layers.** qpdf's default logger emits object
   numbers and byte offsets to stderr — reaching the devtools console — *including for
   files that parse successfully*. `setSuppressWarnings(true)` plus a discarding
   `QPDFLogger`, and `printErr`/`print` stubbed on every module.
   *Testable:* a test opens a file containing a recognisable secret and asserts nothing
   from it reaches console or any error string.
8. ~~**Fuzz target for document open**~~ — **done, PR 2.** The first parser entry point. Note that
   `max_duration_ms` is checkpoint-based and cannot catch a hang, so libFuzzer's own
   `-timeout` is required.
   *Testable:* 10 minutes clean on CI, longer on the regression machine.

**What PR 2 settled, beyond the three items.** Recorded here because two of them change
what later PRs can assume:

- **PDFium is serialised on one dedicated engine thread**, not behind a lock —
  [ADR 0011](adr/0011-pdfium-engine-thread.md). Document handles are ids, not pointers, so
  they are `Send + Sync` with no `unsafe impl` anywhere. Throughput for engine work is
  capped at one core on native, permanently; a thread pool is the escape hatch if that
  ever binds.
- **The `DocumentEngine` trait is handle-based and takes its input by value**, so PR 4 can
  implement it over the JS bridge where nothing can be borrowed. It also requires every
  bridge call to return its value *and* the engine error code in one round trip, because
  `FPDF_GetLastError` is global.
- **`tests/conformance/` holds the shared corpus** — generated fixtures plus
  `expectations.json`, the typed outcome each must produce. Item 12's differential harness
  reads that same file rather than restating the expectations in TypeScript.
- **The clang 18 ASan interop question carried over from PR 1 is resolved: it works.** The
  instrumented `lib/fuzz/libqpdf.a` links and runs under cargo-fuzz's Rust ASan (matching
  `__asan_version_mismatch_check_v8`), with coverage feedback from qpdf's own code.
  Measured on aarch64 only; `fuzz/README.md` has the method and the flags, and item 6's
  qpdf fuzz target should use them.
- **Bindings are deferred, explicitly.** `burrow-types` gained `Clock`, `SystemClock`,
  `ManualClock`, `Deadline`, `Password` and `Limits::with`, and `burrow-engines` gained
  `DocumentEngine`/`OpenOptions`/`pdfium`. None is wired into `burrow-ffi` or
  `burrow-wasm`, and none needs to be yet: `burrow-core`'s surface is unchanged, so no
  binding is stale today, and `DocumentEngine` is an engine seam that `burrow-ops` has no
  operation to expose through. PR 4 supplies the web `Clock` over `performance.now()`;
  uniffi exposure waits for M3. When it comes, **`Password` must cross uniffi as
  `bytes`/`ByteArray`, never `String`** — the type exists precisely because a lossy
  conversion changes the password — and `Deadline`/`ManualClock` should stay internal.
- **Still missing, and known:**
  - No fixture covers *"encrypted, and the password worked"*. That needs an encryptor, so
    it belongs to item 6 with qpdf.
  - **`max_memory_bytes` has no pre-emptive defence against a declared-size bomb.** The
    size-based pre-check is blind to what a file declares: `tests/conformance/fixtures/xref-bomb.pdf`
    is 330 KB, declares a twenty-million-entry cross-reference stream, and drives PDFium to
    allocate ~1.2 GB. A measured post-open check now turns that into `LimitExceeded`
    instead of `Ok`, but the allocation has already happened, and on a constrained device
    the engine can `abort()` first — which is not a panic and cannot be caught. **The fix
    is a structural pre-scan of the declared sizes before the load, and it belongs to
    item 6**, because it needs a parser that is not PDFium. ADR 0011 records the reasoning.
  - **One slow document delays every other caller**, and the victim is told its own
    deadline expired. Measured at 716 ms against a 1 ms budget. A bounded wait and a
    registry-level cap on open documents are both recorded in ADR 0011 and neither is in
    PR 2.
9. **Web binding surface and Web Worker harness**, satisfying ADR 0006's requirements 1
   and 3: one engine instance per worker, the init **promise** memoised (not the result),
   `Module.wasmBinary` supplied, built `-sENVIRONMENT=web,worker`, and shipped with
   `connect-src 'none'` so non-negotiable #1 is enforced by the browser rather than by
   re-auditing minified third-party JS.
   *Testable:* a Playwright test drives a real file through a worker; another asserts the
   CSP blocks any outbound connection.
10. **Worker recovery, per [ADR 0009](adr/0009-web-panic-contract-and-binding-boundary.md).**
    A panic is a catchable trap and the worker *survives* — so recovery is the page's
    decision. Any `Internal` result or wasm exception discards the instance and spawns a
    fresh worker; `Malformed`/`LimitExceeded`/`PasswordRequired` must not cost a worker.
    *Testable:* a test panics deliberately, asserts the instance is discarded, and asserts
    the next operation succeeds on a fresh worker.
11. **wasm size budget in CI.** Record the module size and fail on an unexplained
    regression. The spike measured 6.50 MB raw / 2.20 MB brotli for the full option 1
    payload, 82% of it PDFium — that is the starting point, not a target.
12. **Differential conformance between the two `DocumentEngine` implementations.** The
    native path and the web path are separate implementations of one trait, and almost
    every test exercises only the native one — the web path is otherwise covered just by
    Playwright. So run the **same corpus through both** — native via `cargo test`, web via
    headless Chromium — and assert **identical typed outcomes**. Divergence fails CI.
    *Testable:* a corpus file that returns `Malformed` natively must return `Malformed` on
    the web, and a page count must match exactly. This matters most at M2, where both
    paths have to reach the same redaction verdict; a divergence discovered then would be
    a redaction bug, not a test failure.

### Operations

Each of `merge`, `split`, `rotate`, `reorder`, `compress` ships with:

- core implementation in `burrow-ops`, taking and enforcing `Limits`
- typed error variants for its own failure modes
- **unit tests** for the happy path and each error variant
- **property tests** (proptest) — the invariants below
- **golden-file tests** against committed expected output
- a **fuzz target** for any new parser entry point
- a wasm binding and an Astro tool page at its own indexable URL
- rustdoc, plus a `THIRD_PARTY_NOTICES.md` entry if it added a dependency

Per-operation invariants worth stating up front, because they are what the property tests
assert:

| Operation | Invariant |
|---|---|
| `merge` | Output page count equals the sum of inputs; page order is preserved; merging one document is the identity |
| `split` | Splitting then merging round-trips to the original page sequence; every input page appears exactly once across outputs |
| `rotate` | Four 90° rotations return to the original; rotation is recorded, not re-rasterised |
| `reorder` | Output is a permutation of the input — no page lost, added, or duplicated; the identity permutation is a no-op |
| `compress` | Output is never larger than the input; page count and page dimensions are unchanged; text remains extractable |

### Web app

- One indexable page per tool: title, description, structured data, no client-side routing
- Drag-and-drop, page thumbnails, drag-to-reorder, progress, cancel
- Strict Content-Security-Policy; no third-party scripts, fonts, or analytics on any page
  that touches file content
- Playwright tests for each tool, plus one asserting **no network request carries file
  content**
- Works with JavaScript disabled to the extent of explaining what the page does

### Corpus tooling

- Implement `tools/corpus-fetch.sh`, `corpus-run.sh`, `visual-diff.sh` (currently stubs)
- Register the first corpora in `corpus/manifest.toml` with licenses and checksums
- A headless regression run on the self-hosted `linux-arm64` machine

### Before M1 ships

Moved here from M0, because both depend on the native engines existing.

- **SBOM generation.** A CycloneDX SBOM covering Rust crates *and* the vendored C/C++
  engines. `cargo-cyclonedx` handles the former; the latter needs the engine manifest
  this milestone introduces. An SBOM that omits PDFium would be worse than none, because
  it would look complete.
- **Signed releases.** cosign keyless (OIDC) signing of artifacts and the SBOM, so
  provenance is verifiable without us holding a key.
- Enable `.github/workflows/release.yml`, which is currently a stub that refuses to run.

---

## M2 — redaction with verification

The highest-risk feature in the project. A bug here leaks the secret the user was
removing, so the verifier is part of the feature, not a test of it.

### Gate: re-evaluate option 2 before M2 starts

**This is a gate, not a maybe.** [ADR 0006](adr/0006-wasm-linking-strategy.md) chose
option 1 for M1, and redaction is precisely where option 1's two weaknesses bite hardest:
the shared-glue-globals hazard (a parse against the wrong linear memory returns
confidently wrong data) and trap-versus-unwind semantics. A redaction pass that inspects
one heap and edits another is the failure mode this milestone exists to prevent.

Option 2's measured advantages: `catch_unwind` **works** on `wasm32-unknown-emscripten`,
so ADR 0002's guard rule holds on the web — it cannot under option 1. One heap, so no
cross-heap copy. Slightly smaller.

Its cost is a from-source PDFium `gn`/`ninja` build, and the **open question for this
gate** is whether that build works on a `linux-aarch64` host or has to run on x86-64 CI
runners. The dev and corpus machine is aarch64, so the answer determines whether the build
is reproducible where the regression runs happen.

Decide, record the outcome in an ADR, and only then start redaction.

- Redaction of text, images, annotations, and vector content by region
- **Removal, not concealment** — a black rectangle over text is not redaction
- Scrub every place content hides: text layer, embedded and subsetted fonts, image data,
  annotations, form fields, attachments, metadata, and incremental-update history
- **Automatic post-run verification** on every redaction: re-open the output and assert
  the redacted content is absent by every extraction path we know of. The operation fails
  closed if verification fails; it never returns unverified output.
- HarfBuzz `hb-subset` integration so subsetted fonts cannot retain redacted glyphs
- A `redaction` corpus set with secrets at known locations, plus adversarial cases:
  overlapping glyphs, rotated text, text in XObjects, text as outlines
- Property test: for any region and document, no extraction path yields the redacted bytes

---

## M3 — Android

Native Jetpack Compose app. No WebView.

- uniffi bindings wired up; core cross-compiled for `arm64-v8a` and `armeabi-v7a`
- Compose UI covering the M1 and M2 operations
- Storage Access Framework integration; no file copied outside the app sandbox
- ML Kit for on-device text recognition, used from the app layer only
- Media3 for video
- Instrumented tests on device; a test asserting the app makes no network request while
  processing
- Prerequisites: Android SDK, NDK, JDK 17+ (none present on the current dev machine)

---

## M4 — iOS

Native SwiftUI app. No WebView. Requires macOS and Xcode.

- uniffi Swift bindings; core cross-compiled for `aarch64-apple-ios` and the simulator
- SwiftUI UI covering the M1 and M2 operations
- PDFKit for preview, Vision for on-device text recognition, AVFoundation for video —
  app layer only
- Share sheet and Files integration
- XCUITest coverage, plus a test asserting no network request during processing

---

## M5 — Office to PDF

DOCX, XLSX, PPTX → PDF, on-device.

- Evaluate what is achievable within [ADR 0008](adr/0008-widened-licence-allowlist.md), using
  what each platform already provides rather than writing a layout engine:
  **iOS system rendering**; **docx-preview (Apache-2.0)** on web and Android;
  **LibreOffice (MPL-2.0, on our allowlist)** evaluated as a high-fidelity option. Record
  the outcome in an ADR — the three routes will not agree on output, and that divergence
  is the decision to make explicitly
- Start with DOCX text and paragraph layout; be explicit about unsupported constructs
  rather than producing wrong output silently
- Font handling and substitution, with an OFL-licensed fallback family
- Golden-file corpus; a documented and honest statement of fidelity limits

---

## M6 — PDF to DOCX

The hardest milestone: recovering structure from a format that discards it.

- Layout analysis — reading order, columns, tables, lists
- Text and style recovery, image extraction
- Explicit fidelity limits; never claim round-trip fidelity we cannot deliver
- Golden-file corpus with human-reviewed expected output

---

## Not planned

Recorded so the answer is written down rather than re-litigated:

- **Server-side processing.** Ever. It would break the one guarantee the project makes.
- **Accounts, sync, or cloud storage.** Nothing to store, nowhere to store it.
- **Telemetry on file content.** Not sampled, not hashed, not aggregated.
- **A paid tier or commercial license.** See the README.
- **WebView wrappers for the mobile apps.** See [ADR 0002](adr/0002-rust-core-and-bindings.md).

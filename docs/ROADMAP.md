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

### Foundations

The linking strategy is **settled** by [spike 0001](spikes/0001-wasm-engines.md):
option 1, Emscripten engine modules bridged through JS, with Rust on
`wasm32-unknown-unknown`. See [ADR 0006](adr/0006-wasm-linking-strategy.md) for the
decision and its seven requirements, and [ADR 0004](adr/0004-native-engines.md) for
acquisition. The spike is not production code and nothing from it is reused directly.

1. **Engine acquisition, per ADR 0004.** Prebuilt PDFium `chromium/8044` with its
   trust-on-first-use hash recorded (upstream publishes no signature); qpdf 12.4.1 from
   source, verified against upstream's PGP-signed checksum; zlib and libjpeg **vendored
   with our own checksums**, not taken from Emscripten ports; emsdk pinned and verified.
   *Testable:* a fetch script fails closed on any checksum mismatch, and CI builds all
   engines from the pinned manifest alone.
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
4. **`DocumentEngine` implementation over PDFium** in `burrow-engines`, with every C
   error code mapped to a typed `Error` and no raw code escaping the crate. No sentinel
   that can alias success — `-FPDF_GetLastError()` yields `-0`, and `-0 === 0` in JS — and
   **no error classification by string matching**; use engine codes, never engine prose.
   *Testable:* every PDFium error path has a test asserting the mapped variant, including
   one that a zero error code cannot read as success.
5. **Open and parse with limits enforced**, per
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
8. **Fuzz target for document open** — the first parser entry point. Note that
   `max_duration_ms` is checkpoint-based and cannot catch a hang, so libFuzzer's own
   `-timeout` is required.
   *Testable:* 10 minutes clean on CI, longer on the regression machine.
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

- Evaluate what is achievable within [ADR 0003](adr/0003-permissive-licensing.md), using
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

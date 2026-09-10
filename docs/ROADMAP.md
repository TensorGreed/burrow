# Roadmap

Milestones are sequential. Each one ships something usable before the next begins; a
milestone is not done until CI is green and its
[definition of done](../CLAUDE.md#definition-of-done) is met for every task in it.

Dates are deliberately absent. The order is the commitment.

| Milestone | Scope | State |
|---|---|---|
| [M0](#m0--project-setup) | Project setup | in progress |
| [M1](#m1--core-operations-and-the-web-app) | Merge, split, rotate, reorder, compress — core + web | not started |
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
- [ ] SBOM generation and signed releases (stubbed in `release.yml`, wired up before M1 ships)

---

## M1 — core operations and the web app

The first usable product: five PDF operations, in the core and on the web.

Each operation is one vertical slice and is **not** done until every layer below is
present. This is the `add-operation` checklist; it is not optional per-operation.

### Foundations

1. **Engine acquisition for PDFium** — resolve [ADR 0004](adr/0004-native-engines.md)'s
   open question. Pinned version and checksum; builds for `linux-arm64`, `wasm32`,
   `x86_64-linux`. Provenance recorded in `THIRD_PARTY_NOTICES.md`.
   *Testable:* CI links PDFium and calls one function on all three targets.
2. **`DocumentEngine` implementation over PDFium** in `burrow-engines`, with every C
   error code mapped to a typed `Error` variant and no raw code escaping the crate.
   *Testable:* every PDFium error path has a test asserting the mapped variant.
3. **Open and parse with limits enforced** — `max_input_bytes`, `max_pages`,
   `max_duration_ms`, `max_memory_bytes`.
   *Testable:* a 20k-page document and a 2 GB file both return `LimitExceeded`, not a
   crash or an OOM kill.
4. **qpdf integration for repair** — a damaged file that PDFium rejects is repaired by
   qpdf and retried.
   *Testable:* a corpus of truncated and damaged files; each either succeeds after repair
   or returns `Malformed`.
5. **Fuzz target for document open** — the first parser entry point.
   *Testable:* 10 minutes clean on CI, longer on the regression machine.
6. **wasm-bindgen surface and Web Worker harness** — operations callable from the browser
   with progress reporting, off the main thread.
   *Testable:* a Playwright test drives a real file through a worker.
7. **wasm size budget in CI** — record the module size and fail on an unexplained
   regression.

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

---

## M2 — redaction with verification

The highest-risk feature in the project. A bug here leaks the secret the user was
removing, so the verifier is part of the feature, not a test of it.

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

DOCX, XLSX, PPTX → PDF, on-device, with no permissively licensed converter available to
lean on.

- Evaluate what is achievable within [ADR 0003](adr/0003-permissive-licensing.md);
  LibreOffice is LGPL/MPL and not embeddable under our constraints, so expect to
  implement layout ourselves
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

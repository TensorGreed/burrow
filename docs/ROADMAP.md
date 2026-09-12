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
| **3** ✅ | **qpdf native** behind its own trait, logging suppression, and the "a secret never reaches the console or an error" test | 6, 7 |
| **4a-i** ✅ | **Web path, part 1**: the real qpdf Emscripten module, the `DocumentEngine`/`StructureEngine` web implementations over a bridge trait seam, the wasm binding, the classic worker, the generated CSP, and the engines loading end to end in a browser | 9 (part) |
| **4a-ii** ✅ | Worker recovery per ADR 0009 as an explicit state machine, the main-thread watchdog (clock starting at the worker's ack), a crash-counting circuit breaker, heap-growth recycling with a measured threshold, and the console-silence and zero-requests-after-init tests. [ADR 0015](adr/0015-web-worker-lifecycle.md) | 10 |
| **4b** ✅ | The **differential conformance harness** and the Chromium/Firefox/WebKit matrix. `Stage` on `LimitExceeded`, expectations schema 2, the adversarial corpus, and two measured findings. [ADR 0016](adr/0016-differential-conformance.md) | 12 |
| **4c** ✅ | **Closes foundations.** The `max_memory_bytes` claims corrected everywhere in one pass (#25); the credits page, generated from the licence manifest and reachable from the footer (#16, ADR 0008's second of four surfaces); the first-load size budget, on the total rather than per file; qpdf's `trap_errors` set generated from source and checked against both bindings (ADR 0013 §1); and a ceiling, a milestone and an issue on every `known_gap` | 2, 11 |
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
6. ~~**qpdf integration for structure, encryption, object streams, and linearisation.**~~
   — **done, PR 3.** Behind its own `StructureEngine` trait, through qpdf's **C API only**:
   it catches every C++ exception internally (`qpdf-c.h:113-115`), so no shim was needed
   and no C++ was added. See [ADR 0013](adr/0013-qpdf-c-api-and-prescan.md).
   **The repair question is answered.** ADR 0004's claim, withdrawn by spike 0001, is
   re-established: qpdf reads a 290-byte truncated file (1 page) and a trailer-less one
   (3 pages) that PDFium refuses outright. Pinned by
   `tests/structure.rs::qpdf_recovers_two_files_pdfium_refuses`.
   *Testable:* a corpus of structurally damaged files; each either succeeds or returns
   `Malformed`, and no case aborts.
7. ~~**Engine logging suppressed at both layers.**~~ — **native half done, PR 3.**
   `qpdf_silence_errors` + `qpdf_set_suppress_warnings(true)` + a logger with info, warn
   and error all set to `qpdf_log_dest_discard` — qpdf's own `Pl_Discard`, reachable by
   name from C, so no callback and no Rust code on a C++ stack.
   Confirmed on this build: with the suppression removed qpdf writes
   `WARNING: input (offset 4242): xref not found` and
   `object 3 0 at offset 131` to stderr. `tests/secret_leak.rs` **fails** when it is
   removed, which is what makes the claim checkable rather than asserted.
   ~~*Still open:* `printErr`/`print` stubbing on the wasm modules is PR 4's half.~~ —
   **done, PR 4a-i:** both modules are constructed with `print`/`printErr` as no-ops, and
   qpdf's C++-layer suppression runs on the web path too, from the same Rust. The
   ~~thorough console-silence test — every failure path, canary fixtures, worker consoles as
   well as the page's, and a control that deliberately logs — is PR 4a-ii's.~~ — **done, PR
   4a-ii:** `apps/web/e2e/console-silence.spec.ts`, in all three browsers, over the same canary
   fixture the native test uses (generated by
   `cargo run -p burrow-engines --example make-canary-pdf`, so there is still one generator).
   A mutation sweep found the two layers are **each sufficient on their own** — removing either
   alone leaves the console silent, removing both produces twelve lines of object numbers and
   byte offsets. So "both layers" is genuine defence in depth, and the test cannot say which one
   regressed. [ADR 0015](adr/0015-web-worker-lifecycle.md) §9.
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
**What PR 3 settled, beyond items 6 and 7.**

- **The structural pre-scan closes PR 2's declared-size gap.** Bounded pure Rust,
  `forbid(unsafe_code)`, O(1) allocation, no recursion — deliberately **not** driven by
  qpdf, because a pre-scan running a C++ parser on ungated input would become the new
  attack surface. The xref bomb is now refused before any engine allocates: measured, the
  rejection grows the resident set by under 64 MB where it previously grew by ~1,250 MB.
  Compiled on every platform, so M1 PR 4's web path uses it unchanged.
- **The per-entry cost constant is 64 bytes, and it is not `sum(/W)`.** `/W` is the file's
  encoding width; a cross-reference stream compresses, so 20M entries are 330 KB on disk
  and 1.2 GB in memory. A first attempt compared `entries × sum(W)` (340 MB) against the
  1 GiB ceiling and let the bomb through.
- **qpdf runs on the caller's thread**, not PDFium's engine thread — ADR 0013. Its global
  limits (all the decompression ones default to *unlimited*) and its discarding logger are
  set once behind a `OnceLock`.
- **qpdf's global limits cannot be per-operation.** They take no `qpdf_data`, so a caller's
  `Limits` are applied in Rust and qpdf's globals are a fixed floor under everything.
- **`fuzz/libqpdf.a` is now in `BUILD_MANIFEST.sha256`.** It was linked into fuzz binaries
  and checksummed by nothing.
- **Two findings from security review, both fixed, both guarded.**
  - **`qpdf_is_linearized` aborts the process** on an ordinary 356-byte PDF containing an
    object number above `INT_MAX`. Only qpdf C functions routed through `trap_errors` catch
    C++ exceptions — the header's blanket guarantee is not true per-function — and a
    foreign exception is not a Rust panic, so nothing catches it. `StructureReport` now
    reports only `pages`; `encrypted` and `linearized` are gone, because obtaining them
    safely needs the C++ shim ADR 0013 argues against. The file that aborted is in the
    damaged corpus, so reintroducing an untrapped call aborts the suite.
  - **Three one-line bypasses of the pre-scan**, each restoring the full 2.5 GB
    allocation: a `/Sizes 1 ` decoy before `/Size`, padding it past the dictionary window,
    and junk after `%%EOF` pushing `startxref` past the tail window. All shared one shape —
    make the scan read *nothing*, because "nothing declared" was indistinguishable from
    "nothing to declare". Fixed with delimiter-checked keys, largest-of-all-occurrences,
    and a whole-buffer backstop that runs only when the directed walk comes up empty.
- **Bindings deferred, explicitly.** `StructureEngine`, `CheckOptions`, `StructureReport`,
  `qpdf` and `prescan` are public in `burrow-engines`, which the bindings never depend on —
  `burrow-core`'s surface is unchanged, so nothing is stale. The engine traits stay internal
  until PR 5's first operation gives `burrow-ops` something to expose.
- **Two new fuzz targets.** `prescan` is the only one in the project where coverage-guided
  fuzzing actually steers the code under test — everything else drives a prebuilt black
  box. `qpdf_check` links the **instrumented** qpdf archive, so it does get feedback from
  an engine's own parser; CI runs it on x86-64, which is the open half of PR 2's ASan
  finding.

- **Deferred, and recorded here rather than rediscovered:**
  - **A hang inside an engine cannot be interrupted from inside the process.** Time limits
    are checkpoint-based ([ADR 0007](adr/0007-limit-enforcement-per-platform.md)) and qpdf
    offers no timeout, cancellation or abort hook; PDFium offers none either. A single
    engine call that runs forever is not stopped by anything we have.
    - **Web:** recovery means terminating the worker, which PR 4 builds anyway for
      [ADR 0009](adr/0009-web-panic-contract-and-binding-boundary.md)'s panic contract.
    - **Android (M3):** a separate service process is available and is the obvious route.
    - **iOS (M4):** **no subprocesses are permitted at all**, so process isolation cannot
      be the uniform answer and the two platforms must decide separately.

    This is the same constraint that makes an engine out-of-memory unsurvivable
    (ADR 0011), and it should be settled once for both.
  - **Queue-time attribution.** Carried from PR 2 and still open: one slow document delays
    every other caller, and the delayed caller is told **its own** `max_duration_ms`
    expired. Measured at 716 ms against a 1 ms budget. The fix is a bounded wait that
    distinguishes "the queue was busy" from "your own work was slow", plus a
    registry-level cap on concurrently open documents.

- **Retracted after PR 2 merged:** PR 2 claimed `cargo-deny` cannot see dev-dependencies,
  and that non-negotiable #1's network ban therefore did not cover them. That was wrong —
  it came from `cargo deny list`, which omits them from its output, while `cargo deny
  check` includes them. Corrected in [ADR 0012](adr/0012-ncsa-for-libfuzzer.md) and at the
  top of `deny.toml`, and `tools/check-no-network-deps.sh` now checks the property
  independently of cargo-deny either way.
9. ~~**Web binding surface and Web Worker harness**~~ — **done, PR 4a-i.** One engine
   instance per worker with the init **promise** memoised (not the result); built
   `-sENVIRONMENT=web,worker`; a classic worker, because `pdfium.js` is not modularised and
   needs `importScripts` — which forces `wasm-pack --target no-modules`.
   **ADR 0006 requirement 3 was amended, and it had to be.** It asks for
   `Module.wasmBinary` *and* `connect-src 'none'`, which cancel out: supplying the bytes
   means fetching them, and fetch is what `connect-src` governs. The policy is narrowed
   instead — `default-src 'none'`, no cross-origin source anywhere, and `connect-src`
   naming the **exact content-hashed engine URLs** — with the bytes integrity-pinned via
   `fetch(url, { integrity })` and a streaming compile handed in through
   `instantiateWasm`, which is better than `wasmBinary` on both counts. See
   [ADR 0014](adr/0014-web-engine-loading-and-csp.md).
   **The guarantee is split, and both halves are needed**: no cross-origin request,
   enforced by the browser; no request at all after engine init, enforced by test, because
   CSP ignores query strings. The second half is item 10's PR.
   *Testable:* `apps/web/e2e/` — the engines load and answer under the generated CSP; every
   conformance case matches; cross-origin and non-engine same-origin fetches are blocked;
   a tampered digest fails the load; and one test deliberately *succeeds* at the
   query-string hole so nobody reads the others as a complete guarantee.
10. ~~**Worker recovery, per [ADR 0009](adr/0009-web-panic-contract-and-binding-boundary.md).**~~
    — **done, PR 4a-ii**, as an explicit state machine in `apps/web/src/host/worker-host.js`
    with every dependency injected, so all 25 transitions — including a result arriving after
    the watchdog fired, a crash during initialisation, and a request made while a respawn is in
    flight — are unit tests that run in 46 ms. [ADR 0015](adr/0015-web-worker-lifecycle.md).
    **The watchdog's clock starts at the worker's ack**, not at the page's `postMessage`, with
    engine start-up bounded separately — the web form of the queue-time bug PR 2 fixed
    natively. **The circuit breaker counts crashes, not respawns**: counting respawns meant
    three recycles took the page offline, found by measurement rather than by review.
    **Input is a `Blob`, never a transferred `ArrayBuffer`**, so the caller can still retry the
    same file after its worker is killed.
    *Tested:* `e2e/recovery.spec.ts` poisons a bridge global so a real exception comes back out
    of the wasm module, asserts the instance is discarded and the next operation succeeds on a
    fresh worker; six hostile files cost zero workers; a synchronous hang inside one engine
    call is interrupted.
11. ~~**wasm size budget in CI.**~~ — **done, PR 4c.** `apps/web/size-budget.json`, enforced
    by `apps/web/src/size-budget.test.ts` and reported as a step summary on every PR by
    `tools/report-size-budget.mjs`.

    **Not the module size, which is the wrong thing to budget.** What a user pays is the
    whole first-load payload — the page shell, the worker bundle, all three `.wasm` modules
    and the CSP control file — and budgeting those individually lets three files each grow
    4% while every per-file budget passes. The **total** is the gate (3% headroom, ~67 KB);
    the per-artifact lines (10%) say where it went. A test plants exactly that distributed
    regression against the recorded numbers and fails if the total's headroom is ever loose
    enough to miss it.

    Measured, not inherited: the spike's 6.50 MB raw / 2.20 MB brotli is recorded as the
    origin, but the shipped build has drifted above it — qpdf is now built from source
    against our vendored zlib and libjpeg-turbo (1,199,200 raw against 917,305), and the
    Rust module has grown from 15,266 to 43,072. Current first load is **6,816,078 raw /
    2,232,811 brotli**, 82% of it still PDFium, whose brotli size is unchanged from the
    spike to the byte.

    *The original entry, for reference:* Record the module size and fail on an unexplained
    regression. The spike measured 6.50 MB raw / 2.20 MB brotli for the full option 1
    payload, 82% of it PDFium — that is the starting point, not a target.
12. ~~**Differential conformance between the two `DocumentEngine` implementations.**~~ —
    **done, PR 4b.** [ADR 0016](adr/0016-differential-conformance.md). 21 cases × 2 engines ×
    3 browsers, diffed against the native record, in 22 seconds; the whole corpus runs on
    every PR. The corpus grew from 8 well-formed-or-damaged files to include every adversarial
    file this project has found — the xref bomb, the three pre-scan bypasses security review
    found in PR 3, the object number that used to abort the process, and the two files qpdf
    reads and PDFium refuses.
    **`Error::LimitExceeded` gained a `stage`**, because three separate checks produced an
    identical error and "the same kind reached by a different route" has to read as a
    divergence. **The comparator is a pure function** with planted-divergence unit tests, so
    the defences against a vacuous pass are exercised rather than declared.
    It found three things on its first runs: a pre-scan gap where a 1.4 MB file reaches
    2,437 MB and still returns `Ok` ([#24](https://github.com/TensorGreed/burrow/issues/24));
    that `max_memory_bytes` is **not** the hard ceiling on the web that ADR 0007 claimed
    ([#25](https://github.com/TensorGreed/burrow/issues/25)); and that the web's measured check
    is stronger than native's, which is now the first recorded `platform_expectations` entry.

    *The original entry, for reference:* The
    native path and the web path are separate implementations of one trait, and almost
    every test exercises only the native one — the web path is otherwise covered just by
    Playwright. So run the **same corpus through both** — native via `cargo test`, web via
    headless Chromium — and assert **identical typed outcomes**. Divergence fails CI.
    *Testable:* a corpus file that returns `Malformed` natively must return `Malformed` on
    the web, and a page count must match exactly. This matters most at M2, where both
    paths have to reach the same redaction verdict; a divergence discovered then would be
    a redaction bug, not a test failure.
    **Some differences are by design and must be recorded rather than skipped.** The native
    path cannot interrupt a hang (ADR 0007: checkpoint-based, overshoot is one engine call);
    the web's watchdog can (ADR 0015 §2). So a hang fixture hangs natively and returns
    `LimitExceeded` on the web. [ADR 0015 §10](adr/0015-web-worker-lifecycle.md) specifies the
    `platform_expectations` block — platform, expected typed outcome, and a **mandatory
    reason** — that 4b adds to `expectations.json`, so the harness asserts the recorded
    difference and fails both when a case diverges without a reason and when a recorded
    difference stops happening. A skipped test would record "untested", which is the wrong
    memory to leave for M2.

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

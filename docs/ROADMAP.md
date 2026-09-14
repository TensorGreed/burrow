# Roadmap

Milestones are sequential. Each one ships something usable before the next begins; a
milestone is not done until CI is green and its
[definition of done](../CLAUDE.md#definition-of-done) is met for every task in it.

Dates are deliberately absent. The order is the commitment.

## Ship blockers

Things that must be fixed before anything reaches a user, including a pre-alpha one. This list
is short on purpose: a blocker is not "important", it is "we do not ship with this open".

| | what | why it blocks |
|---|---|---|
| **#62** | Memory-unsafety in the pinned qpdf — two distinct defects | Blocks **M3/M4 only**. Natively it is a hard crash with no sandbox, and opening an attachment is the scenario. **Does not block the web**, and the earlier conditional block on `/merge-pdf` is withdrawn: measured on every path it is a fault natively and a hang on wasm that the watchdog converts into a typed error, with **no silent wrong output observed anywhere**. The argument that blocked merge — *not observed is not cannot happen* — applies to every operation, since the defect is reachable from `open`; used as a blocker criterion it blocks everything indefinitely. The answer to *cannot be excluded* is a detector, and that is ADR 0022. Accepted and recorded: a crafted file freezes an operation for the 60 s watchdog budget before failing. `docs/security/exposure-2026-09-14-qpdf-uaf.md`. |

The remaining row does not block further M1 development. It blocks **deployment**, which is the distinction
worth keeping: work continues, and a build does not go in front of a person until the row is
gone — or, for a conditional row, until its named discharge has landed.

### Discharged

Kept rather than deleted: a blocker that was cleared is evidence about how the bar is applied,
and the table above is only honest if it says what left it and why.

| | what | why it no longer blocks |
|---|---|---|
| **#61** | A damaged-but-openable document silently loses a page | **Silent data loss.** The output is a valid PDF that opens happily with a page missing, and nothing tells the user. It reproduces through `rotate`, which is merged, and through `split`'s build route. Pre-alpha does not make a silent wrong answer acceptable; it makes it harder to notice. **A refusal would not block. Losing the page quietly does.** **Characterised** (2026-09-14): not the write losing a page — qpdf reads the same document as 5 pages without reconstruction and 4 with it, and `open` reports the first while the writer emits the second. **DISCHARGED** (2026-09-14): ADR 0022's verification reads every operation's output back through a fresh engine, sees four pages where five were promised, and returns `OutputRejected`. `a_damaged_document_is_refused_rather_than_losing_a_page_silently` asserts it on every run; the engine-seam defect underneath stays pinned by an `#[ignore]`d reproduction. **The underlying disagreement is not fixed** — which of qpdf's two readings is right is a recovery-posture decision, still open on #61 and deliberately separate. |

**A discharge is a checkable thing, not a judgement call at deploy time.** #61's was #65 /
[ADR 0022](adr/0022-every-operation-verifies-its-own-output.md) — every operation verifying its
own output — and it landed: silent page loss is now a typed refusal, so the row moved to
*Discharged* below and the blockers table has one row left. What #61 keeps open is the posture question, which is not a
ship blocker: a refusal is a correct answer, a quietly short document is not.

**#62's row narrowed rather than gaining a discharge**, and the difference is worth keeping: a
discharge is a thing you build, and what happened there was that a measurement showed the
blocker had been drawn too wide. Correcting scope on evidence is not the same as fixing
something.


| Milestone | Scope | State |
|---|---|---|
| [M0](#m0--project-setup) | Project setup | **complete** |
| [M1](#m1--core-operations-and-the-web-app) | Merge, split, rotate, reorder, compress — core + web | in progress; `merge` ✅, `rotate` ✅, `reorder` ✅, `split` core ✅ (#54 closed), its bridge and page to come, `compress` is the last. #61 discharged by ADR 0022 — see *Ship blockers*. |
| [M2](#m2--redaction-with-verification) | Redaction with verification | not started |
| [M3](#m3--android) | Android app | not started; **gated on #62** — native has no wasm sandbox |
| [M4](#m4--ios) | iOS app | not started; **gated on #62**, as M3 |
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
| **A** ✅ | **The design system the five tool pages share**, established before any tool page existed: six reserved colour tokens with contrast asserted in both themes, one self-hosted OFL face with its provenance pinned, two layout tracks, and the banned-chrome list — written into `apps/web/CLAUDE.md` so the tool pages inherit it | — |
| **B** ✅ | **`merge` in the core**, on the engine a measurement chose rather than a guess. [ADR 0017](adr/0017-merge-engine-and-failure-semantics.md): qpdf, all-or-nothing on any failing input, and limits per input *and* on the total | — |
| **B2** ✅ | **`merge` across the bridge**: the write path on the web engine, a reply that carries bytes, the multi-input worker protocol, seven new qpdf wasm exports and the rebuild, and conformance **schema 3** — operations as a map, cases with an ordered input list | — |
| **B3** ✅ | **`/merge-pdf`**, the first tool page: pick, reorder, remove, merge, download, cancel; every typed error as a sentence for a person; and the Playwright matrix, with console-silence and zero-requests asserted against the page rather than only the harness | — |
| **5+** | The remaining operations, one at a time | — |

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
3. ~~**qpdf crypto flags asserted in CI.**~~ — **done.** `USE_IMPLICIT_CRYPTO=OFF`,
   `REQUIRE_CRYPTO_NATIVE=ON`, set in both build scripts since PR 1. The upstream default
   would link GnuTLS (LGPL-2.1+).

   **Both halves of the testable now exist, and the second was missing until it was
   checked.** The build scripts grep qpdf's CMake configure summary for
   `GNU TLS crypto enabled: OFF` — that asserts what CMake *decided*.
   `tools/check-qpdf-crypto.sh` asserts what reached the **archive**: `QPDFCrypto_native`
   defined, no `QPDFCrypto_gnutls` or `QPDFCrypto_openssl`, and no TLS library symbols
   under any name. Native archives are checked in the `test` job, the wasm one in `web`.

   The gap was not the symbol check alone. The configure grep lives inside a build step
   gated on `if: cache-hit != 'true'`, **so on an ordinary PR with a warm engine cache the
   crypto assertion did not run at all**. The new step is deliberately ungated: it reads the
   artifact, so a restored cache is still examined. Same reasoning as the `test` job's
   "re-verify engine checksums rather than trusting the cache".

   *Testable:* CI fails if the crypto summary changes **when the engines are rebuilt**, or —
   **on every run, warm cache included** — if any crypto provider other than native reaches a
   built artifact. The two halves have different coverage and the qualifier is the point:
   writing this sentence unqualified is how the gap above survived being written down twice.
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

**`split`'s hold is lifted in the core and the operation has not shipped yet.** Those are two
different statements and keeping them apart is the point of this paragraph.

[ADR 0019](adr/0019-how-split-builds-its-outputs.md) §2 states a property that applies to any
operation whose output is a **subset** of its input: the emitted bytes may carry nothing derived
from the excluded content. `split` did not meet it — six measured channels, issue #54 — and
shipping it then would have meant shipping a tool that puts data from pages you did not include
into files you did.

**Issue #54 closed it on the native path.** `core/burrow-engines/src/qpdf/prune.rs` takes back out
what `qpdf_add_page`'s reachability closure drags in, and both gates are green with their controls:
`subset_closure.rs`'s structural property over every object in the source, and `split_no_leak.rs`'s
named channels on top. ADR 0019's 2026-09-14 amendment records which answer each channel took, two
defects the harness caught inside the fix, and the cost. A document that uses layers is **refused**
rather than split, for a reason that amendment gives.

**What remains before `/split-pdf` exists** is the rest of the vertical slice, not the rule:
ADR 0022 verification for split, an `impl PageExtractor for WebQpdf` with the exports and the
size-budget re-measure it implies, and the page. `apps/web/src/production-build.test.ts` keeps the
route and the operation name out of the shipped bundle until then, which is a check rather than an
intention.

**`rotate`, `reorder` and `compress` are not subsetting operations.** Every input page appears
in the output of each: rotate changes a page's `/Rotate`, reorder permutes the page tree,
compress re-encodes. Nothing is excluded, so there is no excluded content for an output to carry
and the property is vacuous for them. They are not waiting on the pruning work, and M1 is not
blocked on it.

They do inherit the *harness*, and they call a **different entry point on it**:
`assert_nothing_lost`, which names the objects that must be there, rather than `assert_closed`,
which asks whether anything trespassed. That distinction is not pedantry — `assert_closed` with
every page included is mathematically vacuous, and an earlier draft of this paragraph claimed
the opposite. Measured: it accepted a one-page output while being told all five pages were
included. An operation that must lose nothing needs an assertion that fails when something is
lost, which is a different question from whether something extra came along.

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
| `merge` ✅ | Output page count equals the sum of inputs; page order is preserved; merging one document is the identity |
| `split` | Splitting then merging round-trips to the original page sequence; every input page appears exactly once across outputs. **All three hold.** The subsetting rule in ADR 0019 §2 was the one that did not, and #54 closed it: `subset_closure.rs` asserts the structural property — no object belonging only to excluded pages — and `split_no_leak.rs` keeps §2a's named channels as regression cases, because neither layer can see what the other does. A document using optional content is refused rather than split; ADR 0019's 2026-09-14 amendment says why, and what pruning costs. Not yet on the web: no bridge, no page, no ADR 0022 promise. |
| `rotate` ✅ | Four 90° rotations return to the original; rotation is recorded, not re-rasterised. **Both hold**: `four_ninety_degree_rotations_return_to_the_original`, and `every_page_s_content_stream_comes_out_byte_identical` — which searches each page's operator run in the decompressed output rather than comparing whole stream bodies, a narrower claim than its name suggests. |
| `reorder` ✅ | Output is a permutation of the input — no page lost, added, or duplicated; the identity permutation is a no-op. **Both hold**: `any_permutation_is_carried_out_exactly` reads the order back out of the emitted bytes, and `the_identity_permutation_is_a_no_op` asserts it on the page ORDER rather than on the bytes — qpdf rewrites the file it is asked to write, so the output is never byte-identical to the input and no operation here preserves a signature. The page tree is flattened by any real permutation and left alone by the identity ([ADR 0021](adr/0021-how-reorder-permutes-a-page-tree.md)); `reorder_keeps_everything.rs` requires `content` and `navigation` whole and states that third-kind loss by name. The bridge is built and four conformance cases compare the two implementations, using the rotations vector as the observable. **They are not equally strong**, which the generator spells out per case: `reorder-a-document-where-one-page-differs` is the one a wrong permutation fails; `reorder-a-document-that-inherits` catches a flattening that drops an inherited `/Rotate`; the other two assert the page count and that nothing was lost. `/reorder-pdf` ships. Without thumbnails (#57) an order is typed rather than dragged, so the page completes a partial one by a stated rule — *the pages you list come first, and everything else keeps its current order after them* — and **shows the resulting order before anything runs**, which is what keeps that a rule rather than a guess. An identity is refused by the page: the core accepts it, but running it rewrites somebody's file to no effect. |
| `compress` | Output is never larger than the input; page count and page dimensions are unchanged; text remains extractable |

### Two findings from fuzzing `reorder`, neither of which is reorder's

Both were found in the same run, and both were only reachable because the target's corpus was
**seeded with real PDFs** for the first time. That is the third finding, and the one that
generalises: `fuzz/corpus/` is gitignored and nothing here has ever shipped a seed set, so
every "runs clean for 60 s" this project has recorded was measured against a corpus grown from
random bytes. Measured, with a defect deliberately planted: 577,209 unseeded executions found
nothing, and one seed failed on the first execution.

- **#61 — a damaged-but-openable document silently loses a page on write.** Opens as 5, writes
  4, no error, valid output. Reproduces through `rotate` (shipped) and through a plain write,
  so it belongs to the write path rather than to any operation. **Detected and refused since
  ADR 0022**: no caller receives the short document. The engine-seam defect underneath is still
  pinned by an `#[ignore]`d reproduction in `reorder_keeps_everything.rs` and a CI step that
  requires it to keep reproducing. The whole input class — damaged enough to be wrong, intact enough to open — had
  no coverage: every damaged fixture in the conformance corpus is refused at open.
- **#62 — memory-unsafety in the pinned qpdf 12.4.1, on the open path.** Reproduced from `rotate`, `merge` and `qpdf_check` — it is reached by *opening* a document, so every target and every operation can hit it, which is why seeded fuzzing is nightly-only until it closes. Upstream's code,
  reachable from opening any untrusted document. Private disclosure pending; no reproducer is
  in this repository. ADR 0013's trapping is unaffected and does not help — it catches C++
  exceptions, which this is not. The web build's wasm sandbox is a real mitigation; the native
  paths have none, and nothing native ships today.

### Web app

- One indexable page per tool: title, description, structured data, no client-side routing
- Drag-and-drop, drag-to-reorder, progress, cancel
- **Page thumbnails are deferred to #57**, with the render capability, the bridge method and
  the memory ceiling they need as its scope. ADR 0020 is the decision: `rotate` v1 selects by
  page number and range instead, and `/split-pdf` will want thumbnails too, so they are built
  once for both rather than bolted onto whichever page reaches for them first.
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

### Gate: CLOSED, 2026-09-12. M2 is planned against option 1.

**This was a gate, not a maybe, and it is now decided.** [Spike 0003](spikes/0003-pdfium-source-build.md)
answered the two inputs it was waiting on, and [ADR 0006](adr/0006-wasm-linking-strategy.md)'s
2026-09-12 amendment records the resolution: **option 1 is settled, not deferred again.** No
from-source PDFium build is scheduled. Redaction can be planned and built on the current
linking strategy.

The four reasons, in brief — the amendment carries them in full:

1. **Upstream PDFium has no WASM target.** One grep hit across the whole tree, in
   `third_party/harfbuzz/BUILD.gn`, and it is HarfBuzz's own shaper filenames; `DEPS` mentions
   Emscripten zero times. Option 2 is therefore a **fork of `pdfium-binaries`' patch set**
   carried across every bump, not a build.
2. **The aarch64 question is answered, and it was the wrong question.** `depot_tools` and `gn`
   both work here; the "Platform linux-arm64 is not supported" error was a relative-path
   resolution bug. Recorded in the ADR because a false blocker would have been an input to an
   architectural decision.
3. **The shared-glue-globals hazard is closed on both engines** — structurally for PDFium (one
   bundle, one evaluation per worker scope), and for qpdf by the memoised init promise plus
   `apps/web/src/worker/init-memoisation.test.ts`, which reverts the line to a boolean flag in a
   copy and asserts the invariant goes red.
4. **`catch_unwind` on the web is not required for M2.** Redaction safety rests on never
   emitting output unless verification passed, and panic payloads are discarded by policy
   anyway (ADR 0009), so unwinding would carry nothing we keep. Trap → instance fatal →
   fresh worker is implemented, tested in three browsers, and measured at 73–102 ms.

**One hazard is NOT closed by this, and M2 has to carry it.** Reason 3 closes the *same-engine*
route — two instances of one engine with the bridge rebound between them. PDFium and qpdf remain
**separate modules with separate linear memories**, so a redaction pass that inspects content in
one heap and emits output from the other is still "a redaction pass that inspects one heap and
edits another" — the failure mode this milestone exists to prevent. It is latent (no operation
spans both engines today) and it is not a reason to reopen the gate, because option 2 would not
obviously fix it either. It is a design requirement, R10 below.

**Three requirements come with this decision — R8, R9 and R10 — and they are what make
"`catch_unwind` is not required" true.** They are listed with the rest of M2's work below,
each with the check that has to exist, rather than left as prose in a gate section nobody
reads twice.

**Two named triggers would reopen it**, and nothing else: a true per-operation memory bound
becoming a requirement ([#25](https://github.com/TensorGreed/burrow/issues/25)), or
`pdfium-binaries` ceasing to publish at a version we need — the single external dependency
option 1 rests on, tracked as a supply-chain risk with no action now ([#44](https://github.com/TensorGreed/burrow/issues/44)).

Neither the memory ceiling nor #25 is resolved by this. [Spike 0002](spikes/0002-wasm-memory-ceiling.md)
remains unadopted, and its ceiling is per *worker*, so a caller's own `max_memory_bytes` still
bounds nothing.

### Requirements carried in from the linking gate

Conditions of ADR 0006's decision, not advice. Each is **unsatisfied until its check exists and
has been shown to fail without the property** — a requirement with no check is exactly the kind
of claim that reads as coverage. See the ADR's 2026-09-12 amendment for why each one exists.

| | requirement | check that must exist |
|---|---|---|
| **R8** | Redaction output is a single value returned from one Rust call, posted only after that call returns success. Progress may report *position*; it may not emit *content*. | `redaction-emission.spec.ts` — record every `postMessage`; assert no message before the final reply carries bytes, and exactly one carries output |
| **R9** | Nothing is written outside the wasm heap before verification passes — no OPFS, no File System Access, no `blob:` URL handed to the page. | `redaction-no-side-channel.spec.ts` — stub `getDirectory`, `showSaveFilePicker`, `createObjectURL`; assert none is called before the verified reply |
| **R10** | Verification runs on the exact byte sequence that is emitted, in the heap it is emitted from — never a sibling heap's copy. | `redact_verify_same_bytes` in `core/burrow-ops`, plus a conformance case for a both-engine path |

Under an unwind, violating R8 or R9 is recoverable: Rust returns `Err`, the buffer drops, a
`Drop` impl deletes the file. Under a trap — which is what the web has — they are not. R10 is
the one that has nothing to do with panics: it exists because PDFium and qpdf have separate
linear memories, so "verified" against one heap's copy says nothing about the bytes leaving the
other.

### The work

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

# 0004. Native engine selection

Date: 2026-09-10

## Status

Accepted — the acquisition question is resolved by
[spike 0001](../spikes/0001-wasm-engines.md). See *Acquisition (resolved)*.

## Context

burrow does not write its own PDF parser or JPEG encoder. Doing so would take years to
reach the correctness of existing engines on real-world files, and real-world files are
what we have to handle. So the core wraps established native engines.

The constraint that decides the shortlist is [ADR 0003](0003-permissive-licensing.md):
permissive licenses only. This rules out the most capable options in the space —
Ghostscript and MuPDF (AGPL without a commercial license), Poppler (GPL) — which are also
the ones most commonly recommended.

The second constraint is that every engine has to build for **five** target shapes:
`wasm32-unknown-unknown`, `aarch64-apple-ios`, two Android ABIs, and Linux/macOS for
development and regression runs. A C++ engine that assumes threads, filesystem access, or
`dlopen` is expensive on wasm regardless of its license.

The third is size. A wasm module the browser has to download before the first operation
is a user-facing cost, and PDFium is large.

## Decision

We will build on these engines, each chosen for a distinct job rather than overlapping:

| Engine | Job | License |
|---|---|---|
| **PDFium** | Render, edit page objects, merge/split | BSD-3-Clause |
| **qpdf** | Structure, encryption, repair, object streams | Apache-2.0 |
| **HarfBuzz** (`hb-subset`) | Font subsetting | MIT (Old Style) |
| **jbig2enc** | Black-and-white scan compression | Apache-2.0 — **verify** |
| **mozjpeg** | JPEG encoding | BSD-3-Clause / IJG |
| **libwebp** | WebP encode/decode | BSD-3-Clause |
| **libavif** | AVIF encode/decode | BSD-2-Clause |

PDFium and qpdf are complementary, not redundant: PDFium is the right tool for content
and rendering, qpdf for **document structure, encryption, object streams, and
linearisation**. Expect operations to use both.

**Corrected by spike 0001:** this ADR originally justified qpdf partly as the repair path
for files "PDFium refuses". That premise is weaker than assumed — PDFium silently
reconstructed a *fully corrupted* xref table and returned the correct page count
(Finding 4). qpdf reconstructed it too. So "repair" is not currently a reason to carry
qpdf; the other four jobs are. If repair is to be claimed again, it needs a harder test
case that PDFium actually fails, and that case belongs in the corpus.

Every engine is wrapped behind a trait in `burrow-engines` (see
[ADR 0002](0002-rust-core-and-bindings.md)), so `burrow-ops` never touches a C API and
`unsafe` stays in one crate.

**Acquisition — decided per engine in M1.** The tradeoff, recorded now:

- *Prebuilt binaries*, e.g. [pdfium-binaries](https://github.com/bblanchon/pdfium-binaries),
  which publishes builds for Linux ARM64, iOS, Android, and wasm. Vastly less build
  complexity and fast CI. Costs: a third-party build in our supply chain, provenance we
  must verify ourselves, no control over compile flags, and a dependency on someone else's
  release cadence for security fixes.
- *Building from source.* Full control over flags — which matters, since we want the
  smallest possible wasm and want to disable engine features we do not use. PDFium's
  `gn`/`ninja` build with its own toolchain is a substantial and slow undertaking to
  reproduce across five targets.

The likely landing point is prebuilt PDFium for early milestones — with pinned checksums
and recorded provenance — and source builds for the smaller engines (mozjpeg, libwebp,
libavif, HarfBuzz) where the build is a straightforward CMake invocation. This is not yet
committed; M1 will confirm or revise it and this ADR will be updated to `Accepted` with
the outcome.

Non-negotiable regardless of route: pinned versions and checksums, recorded provenance,
no fetching at build time from an unpinned URL, and a license check on the engine *and
its bundled third-party code* before it is vendored. PDFium in particular bundles
components under several licenses, and jbig2enc pulls in Leptonica (BSD-2-Clause) — both
need auditing as units, not by their headline license.

## Acquisition (resolved)

Decided by [spike 0001](../spikes/0001-wasm-engines.md), together with
[ADR 0006](0006-wasm-linking-strategy.md) — the two could not be settled separately.

### PDFium — prebuilt

Prebuilt from [pdfium-binaries](https://github.com/bblanchon/pdfium-binaries), release
**`chromium/8044`**, WebAssembly package, sha256
`2528dfb9762a5325f141fa29868257da51dd05952b13e2c02a08d4de00711fe1`.

Two things must be recorded honestly, because they are the weakest links in our supply
chain:

- **Upstream publishes no checksums and no signature.** Not a signed checksum file, not a
  sigstore bundle, nothing. Our hash is **trust-on-first-use** — a value we captured on
  first download and now enforce. It detects a *change*; it cannot detect that the
  original download was already wrong.
- **Upstream marks the WebAssembly build experimental**
  ([issue #28](https://github.com/bblanchon/pdfium-binaries/issues/28)).

Compare qpdf, which ships a PGP-clearsigned `.sha256` and a sigstore bundle — our qpdf
tarball was verified against upstream's signed checksum, not merely against itself. That
asymmetry is the argument for eventually building PDFium ourselves, and it is a second
reason to revisit the pre-M2 gate in ADR 0006.

There is also **no wasm `libpdfium.a`**: the package contains `pdfium.{html,js,wasm}` and
no static library or CMake config. That is what forces option 1 in ADR 0006.

### qpdf — from source

Built from source, **12.4.1**, sha256
`f045aa277be2356ff53a89a8622945958291177d2483afc20ede7c8a8cd3873c`, verified against
upstream's PGP-clearsigned checksum file.

**Configured with `-DUSE_IMPLICIT_CRYPTO=OFF -DREQUIRE_CRYPTO_NATIVE=ON`, asserted in
CI.** `USE_IMPLICIT_CRYPTO` defaults **ON** upstream, and qpdf ships a GnuTLS backend —
**LGPL-2.1-or-later**. A default build on any host where GnuTLS is discoverable would
statically link LGPL code, exactly the case ADR 0003 says we cannot satisfy. Relying on
"GnuTLS isn't installed in the container" is not enforcement: CI must fail if the crypto
summary changes or `QPDFCrypto_gnutls` appears in the link.

Also required: build with `-fexceptions` (or `-fwasm-exceptions` if option 2 is ever
adopted — the models must match Rust's). Without an exception flag a `throw` **aborts the
module**, losing qpdf's error reporting and the WASM instance with it.

### zlib and libjpeg — vendored by us, not taken from Emscripten ports

The spike used Emscripten's ports (`embuilder build zlib libjpeg`). That is a build-time
fetch pinned by *Emscripten*, not by us, and it delivered a surprise: the libjpeg port is
**IJG libjpeg 9f**, not libjpeg-turbo, and Emscripten's own port description calls it
"BSD license" when the actual terms are the IJG licence. A tool's self-description was
simply wrong.

So both are vendored with **our own pinned versions and checksums**. libjpeg 9f is also a
branch most distributions do not track, with no SIMD — a maintenance and hardening concern
for a parser fed hostile input, independent of licensing.

### emsdk — pinned

The Emscripten SDK contributes musl-derived libc, compiler-rt, and the JS glue to every
shipped wasm artifact. It is a **shipped dependency**, not just a tool, and must be pinned
by version and verified like any other. The spike pinned it by version only (6.0.9)
without verification; that is not sufficient.

### Native Linux — added by M1 PR 1

The original acquisition section covered only the **WASM** package, which was a gap:
`cargo test`, the fuzz targets (libFuzzer is native), and the corpus regression runs on
the GB10 all run **native**. So `linux-aarch64` and `linux-x86_64` are acquired too.

**PDFium: prebuilt, from the same release as the WASM build.**
`pdfium-linux-arm64.tgz` and `pdfium-linux-x64.tgz` at `chromium/8044` — the same release,
so every target agrees on the engine version. Hashes are **trust-on-first-use**, like the
WASM package, for the same reason: upstream publishes no checksums and no signature.

Both the tarball **and the extracted `libpdfium.so`** are pinned. The tarball hash says
nothing about what was unpacked, so `engines/pins.toml` carries a separate `so_sha256`
per architecture and `core/burrow-engines/build.rs` re-verifies the library it is about to
link. A vendor tree tampered with after fetching fails the build, not the test suite.

**Native linkage is DYNAMIC, and that is not a choice.** No static archive is published
for any platform: upstream's `build.yml` suffixes static artifacts `-static`, and no such
asset exists at this release. `07-stage.sh` stages `libpdfium.so` for `linux-shared`.

So `build.rs` links the shared library at build time — a recorded `DT_NEEDED` entry, not
a `dlopen` of a searched path — with an rpath computed from `CARGO_MANIFEST_DIR` and
`-Wl,--disable-new-dtags` so **`DT_RPATH`** is emitted rather than `DT_RUNPATH`. The
difference matters: `DT_RUNPATH` is overridable by `LD_LIBRARY_PATH`, `DT_RPATH` is not,
so the pinned copy cannot be shadowed by an environment variable. A test reads
`/proc/self/maps` after PDFium initialises and asserts the mapped library is the pinned
file under `engines/vendor/` — linking is not enough on its own, because the loader could
satisfy the dependency from `/usr/lib` and every other test would still pass.

**This linkage is dev/CI only.** It is how the test suite and corpus runner reach the
engine on Linux. It is not a shipping decision:

- **Shipped mobile linkage is decided at M3/M4.** Android and iOS have their own
  constraints (App Store rules on dynamic libraries, NDK packaging) and neither is
  acquired yet.
- **ADR 0006's pre-M2 gate now also covers static native archives**, not just the wasm
  question. If we take on a from-source PDFium build, static native linking comes with it
  and this section should be revisited.

**qpdf: from source, same tarball as the WASM build.** The same
PGP-clearsigned 12.4.1 source, built for the host architecture with the same
`-DUSE_IMPLICIT_CRYPTO=OFF -DREQUIRE_CRYPTO_NATIVE=ON`. `engines/build-native.sh` greps
the configure log and **exits non-zero** unless all three of "GNU TLS crypto enabled: OFF",
"OpenSSL crypto enabled: OFF" and "Native crypto enabled: ON" appear — the flags are not
trusted to have taken effect.

**zlib and libjpeg-turbo are vendored for native too**, from the same pinned sources as
the WASM build, so both paths link identical versions. That removes a divergence source
before the differential conformance harness (M1 item 12) has to explain one.

### Fuzzing a prebuilt engine — what it does and does not test

`engines/build-native.sh` produces a **second** qpdf archive instrumented with
`-fsanitize=fuzzer-no-link,address`. We can do that because we build qpdf from source.

**We cannot do it for PDFium.** The prebuilt has no sanitizer instrumentation and no
coverage feedback, so a fuzz target that goes through it exercises **our wrapper, our
error mapping, and our `Limits` enforcement** — not PDFium's internals. libFuzzer will be
driving a black box: crashes inside PDFium would still be detected as crashes, but
coverage-guided exploration of PDFium's own parser will not happen.

That is an acceptable division of labour, and it should be stated rather than assumed:
**PDFium's internals are fuzzed upstream by OSS-Fuzz**, continuously and with
instrumentation we are not going to match. Our fuzz targets exist to prove *our* boundary
is sound — that hostile input becomes a typed error, that no limit is escaped, and that
nothing panics or aborts. Claiming our fuzzing covers PDFium would be false.

If we ever build PDFium from source (ADR 0006's pre-M2 gate), instrumenting it becomes
possible and this changes.

### Licensing

All of the above is subject to
[ADR 0008](0008-widened-licence-allowlist.md), which widened the allowlist to admit the
`FTL`, `IJG`, `libpng-2.0` and `LicenseRef-AGG-2.3` components PDFium bundles, and records
the two mandatory credit lines. Every component is enumerated in
[`engines/licenses.toml`](../../engines/licenses.toml) and checked in CI.

### The five unaudited engines

HarfBuzz, jbig2enc (and Leptonica), mozjpeg, libwebp and libavif have **not** been through
a licence audit or a wasm build. Nothing above applies to them. jbig2enc is flagged
"verify" above and remains the likeliest to need its own ADR.

## Consequences

We inherit mature, well-fuzzed engines instead of writing parsers, which is the only
realistic way to reach acceptable correctness on real files.

We also inherit their vulnerabilities, their build systems, and their release cadence.
PDFium's CVE stream becomes ours to track, and wrapping C++ in Rust puts the highest-risk
code in the project at the `burrow-engines` boundary. Engines are also the main driver of
binary size — the wasm budget will be dominated by PDFium, and CI should track it as a
budget rather than discover it at release.

Choosing permissive engines means occasionally accepting a capability gap against what
Ghostscript or MuPDF would give us. Some conversions and repairs will simply be worse.
That cost is accepted in ADR 0003 and is felt here.

Upstream security fixes must be tracked actively per engine; `cargo-audit` covers Rust
crates and knows nothing about vendored C/C++.

## Alternatives considered

**Ghostscript or MuPDF.** The most capable options for conversion and rendering. Excluded
by ADR 0003; not reconsidered here.

**Pure-Rust PDF crates** (`lopdf`, `pdf-rs`, `printpdf`). Attractive: no FFI, no `unsafe`,
trivial cross-compilation, permissive licenses. Rejected as the primary engine because
none is close to PDFium on malformed real-world files, which is the case that matters.
Worth revisiting for narrow, structural operations where a full engine is overkill.

**PDF.js in the web app.** Would solve the web target elegantly, Apache-2.0, battle-tested.
Rejected: it is a fourth implementation, web-only, and reintroduces exactly the
multiple-implementations problem ADR 0002 exists to prevent — including for redaction.

**One engine for everything.** Simpler dependency graph. Rejected: no permissively
licensed engine covers both content editing and structural repair well, and forcing one
to do both would mean writing the missing half ourselves anyway.

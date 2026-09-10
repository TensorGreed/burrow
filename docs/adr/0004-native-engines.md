# 0004. Native engine selection

Date: 2026-09-10

## Status

Proposed — the engine list is settled; the acquisition strategy per engine is decided in
M1 when the first operations need it.

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
and rendering, qpdf for document structure, encryption, and repairing damaged files.
Expect operations to use both — and note that a file PDFium refuses may still be
recoverable by qpdf first.

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

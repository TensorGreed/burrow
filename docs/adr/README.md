# Architecture decision records

One file per decision, numbered, never rewritten. See
[0001](0001-record-architecture-decisions.md) for the process and
[`template.md`](template.md) for the shape.

| # | Decision | Status |
|---|---|---|
| [0001](0001-record-architecture-decisions.md) | Record architecture decisions in ADRs | Accepted |
| [0002](0002-rust-core-and-bindings.md) | A shared Rust core with uniffi and wasm bindings | Accepted, amended by [0009](0009-web-panic-contract-and-binding-boundary.md) |
| [0003](0003-permissive-licensing.md) | Permissive licenses only, enforced by CI | Superseded by [0008](0008-widened-licence-allowlist.md), amended by [0012](0012-ncsa-for-libfuzzer.md) |
| [0004](0004-native-engines.md) | Native engine selection | Accepted, repair claim re-established by [0013](0013-qpdf-c-api-and-prescan.md) |
| [0005](0005-web-stack.md) | Astro with Svelte islands for the web app | Accepted |
| [0006](0006-wasm-linking-strategy.md) | WASM linking strategy for the C/C++ engines | Accepted (option 1) |
| [0007](0007-limit-enforcement-per-platform.md) | Per-platform enforcement of time and memory limits | Accepted |
| [0008](0008-widened-licence-allowlist.md) | Widened licence allowlist for bundled native engines | Accepted, corrected by [0010](0010-harfbuzz-and-icu-in-pdfium.md), amended by [0012](0012-ncsa-for-libfuzzer.md) |
| [0009](0009-web-panic-contract-and-binding-boundary.md) | The web panic contract, and where the binding boundary really is | Accepted |
| [0010](0010-harfbuzz-and-icu-in-pdfium.md) | HarfBuzz and ICU inside PDFium: licences, and detecting undeclared components | Accepted |
| [0011](0011-pdfium-engine-thread.md) | Every PDFium call runs on one dedicated engine thread | Accepted |
| [0012](0012-ncsa-for-libfuzzer.md) | NCSA joins the allowlist, for libFuzzer | Accepted |
| [0013](0013-qpdf-c-api-and-prescan.md) | qpdf through its C API, on the caller's thread, behind a Rust pre-scan | Accepted |
| [0014](0014-web-engine-loading-and-csp.md) | How the web loads its engines, and what the CSP actually guarantees | Accepted — amends [0006](0006-wasm-linking-strategy.md) requirement 3 |
| [0015](0015-web-worker-lifecycle.md) | The web worker lifecycle: watchdog, recovery, circuit breaker and recycling | Accepted — amends [0007](0007-limit-enforcement-per-platform.md), completes [0009](0009-web-panic-contract-and-binding-boundary.md) |
| [0016](0016-differential-conformance.md) | The differential conformance harness, and what it found | Accepted — amends [0007](0007-limit-enforcement-per-platform.md) |
| [0017](0017-merge-engine-and-failure-semantics.md) | Which engine merges, and what a merge does when one input fails | Accepted |
| [0018](0018-when-the-engines-load.md) | When the engines load, and what bounds a start-up that is slow | Accepted |
| [0019](0019-how-split-builds-its-outputs.md) | How a split builds its outputs, and what a subsetting operation may not carry | Accepted, amended 2026-09-14 |
| [0020](0020-rotate-ships-without-thumbnails.md) | Rotate ships without page thumbnails | Accepted — deferred work is [0026](0026-how-rendering-loads-without-returning-to-the-old-payload.md) and #57 |
| [0021](0021-how-reorder-permutes-a-page-tree.md) | Reorder permutes in place, and accepts a flattened page tree | Accepted |
| [0022](0022-every-operation-verifies-its-own-output.md) | Every operation verifies its own output before returning it | Accepted |
| [0023](0023-how-an-operation-delivers-more-than-one-document.md) | How an operation delivers more than one document | Accepted |
| [0024](0024-how-burrow-is-deployed.md) | How burrow is deployed | Accepted, amended 2026-09-16 (the custom domain) |
| [0025](0025-what-compress-does-and-what-it-refuses-to-do.md) | What `compress` does, and what it refuses to do | Accepted |
| [0026](0026-how-rendering-loads-without-returning-to-the-old-payload.md) | How rendering loads, without returning to the old payload | Accepted — amends [spike 0004](../spikes/0004-the-first-load-budget-before-compress.md) |
